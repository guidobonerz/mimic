//! ZEXDOC and ZEXALL (Frank Cringle's Z80 instruction exercisers, GPL-2.0
//! test programs fetched by `tools/fetch-testdata.sh`, never committed).
//! They run as CP/M programs: loaded at `$0100`, printing through BDOS
//! calls at `$0005`, ending with a jump to `$0000`. Each of the 67 groups
//! prints "OK" or "ERROR" with the CRC of its results. ZEXDOC checks the
//! documented flags, ZEXALL all of them.
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-z80 --release --test zex -- --ignored --nocapture
//!
//! Each takes minutes (about 46 × 10⁹ T-states for ZEXALL).

use std::path::PathBuf;

use cpu_z80::Z80;
use emu_core::testing::FlatBus;
use emu_core::{Bus, Size};

fn program(name: &str) -> Vec<u8> {
    let dir = std::env::var_os("EMU_TESTDATA_DIR")
        .expect("EMU_TESTDATA_DIR not set; run tools/fetch-testdata.sh first");
    let path = PathBuf::from(dir).join("zex").join(format!("{name}.com"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Run a CP/M program to its warm boot; returns what it printed.
fn run_cpm(code: &[u8]) -> String {
    let mut bus = FlatBus::new(16);
    for (i, &b) in code.iter().enumerate() {
        bus.write(0x100 + i as u32, Size::Byte, u32::from(b));
    }
    // $0005: BDOS entry (handled below, then RET); $0006: top of the TPA.
    bus.write(0x0005, Size::Byte, 0xc9);
    bus.write(0x0006, Size::Byte, 0x00);
    bus.write(0x0007, Size::Byte, 0xf0);
    let mut cpu = Z80::new();
    cpu.pc = 0x100;
    cpu.sp = 0xf000;
    let mut out = String::new();
    loop {
        match cpu.pc {
            0x0000 => return out,
            0x0005 => match cpu.c {
                2 => out.push(char::from(cpu.e)),
                9 => {
                    let mut a = cpu.de();
                    loop {
                        let ch = bus.peek(u32::from(a), Size::Byte) as u8;
                        if ch == b'$' {
                            break;
                        }
                        out.push(char::from(ch));
                        a = a.wrapping_add(1);
                    }
                    print!("{}", &out[out.rfind('\n').map_or(0, |i| i + 1)..]);
                }
                _ => {}
            },
            _ => {}
        }
        cpu.step(&mut bus);
    }
}

fn check(name: &str) {
    let out = run_cpm(&program(name));
    println!("{out}");
    let errors: Vec<&str> = out.lines().filter(|l| l.contains("ERROR")).collect();
    assert!(
        errors.is_empty(),
        "{name}: {} failing groups:\n{}",
        errors.len(),
        errors.join("\n")
    );
    assert!(
        out.contains("Tests complete"),
        "{name} did not finish:\n{out}"
    );
}

#[test]
#[ignore = "needs ZEXDOC (tools/fetch-testdata.sh) in EMU_TESTDATA_DIR; takes minutes"]
fn zexdoc() {
    check("zexdoc");
}

#[test]
#[ignore = "needs ZEXALL (tools/fetch-testdata.sh) in EMU_TESTDATA_DIR; takes minutes"]
fn zexall() {
    check("zexall");
}
