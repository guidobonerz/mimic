//! Tom Harte's SingleStepTests for the SPC700 (`spc700/v1`, MIT): per
//! opcode 1000 random states with the final state and every bus cycle
//! (reads with their address, also the dummy reads whose value the test
//! does not give; writes; "wait" for internal cycles).
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-spc700 --release --test harte -- --ignored --nocapture
//!
//! `EMU_HARTE_ONLY=e8,3f` only those opcodes.

use std::path::PathBuf;

use cpu_spc700::{Spc700, SpcBus};
use serde::Deserialize;

#[derive(Deserialize)]
struct State {
    pc: u16,
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    psw: u8,
    ram: Vec<(u16, u8)>,
}

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    cycles: Vec<(Option<u16>, Option<u8>, String)>,
}

struct Ram {
    mem: Vec<u8>,
    log: Vec<(Option<u16>, Option<u8>, char)>,
}

impl SpcBus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        let v = self.mem[usize::from(addr)];
        self.log.push((Some(addr), Some(v), 'r'));
        v
    }
    fn write(&mut self, addr: u16, v: u8) {
        self.mem[usize::from(addr)] = v;
        self.log.push((Some(addr), Some(v), 'w'));
    }
    fn idle(&mut self) {
        self.log.push((None, None, 'i'));
    }
    fn peek(&self, addr: u16) -> u8 {
        self.mem[usize::from(addr)]
    }
}

fn run(t: &Test) -> Result<(), String> {
    let i = &t.initial;
    let mut cpu = Spc700::new();
    cpu.pc = i.pc;
    cpu.a = i.a;
    cpu.x = i.x;
    cpu.y = i.y;
    cpu.sp = i.sp;
    cpu.psw = i.psw;
    let mut ram = Ram {
        mem: vec![0; 0x10000],
        log: Vec::new(),
    };
    for &(a, v) in &i.ram {
        ram.mem[usize::from(a)] = v;
    }
    cpu.step(&mut ram);
    // SLEEP and STOP: the test records cycles after the instruction too.
    let n = t.cycles.len();
    while ram.log.len() < n && cpu.halted {
        cpu.step(&mut ram);
    }
    let f = &t.expected;
    for (name, g, w) in [
        ("pc", u32::from(cpu.pc), u32::from(f.pc)),
        ("a", u32::from(cpu.a), u32::from(f.a)),
        ("x", u32::from(cpu.x), u32::from(f.x)),
        ("y", u32::from(cpu.y), u32::from(f.y)),
        ("sp", u32::from(cpu.sp), u32::from(f.sp)),
        ("psw", u32::from(cpu.psw), u32::from(f.psw)),
    ] {
        if g != w {
            return Err(format!("{name}: got {g:#x}, expected {w:#x}"));
        }
    }
    for &(a, v) in &f.ram {
        let g = ram.mem[usize::from(a)];
        if g != v {
            return Err(format!("ram[{a:04x}]: got {g:02x}, expected {v:02x}"));
        }
    }
    let expected: Vec<(Option<u16>, Option<u8>, char)> = t
        .cycles
        .iter()
        .map(|(a, v, k)| match k.as_str() {
            "wait" => (None, None, 'i'),
            "write" => (*a, *v, 'w'),
            _ => (*a, *v, 'r'),
        })
        .collect();
    let same = ram.log.len() == expected.len()
        && ram
            .log
            .iter()
            .zip(&expected)
            .all(|(g, e)| g.0 == e.0 && g.2 == e.2 && (e.1.is_none() || g.1 == e.1));
    if !same {
        let fmt = |l: &[(Option<u16>, Option<u8>, char)]| {
            l.iter()
                .map(|(a, v, k)| match (a, v) {
                    (Some(a), Some(v)) => format!("{k}{a:04x}={v:02x}"),
                    (Some(a), None) => format!("{k}{a:04x}"),
                    _ => k.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        return Err(format!(
            "cycles\n    got      {}\n    expected {}",
            fmt(&ram.log),
            fmt(&expected)
        ));
    }
    Ok(())
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh)"]
fn harte_spc700() {
    let dir = PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
        .join("spc700")
        .join("v1");
    let only: Vec<String> = std::env::var("EMU_HARTE_ONLY")
        .map(|s| s.split(',').map(|x| x.trim().to_lowercase()).collect())
        .unwrap_or_default();
    let mut failures = Vec::new();
    let mut total = 0;
    for op in 0..=255u8 {
        if !only.is_empty() && !only.iter().any(|o| *o == format!("{op:02x}")) {
            continue;
        }
        let path = dir.join(format!("{op:02x}.json"));
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let tests: Vec<Test> = serde_json::from_str(&text).unwrap();
        let mut failed = 0;
        for t in &tests {
            total += 1;
            if let Err(why) = run(t) {
                failed += 1;
                if failed == 1 {
                    failures.push(format!("{op:02x} [{}]: {why}", t.name));
                }
            }
        }
        if failed > 1 {
            failures.push(format!("{op:02x}: {failed} failing"));
        }
    }
    for f in &failures {
        eprintln!("{f}");
    }
    eprintln!("{total} tests, {} failure lines", failures.len());
    assert!(failures.is_empty());
}
