//! GDB Remote Serial Protocol stub with a live-picture extension, so GDB
//! (and front ends built on it) debug any machine here.
//!
//! Implemented after the GDB manual, appendix "GDB Remote Serial
//! Protocol" (packets, stop replies, `qXfer:features`, target
//! descriptions); no GDB or emulator code is used. As with the VICE
//! adapter, each packet runs on the emulation thread against the
//! [`Session`] together with this connection's state ([`GdbState`]); the
//! network task only frames bytes.
//!
//! Mapping to the native session:
//!
//! - **Architecture** comes from the main CPU's register names: `D0`–`A7`
//!   is a 68000 (target XML `org.gnu.gdb.m68k.core` plus `usp`/`ssp`;
//!   big-endian, which a target description cannot say: without an ELF
//!   file GDB needs `set endian big`, which `tools/gdb/mimic.py` does),
//!   `IX` a Z80 (GDB's `z80` register order: af bc de hl sp
//!   pc ix iy af' bc' de' hl' ir), `X`/`Y`/`P` a 6502 (a x y s p pc; stock
//!   GDB has no 6502, the description serves other RSP clients).
//! - **Threads:** one per debug target (`devices()`) of the main CPU's
//!   architecture, main CPU first (thread 1): the C64's `cpu` and
//!   `drive8`. Registers and memory follow the selected thread (`Hg`);
//!   memory is the thread's first memory space, read with peeks only
//!   (golden rule 2). Stepping always steps the main CPU.
//! - **Breakpoints:** `Z0`/`Z1` are exec breakpoints in the thread's space
//!   (no memory is patched), `Z2`/`Z3`/`Z4` write/read/access watchpoints.
//!   They belong to the connection and go away when it closes.
//! - **Stops** answer as `T` packets with `thread:`, `swbreak:`/`hwbreak:`
//!   or `watch:`/`rwatch:`/`awatch:` and the mimic key
//!   `beam:<frame>,<line>,<cycle>;` (hex; GDB ignores unknown keys).
//!   SIGTRAP for breakpoints and steps, SIGINT after Ctrl-C, SIGILL for a
//!   jammed or faulted CPU.
//!
//! The mimic extension (`mimic-frame+` in `qSupported`):
//!
//! - `qMimic.frame[:screen=<name>;format=png|qoi;scale=<n>;filter=raw|crt;profile=<p>]`
//!   returns the current (possibly partial) picture, binary-escaped like
//!   `X` packet data (`}` + byte ^ 0x20 for `#`, `$`, `}`, `*`).
//! - `qMimic.beam` returns `<frame>,<line>,<cycle>` (hex), `E01` without a
//!   beam.
//! - `monitor` (`qRcmd`) commands: `help`, `beam`, `devices`, `screen
//!   [name]`, `snapshot <file> [crt]`, `preview`, `reset [hard]`.
//!
//! `tools/gdb/mimic.py` uses these from GDB's Python API.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

use emu_core::{DeviceId, MemSpaceId, ScreenId, Size, StopReason};
use emu_preview::{Filter, Format, OwnedFrame};
use video_crt::Profile;

use crate::protocol::{BreakSpec, Command};
use crate::session::Session;

/// The IANA port for GDB remote (`gdbremote`).
pub const DEFAULT_PORT: u16 = 2159;
/// Largest packet accepted (advertised as `PacketSize`).
pub const PACKET_SIZE: usize = 0x4000;
/// Longest data read in one `m` packet.
const MAX_READ: usize = (PACKET_SIZE - 16) / 2;

/// Signals in stop replies.
mod signal {
    pub const SIGINT: u8 = 2;
    pub const SIGILL: u8 = 4;
    pub const SIGTRAP: u8 = 5;
}

/// What arrived from the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    /// `$…#cs` with a valid checksum; the payload, still escaped.
    Data(Vec<u8>),
    /// A packet with a wrong checksum: ask for it again.
    Corrupt,
    /// `+`
    Ack,
    /// `-`: resend the last reply.
    Nack,
    /// Ctrl-C (0x03).
    Interrupt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    TooLong,
}

/// Parse one packet from the front of `buf`. `Ok(None)` if more bytes are
/// needed; otherwise the packet and the number of bytes it used. Noise
/// before a `$` is skipped.
pub fn parse(buf: &[u8]) -> Result<Option<(Packet, usize)>, FrameError> {
    let mut i = 0;
    while let Some(&c) = buf.get(i) {
        match c {
            b'+' => return Ok(Some((Packet::Ack, i + 1))),
            b'-' => return Ok(Some((Packet::Nack, i + 1))),
            0x03 => return Ok(Some((Packet::Interrupt, i + 1))),
            b'$' => break,
            _ => i += 1,
        }
    }
    if i == buf.len() {
        return if i == 0 {
            Ok(None)
        } else {
            // Only noise so far: drop it.
            Ok(Some((Packet::Ack, i)))
        };
    }
    let start = i + 1;
    let Some(hash) = buf[start..].iter().position(|&c| c == b'#') else {
        if buf.len() - start > PACKET_SIZE * 4 {
            return Err(FrameError::TooLong);
        }
        return Ok(None);
    };
    let end = start + hash;
    if buf.len() < end + 3 {
        return Ok(None);
    }
    let data = &buf[start..end];
    let sum = data.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let given = std::str::from_utf8(&buf[end + 1..end + 3])
        .ok()
        .and_then(|s| u8::from_str_radix(s, 16).ok());
    let packet = if given == Some(sum) {
        Packet::Data(data.to_vec())
    } else {
        Packet::Corrupt
    };
    Ok(Some((packet, end + 3)))
}

/// Frame a reply: `$data#cs`.
pub fn frame(data: &[u8]) -> Vec<u8> {
    let sum = data.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let mut out = Vec::with_capacity(data.len() + 4);
    out.push(b'$');
    out.extend_from_slice(data);
    out.extend_from_slice(format!("#{sum:02x}").as_bytes());
    out
}

/// Binary data for a reply: `#`, `$`, `}` and `*` become `}` + byte ^ 0x20.
pub fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 16);
    for &b in data {
        if matches!(b, b'#' | b'$' | b'}' | b'*') {
            out.push(b'}');
            out.push(b ^ 0x20);
        } else {
            out.push(b);
        }
    }
    out
}

/// Undo [`escape`] (the data of `X` packets).
pub fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut it = data.iter();
    while let Some(&b) = it.next() {
        if b == b'}' {
            if let Some(&n) = it.next() {
                out.push(n ^ 0x20);
            }
        } else {
            out.push(b);
        }
    }
    out
}

fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn num(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 16).ok()
}

/// The processor families GDB knows a register layout for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    M68k,
    Z80,
    Mos6502,
}

/// Where a GDB register's value comes from.
#[derive(Clone, Copy, Debug)]
enum Src {
    Native(&'static str),
    /// High byte, low byte (Z80 register pairs, `ir`).
    Pair(&'static str, &'static str),
}

#[derive(Clone, Copy, Debug)]
struct RegDef {
    name: &'static str,
    bits: u8,
    src: Src,
    kind: &'static str,
}

const fn reg(name: &'static str, bits: u8, native: &'static str) -> RegDef {
    RegDef {
        name,
        bits,
        src: Src::Native(native),
        kind: "",
    }
}

const fn ptr(name: &'static str, bits: u8, native: &'static str, kind: &'static str) -> RegDef {
    RegDef {
        name,
        bits,
        src: Src::Native(native),
        kind,
    }
}

const fn pair(name: &'static str, hi: &'static str, lo: &'static str) -> RegDef {
    RegDef {
        name,
        bits: 16,
        src: Src::Pair(hi, lo),
        kind: "",
    }
}

/// GDB's m68k core feature, then mimic's extra stack pointers.
const M68K_CORE: usize = 18;
const M68K_REGS: [RegDef; 20] = [
    reg("d0", 32, "D0"),
    reg("d1", 32, "D1"),
    reg("d2", 32, "D2"),
    reg("d3", 32, "D3"),
    reg("d4", 32, "D4"),
    reg("d5", 32, "D5"),
    reg("d6", 32, "D6"),
    reg("d7", 32, "D7"),
    ptr("a0", 32, "A0", "data_ptr"),
    ptr("a1", 32, "A1", "data_ptr"),
    ptr("a2", 32, "A2", "data_ptr"),
    ptr("a3", 32, "A3", "data_ptr"),
    ptr("a4", 32, "A4", "data_ptr"),
    ptr("a5", 32, "A5", "data_ptr"),
    ptr("fp", 32, "A6", "data_ptr"),
    ptr("sp", 32, "A7", "data_ptr"),
    reg("ps", 32, "SR"),
    ptr("pc", 32, "PC", "code_ptr"),
    ptr("usp", 32, "USP", "data_ptr"),
    ptr("ssp", 32, "SSP", "data_ptr"),
];

const Z80_REGS: [RegDef; 13] = [
    pair("af", "A", "F"),
    pair("bc", "B", "C"),
    pair("de", "D", "E"),
    pair("hl", "H", "L"),
    ptr("sp", 16, "SP", "data_ptr"),
    ptr("pc", 16, "PC", "code_ptr"),
    reg("ix", 16, "IX"),
    reg("iy", 16, "IY"),
    reg("af'", 16, "AF'"),
    reg("bc'", 16, "BC'"),
    reg("de'", 16, "DE'"),
    reg("hl'", 16, "HL'"),
    pair("ir", "I", "R"),
];

const MOS6502_REGS: [RegDef; 6] = [
    reg("a", 8, "A"),
    reg("x", 8, "X"),
    reg("y", 8, "Y"),
    reg("s", 8, "S"),
    reg("p", 8, "P"),
    ptr("pc", 16, "PC", "code_ptr"),
];

impl Arch {
    /// Recognise a CPU by its register names.
    pub fn detect(names: &[&str]) -> Option<Self> {
        let has = |n: &str| names.iter().any(|r| r.eq_ignore_ascii_case(n));
        if has("D0") && has("A7") && has("SR") {
            Some(Arch::M68k)
        } else if has("IX") && has("AF'") {
            Some(Arch::Z80)
        } else if has("X") && has("Y") && has("P") {
            Some(Arch::Mos6502)
        } else {
            None
        }
    }

    fn regs(self) -> &'static [RegDef] {
        match self {
            Arch::M68k => &M68K_REGS,
            Arch::Z80 => &Z80_REGS,
            Arch::Mos6502 => &MOS6502_REGS,
        }
    }

    fn big_endian(self) -> bool {
        self == Arch::M68k
    }

    /// The target description GDB reads with `qXfer:features:read`.
    pub fn target_xml(self) -> String {
        let mut x = String::from(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE target SYSTEM \"gdb-target.dtd\">\n<target version=\"1.0\">\n",
        );
        let feature = |x: &mut String, name: &str, regs: &[RegDef]| {
            let _ = writeln!(x, "<feature name=\"{name}\">");
            for r in regs {
                let ty = if r.kind.is_empty() {
                    String::new()
                } else {
                    format!(" type=\"{}\"", r.kind)
                };
                let _ = writeln!(x, "<reg name=\"{}\" bitsize=\"{}\"{ty}/>", r.name, r.bits);
            }
            x.push_str("</feature>\n");
        };
        match self {
            Arch::M68k => {
                x.push_str("<architecture>m68k</architecture>\n<osabi>none</osabi>\n");
                feature(&mut x, "org.gnu.gdb.m68k.core", &M68K_REGS[..M68K_CORE]);
                feature(&mut x, "org.mimic.m68k.stack", &M68K_REGS[M68K_CORE..]);
            }
            // GDB's z80 port has a fixed register set; naming the
            // architecture is enough.
            Arch::Z80 => x.push_str("<architecture>z80</architecture>\n<osabi>none</osabi>\n"),
            Arch::Mos6502 => feature(&mut x, "org.mimic.6502", &MOS6502_REGS),
        }
        x.push_str("</target>\n");
        x
    }
}

/// One GDB breakpoint: the native breakpoints behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GdbBp {
    bps: Vec<u32>,
    /// `Z` type 0–4.
    kind: u8,
    thread: usize,
}

/// Per-connection state.
#[derive(Clone, Debug, Default)]
pub struct GdbState {
    arch: Option<Arch>,
    /// Debug targets shown as threads (index + 1 = thread id).
    threads: Vec<DeviceId>,
    /// Thread for registers and memory (`Hg`), index.
    g_thread: usize,
    /// (type, addr, kind) → native breakpoints.
    bps: BTreeMap<(u8, u32, u32), GdbBp>,
    no_ack: bool,
    /// A `c`/`vCont;c` waits for its stop reply.
    waiting: bool,
    /// The last reply, for `-`.
    last: Vec<u8>,
    /// `monitor screen`: picture for frames and snapshots.
    screen: Option<String>,
}

impl GdbState {
    /// Look at the machine: architecture and threads.
    fn init(&mut self, s: &mut Session) {
        if self.arch.is_some() {
            return;
        }
        let devices = s.machine().devices();
        let arch_of = |s: &mut Session, d: DeviceId| {
            let regs = s.machine_mut().debug_target(d)?.regs();
            let names: Vec<&str> = regs.0.iter().map(|r| r.name).collect();
            Arch::detect(&names)
        };
        let main = devices.first().copied();
        self.arch = main.and_then(|d| arch_of(s, d));
        self.threads = devices
            .iter()
            .copied()
            .filter(|&d| self.arch.is_some() && arch_of(s, d) == self.arch)
            .collect();
        if self.threads.is_empty() {
            self.threads.extend(main);
        }
    }

    fn thread_device(&self, t: usize) -> Option<DeviceId> {
        self.threads.get(t).copied()
    }

    /// Thread index of a GDB thread id (`-1`/`0`: any, the current one).
    fn thread_index(&self, id: &str) -> Option<usize> {
        match id {
            "-1" | "0" => Some(self.g_thread),
            _ => {
                let n = usize::from_str_radix(id.trim_start_matches("p1."), 16).ok()?;
                (1..=self.threads.len()).contains(&n).then(|| n - 1)
            }
        }
    }
}

/// The memory space a thread's registers and memory refer to.
fn thread_space(s: &Session, dev: DeviceId) -> Option<(MemSpaceId, u8)> {
    s.machine()
        .memory_spaces()
        .iter()
        .find(|m| m.device == dev)
        .map(|m| (m.id, m.addr_bits))
}

/// Answer one packet; returns the bytes to send (acks included).
pub fn handle(st: &mut GdbState, s: &mut Session, packet: &Packet) -> Vec<u8> {
    st.init(s);
    let mut out = Vec::new();
    let reply = match packet {
        Packet::Ack => return out,
        Packet::Nack => return st.last.clone(),
        Packet::Corrupt => return b"-".to_vec(),
        Packet::Interrupt => {
            if !s.is_running() {
                // Nothing to interrupt; GDB still wants a stop reply.
                if !st.waiting {
                    return out;
                }
            }
            let _ = s.command(Command::Pause);
            st.waiting = false;
            Some(stop_reply(st, s, Some(signal::SIGINT)))
        }
        Packet::Data(data) => {
            if !st.no_ack {
                out.push(b'+');
            }
            dispatch(st, s, data)
        }
    };
    if let Some(r) = reply {
        let framed = frame(&r);
        st.last = framed.clone();
        out.extend(framed);
    }
    if !s.is_running() {
        s.notify_frame();
    }
    out
}

/// A `stopped` event from the session: the stop reply for a `c`.
pub fn on_event(st: &mut GdbState, s: &mut Session, ev: &Value) -> Vec<u8> {
    if ev["event"] != "stopped" || !st.waiting || s.is_running() {
        return Vec::new();
    }
    st.waiting = false;
    let r = frame(&stop_reply(st, s, None));
    st.last = r.clone();
    r
}

/// The connection closed: remove its breakpoints.
pub fn disconnect(st: &mut GdbState, s: &mut Session) {
    for (_, bp) in std::mem::take(&mut st.bps) {
        for id in bp.bps {
            let _ = s.command(Command::BreakRemove { bp: id });
        }
    }
}

/// `None`: no reply now (a continue waits for the stop).
fn dispatch(st: &mut GdbState, s: &mut Session, data: &[u8]) -> Option<Vec<u8>> {
    // Commands other than Ctrl-C never arrive while GDB waits, but a stray
    // one must not act on a running machine.
    if s.is_running() && !st.waiting {
        let _ = s.command(Command::Pause);
    }
    let Some((&first, rest)) = data.split_first() else {
        return Some(Vec::new());
    };
    // Binary payloads first; everything else is text.
    if first == b'X' {
        return Some(write_binary(st, s, rest));
    }
    let text = String::from_utf8_lossy(rest).into_owned();
    let r: Vec<u8> = match first {
        b'?' => {
            if s.is_running() {
                let _ = s.command(Command::Pause);
            }
            st.waiting = false;
            stop_reply(st, s, None)
        }
        b'g' => read_registers(st, s)
            .map(|v| hex(&v).into_bytes())
            .unwrap_or_else(|| b"E01".to_vec()),
        b'G' => ok_or(write_registers(st, s, &text)),
        b'p' => match num(&text).and_then(|n| read_register(st, s, n as usize)) {
            Some(v) => hex(&v).into_bytes(),
            None => b"E01".to_vec(),
        },
        b'P' => {
            let r = text.split_once('=').and_then(|(n, v)| {
                let n = num(n)? as usize;
                write_register(st, s, n, &unhex(v)?)
            });
            ok_or(r)
        }
        b'm' => read_memory(st, s, &text).unwrap_or_else(|| b"E01".to_vec()),
        b'M' => {
            let r = text.split_once(':').and_then(|(range, data)| {
                let (addr, len) = range.split_once(',')?;
                let bytes = unhex(data)?;
                (bytes.len() == num(len)? as usize).then_some(())?;
                write_memory(st, s, num(addr)?, &bytes)
            });
            ok_or(r)
        }
        b'H' => {
            let (op, id) = text.split_at(text.len().min(1));
            match (op, st.thread_index(id)) {
                ("g", Some(t)) => {
                    st.g_thread = t;
                    b"OK".to_vec()
                }
                // Continue/step always run the whole machine.
                ("c", Some(_)) => b"OK".to_vec(),
                _ => b"E01".to_vec(),
            }
        }
        b'T' => match st.thread_index(&text) {
            Some(_) => b"OK".to_vec(),
            None => b"E01".to_vec(),
        },
        b'c' => return resume(st, s, &text),
        b's' => step(st, s, &text),
        b'Z' | b'z' => breakpoint(st, s, first == b'Z', &text),
        b'D' => {
            disconnect(st, s);
            st.waiting = false;
            let _ = s.command(Command::Resume);
            b"OK".to_vec()
        }
        b'k' => {
            // The emulator outlives GDB: drop the breakpoints, stay paused.
            disconnect(st, s);
            return None;
        }
        b'R' => {
            let _ = s.command(Command::Reset { kind: None });
            return None;
        }
        b'q' => query(st, s, &text),
        b'Q' => match text.as_str() {
            "StartNoAckMode" => {
                // The OK is still acknowledged; nothing after it.
                st.no_ack = true;
                b"OK".to_vec()
            }
            _ => Vec::new(),
        },
        b'v' => return v_packet(st, s, &text),
        _ => Vec::new(),
    };
    Some(r)
}

fn ok_or(r: Option<()>) -> Vec<u8> {
    match r {
        Some(()) => b"OK".to_vec(),
        None => b"E01".to_vec(),
    }
}

fn reg_value(regs: &emu_core::RegisterSet, src: Src) -> u32 {
    match src {
        Src::Native(n) => regs.get(n).unwrap_or(0),
        Src::Pair(hi, lo) => {
            (regs.get(hi).unwrap_or(0) & 0xff) << 8 | (regs.get(lo).unwrap_or(0) & 0xff)
        }
    }
}

fn reg_bytes(arch: Arch, def: &RegDef, v: u32) -> Vec<u8> {
    let n = usize::from(def.bits / 8);
    let le = v.to_le_bytes();
    let mut b = le[..n].to_vec();
    if arch.big_endian() {
        b.reverse();
    }
    b
}

fn bytes_value(arch: Arch, bytes: &[u8]) -> u32 {
    let mut b = bytes.to_vec();
    if arch.big_endian() {
        b.reverse();
    }
    b.iter().rev().fold(0, |a, &x| a << 8 | u32::from(x))
}

fn read_registers(st: &GdbState, s: &mut Session) -> Option<Vec<u8>> {
    let arch = st.arch?;
    let dev = st.thread_device(st.g_thread)?;
    let regs = s.machine_mut().debug_target(dev)?.regs();
    Some(
        arch.regs()
            .iter()
            .flat_map(|d| reg_bytes(arch, d, reg_value(&regs, d.src)))
            .collect(),
    )
}

fn read_register(st: &GdbState, s: &mut Session, n: usize) -> Option<Vec<u8>> {
    let arch = st.arch?;
    let def = arch.regs().get(n)?;
    let dev = st.thread_device(st.g_thread)?;
    let regs = s.machine_mut().debug_target(dev)?.regs();
    Some(reg_bytes(arch, def, reg_value(&regs, def.src)))
}

fn set_one(s: &mut Session, dev: DeviceId, def: &RegDef, v: u32) -> Option<()> {
    let t = s.machine_mut().debug_target(dev)?;
    match def.src {
        Src::Native(n) => t.set_reg(n, v),
        Src::Pair(hi, lo) => {
            t.set_reg(hi, v >> 8 & 0xff);
            t.set_reg(lo, v & 0xff);
        }
    }
    Some(())
}

fn write_register(st: &GdbState, s: &mut Session, n: usize, bytes: &[u8]) -> Option<()> {
    let arch = st.arch?;
    let def = arch.regs().get(n)?;
    if bytes.len() != usize::from(def.bits / 8) {
        return None;
    }
    let dev = st.thread_device(st.g_thread)?;
    set_one(s, dev, def, bytes_value(arch, bytes))
}

fn write_registers(st: &GdbState, s: &mut Session, text: &str) -> Option<()> {
    let arch = st.arch?;
    let bytes = unhex(text)?;
    let dev = st.thread_device(st.g_thread)?;
    let mut i = 0;
    for def in arch.regs() {
        let n = usize::from(def.bits / 8);
        let Some(b) = bytes.get(i..i + n) else {
            break;
        };
        // Leave untouched what GDB marks as unavailable ("xx" is not hex).
        set_one(s, dev, def, bytes_value(arch, b))?;
        i += n;
    }
    Some(())
}

/// `m addr,len`: bytes until the first unreadable address.
fn read_memory(st: &GdbState, s: &mut Session, text: &str) -> Option<Vec<u8>> {
    let (addr, len) = text.split_once(',')?;
    let (addr, len) = (num(addr)?, (num(len)? as usize).min(MAX_READ));
    let dev = st.thread_device(st.g_thread)?;
    let (space, bits) = thread_space(s, dev)?;
    let limit = 1u64 << bits;
    let target = s.machine_mut().debug_target(dev)?;
    let mut data = Vec::with_capacity(len);
    for i in 0..len as u64 {
        let a = u64::from(addr) + i;
        if a >= limit {
            break;
        }
        match target.peek(space, a as u32, Size::Byte) {
            Some(v) => data.push(v as u8),
            None => break,
        }
    }
    if data.is_empty() && len > 0 {
        return Some(b"E14".to_vec());
    }
    Some(hex(&data).into_bytes())
}

fn write_memory(st: &GdbState, s: &mut Session, addr: u32, bytes: &[u8]) -> Option<()> {
    let dev = st.thread_device(st.g_thread)?;
    let (space, bits) = thread_space(s, dev)?;
    if u64::from(addr) + bytes.len() as u64 > 1u64 << bits {
        return None;
    }
    let target = s.machine_mut().debug_target(dev)?;
    for (i, &b) in bytes.iter().enumerate() {
        if !target.poke(space, addr + i as u32, Size::Byte, u32::from(b)) {
            return None;
        }
    }
    Some(())
}

/// `X addr,len:binary`
fn write_binary(st: &GdbState, s: &mut Session, rest: &[u8]) -> Vec<u8> {
    let Some(colon) = rest.iter().position(|&c| c == b':') else {
        return b"E01".to_vec();
    };
    let head = String::from_utf8_lossy(&rest[..colon]).into_owned();
    let data = unescape(&rest[colon + 1..]);
    let r = head.split_once(',').and_then(|(addr, len)| {
        (data.len() == num(len)? as usize).then_some(())?;
        if data.is_empty() {
            // GDB probes for X support with an empty write.
            return Some(());
        }
        write_memory(st, s, num(addr)?, &data)
    });
    ok_or(r)
}

fn resume(st: &mut GdbState, s: &mut Session, addr: &str) -> Option<Vec<u8>> {
    if let Some(pc) = num(addr) {
        let dev = st.thread_device(0)?;
        let def = st.arch?.regs().iter().find(|r| r.name == "pc")?;
        set_one(s, dev, def, pc);
    }
    match s.command(Command::Resume) {
        Ok(_) => {
            st.waiting = true;
            None
        }
        Err(_) => Some(b"E01".to_vec()),
    }
}

fn step(st: &mut GdbState, s: &mut Session, addr: &str) -> Vec<u8> {
    if let Some(pc) = num(addr) {
        if let (Some(dev), Some(def)) = (
            st.thread_device(0),
            st.arch
                .and_then(|a| a.regs().iter().find(|r| r.name == "pc")),
        ) {
            set_one(s, dev, def, pc);
        }
    }
    match s.command(Command::StepInstr { count: Some(1) }) {
        Ok(_) => stop_reply(st, s, None),
        Err(_) => b"E01".to_vec(),
    }
}

/// `vCont`, `vMustReplyEmpty` …
fn v_packet(st: &mut GdbState, s: &mut Session, text: &str) -> Option<Vec<u8>> {
    if text == "Cont?" {
        return Some(b"vCont;c;C;s;S".to_vec());
    }
    if let Some(actions) = text.strip_prefix("Cont;") {
        // The first action decides: the machine runs as a whole.
        let action = actions.split(';').next().unwrap_or("");
        let verb = action.split(':').next().unwrap_or("");
        return match verb.chars().next() {
            Some('c' | 'C') => resume(st, s, ""),
            Some('s' | 'S') => Some(step(st, s, "")),
            _ => Some(b"E01".to_vec()),
        };
    }
    if text.starts_with("Kill") {
        disconnect(st, s);
        return Some(b"OK".to_vec());
    }
    Some(Vec::new())
}

/// `Z`/`z type,addr,kind`
fn breakpoint(st: &mut GdbState, s: &mut Session, insert: bool, text: &str) -> Vec<u8> {
    let mut it = text.split(';').next().unwrap_or("").split(',');
    let (Some(ty), Some(addr), Some(len)) = (
        it.next().and_then(num),
        it.next().and_then(num),
        it.next().and_then(num),
    ) else {
        return b"E01".to_vec();
    };
    if ty > 4 {
        return Vec::new();
    }
    let key = (ty as u8, addr, len);
    if !insert {
        return match st.bps.remove(&key) {
            Some(bp) => {
                for id in bp.bps {
                    let _ = s.command(Command::BreakRemove { bp: id });
                }
                b"OK".to_vec()
            }
            // Already gone (e.g. removed twice): fine for GDB.
            None => b"OK".to_vec(),
        };
    }
    if st.bps.contains_key(&key) {
        return b"OK".to_vec();
    }
    let thread = st.g_thread;
    let Some(dev) = st.thread_device(thread) else {
        return b"E01".to_vec();
    };
    let Some((space, _)) = thread_space(s, dev) else {
        return b"E01".to_vec();
    };
    let kinds: &[&str] = match ty {
        0 | 1 => &["exec"],
        2 => &["write"],
        3 => &["read"],
        _ => &["read", "write"],
    };
    let mut ids = Vec::new();
    for kind in kinds {
        let spec = BreakSpec {
            kind: Some((*kind).into()),
            space: Some(space.0),
            addr: Some(addr),
            len: (ty >= 2).then_some(len.max(1)),
            ..BreakSpec::default()
        };
        match s.command(Command::BreakAdd(spec)) {
            Ok(v) => ids.extend(v["bp"].as_u64().map(|b| b as u32)),
            Err(_) => {
                for id in ids {
                    let _ = s.command(Command::BreakRemove { bp: id });
                }
                return b"E01".to_vec();
            }
        }
    }
    st.bps.insert(
        key,
        GdbBp {
            bps: ids,
            kind: ty as u8,
            thread,
        },
    );
    b"OK".to_vec()
}

/// `T` packet for the session's current stop.
fn stop_reply(st: &GdbState, s: &mut Session, signal: Option<u8>) -> Vec<u8> {
    let reason = s.last_stop().unwrap_or(StopReason::StepDone);
    let find = |id: u32| st.bps.values().find(|b| b.bps.contains(&id));
    let mut thread = 0;
    let mut extra = String::new();
    let mut sig = signal::SIGTRAP;
    match reason {
        StopReason::Breakpoint(id) => {
            if let Some(bp) = find(id.0) {
                thread = bp.thread;
                extra = if bp.kind == 1 {
                    "hwbreak:;"
                } else {
                    "swbreak:;"
                }
                .into();
            }
        }
        StopReason::Watchpoint { id, addr, .. } => {
            if let Some(bp) = find(id.0) {
                thread = bp.thread;
                let key = match bp.kind {
                    2 => "watch",
                    3 => "rwatch",
                    _ => "awatch",
                };
                extra = format!("{key}:{addr:x};");
            }
        }
        StopReason::CpuJam { device, .. } | StopReason::Fault { device, .. } => {
            sig = signal::SIGILL;
            thread = st.threads.iter().position(|&d| d == device).unwrap_or(0);
        }
        StopReason::FrameDone | StopReason::StepDone => {}
    }
    let sig = signal.unwrap_or(sig);
    let mut r = format!("T{sig:02x}thread:{:x};{extra}", thread + 1);
    if let Some(b) = s.machine().beam() {
        let _ = write!(r, "beam:{:x},{:x},{:x};", b.frame, b.line, b.cycle);
    }
    r.into_bytes()
}

fn query(st: &mut GdbState, s: &mut Session, text: &str) -> Vec<u8> {
    // `qRcmd,…` and `qThreadExtraInfo,…` separate with a comma.
    let (name, args) = match text.find([':', ',']) {
        Some(i) => (&text[..i], &text[i + 1..]),
        None => (text, ""),
    };
    match name {
        "Supported" => format!(
            "PacketSize={PACKET_SIZE:x};qXfer:features:read+;QStartNoAckMode+;\
             swbreak+;hwbreak+;vContSupported+;mimic-frame+"
        )
        .into_bytes(),
        "Attached" => b"1".to_vec(),
        "C" => format!("QC{:x}", st.g_thread + 1).into_bytes(),
        "fThreadInfo" => {
            let ids: Vec<String> = (1..=st.threads.len()).map(|t| format!("{t:x}")).collect();
            format!("m{}", ids.join(",")).into_bytes()
        }
        "sThreadInfo" => b"l".to_vec(),
        "ThreadExtraInfo" => match st.thread_index(args).and_then(|t| st.thread_device(t)) {
            Some(d) => hex(d.0.as_bytes()).into_bytes(),
            None => b"E01".to_vec(),
        },
        "Symbol" => b"OK".to_vec(),
        "Offsets" => b"Text=0;Data=0;Bss=0".to_vec(),
        "Xfer" => xfer(st, args),
        "Rcmd" => {
            let cmd = unhex(args)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let out = monitor(st, s, cmd.trim());
            hex(out.as_bytes()).into_bytes()
        }
        "Mimic.beam" => match s.machine().beam() {
            Some(b) => format!("{:x},{:x},{:x}", b.frame, b.line, b.cycle).into_bytes(),
            None => b"E01".to_vec(),
        },
        "Mimic.frame" => match picture(st, s, args) {
            Ok(img) => escape(&img),
            Err(_) => b"E01".to_vec(),
        },
        _ => Vec::new(),
    }
}

/// `qXfer:features:read:target.xml:offset,length`
fn xfer(st: &GdbState, args: &str) -> Vec<u8> {
    let mut it = args.splitn(4, ':');
    let (Some("features"), Some("read"), Some(annex), Some(range)) =
        (it.next(), it.next(), it.next(), it.next())
    else {
        return Vec::new();
    };
    if annex != "target.xml" {
        return b"E00".to_vec();
    }
    let Some(arch) = st.arch else {
        return b"E00".to_vec();
    };
    let Some((off, len)) = range
        .split_once(',')
        .and_then(|(o, l)| Some((num(o)? as usize, num(l)? as usize)))
    else {
        return b"E00".to_vec();
    };
    let xml = arch.target_xml().into_bytes();
    let start = off.min(xml.len());
    let end = (start + len).min(xml.len());
    let mut r = vec![if end == xml.len() { b'l' } else { b'm' }];
    r.extend(escape(&xml[start..end]));
    r
}

/// Parameters of `qMimic.frame` and `monitor snapshot`.
#[derive(Clone, Debug, PartialEq)]
struct PictureOpts {
    screen: Option<String>,
    format: Format,
    scale: u32,
    crt: bool,
    profile: Option<String>,
}

fn parse_picture_opts(args: &str) -> Result<PictureOpts, String> {
    let mut o = PictureOpts {
        screen: None,
        format: Format::Png,
        scale: 1,
        crt: false,
        profile: None,
    };
    for kv in args.split(';').filter(|kv| !kv.is_empty()) {
        let (k, v) = kv.split_once('=').ok_or(format!("bad parameter '{kv}'"))?;
        match k {
            "screen" => o.screen = Some(v.into()),
            "format" => {
                o.format = match v {
                    "png" => Format::Png,
                    "qoi" => Format::Qoi,
                    _ => return Err(format!("unknown format '{v}'")),
                }
            }
            "scale" => {
                o.scale = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=8).contains(n))
                    .ok_or("scale must be 1–8")?
            }
            "filter" => {
                o.crt = match v {
                    "raw" => false,
                    "crt" => true,
                    _ => return Err(format!("unknown filter '{v}'")),
                }
            }
            "profile" => o.profile = Some(v.into()),
            _ => return Err(format!("unknown parameter '{k}'")),
        }
    }
    Ok(o)
}

/// Encode the current picture.
fn picture(st: &GdbState, s: &Session, args: &str) -> Result<Vec<u8>, String> {
    let o = parse_picture_opts(args)?;
    let m = s.machine();
    let name = o.screen.as_deref().or(st.screen.as_deref());
    let id = match name {
        None => ScreenId::MAIN,
        Some(n) => m
            .screens()
            .iter()
            .find(|i| i.name == n)
            .map(|i| i.id)
            .ok_or(format!("unknown screen '{n}'"))?,
    };
    let frame = OwnedFrame::from_screen(m, id, s.is_running()).ok_or("no such screen")?;
    let filter = if o.crt {
        let profile = match &o.profile {
            Some(p) => Profile::preset(p).ok_or(format!("unknown profile '{p}'"))?,
            None => Profile::default_for(s.machine_id()),
        };
        Filter::Crt {
            profile,
            scale: o.scale.max(2),
        }
    } else {
        Filter::Raw { scale: o.scale }
    };
    Ok(o.format.encode(&filter.render(&frame)))
}

const MONITOR_HELP: &str = "\
mimic monitor commands:
  beam                    frame, raster line and cycle
  devices                 debug targets (threads) and memory spaces
  screen [name]           list the machine's screens or pick one for pictures
  snapshot <file> [crt]   write the current picture as PNG
  preview                 start (or name) the live preview server
  reset [hard]            reset (soft: RESET line, hard: power cycle)
";

fn monitor(st: &mut GdbState, s: &mut Session, cmd: &str) -> String {
    let mut words = cmd.split_whitespace();
    let verb = words.next().unwrap_or("help");
    let args: Vec<&str> = words.collect();
    match verb {
        "help" => MONITOR_HELP.into(),
        "beam" => match s.machine().beam() {
            Some(b) => format!(
                "frame {} line {} cycle {} hpos {}\n",
                b.frame, b.line, b.cycle, b.hpos
            ),
            None => "this machine has no beam\n".into(),
        },
        "devices" => {
            let mut out = String::new();
            for d in s.machine().devices() {
                let t = st.threads.iter().position(|&x| x == d);
                let _ = match t {
                    Some(t) => writeln!(out, "thread {}: {}", t + 1, d.0),
                    None => writeln!(out, "(not a thread): {}", d.0),
                };
            }
            for m in s.machine().memory_spaces() {
                let _ = writeln!(
                    out,
                    "space {} '{}' of {} ({} address bits)",
                    m.id.0, m.name, m.device.0, m.addr_bits
                );
            }
            out
        }
        "screen" => {
            let screens = s.machine().screens();
            match args.first() {
                None => {
                    let current = st.screen.as_deref().unwrap_or("main");
                    screens
                        .iter()
                        .map(|i| {
                            let mark = if i.name == current { " (selected)" } else { "" };
                            format!("{}{mark}\n", i.name)
                        })
                        .collect()
                }
                Some(&n) if screens.iter().any(|i| i.name == n) => {
                    st.screen = Some(n.into());
                    format!("pictures now show screen '{n}'\n")
                }
                Some(n) => format!("unknown screen '{n}'\n"),
            }
        }
        "snapshot" => {
            let Some(path) = args.first() else {
                return "usage: snapshot <file> [crt]\n".into();
            };
            let opts = if args.get(1) == Some(&"crt") {
                "filter=crt"
            } else {
                ""
            };
            match picture(st, s, opts) {
                Ok(png) => match std::fs::write(path, png) {
                    Ok(()) => format!("wrote {path}\n"),
                    Err(e) => format!("{path}: {e}\n"),
                },
                Err(e) => format!("{e}\n"),
            }
        }
        "preview" => match s.preview_url() {
            Ok(url) => format!("{url}\n"),
            Err(e) => format!("{e}\n"),
        },
        "reset" => {
            let kind = args.first().map(|k| (*k).to_string());
            match s.command(Command::Reset { kind }) {
                Ok(_) => "reset\n".into(),
                Err(e) => format!("{e}\n"),
            }
        }
        other => format!("unknown command '{other}'; try 'monitor help'\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_checksums() {
        assert_eq!(frame(b"OK"), b"$OK#9a");
        let (p, used) = parse(b"+$g#67rest").unwrap().unwrap();
        assert_eq!((p, used), (Packet::Ack, 1));
        let (p, used) = parse(b"$g#67rest").unwrap().unwrap();
        assert_eq!((p, used), (Packet::Data(b"g".to_vec()), 5));
        assert_eq!(parse(b"$g#68").unwrap().unwrap().0, Packet::Corrupt);
        assert_eq!(parse(b"$g#6").unwrap(), None);
        assert_eq!(parse(b"\x03").unwrap().unwrap().0, Packet::Interrupt);
    }

    #[test]
    fn escapes_round_trip() {
        let data = [b'#', b'$', b'}', b'*', 0, 0xff];
        let e = escape(&data);
        assert_eq!(e, [b'}', 0x03, b'}', 0x04, b'}', 0x5d, b'}', 0x0a, 0, 0xff]);
        assert_eq!(unescape(&e), data);
    }

    #[test]
    fn detects_architectures_and_describes_them() {
        assert_eq!(
            Arch::detect(&["D0", "A7", "PC", "SR", "USP"]),
            Some(Arch::M68k)
        );
        assert_eq!(Arch::detect(&["PC", "SP", "IX", "AF'"]), Some(Arch::Z80));
        assert_eq!(
            Arch::detect(&["PC", "A", "X", "Y", "S", "P"]),
            Some(Arch::Mos6502)
        );
        assert_eq!(Arch::detect(&["PC", "COP1LC"]), None);
        let xml = Arch::M68k.target_xml();
        assert!(xml.contains("<feature name=\"org.gnu.gdb.m68k.core\">"));
        assert!(xml.contains("<reg name=\"fp\" bitsize=\"32\" type=\"data_ptr\"/>"));
        assert!(Arch::Z80
            .target_xml()
            .contains("<architecture>z80</architecture>"));
    }

    #[test]
    fn register_bytes_follow_the_byte_order() {
        let pc = &M68K_REGS[17];
        assert_eq!(reg_bytes(Arch::M68k, pc, 0x00fc_00d2), [0, 0xfc, 0, 0xd2]);
        assert_eq!(bytes_value(Arch::M68k, &[0, 0xfc, 0, 0xd2]), 0x00fc_00d2);
        let pc = &MOS6502_REGS[5];
        assert_eq!(reg_bytes(Arch::Mos6502, pc, 0xe5cf), [0xcf, 0xe5]);
        assert_eq!(bytes_value(Arch::Mos6502, &[0xcf, 0xe5]), 0xe5cf);
    }

    #[test]
    fn picture_options() {
        let o = parse_picture_opts("screen=vdc;format=qoi;scale=2;filter=crt").unwrap();
        assert_eq!(o.screen.as_deref(), Some("vdc"));
        assert_eq!((o.format, o.scale, o.crt), (Format::Qoi, 2, true));
        assert!(parse_picture_opts("scale=9").is_err());
        assert!(parse_picture_opts("bogus=1").is_err());
    }
}
