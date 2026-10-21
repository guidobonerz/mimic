//! Tom Harte's SingleStepTests for the 68000 (`680x0/68000/v1`, MIT): per
//! instruction 8000+ random initial states with the expected final state
//! and every bus transaction (reads, writes, TAS cycles, internal clocks).
//!
//! Checked: D0–D7, A0–A6, USP, SSP, SR, PC, the prefetch queue, RAM, the
//! clock count, and the transactions in order with their length, function
//! code, address, size and value.
//!
//! Data: `tools/fetch-testdata.sh`, then set `EMU_TESTDATA_DIR`.
//! By default every 10th test per file runs; `EMU_HARTE_FULL=1` runs all;
//! `EMU_HARTE_ONLY=<prefix>` limits the run to files starting with it
//! (e.g. `MOVE`). Run with
//! `cargo test -p cpu-m68k --release --test harte -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use cpu_m68k::{Access, Step, TimedBus68, M68000};
use emu_core::{Bus, Size};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct State {
    d0: u32,
    d1: u32,
    d2: u32,
    d3: u32,
    d4: u32,
    d5: u32,
    d6: u32,
    d7: u32,
    a0: u32,
    a1: u32,
    a2: u32,
    a3: u32,
    a4: u32,
    a5: u32,
    a6: u32,
    usp: u32,
    ssp: u32,
    sr: u16,
    pc: u32,
    prefetch: [u16; 2],
    ram: Vec<(u32, u8)>,
}

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    length: u32,
    transactions: Vec<Value>,
}

/// A transaction: an idle stretch, or a bus cycle with its kind ("r", "w",
/// "t" for TAS), length, function code, address, size and value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tx {
    Idle(u32),
    Bus {
        kind: &'static str,
        clocks: u32,
        fc: u8,
        addr: u32,
        word: bool,
        value: u32,
    },
}

#[derive(Clone, Copy)]
struct Pending {
    access: Access,
    fc: u8,
    addr: u32,
    size: Size,
    t: u32,
}

struct RecordingBus {
    mem: BTreeMap<u32, u8>,
    pending: Option<Pending>,
    /// TAS: the read is logged, its write half is not.
    rmw_write: bool,
    /// Bus cycles with their start clock.
    log: Vec<(u32, Tx)>,
}

impl RecordingBus {
    fn byte(&self, a: u32) -> u8 {
        self.mem.get(&(a & 0xff_ffff)).copied().unwrap_or(0)
    }

    fn record(&mut self, value: u32) {
        let Some(p) = self.pending.take() else {
            return;
        };
        let (kind, clocks) = match p.access {
            Access::ReadModifyWrite => ("t", 10),
            Access::Write => ("w", 4),
            Access::IntAck => ("i", 4),
            Access::Read => ("r", 4),
        };
        self.log.push((
            p.t,
            Tx::Bus {
                kind,
                clocks,
                fc: p.fc,
                addr: p.addr,
                word: p.size == Size::Word,
                value,
            },
        ));
    }
}

impl TimedBus68 for RecordingBus {
    fn bus_cycle(&mut self, access: Access, fc: u8, addr: u32, size: Size, t: u32) -> u32 {
        self.pending = Some(Pending {
            access,
            fc,
            addr,
            size,
            t,
        });
        0
    }
}

impl Bus for RecordingBus {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = self.peek(addr, size);
        if self
            .pending
            .is_some_and(|p| p.access == Access::ReadModifyWrite)
        {
            // TAS: logged with the value it writes.
            self.rmw_write = true;
        } else {
            self.record(v);
        }
        v
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        match size {
            Size::Byte => u32::from(self.byte(addr)),
            _ => u32::from(self.byte(addr)) << 8 | u32::from(self.byte(addr + 1)),
        }
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        self.rmw_write = false;
        self.record(v & if size == Size::Byte { 0xff } else { 0xffff });
        let a = addr & 0xff_ffff;
        match size {
            Size::Byte => {
                self.mem.insert(a, v as u8);
            }
            _ => {
                self.mem.insert(a, (v >> 8) as u8);
                self.mem.insert((a + 1) & 0xff_ffff, v as u8);
            }
        }
    }

    fn addr_bits(&self) -> u8 {
        24
    }
}

fn parse_tx(v: &Value) -> Tx {
    let a = v.as_array().expect("transaction array");
    let kind = a[0].as_str().unwrap_or_default();
    let clocks = a[1].as_u64().unwrap_or(0) as u32;
    if kind == "n" {
        return Tx::Idle(clocks);
    }
    Tx::Bus {
        kind: match kind {
            "r" => "r",
            "w" => "w",
            "t" => "t",
            _ => "?",
        },
        clocks,
        fc: a[2].as_u64().unwrap_or(0) as u8,
        addr: a[3].as_u64().unwrap_or(0) as u32,
        word: a[4].as_str() == Some(".w"),
        value: a[5].as_u64().unwrap_or(0) as u32,
    }
}

/// Merge neighbouring idle entries and drop empty ones.
fn normalize(list: Vec<Tx>) -> Vec<Tx> {
    let mut out: Vec<Tx> = Vec::new();
    for t in list {
        match (out.last_mut(), &t) {
            (_, Tx::Idle(0)) => {}
            (Some(Tx::Idle(a)), Tx::Idle(b)) => *a += b,
            _ => out.push(t),
        }
    }
    out
}

fn set_state(cpu: &mut M68000, s: &State) {
    cpu.d = [s.d0, s.d1, s.d2, s.d3, s.d4, s.d5, s.d6, s.d7];
    cpu.a[..7].copy_from_slice(&[s.a0, s.a1, s.a2, s.a3, s.a4, s.a5, s.a6]);
    cpu.set_sr(s.sr);
    cpu.set_usp(s.usp);
    cpu.set_ssp(s.ssp);
    cpu.set_pc_and_queue(s.pc, s.prefetch);
}

fn regs(cpu: &M68000) -> Vec<(String, u32)> {
    let mut v = Vec::new();
    for i in 0..8 {
        v.push((format!("d{i}"), cpu.d[i]));
    }
    for i in 0..7 {
        v.push((format!("a{i}"), cpu.a[i]));
    }
    v.push(("usp".into(), cpu.usp()));
    v.push(("ssp".into(), cpu.ssp()));
    v.push(("sr".into(), u32::from(cpu.sr())));
    v.push(("pc".into(), cpu.pc()));
    let q = cpu.prefetch_queue();
    v.push(("ird".into(), u32::from(q[0])));
    v.push(("irc".into(), u32::from(q[1])));
    v
}

fn expected_regs(s: &State) -> Vec<(String, u32)> {
    let mut v = Vec::new();
    for (i, d) in [s.d0, s.d1, s.d2, s.d3, s.d4, s.d5, s.d6, s.d7]
        .iter()
        .enumerate()
    {
        v.push((format!("d{i}"), *d));
    }
    for (i, a) in [s.a0, s.a1, s.a2, s.a3, s.a4, s.a5, s.a6]
        .iter()
        .enumerate()
    {
        v.push((format!("a{i}"), *a));
    }
    v.push(("usp".into(), s.usp));
    v.push(("ssp".into(), s.ssp));
    v.push(("sr".into(), u32::from(s.sr)));
    v.push(("pc".into(), s.pc & 0xff_ffff));
    v.push(("ird".into(), u32::from(s.prefetch[0])));
    v.push(("irc".into(), u32::from(s.prefetch[1])));
    v
}

fn run(test: &Test) -> Result<(), String> {
    let mut bus = RecordingBus {
        mem: test.initial.ram.iter().copied().collect(),
        pending: None,
        rmw_write: false,
        log: Vec::new(),
    };
    let mut cpu = M68000::new();
    set_state(&mut cpu, &test.initial);
    let clocks = match cpu.step_timed(&mut bus) {
        Step::Done { clocks } | Step::Stopped { clocks } => clocks,
        Step::Halted => return Err("halted".into()),
    };
    let mut errors = Vec::new();
    for ((name, got), (_, want)) in regs(&cpu).iter().zip(expected_regs(&test.expected)) {
        if *got != want {
            errors.push(format!("{name} {got:#x} want {want:#x}"));
        }
    }
    for &(addr, want) in &test.expected.ram {
        let got = bus.byte(addr);
        if got != want {
            errors.push(format!("ram[{addr:#x}] {got:#04x} want {want:#04x}"));
        }
    }
    if clocks != test.length {
        errors.push(format!("{clocks} clocks, want {}", test.length));
    }
    let mut got = Vec::new();
    let mut at = 0;
    for (t, tx) in &bus.log {
        got.push(Tx::Idle(t.saturating_sub(at)));
        if let Tx::Bus { clocks, .. } = tx {
            at = t + clocks;
        }
        got.push(tx.clone());
    }
    got.push(Tx::Idle(clocks.saturating_sub(at)));
    let got = normalize(got);
    let want = normalize(test.transactions.iter().map(parse_tx).collect());
    if got != want {
        errors.push(format!("transactions\n    got  {got:?}\n    want {want:?}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Tests whose expected state contradicts the instruction (corrupt data):
/// skipped.
const BROKEN: &[&str] = &[
    // ASL.B #1,D2 on $7D must give $FA with C clear; the data expects
    // D2 = $6461D390 and C set.
    "e502 [ASL.b Q, D2] 1761",
    // ASL.B changes only the low byte; the data changes all of D2.
    "e502 [ASL.b Q, D2] 1583",
    // The only divide-by-zero case in the data: it stacks the address of
    // the DIVU itself, the manual (M68000PRM 6.3.9) the next instruction's.
    "80ef [DIVU (d16, A7), D0] 5745",
];

fn load(path: &PathBuf) -> Vec<Test> {
    let mut gz = flate2::read::GzDecoder::new(std::fs::File::open(path).expect("test file"));
    let mut text = String::new();
    gz.read_to_string(&mut text).expect("gzip");
    serde_json::from_str(&text).expect("json")
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh)"]
fn harte_68000() {
    let dir =
        PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR not set"))
            .join("680x0/68000/v1");
    let full = std::env::var_os("EMU_HARTE_FULL").is_some();
    let only = std::env::var("EMU_HARTE_ONLY").unwrap_or_default();
    let show: usize = std::env::var("EMU_HARTE_SHOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("test directory")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.to_string_lossy().ends_with(".json.gz"))
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| only.split(',').any(|o| n.to_string_lossy().starts_with(o)))
        })
        .collect();
    files.sort();
    let (mut total, mut failed) = (0usize, 0usize);
    let mut bad_files = Vec::new();
    for path in &files {
        let tests = load(path);
        let (mut n, mut f, mut shown) = (0, 0, 0);
        for (i, t) in tests.iter().enumerate() {
            if (!full && i % 10 != 0) || BROKEN.contains(&t.name.as_str()) {
                continue;
            }
            n += 1;
            if let Err(e) = run(t) {
                f += 1;
                if shown < show {
                    eprintln!("  {}: {e}", t.name);
                    shown += 1;
                }
            }
        }
        let name = path.file_name().unwrap().to_string_lossy();
        eprintln!("{name}: {}/{n}", n - f);
        if f > 0 {
            bad_files.push(format!("{name} {f}"));
        }
        total += n;
        failed += f;
    }
    eprintln!(
        "total {}/{total}; failing files: {bad_files:?}",
        total - failed
    );
    assert_eq!(failed, 0, "{failed} of {total} tests failed");
}
