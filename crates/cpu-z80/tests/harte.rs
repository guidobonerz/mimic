//! Tom Harte's SingleStepTests for the Z80 (`z80/v1`, MIT): per instruction
//! 1000 random initial states with the expected final state, the bus states
//! of every T-state and the I/O transactions.
//!
//! Checked: every register including MEMPTR (`wz`), Q, P and the EI flag,
//! RAM, the T-state count, the order and kind of the memory and I/O
//! accesses, the T-state each of them happens on (the M-cycle placement of
//! [`Z80::step_timed`]) and the bytes written to ports. With
//! [`Z80::idle_cycles`] on, every internal T-state must be reported once,
//! with the address the trace shows on the bus then.
//!
//! Data: `tools/fetch-testdata.sh`, then set `EMU_TESTDATA_DIR`.
//! By default every 10th test per file runs; `EMU_HARTE_FULL=1` runs all;
//! `EMU_HARTE_ONLY=<prefix>` limits the run to files starting with it
//! (e.g. `ed`).
//! Run with `cargo test -p cpu-z80 --release -- --ignored harte`.

use std::path::PathBuf;

use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use emu_core::{Bus, Size};
use serde::Deserialize;

#[derive(Deserialize)]
struct State {
    pc: u16,
    sp: u16,
    a: u8,
    b: u8,
    c: u8,
    d: u8,
    e: u8,
    f: u8,
    h: u8,
    l: u8,
    i: u8,
    r: u8,
    ei: u8,
    wz: u16,
    ix: u16,
    iy: u16,
    af_: u16,
    bc_: u16,
    de_: u16,
    hl_: u16,
    im: u8,
    p: u8,
    q: u8,
    iff1: u8,
    iff2: u8,
    ram: Vec<(u16, u8)>,
}

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    cycles: Vec<(Option<u16>, Option<u8>, String)>,
    #[serde(default)]
    ports: Vec<(u16, u8, String)>,
}

/// One bus access: address, kind ("mr", "mw", "ir", "iw") and T-state.
type Logged = (u16, &'static str, u32);

/// Flat 64 KiB RAM plus scripted port reads; logs every access.
struct RecordingBus {
    mem: Vec<u8>,
    port_reads: Vec<(u16, u8)>,
    port_writes: Vec<(u16, u8)>,
    log: Vec<Logged>,
    /// T-state of the access announced by `wait_states`.
    t: u32,
    /// T-states of the M-cycles announced so far.
    access_t: u32,
    /// Internal T-states reported: (T-state, address).
    idle: Vec<(u32, u16)>,
}

impl TimedBus for RecordingBus {
    fn wait_states(&mut self, access: Access, t: u32) -> u32 {
        self.t = t;
        self.access_t += match access {
            Access::Fetch | Access::IoRead | Access::IoWrite => 4,
            Access::Read | Access::Write => 3,
            Access::IntAck => 7,
        };
        0
    }

    fn idle(&mut self, t: u32, addr: u16) -> u32 {
        self.idle.push((t, addr));
        0
    }
}

impl Bus for RecordingBus {
    fn read(&mut self, addr: u32, _: Size) -> u32 {
        if addr >= IO_BASE {
            let port = (addr - IO_BASE) as u16;
            self.log.push((port, "ir", self.t));
            let i = self.port_reads.iter().position(|&(p, _)| p == port);
            return u32::from(i.map_or(0xff, |i| self.port_reads.remove(i).1));
        }
        self.log.push((addr as u16, "mr", self.t));
        u32::from(self.mem[addr as usize & 0xffff])
    }

    fn peek(&self, addr: u32, _: Size) -> u32 {
        u32::from(self.mem[addr as usize & 0xffff])
    }

    fn write(&mut self, addr: u32, _: Size, v: u32) {
        if addr >= IO_BASE {
            let port = (addr - IO_BASE) as u16;
            self.log.push((port, "iw", self.t));
            self.port_writes.push((port, v as u8));
            return;
        }
        self.mem[addr as usize & 0xffff] = v as u8;
        self.log.push((addr as u16, "mw", self.t));
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

fn data_dir() -> PathBuf {
    let dir = std::env::var_os("EMU_TESTDATA_DIR")
        .expect("EMU_TESTDATA_DIR not set; run tools/fetch-testdata.sh first");
    PathBuf::from(dir).join("z80").join("v1")
}

fn set_state(cpu: &mut Z80, s: &State) {
    cpu.pc = s.pc;
    cpu.sp = s.sp;
    cpu.a = s.a;
    cpu.f = s.f;
    cpu.b = s.b;
    cpu.c = s.c;
    cpu.d = s.d;
    cpu.e = s.e;
    cpu.h = s.h;
    cpu.l = s.l;
    cpu.i = s.i;
    cpu.r = s.r;
    cpu.ei = s.ei != 0;
    cpu.wz = s.wz;
    cpu.ix = s.ix;
    cpu.iy = s.iy;
    cpu.af_ = s.af_;
    cpu.bc_ = s.bc_;
    cpu.de_ = s.de_;
    cpu.hl_ = s.hl_;
    cpu.im = s.im;
    cpu.p = s.p != 0;
    cpu.q = s.q;
    cpu.iff1 = s.iff1 != 0;
    cpu.iff2 = s.iff2 != 0;
}

/// Run one test; returns a description of the first mismatch.
fn run(test: &Test) -> Result<(), String> {
    let mut bus = RecordingBus {
        mem: vec![0; 0x10000],
        port_reads: test
            .ports
            .iter()
            .filter(|(_, _, k)| k == "r")
            .map(|&(p, v, _)| (p, v))
            .collect(),
        port_writes: Vec::new(),
        log: Vec::new(),
        t: 0,
        access_t: 0,
        idle: Vec::new(),
    };
    for &(addr, v) in &test.initial.ram {
        bus.mem[usize::from(addr)] = v;
    }
    let mut cpu = Z80::new();
    set_state(&mut cpu, &test.initial);
    cpu.idle_cycles = true;

    let t = cpu.step_timed(&mut bus);
    if t as usize != test.cycles.len() {
        return Err(format!(
            "instruction took {t} of {} T-states",
            test.cycles.len()
        ));
    }

    if bus.access_t + bus.idle.len() as u32 != t {
        return Err(format!(
            "{} T-states in M-cycles and {} internal ones reported of {t}",
            bus.access_t,
            bus.idle.len()
        ));
    }
    for &(at, addr) in &bus.idle {
        let want = test.cycles.get(at as usize).and_then(|c| c.0);
        if want != Some(addr) {
            return Err(format!(
                "internal T-state {at} with address {addr:#06x}, the trace has {want:04x?}"
            ));
        }
    }

    let expected_log: Vec<Logged> = test
        .cycles
        .iter()
        .enumerate()
        .filter_map(|(i, (addr, _, pins))| {
            let p = pins.as_bytes();
            let kind = match (p[0], p[1], p[2], p[3]) {
                (b'r', _, b'm', _) => "mr",
                (_, b'w', b'm', _) => "mw",
                (b'r', _, _, b'i') => "ir",
                (_, b'w', _, b'i') => "iw",
                _ => return None,
            };
            Some((addr.unwrap_or(0), kind, i as u32))
        })
        .collect();
    if bus.log != expected_log {
        return Err(format!(
            "bus accesses\n  got      {:?}\n  expected {:?}",
            bus.log, expected_log
        ));
    }
    let expected_writes: Vec<(u16, u8)> = test
        .ports
        .iter()
        .filter(|(_, _, k)| k == "w")
        .map(|&(p, v, _)| (p, v))
        .collect();
    if bus.port_writes != expected_writes {
        return Err(format!(
            "port writes {:?}, expected {:?}",
            bus.port_writes, expected_writes
        ));
    }

    let f = &test.expected;
    let checks: [(&str, u32, u32); 25] = [
        ("PC", cpu.pc.into(), f.pc.into()),
        ("SP", cpu.sp.into(), f.sp.into()),
        ("A", cpu.a.into(), f.a.into()),
        ("F", cpu.f.into(), f.f.into()),
        ("B", cpu.b.into(), f.b.into()),
        ("C", cpu.c.into(), f.c.into()),
        ("D", cpu.d.into(), f.d.into()),
        ("E", cpu.e.into(), f.e.into()),
        ("H", cpu.h.into(), f.h.into()),
        ("L", cpu.l.into(), f.l.into()),
        ("I", cpu.i.into(), f.i.into()),
        ("R", cpu.r.into(), f.r.into()),
        ("IX", cpu.ix.into(), f.ix.into()),
        ("IY", cpu.iy.into(), f.iy.into()),
        ("AF'", cpu.af_.into(), f.af_.into()),
        ("BC'", cpu.bc_.into(), f.bc_.into()),
        ("DE'", cpu.de_.into(), f.de_.into()),
        ("HL'", cpu.hl_.into(), f.hl_.into()),
        ("WZ", cpu.wz.into(), f.wz.into()),
        ("IM", cpu.im.into(), f.im.into()),
        ("IFF1", u32::from(cpu.iff1), f.iff1.into()),
        ("IFF2", u32::from(cpu.iff2), f.iff2.into()),
        ("EI", u32::from(cpu.ei), f.ei.into()),
        ("P", u32::from(cpu.p), f.p.into()),
        ("Q", cpu.q.into(), f.q.into()),
    ];
    for (name, got, want) in checks {
        if got != want {
            return Err(format!("{name}: got {got:#x}, expected {want:#x}"));
        }
    }
    for &(addr, want) in &f.ram {
        let got = bus.mem[usize::from(addr)];
        if got != want {
            return Err(format!(
                "RAM[{addr:#06x}]: got {got:#04x}, expected {want:#04x}"
            ));
        }
    }
    Ok(())
}

#[test]
#[ignore = "needs Harte Z80 test data (tools/fetch-testdata.sh) in EMU_TESTDATA_DIR"]
fn harte_z80() {
    let full = std::env::var_os("EMU_HARTE_FULL").is_some_and(|v| v == "1");
    let step = if full { 1 } else { 10 };
    let mut files: Vec<PathBuf> = std::fs::read_dir(data_dir())
        .expect("test data directory")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    match std::env::var("EMU_HARTE_ONLY") {
        Ok(prefix) => files.retain(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
        }),
        Err(_) => assert!(files.len() > 1500, "only {} test files", files.len()),
    }
    let mut failures = Vec::new();
    let mut count = 0;
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let tests: Vec<Test> = serde_json::from_str(&text).unwrap();
        for test in tests.iter().step_by(step) {
            count += 1;
            if let Err(e) = run(test) {
                failures.push(format!("{}: {e}", test.name));
                break;
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files fail ({count} tests run):\n{}",
        failures.len(),
        files.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
