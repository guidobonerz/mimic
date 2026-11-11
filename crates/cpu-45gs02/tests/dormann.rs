//! Klaus Dormann's 6502 functional test (GPL-3.0 test program, fetched by
//! `tools/fetch-testdata.sh`, never committed) on the 45GS02: the test uses
//! the documented NMOS opcodes only, which the 4510 personality keeps. The
//! image is loaded at $0000 and started at $0400; reaching the `JMP *` at
//! $3469 means success.
//!
//! Run with `EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-45gs02
//! --release -- --ignored dormann`.

use cpu_45gs02::{Cpu45gs02, Speed};
use emu_core::testing::FlatBus;
use emu_core::{Cpu, CycleResult};

const SUCCESS: u32 = 0x3469;

#[test]
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh)"]
fn dormann_functional_test() {
    let dir = std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR not set");
    let path = std::path::Path::new(&dir)
        .join("dormann")
        .join("6502_functional_test.bin");
    let image = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut bus = FlatBus::new(16);
    bus.load(0, &image);
    let mut cpu = Cpu45gs02::new();
    cpu.set_speed(Speed::Mhz40);
    cpu.pc = 0x0400;
    let mut prev = u32::MAX;
    for _ in 0..400_000_000u64 {
        if let CycleResult::InstrDone { next_pc } = cpu.tick(&mut bus) {
            if next_pc == prev {
                assert_eq!(
                    next_pc, SUCCESS,
                    "trapped at ${next_pc:04X} (see the listing for the failed test)"
                );
                return;
            }
            prev = next_pc;
        }
    }
    panic!("no trap, PC=${:04X}", cpu.pc);
}
