//! A timing probe (ignored): the bare CPU on a flat bus, a JMP loop at
//! 40 MHz for the CPU ticks of 200 MEGA65 fields.
//!
//!     cargo test -p cpu-45gs02 --release --test speed_probe -- --ignored --nocapture

use cpu_45gs02::{Cpu45gs02, Speed};
use emu_core::testing::FlatBus;
use emu_core::Cpu;

#[test]
#[ignore = "timing probe"]
fn zz_bare_cpu_speed() {
    let mut bus = FlatBus::new(16);
    bus.load(0x1000, &[0x4c, 0x00, 0x10]);
    let mut cpu = Cpu45gs02::new();
    cpu.pc = 0x1000;
    cpu.set_speed(Speed::Mhz40);
    let t = std::time::Instant::now();
    for _ in 0..162_000_000u32 {
        cpu.tick(&mut bus);
    }
    eprintln!("162M ticks: {:?}", t.elapsed());
}
