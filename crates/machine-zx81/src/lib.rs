//! Sinclair ZX81 (and the Timex Sinclair 1000, its 60 Hz twin).
//!
//! Z80A at 3.25 MHz, the ULA (`chips-sinclair`: video from the CPU's
//! display-file fetches, NMI generator, port `$FE`), 8 KB ROM and 1 KB
//! RAM or the 16 KB RAM pack (see [`memory`]). No sound.
//!
//! Models: "zx81" (default; PAL, 50 Hz, 310-line fields) and "ts1000"
//! (NTSC, 60 Hz: port `$FE` bit 6 reads 0 and the ROM makes 262-line
//! fields). Options:
//! - `ram`: "16k" (default), "1k" (the bare machine), "2k" (TS1000).
//! - `traps`: "on" (default) or "off" — the LOAD and SAVE routines of the
//!   Sinclair ROM are intercepted to read and write the ".P" file in the
//!   tape slot (the program's bytes from `$4009` to E_LINE, as the ROM
//!   saves them); with "off" the ROM runs its tape code (the EAR line is
//!   silent, so it waits).
//! - `roms`: "stub" runs a small built-in display program ([`stub`]).
//!
//! Media slot "tape" (kind Tape): a `.P`/`.81` file.
//!
//! ROM: role "zx81" (8 KB; the "improved" third edition is preferred).
//! User-supplied, never in the repository.
//!
//! **Timing.** The Z80 runs whole instructions with [`Z80::step_timed`];
//! before each bus access the bus brings the ULA up to that T-state (one
//! ULA tick per T-state: two pixels). On an opcode fetch from `$8000`
//! upwards the ULA may replace the byte by a NOP and fetch the character's
//! pattern through the I register (see `chips_sinclair::ula`). /INT follows
//! R bit 6; the NMI generator's pulses are latched and taken after the
//! instruction; a halted CPU is released at a fixed line phase (the ULA's
//! /WAIT). Without a VSYNC for a field and a half (FAST mode, the tape
//! routines) the picture is blanked and a frame reported anyway.
//! [`Machine::step_cycle`] runs one instruction.
//!
//! Sources: Sinclair ZX81 Service Manual; Ian Logan and Frank O'Hara, *The
//! Complete ZX81 ROM Disassembly* (NEXT-PROG $0347, HEADER $02FF, SLOW/FAST
//! $0207, system variables); Wilf Rigter's ZX81 video notes. The implementation
//! is this project's own.

#![forbid(unsafe_code)]

pub mod gfx;
pub mod keyboard;
pub mod memory;
pub mod stub;

use chips_sinclair::ula::{self, Ula};
use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{
    DriveStatus, Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId,
};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use memory::{Memory, ROM_SIZE};

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
pub const SLOT_TAPE: SlotId = SlotId(0);

pub const CPU_HZ: u32 = 3_250_000;

/// ROM points the tape traps use (Logan/O'Hara): NEXT-PROG, where LOAD
/// starts reading bytes after its name was evaluated; HEADER, where SAVE
/// starts the tape output after its name; and SLOW/FAST, where both end.
pub const ROM_LOAD: u16 = 0x0347;
pub const ROM_SAVE: u16 = 0x02ff;
pub const ROM_SLOW_FAST: u16 = 0x0207;
/// System variables: E_LINE (end of the program area) and the start of a
/// saved program (VERSN at $4009).
pub const E_LINE: u16 = 0x4014;
pub const PROGRAM_START: u16 = 0x4009;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "zx81",
        name: "Sinclair ZX81",
        models: &["zx81", "ts1000"],
        create: |config| Ok(Box::new(Zx81::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Zx81,
    Ts1000,
}

/// The tape slot: one ".P" file, read by the LOAD trap, written by SAVE.
#[derive(Default)]
struct PTape {
    media: Option<Media>,
    bytes: Vec<u8>,
    dirty: bool,
}

impl PTape {
    fn write_back(&mut self) -> Result<(), MediaError> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        match &mut self.media {
            Some(m) => m.backend.write(0, &self.bytes),
            None => Ok(()),
        }
    }
}

impl MediaSlot for PTape {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        if bytes.is_empty() || bytes.len() > 0x4000 - 9 {
            return Err(MediaError::Format(format!(
                ".P file of {} bytes (1 to {})",
                bytes.len(),
                0x4000 - 9
            )));
        }
        let _ = self.eject();
        self.media = Some(media);
        self.bytes = bytes;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.bytes.clear();
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: false,
            error: false,
            track: None,
        }
    }
}

/// The Z80's bus for one instruction: brings the ULA up to each access,
/// turns display-file fetches into NOPs and pixels, routes debugger hooks.
struct ZxBus<'a> {
    mem: &'a mut Memory,
    ula: &'a mut Ula,
    keys: &'a [u8; 8],
    /// T-states the ULA has run.
    chips_now: &'a mut u64,
    /// T-state the instruction started on.
    start: u64,
    /// The coming read is an opcode fetch (M1).
    fetch: bool,
    /// Align the coming fetch (the NMI acknowledge of a halted CPU) to the
    /// ULA's release point.
    align: bool,
    i: u8,
    halted: bool,
    /// NMI pulses seen while running the instruction.
    nmi: bool,
    /// A field ended during the instruction.
    field_done: bool,
    sixty_hz: bool,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

/// Run the ULA up to (not including) T-state `to`.
fn catch_up(ula: &mut Ula, now: &mut u64, to: u64, nmi: &mut bool) {
    while *now < to {
        let out = ula.tick();
        *nmi |= out.nmi;
        *now += 1;
    }
}

impl ZxBus<'_> {
    fn raw_read(&mut self, addr: u32) -> u8 {
        if addr >= IO_BASE {
            let port = addr as u16;
            if port & 1 == 0 {
                // Rows whose address line is low.
                let mut keys = 0;
                for (row, k) in self.keys.iter().enumerate() {
                    if port & (0x100 << row) == 0 {
                        keys |= k;
                    }
                }
                let v = self.ula.in_fe(keys);
                return if self.sixty_hz { v & !0x40 } else { v };
            }
            return 0xff;
        }
        let byte = self.mem.read(addr as u16);
        if std::mem::take(&mut self.fetch) {
            let (seen, pattern) = self.ula.m1(addr as u16, byte, self.halted);
            if let Some(low) = pattern {
                let invert = low & 0x8000 != 0;
                let at = u16::from(self.i) << 8 | (low & 0x01ff);
                let p = self.mem.read(at);
                self.ula.load_pattern(p, invert);
            }
            return seen;
        }
        byte
    }
}

impl Bus for ZxBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = u32::from(self.raw_read(addr));
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let seen = dbg.on_read(CPU_SPACE, addr, size, v);
                dbg.trace_read(CPU_SPACE, addr, size, seen, self.beam);
                seen
            }
            _ => v,
        }
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.mem, self.ula, self.keys, addr))
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, size, v & 0xff, self.beam);
                if !dbg.on_write(CPU_SPACE, addr, size, v & 0xff) {
                    return;
                }
            }
        }
        if addr >= IO_BASE {
            self.field_done |= self.ula.out(addr as u16);
        } else {
            self.mem.write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for ZxBus<'_> {
    fn wait_states(&mut self, access: Access, t: u32) -> u32 {
        let at = self.start + u64::from(t);
        catch_up(self.ula, self.chips_now, at, &mut self.nmi);
        self.fetch = access == Access::Fetch;
        if self.fetch && std::mem::take(&mut self.align) {
            // /WAIT until the ULA lets the halted CPU acknowledge the NMI.
            let w = Ula::nmi_release_t(self.ula.hcount);
            catch_up(self.ula, self.chips_now, at + u64::from(w), &mut self.nmi);
            return w;
        }
        0
    }
}

fn peek(mem: &Memory, ula: &Ula, keys: &[u8; 8], addr: u32) -> u8 {
    if addr >= IO_BASE {
        let port = addr as u16;
        if port & 1 != 0 {
            return 0xff;
        }
        let mut k = 0;
        for (row, v) in keys.iter().enumerate() {
            if port & (0x100 << row) == 0 {
                k |= v;
            }
        }
        (!k & 0x1f) | 0x60 | u8::from(ula.ear) << 7
    } else {
        mem.read(addr as u16)
    }
}

struct PeekBus<'a>(&'a Memory, &'a Ula, &'a [u8; 8]);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.0, self.1, self.2, addr))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

pub struct Zx81 {
    model: Model,
    cpu: Z80,
    mem: Memory,
    ula: Ula,
    /// Keyboard rows, bits 0–4 (1 = pressed).
    keys: [u8; 8],
    /// T-states since power-on.
    sched: Scheduler,
    input: InputQueue,
    palette: [Rgb8; 2],
    field_done_pending: bool,
    traps: bool,
    tape: PTape,
    /// T-state of the last field boundary (VSYNC end or forced).
    last_field: u64,
}

/// Without a VSYNC for this long (a field and a half) the picture is
/// blanked and a frame reported anyway (FAST mode shows nothing).
const FIELD_TIMEOUT: u64 = 3_250_000 / 50 * 3 / 2;

impl Zx81 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("zx81") => Model::Zx81,
            Some("ts1000") => Model::Ts1000,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let ram = match config.options.get("ram").map(String::as_str) {
            None | Some("16k") => 0x4000,
            Some("1k") => 0x400,
            Some("2k") => 0x800,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "ram option '{other}' (1k, 2k or 16k)"
                )))
            }
        };
        let traps = match config.options.get("traps").map(String::as_str) {
            None | Some("on") => true,
            Some("off") => false,
            Some(other) => return Err(ConfigError::Invalid(format!("traps option '{other}'"))),
        };
        let rom = match config.options.get("roms").map(String::as_str) {
            Some("stub") => stub::rom(),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => {
                let r = config
                    .roms
                    .get("zx81")
                    .ok_or_else(|| ConfigError::MissingRom("zx81".into()))?;
                if r.len() != ROM_SIZE {
                    return Err(ConfigError::Invalid(format!(
                        "zx81 ROM must be {ROM_SIZE} bytes, got {}",
                        r.len()
                    )));
                }
                r.clone()
            }
        };
        let mut m = Self {
            model,
            cpu: Z80::new(),
            mem: Memory::new(rom, ram),
            ula: Ula::new(),
            keys: [0; 8],
            sched: Scheduler::new(),
            input: InputQueue::new(),
            palette: [
                Rgb8 {
                    r: 0xf0,
                    g: 0xf0,
                    b: 0xf0,
                },
                Rgb8 { r: 0, g: 0, b: 0 },
            ],
            field_done_pending: false,
            traps,
            tape: PTape::default(),
            last_field: 0,
        };
        m.cpu.reset();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        BeamPos {
            frame: self.ula.fields,
            line: self.ula.line.min(u16::MAX as u32) as u16,
            cycle: self.ula.hcount as u16,
            hpos: (self.ula.hcount as u16 * 2).saturating_sub(ula::H_OFFSET as u16 * 2),
        }
    }

    /// The tape traps: LOAD copies the .P file in, SAVE writes it out;
    /// both continue at SLOW/FAST like the ROM's routines.
    fn trap(&mut self) -> bool {
        if !self.traps || self.tape.media.is_none() {
            return false;
        }
        match self.cpu.pc {
            ROM_LOAD => {
                let bytes = self.tape.bytes.clone();
                for (i, b) in bytes.iter().enumerate() {
                    self.mem.write(PROGRAM_START.wrapping_add(i as u16), *b);
                }
            }
            ROM_SAVE => {
                let end = u16::from_le_bytes([self.mem.read(E_LINE), self.mem.read(E_LINE + 1)]);
                if end > PROGRAM_START {
                    self.tape.bytes = (PROGRAM_START..end).map(|a| self.mem.read(a)).collect();
                    self.tape.dirty = true;
                }
            }
            _ => return false,
        }
        self.cpu.pc = ROM_SLOW_FAST;
        true
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        self.trap();
        // /INT is A6 of the refresh address: the R register's bit 6 as put
        // on the bus by this instruction's M1 cycle. The Z80 samples it at
        // the instruction's end, so it counts for the next instruction.
        let r_before = self.cpu.r;
        let start = self.sched.now().0;
        let mut chips_now = start;
        let beam = self.beam_pos();
        let (t, nmi, field) = {
            let mut bus = ZxBus {
                mem: &mut self.mem,
                ula: &mut self.ula,
                keys: &self.keys,
                chips_now: &mut chips_now,
                start,
                fetch: false,
                align: self.cpu.halted && self.cpu.nmi_pending(),
                i: self.cpu.i,
                halted: self.cpu.halted,
                nmi: false,
                field_done: false,
                sixty_hz: self.model == Model::Ts1000,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            let t = self.cpu.step_timed(&mut bus);
            (t, bus.nmi, bus.field_done)
        };
        let end = start + u64::from(t);
        let mut nmi = nmi;
        catch_up(&mut self.ula, &mut chips_now, end, &mut nmi);
        self.sched.advance(u64::from(t));
        self.cpu.set_int(r_before & 0x40 == 0);
        if nmi {
            self.cpu.set_nmi(true);
            self.cpu.set_nmi(false);
        }
        let entered = self.cpu.take_interrupt();
        let mut field = field;
        if field {
            self.last_field = end;
        } else if end - self.last_field > FIELD_TIMEOUT {
            self.ula.force_field();
            self.last_field = end;
            field = true;
        }
        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            let next_pc = u32::from(self.cpu.pc);
            if dbg.trace_instr_armed() {
                dbg.trace_instr(CPU, next_pc, self.beam_pos());
            }
            if dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.exec_armed(CPU_SPACE) {
                dbg.check_exec(CPU_SPACE, next_pc);
            }
            if dbg.raster_armed() {
                let b = self.beam_pos();
                dbg.check_raster(b.line, b.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                self.field_done_pending |= field;
                return Some(stop);
            }
        }
        field.then_some(StopReason::FrameDone)
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.key(machine_key.0, down);
            }
            (InputTarget::Panel, InputEvent::Control { id: _, value: 1 }) => {
                Machine::reset(self, ResetKind::Hard);
            }
            _ => {}
        }
    }

    fn key(&mut self, code: u16, down: bool) {
        let (row, col) = (usize::from(code / 5), code % 5);
        if let (Some(r), true) = (self.keys.get_mut(row), col < 5) {
            if down {
                *r |= 1 << col;
            } else {
                *r &= !(1 << col);
            }
        }
    }

    fn type_text(&mut self, text: &str) {
        let (events, _) = keyboard::typed_events(text, self.sched.now().0);
        for e in events {
            self.input.push(e);
        }
    }

    fn signal(&self) -> SignalInfo {
        let (standard, lines) = match self.model {
            Model::Zx81 => (VideoStandard::Pal, ula::LINES_PAL),
            Model::Ts1000 => (VideoStandard::Ntsc, 262),
        };
        SignalInfo {
            pixel_clock_hz: CPU_HZ * ula::PIXELS_PER_T,
            active_pixels: ula::WIDTH,
            total_pixels: ula::T_PER_LINE * ula::PIXELS_PER_T,
            lines_per_field: lines,
            standard,
            output: OutputType::Monochrome,
            field: FieldParity::Progressive,
        }
    }
}

/// Typing for tests and tools: queue key presses for `text` (SHIFT where
/// the character needs it; letters are the keys' keywords in K mode).
pub fn type_text(m: &mut Zx81, text: &str) {
    m.type_text(text);
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Zx81, addr: u16) -> u8 {
    m.mem.read(addr)
}

impl Machine for Zx81 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        if self.ula.line == 0 && self.ula.hcount == 0 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE && f.addr < IO_BASE {
                    for i in 0..f.size.bytes() {
                        self.mem
                            .write((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
                    }
                }
            }
        }
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.instruction(None);
        StopReason::StepDone
    }

    /// The ZX81 has no reset button: power off and on (RAM cleared).
    fn reset(&mut self, _kind: ResetKind) {
        self.cpu = Z80::new();
        self.cpu.reset();
        self.mem.clear();
        self.ula.reset();
        self.keys = [0; 8];
        self.input.clear();
        self.field_done_pending = false;
        self.last_field = self.sched.now().0;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: ula::WIDTH,
            height: ula::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.ula.pixels(),
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn gfx_views(&self) -> Vec<emu_core::GfxViewInfo> {
        self.views()
    }

    /// The ULA's picture window starts [`ula::V_OFFSET`] lines after
    /// VSYNC; [`BeamPos::hpos`] counts from its left edge.
    fn frame_row(&self, line: u16) -> Option<u32> {
        let row = u32::from(line).checked_sub(ula::V_OFFSET)?;
        (row < ula::HEIGHT).then_some(row)
    }

    /// See [`gfx`]: the character set and the display file.
    fn gfx_view(&self, id: &str, opts: &emu_core::GfxOptions) -> Option<emu_core::GfxView> {
        self.view(id, opts)
    }

    fn audio(&mut self, _out: &mut Vec<i16>) -> emu_core::AudioFormat {
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 17,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let u = &self.ula;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        vec![ChipInfo {
            name: "ula",
            regs: vec![
                reg("HCOUNT", u.hcount, 8),
                reg("LINE", u.line, 16),
                reg("LCNTR", u32::from(u.lcntr), 3),
                reg("NMI_ON", u32::from(u.nmi_on), 1),
                reg("VSYNC", u32::from(u.vsync), 1),
                reg("EAR", u32::from(u.ear), 1),
            ],
        }]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == CPU).then_some(self as &mut dyn DebugTarget)
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        vec![emu_core::ControlInfo::button(0, "POWER", false)]
    }

    fn ports(&self) -> Vec<PortInfo> {
        Vec::new()
    }

    fn connect(&mut self, port: PortId, _dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        Err(PortError::NoSuchPort(port))
    }

    fn disconnect(&mut self, _port: PortId) -> Option<Box<dyn PortDevice>> {
        None
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        vec![MediaSlotInfo {
            id: SLOT_TAPE,
            name: "tape",
            kind: MediaKind::Tape,
            inserted: self.tape.media.is_some(),
        }]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        (slot == SLOT_TAPE).then_some(&mut self.tape as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"ZX81", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.field_done_pending);
            w.raw(&self.keys);
            w.u64(self.last_field);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.ula.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"ZX81", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let field_done_pending = c.bool()?;
        let mut keys = [0u8; 8];
        c.raw_into(&mut keys)?;
        let last_field = c.u64()?;
        let mut cpu = self.cpu.clone();
        let mut mem = self.mem.clone();
        let mut ula = self.ula.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        mem.load(&mut r)?;
        ula.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        self.cpu = cpu;
        self.mem = mem;
        self.ula = ula;
        self.sched = sched;
        self.input = input;
        self.keys = keys;
        self.field_done_pending = field_done_pending;
        self.last_field = last_field;
        Ok(())
    }
}

impl DebugTarget for Zx81 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != CPU_SPACE {
            return None;
        }
        let bus = PeekBus(&self.mem, &self.ula, &self.keys);
        Some((0..size.bytes()).fold(0, |v, i| {
            v | bus.peek(addr.wrapping_add(i), Size::Byte) << (8 * i)
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE || addr >= IO_BASE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = (addr.wrapping_add(i) & 0xffff) as u16;
            if a & 0x4000 == 0 {
                return false;
            }
            self.mem.write(a, (v >> (8 * i)) as u8);
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu
            .disassemble(&PeekBus(&self.mem, &self.ula, &self.keys), addr)
    }
}

#[cfg(test)]
mod tests;
