//! Tom Harte's SingleStepTests for the 65816 (`65816/v1`, MIT): per opcode
//! 10 000 random states in native (`xx.n.json`) and 10 000 in emulation
//! mode (`xx.e.json`), with the final state and every bus cycle (address,
//! value or none for internal cycles, read/write).
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-65816 --release --test harte -- --ignored --nocapture
//!
//! Every 10th test by default; `EMU_HARTE_FULL=1` runs all,
//! `EMU_HARTE_ONLY=a9,ea` only those opcodes.

use std::path::PathBuf;

use cpu_65816::{Bus816, Cpu65816};
use serde::Deserialize;

#[derive(Deserialize)]
struct State {
    pc: u16,
    s: u16,
    p: u8,
    a: u16,
    x: u16,
    y: u16,
    dbr: u8,
    d: u16,
    pbr: u8,
    e: u8,
    ram: Vec<(u32, u8)>,
}

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    cycles: Vec<(Option<u32>, Option<u8>, String)>,
}

struct Ram {
    mem: std::collections::HashMap<u32, u8>,
    log: Vec<(u32, Option<u8>, char)>,
}

impl Bus816 for Ram {
    fn read(&mut self, addr: u32) -> u8 {
        let v = self.mem.get(&addr).copied().unwrap_or(0);
        self.log.push((addr, Some(v), 'r'));
        v
    }
    fn write(&mut self, addr: u32, v: u8) {
        self.mem.insert(addr, v);
        self.log.push((addr, Some(v), 'w'));
    }
    fn idle(&mut self, addr: u32) {
        self.log.push((addr, None, 'i'));
    }
    fn peek(&self, addr: u32) -> u8 {
        self.mem.get(&addr).copied().unwrap_or(0)
    }
}

fn run(t: &Test, check_io_addr: bool) -> Result<(), String> {
    let i = &t.initial;
    let mut cpu = Cpu65816::new();
    cpu.pc = i.pc;
    cpu.s = i.s;
    cpu.p = i.p;
    cpu.a = i.a;
    cpu.x = i.x;
    cpu.y = i.y;
    cpu.dbr = i.dbr;
    cpu.d = i.d;
    cpu.pbr = i.pbr;
    cpu.e = i.e != 0;
    let mut ram = Ram {
        mem: i.ram.iter().copied().collect(),
        log: Vec::new(),
    };
    cpu.step(&mut ram);
    // The block moves' tests cover the whole move.
    let op = i
        .ram
        .iter()
        .find(|(a, _)| *a == (u32::from(i.pbr) << 16 | u32::from(i.pc)))
        .map(|r| r.1);
    if matches!(op, Some(0x44 | 0x54)) {
        while cpu.pc == i.pc && ram.log.len() < 20_000 {
            cpu.step(&mut ram);
        }
    }
    let f = &t.expected;
    let got = [
        ("pc", u32::from(cpu.pc)),
        ("s", u32::from(cpu.s)),
        ("p", u32::from(cpu.p)),
        ("a", u32::from(cpu.a)),
        ("x", u32::from(cpu.x)),
        ("y", u32::from(cpu.y)),
        ("dbr", u32::from(cpu.dbr)),
        ("d", u32::from(cpu.d)),
        ("pbr", u32::from(cpu.pbr)),
        ("e", u32::from(cpu.e)),
    ];
    let want = [
        f.pc as u32,
        u32::from(f.s),
        u32::from(f.p),
        u32::from(f.a),
        u32::from(f.x),
        u32::from(f.y),
        u32::from(f.dbr),
        u32::from(f.d),
        u32::from(f.pbr),
        u32::from(f.e),
    ];
    for ((name, g), w) in got.iter().zip(want) {
        if *g != w {
            return Err(format!("{name}: got {g:#x}, expected {w:#x}"));
        }
    }
    for &(addr, v) in &f.ram {
        let g = ram.mem.get(&addr).copied().unwrap_or(0);
        if g != v {
            return Err(format!("ram[{addr:06x}]: got {g:02x}, expected {v:02x}"));
        }
    }
    let expected: Vec<(u32, Option<u8>, char)> = t
        .cycles
        .iter()
        // WAI's wait state has no address.
        .filter_map(|(a, v, flags)| {
            let kind = match v {
                None => 'i',
                Some(_) => flags.as_bytes()[3] as char,
            };
            Some(((*a)?, *v, kind))
        })
        .collect();
    let norm = |log: &[(u32, Option<u8>, char)]| -> Vec<(u32, Option<u8>, char)> {
        log.iter()
            .map(|&(a, v, k)| (if k == 'i' && !check_io_addr { 0 } else { a }, v, k))
            .collect()
    };
    if norm(&ram.log) != norm(&expected) {
        let fmt = |l: &[(u32, Option<u8>, char)]| {
            l.iter()
                .map(|(a, v, k)| match v {
                    Some(v) => format!("{k}{a:06x}={v:02x}"),
                    None => format!("{k}{a:06x}"),
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
fn harte_65816() {
    let dir = PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
        .join("65816")
        .join("v1");
    let stride = if std::env::var_os("EMU_HARTE_FULL").is_some() {
        1
    } else {
        10
    };
    let only: Vec<String> = std::env::var("EMU_HARTE_ONLY")
        .map(|s| s.split(',').map(|x| x.trim().to_lowercase()).collect())
        .unwrap_or_default();
    let check_io = std::env::var("EMU_HARTE_IO").map_or(true, |v| v != "0");
    let mut failures = Vec::new();
    let mut total = 0;
    for op in 0..=255u8 {
        if !only.is_empty() && !only.iter().any(|o| *o == format!("{op:02x}")) {
            continue;
        }
        for mode in ["n", "e"] {
            let path = dir.join(format!("{op:02x}.{mode}.json"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let tests: Vec<Test> = serde_json::from_str(&text).unwrap();
            let mut failed = 0;
            for t in tests.iter().step_by(stride) {
                total += 1;
                if let Err(why) = run(t, check_io) {
                    failed += 1;
                    if failed == 1 {
                        failures.push(format!("{op:02x}.{mode} [{}]: {why}", t.name));
                    }
                }
            }
            if failed > 1 {
                failures.push(format!("{op:02x}.{mode}: {failed} failing"));
            }
        }
    }
    for f in &failures {
        eprintln!("{f}");
    }
    eprintln!("{total} tests, {} failure lines", failures.len());
    assert!(
        failures.is_empty(),
        "{} failure lines of {total} tests",
        failures.len()
    );
}
