//! Tom Harte's SingleStepTests (`65x02/6502/v1`, MIT): per opcode 10 000
//! random initial states with the expected final state and the exact
//! cycle-by-cycle bus trace.
//!
//! Data: `tools/fetch-testdata.sh`, then set `EMU_TESTDATA_DIR`.
//! By default every 10th test per opcode runs; `EMU_HARTE_FULL=1` runs all.
//! Run with `cargo test -p cpu-6502 --release -- --ignored harte`.

use std::path::PathBuf;

use cpu_6502::Cpu6502;
use emu_core::{Bus, Cpu, CycleResult, Size};
use serde::Deserialize;

#[derive(Deserialize)]
struct State {
    pc: u16,
    s: u8,
    a: u8,
    x: u8,
    y: u8,
    p: u8,
    ram: Vec<(u16, u8)>,
}

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    cycles: Vec<(u16, u8, String)>,
}

/// Flat 64 KiB RAM that logs every access.
struct RecordingBus {
    mem: Vec<u8>,
    log: Vec<(u16, u8, &'static str)>,
}

impl Bus for RecordingBus {
    fn read(&mut self, addr: u32, _: Size) -> u32 {
        let v = self.mem[addr as usize & 0xffff];
        self.log.push((addr as u16, v, "read"));
        u32::from(v)
    }

    fn peek(&self, addr: u32, _: Size) -> u32 {
        u32::from(self.mem[addr as usize & 0xffff])
    }

    fn write(&mut self, addr: u32, _: Size, v: u32) {
        self.mem[addr as usize & 0xffff] = v as u8;
        self.log.push((addr as u16, v as u8, "write"));
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

fn data_dir() -> PathBuf {
    let dir = std::env::var_os("EMU_TESTDATA_DIR")
        .expect("EMU_TESTDATA_DIR not set; run tools/fetch-testdata.sh first");
    PathBuf::from(dir).join("65x02").join("6502").join("v1")
}

/// Run one test; returns a description of the first mismatch.
fn run(test: &Test) -> Result<(), String> {
    let mut bus = RecordingBus {
        mem: vec![0; 0x10000],
        log: Vec::new(),
    };
    for &(addr, v) in &test.initial.ram {
        bus.mem[usize::from(addr)] = v;
    }
    let mut cpu = Cpu6502::default();
    let i = &test.initial;
    for (name, v) in [
        ("PC", u32::from(i.pc)),
        ("S", u32::from(i.s)),
        ("A", u32::from(i.a)),
    ] {
        cpu.set_reg(name, v);
    }
    for (name, v) in [("X", i.x), ("Y", i.y), ("P", i.p)] {
        cpu.set_reg(name, u32::from(v));
    }

    // A jammed CPU keeps returning `Jammed` while the JAM tests record its
    // bus activity, so only `InstrDone` ends an instruction early.
    let mut last = CycleResult::Busy;
    for n in 0..test.cycles.len() {
        last = cpu.tick(&mut bus);
        let finished = matches!(last, CycleResult::InstrDone { .. });
        if finished && n + 1 < test.cycles.len() {
            return Err(format!(
                "finished after {} of {} cycles",
                n + 1,
                test.cycles.len()
            ));
        }
    }
    if last == CycleResult::Busy {
        return Err(format!("not finished after {} cycles", test.cycles.len()));
    }

    let expected_log: Vec<(u16, u8, &str)> = test
        .cycles
        .iter()
        .map(|(a, v, k)| (*a, *v, k.as_str()))
        .collect();
    if bus.log != expected_log {
        return Err(format!(
            "bus trace\n  got      {:?}\n  expected {:?}",
            bus.log, expected_log
        ));
    }

    let regs = cpu.regs();
    let f = &test.expected;
    let expected_regs = [
        ("PC", u32::from(f.pc)),
        ("S", u32::from(f.s)),
        ("A", u32::from(f.a)),
        ("X", u32::from(f.x)),
        ("Y", u32::from(f.y)),
        ("P", u32::from(f.p)),
    ];
    for (name, want) in expected_regs {
        let got = regs.get(name).unwrap_or(u32::MAX);
        // B (bit 4) has no storage in the real P register; only the JAM test
        // files carry it in their states.
        let ignore = if name == "P" { 0x10 } else { 0 };
        if (got ^ want) & !ignore != 0 {
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
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh)"]
fn harte_6502_all_opcodes() {
    let dir = data_dir();
    let stride = if std::env::var_os("EMU_HARTE_FULL").is_some() {
        1
    } else {
        10
    };
    let mut failures = Vec::new();
    let mut total = 0usize;

    for opcode in 0..=255u8 {
        let path = dir.join(format!("{opcode:02x}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let tests: Vec<Test> = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()));
        let mut failed_here = 0;
        for test in tests.iter().step_by(stride) {
            total += 1;
            if let Err(why) = run(test) {
                failed_here += 1;
                // Report only the first failure per opcode to keep output readable.
                if failed_here == 1 {
                    failures.push(format!("{:02x} [{}]: {why}", opcode, test.name));
                }
            }
        }
        if failed_here > 1 {
            failures.push(format!(
                "{opcode:02x}: {failed_here} failing tests in total"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {total} checks failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
