//! Klaus Dormann's 6502 functional test (GPL-3.0 test program, fetched, never
//! committed). The prebuilt image is loaded at $0000 and started at $0400;
//! the test ends in a `JMP *` trap. Reaching the trap at $3469 means success
//! (address from `6502_functional_test.lst` of the pinned commit).
//!
//! Run with `cargo test -p cpu-6502 --release -- --ignored dormann`.

use cpu_6502::Cpu6502;
use emu_core::testing::FlatBus;
use emu_core::{Cpu, CycleResult};

const START: u32 = 0x0400;
const SUCCESS: u32 = 0x3469;
/// The full test runs about 96 million cycles; this is a generous upper bound.
const MAX_CYCLES: u64 = 200_000_000;

#[test]
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh)"]
fn dormann_functional_test() {
    let dir = std::env::var_os("EMU_TESTDATA_DIR")
        .expect("EMU_TESTDATA_DIR not set; run tools/fetch-testdata.sh first");
    let path = std::path::Path::new(&dir)
        .join("dormann")
        .join("6502_functional_test.bin");
    let image =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert_eq!(
        image.len(),
        0x10000,
        "image must cover the full address space"
    );

    let mut bus = FlatBus::new(16);
    bus.load(0, &image);
    let mut cpu = Cpu6502::default();
    cpu.set_reg("PC", START);

    let mut prev_pc = u32::MAX;
    for _ in 0..MAX_CYCLES {
        match cpu.tick(&mut bus) {
            CycleResult::InstrDone { next_pc } => {
                if next_pc == prev_pc {
                    // `JMP *`: the program counter no longer moves.
                    assert_eq!(
                        next_pc, SUCCESS,
                        "trapped at ${next_pc:04X} (see the listing for the failed test)"
                    );
                    return;
                }
                prev_pc = next_pc;
            }
            CycleResult::Jammed => panic!("CPU jammed at ${:04X}", cpu.pc()),
            CycleResult::Busy => {}
        }
    }
    panic!("no trap within {MAX_CYCLES} cycles, PC=${:04X}", cpu.pc());
}
