//! The CP/M 8080 test programs (`tools/fetch-testdata.sh` puts them into
//! `testdata/8080`): TST8080 (Microcosm), 8080PRE, 8080EXM (Ian Bartholomew,
//! after Frank Cringle's exerciser), CPUTEST (Supersoft). They run at $0100
//! with BDOS calls 2 and 9 at address 5 and print their verdict.
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-8080 --release --test cpm -- --ignored --nocapture

use cpu_8080::{StatusBus, I8080};
use emu_core::{Bus, Size};

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u32, _s: Size) -> u32 {
        u32::from(self.0.get(addr as usize).copied().unwrap_or(0xff))
    }
    fn peek(&self, addr: u32, _s: Size) -> u32 {
        u32::from(self.0.get(addr as usize).copied().unwrap_or(0xff))
    }
    fn write(&mut self, addr: u32, _s: Size, v: u32) {
        if let Some(b) = self.0.get_mut(addr as usize) {
            *b = v as u8;
        }
    }
    fn addr_bits(&self) -> u8 {
        17
    }
}

impl StatusBus for Ram {
    fn cycle(&mut self, _status: u8, _addr: u16, _data: u8, _t: u32) {}
}

/// Run a .COM file; returns its output and the T-states.
fn run(name: &str) -> Option<(String, u64)> {
    let dir = std::env::var("EMU_TESTDATA_DIR").ok()?;
    let code = std::fs::read(format!("{dir}/8080/{name}")).expect("fetch-testdata.sh");
    let mut ram = vec![0; 0x10000];
    ram[0x100..0x100 + code.len()].copy_from_slice(&code);
    ram[5] = 0xc9; // RET after the BDOS call
    let mut bus = Ram(ram);
    let mut cpu = I8080::new();
    cpu.pc = 0x100;
    let mut out = String::new();
    let mut t = 0u64;
    loop {
        if cpu.pc == 5 {
            t += 10; // the reference counts an OUT at the BDOS entry
            match cpu.c {
                2 => out.push(char::from(cpu.e)),
                9 => {
                    let mut a = usize::from(cpu.d) << 8 | usize::from(cpu.e);
                    while bus.0[a] != b'$' {
                        out.push(char::from(bus.0[a]));
                        a += 1;
                    }
                }
                _ => {}
            }
        }
        t += u64::from(cpu.step(&mut bus));
        if cpu.pc == 0 {
            t += 10;
            break;
        }
    }
    print!("{name}: {out}");
    println!("\n{name}: {t} T-states");
    Some((out, t))
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn tst8080() {
    let Some((out, t)) = run("TST8080.COM") else {
        return;
    };
    assert!(out.contains("CPU IS OPERATIONAL"), "{out}");
    assert_eq!(t, 4924);
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn pre8080() {
    let Some((out, t)) = run("8080PRE.COM") else {
        return;
    };
    assert!(out.contains("8080 Preliminary tests complete"), "{out}");
    assert_eq!(t, 7817);
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn cputest() {
    let Some((out, t)) = run("CPUTEST.COM") else {
        return;
    };
    assert!(out.contains("CPU TESTS OK"), "{out}");
    assert_eq!(t, 255_653_383);
}

/// The instruction exerciser: every instruction group against CRCs from a
/// real 8080; minutes.
#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn exm8080() {
    let Some((out, t)) = run("8080EXM.COM") else {
        return;
    };
    assert!(!out.contains("ERROR"), "{out}");
    assert_eq!(t, 23_803_381_171);
}
