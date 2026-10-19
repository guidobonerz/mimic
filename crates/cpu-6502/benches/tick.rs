//! Baseline for golden rule 5: run before/after touching `tick` paths with
//! `cargo bench -p cpu-6502`.

use cpu_6502::Cpu6502;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use emu_core::testing::FlatBus;
use emu_core::Cpu;

const CYCLES: u64 = 1_000_000;

/// A loop mixing loads, stores, RMW, indexed modes and branches.
const PROGRAM: [u8; 17] = [
    0xa2, 0x00, // LDX #0
    0xbd, 0x00, 0x20, // loop: LDA $2000,X
    0x9d, 0x00, 0x30, // STA $3000,X
    0xfe, 0x00, 0x30, // INC $3000,X
    0xe8, // INX
    0xd0, 0xf4, // BNE loop
    0x4c, 0x00, 0x04, // JMP $0400
];

fn tick(c: &mut Criterion) {
    let mut group = c.benchmark_group("cpu-6502");
    group.throughput(Throughput::Elements(CYCLES));
    group.bench_function("tick 1M cycles", |b| {
        let mut bus = FlatBus::new(16);
        bus.load(0x0400, &PROGRAM);
        let mut cpu = Cpu6502::default();
        cpu.set_reg("PC", 0x0400);
        b.iter(|| {
            for _ in 0..CYCLES {
                std::hint::black_box(cpu.tick(&mut bus));
            }
        });
    });
    group.finish();
}

criterion_group!(benches, tick);
criterion_main!(benches);
