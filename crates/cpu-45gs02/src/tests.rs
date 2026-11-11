//! Per-instruction tests after the MEGA65 user guide's descriptions and
//! examples.

use emu_core::testing::FlatBus;
use emu_core::{Bus, Cpu, CycleResult, Flow, Size};

use crate::flag::{C, E, I, N, V, Z};
use crate::{Cpu45gs02, Speed, UNMAPPED};

/// A CPU with `code` at $1000 (PC there), 16 MB of RAM.
fn setup(code: &[u8]) -> (Cpu45gs02, FlatBus) {
    let mut bus = FlatBus::new(24);
    bus.load(0x1000, code);
    let mut cpu = Cpu45gs02::new();
    cpu.pc = 0x1000;
    cpu.set_speed(Speed::Mhz40);
    (cpu, bus)
}

/// Run one instruction; its cycles.
fn step(cpu: &mut Cpu45gs02, bus: &mut FlatBus) -> u32 {
    let mut n = 0;
    loop {
        n += 1;
        if let CycleResult::InstrDone { .. } = cpu.tick(bus) {
            return n;
        }
    }
}

fn run(cpu: &mut Cpu45gs02, bus: &mut FlatBus, instructions: usize) {
    for _ in 0..instructions {
        step(cpu, bus);
    }
}

#[test]
fn loads_set_flags_and_stz_stores_the_z_register() {
    // LDZ #$80, STZ $20, LDA #$00
    let (mut cpu, mut bus) = setup(&[0xa3, 0x80, 0x64, 0x20, 0xa9, 0x00]);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.z, 0x80);
    assert_ne!(cpu.p & N, 0);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(bus.mem[0x20], 0x80, "STZ stores Z, not zero");
    run(&mut cpu, &mut bus, 1);
    assert_ne!(cpu.p & Z, 0);
}

#[test]
fn the_base_page_register_moves_the_zero_page() {
    // LDA #$12, TAB, LDA #$55, STA $34  -> $1234
    let (mut cpu, mut bus) = setup(&[0xa9, 0x12, 0x5b, 0xa9, 0x55, 0x85, 0x34]);
    run(&mut cpu, &mut bus, 4);
    assert_eq!(bus.mem[0x1234], 0x55);
    assert_eq!(bus.mem[0x0034], 0);
}

#[test]
fn the_stack_is_8_bit_with_e_set_and_16_bit_without() {
    // LDX #$00, TXS, PHA, PHA: with E the low byte wraps in page 1.
    let (mut cpu, mut bus) = setup(&[0xa2, 0x00, 0x9a, 0x48, 0x48]);
    run(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.sp, 0x01ff);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.sp, 0x01fe);
    // CLE: SP counts through pages.
    let (mut cpu, mut bus) = setup(&[0x02, 0xa2, 0x00, 0x9a, 0x48, 0x48]);
    run(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.sp, 0x00ff);
    assert_eq!(cpu.p & E, 0);
    // TSY/TYS move SPH.
    let (mut cpu, mut bus) = setup(&[0xa0, 0x30, 0x2b, 0xa0, 0x00, 0x0b]);
    run(&mut cpu, &mut bus, 4);
    assert_eq!((cpu.sp >> 8, cpu.y), (0x30, 0x30));
}

#[test]
fn long_branches_bsr_and_rts_with_an_argument() {
    // $1000 BSR +$0100 -> $1102 (relative to the last byte, $1002).
    let (mut cpu, mut bus) = setup(&[0x63, 0x00, 0x01]);
    // $1102: PHW #$ABCD (two argument bytes), RTS #2... the routine is
    // called, so the arguments are under the return address: use the
    // simplest form, RTS #0.
    bus.load(0x1102, &[0x62, 0x00]);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x1102);
    assert_eq!((bus.mem[0x01ff], bus.mem[0x01fe]), (0x10, 0x02));
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x1003);
    assert_eq!(cpu.sp, 0x01ff);
    // BNE back by a 16-bit offset.
    let (mut cpu, mut bus) = setup(&[0xd3, 0xfe, 0xff]);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x1000, "offset -2 from the last byte");
}

#[test]
fn rts_with_an_argument_drops_the_callers_bytes() {
    // PHW #$1234 (2 bytes), JSR $2000; $2000: RTS #2.
    let (mut cpu, mut bus) = setup(&[0xf4, 0x34, 0x12, 0x20, 0x00, 0x20]);
    bus.load(0x2000, &[0x62, 0x02]);
    run(&mut cpu, &mut bus, 1);
    // PHW pushes the low byte first: the high byte ends at the lower
    // address.
    assert_eq!((bus.mem[0x01ff], bus.mem[0x01fe]), (0x34, 0x12));
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.pc, 0x1006);
    assert_eq!(cpu.sp, 0x01ff);
}

#[test]
fn word_instructions() {
    // INW $12 on $56FF, DEW $14 on $0000, ASW $2000 on $A987, ROW $2002.
    let (mut cpu, mut bus) = setup(&[
        0xe3, 0x12, 0xc3, 0x14, 0xcb, 0x00, 0x20, 0x38, 0xeb, 0x02, 0x20,
    ]);
    bus.load(0x12, &[0xff, 0x56]);
    bus.load(0x2000, &[0x87, 0xa9, 0x00, 0x40]);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(&bus.mem[0x12..0x14], &[0x00, 0x57]);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(&bus.mem[0x14..0x16], &[0xff, 0xff]);
    assert_ne!(cpu.p & N, 0);
    run(&mut cpu, &mut bus, 1);
    // The guide's example: $A987 << 1 = $530E, carry set.
    assert_eq!(&bus.mem[0x2000..0x2002], &[0x0e, 0x53]);
    assert_ne!(cpu.p & C, 0);
    run(&mut cpu, &mut bus, 2);
    // SEC, ROW $2002 on $4000: $8001, N set, carry from bit 15 clear.
    assert_eq!(&bus.mem[0x2002..0x2004], &[0x01, 0x80]);
    assert_eq!(cpu.p & C, 0);
    assert_ne!(cpu.p & N, 0);
}

#[test]
fn neg_asr_bit_immediate_and_the_bit_instructions() {
    // LDA #$01, NEG, ASR A, BIT #$C0
    let (mut cpu, mut bus) = setup(&[0xa9, 0x01, 0x42, 0x43, 0x89, 0xc0]);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.a, 0xff);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.a, 0xff, "ASR keeps the sign");
    assert_ne!(cpu.p & C, 0);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.p & (N | V), N | V, "BIT # sets N and V");
    // SMB3 $10, RMB0 $10, BBS3 $10,+2, TSB $11, TRB $11
    let (mut cpu, mut bus) = setup(&[
        0xb7, 0x10, 0x07, 0x10, 0xbf, 0x10, 0x02, 0, 0, 0xa9, 0x0f, 0x04, 0x11,
    ]);
    bus.mem[0x10] = 0x01;
    bus.mem[0x11] = 0xf0;
    run(&mut cpu, &mut bus, 3);
    assert_eq!(bus.mem[0x10], 0x08);
    assert_eq!(cpu.pc, 0x1009, "BBS3 taken");
    run(&mut cpu, &mut bus, 2);
    assert_eq!(bus.mem[0x11], 0xff);
    assert_ne!(cpu.p & Z, 0, "A and M had no bit in common");
}

#[test]
fn stack_relative_indirect_indexed() {
    // LDY #$01, LDA ($02,SP),Y: SP = $01F0, pointer at $01F2 -> $3000.
    let (mut cpu, mut bus) = setup(&[0xa0, 0x01, 0xe2, 0x02]);
    cpu.sp = 0x01f0;
    bus.load(0x01f2, &[0x00, 0x30]);
    bus.mem[0x3001] = 0x77;
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.a, 0x77);
}

#[test]
fn map_follows_the_guides_ethernet_example() {
    // LDA #$FF, LDX #$0F, LDY #0, LDZ #0, MAP; LDA #$80, LDX #$8D, MAP, EOM
    let (mut cpu, mut bus) = setup(&[
        0xa9, 0xff, 0xa2, 0x0f, 0xa0, 0x00, 0xa3, 0x00, 0x5c, 0xa9, 0x80, 0xa2, 0x8d, 0x5c, 0xea,
    ]);
    run(&mut cpu, &mut bus, 5);
    assert!(cpu.map_lock);
    run(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.map.translate(0x6800), 0x0ffd_e800);
    assert_eq!(
        cpu.map.translate(0x1000),
        UNMAPPED | 0x1000,
        "block 0 unmapped"
    );
    assert_eq!(cpu.map.translate(0x9000), UNMAPPED | 0x9000);
    run(&mut cpu, &mut bus, 1);
    assert!(!cpu.map_lock, "EOM ends the lock");
    // The upper half: $48000 offset makes $8000 -> $50000.
    let mut m = crate::Map::default();
    m.set(0, 0, 0x80, 0x14);
    assert_eq!(m.translate(0x8000), 0x5_0000);
    assert_eq!(m.translate(0xa000), UNMAPPED | 0xa000);
}

#[test]
fn map_holds_interrupts_off_until_eom() {
    // SEI not set: I is set after new(); clear it. MAP, NOP(EOM).
    let (mut cpu, mut bus) = setup(&[0x58, 0x5c, 0xea, 0xea]);
    bus.load(0xfffe, &[0x00, 0x40]);
    run(&mut cpu, &mut bus, 2);
    cpu.set_irq(true);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x1003, "EOM ran, the IRQ waited");
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x4000);
    assert_ne!(cpu.p & I, 0);
    // NMIs too, and they are edges.
    let (mut cpu, mut bus) = setup(&[0x5c, 0xea, 0xea]);
    bus.load(0xfffa, &[0x00, 0x50]);
    run(&mut cpu, &mut bus, 1);
    cpu.set_nmi(true);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x1002);
    assert_eq!(step(&mut cpu, &mut bus), 7);
    assert_eq!(cpu.pc, 0x5000);
}

#[test]
fn flat_addressing_reads_a_32_bit_pointer() {
    // LDZ #$02, NOP, LDA ($45),Z with $45 = 78 56 34 00 -> $345678 + 2
    let (mut cpu, mut bus) = setup(&[0xa3, 0x02, 0xea, 0xb2, 0x45, 0xea, 0x92, 0x45]);
    bus.load(0x45, &[0x78, 0x56, 0x34, 0x00]);
    bus.mem[0x34_567a] = 0x99;
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.a, 0x99);
    assert_eq!(cpu.pc, 0x1005, "the prefix and the instruction are one");
    // STA [$45],Z writes there too; without the prefix (zp),Z is 16-bit.
    cpu.a = 0x11;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(bus.mem[0x34_567a], 0x11);
    let (mut cpu, mut bus) = setup(&[0xb2, 0x45]);
    bus.load(0x45, &[0x78, 0x56, 0x34, 0x00]);
    bus.mem[0x5678] = 0x42;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.a, 0x42);
}

#[test]
fn q_instructions_follow_the_guides_addition_example() {
    // CLC, LDQ $1234 (NEG NEG LDA), ADCQ $1238, STQ $123C
    let (mut cpu, mut bus) = setup(&[
        0x18, 0x42, 0x42, 0xad, 0x34, 0x12, 0x42, 0x42, 0x6d, 0x38, 0x12, 0x42, 0x42, 0x8d, 0x3c,
        0x12,
    ]);
    bus.load(0x1234, &0x1234_ffffu32.to_le_bytes());
    bus.load(0x1238, &0x0000_0001u32.to_le_bytes());
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.q(), 0x1234_ffff);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.q(), 0x1235_0000);
    assert_eq!(cpu.p & (C | Z | N), 0);
    assert_eq!(&bus.mem[0x123c..0x1240], &0x1235_0000u32.to_le_bytes());
    assert_eq!(cpu.pc, 0x1010);
}

#[test]
fn q_shifts_compare_bit_and_the_z_rule_of_indirect_q() {
    // LDQ $10; ASLQ; INQ; CMPQ $14
    let (mut cpu, mut bus) = setup(&[
        0x42, 0x42, 0xa5, 0x10, 0x42, 0x42, 0x0a, 0x42, 0x42, 0x1a, 0x42, 0x42, 0xc5, 0x14,
    ]);
    bus.load(0x10, &0x8000_0001u32.to_le_bytes());
    bus.load(0x14, &0x0000_0003u32.to_le_bytes());
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.q(), 0x0000_0002);
    assert_ne!(cpu.p & C, 0);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.q(), 3);
    run(&mut cpu, &mut bus, 1);
    assert_ne!(cpu.p & Z, 0);
    assert_ne!(cpu.p & C, 0);
    // BITQ $18: N and V from bits 31 and 30.
    let (mut cpu, mut bus) = setup(&[0x42, 0x42, 0x24, 0x18]);
    bus.load(0x18, &0xc000_0000u32.to_le_bytes());
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.p & (N | V | Z), N | V | Z);
    // ORQ ($20) does not add Z; LDQ ($20),Z does.
    let (mut cpu, mut bus) = setup(&[0x42, 0x42, 0x12, 0x20, 0x42, 0x42, 0xb2, 0x20]);
    bus.load(0x20, &[0x00, 0x30]);
    bus.load(0x3000, &0x0000_0100u32.to_le_bytes());
    bus.load(0x3004, &0x5555_5555u32.to_le_bytes());
    cpu.set_q(0x0400_0000);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.q(), 0x0400_0100);
    cpu.set_q(0x0400_0000);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.q(), 0x5555_5555, "LDQ (zp),Z adds Z = 4");
}

#[test]
fn flat_q_reaches_far_memory() {
    // NEG NEG NOP STA ($40) -> STQ [$40]
    let (mut cpu, mut bus) = setup(&[0x42, 0x42, 0xea, 0x92, 0x40]);
    bus.load(0x40, &0x0012_3450u32.to_le_bytes());
    cpu.set_q(0xdead_beef);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(
        &bus.mem[0x12_3450..0x12_3454],
        &0xdead_beefu32.to_le_bytes()
    );
    assert_eq!(cpu.pc, 0x1005);
}

#[test]
fn neg_neg_before_an_opcode_without_q_form_is_two_negs() {
    // NEG NEG LDA #$05: LDA # has no Q form.
    let (mut cpu, mut bus) = setup(&[0xa9, 0x03, 0x42, 0x42, 0xa9, 0x05]);
    run(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.a, 0x03);
    assert_eq!(cpu.pc, 0x1004);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.a, 0x05);
}

#[test]
fn decimal_mode() {
    // SED, CLC, LDA #$19, ADC #$28 = $47; SEC, SBC #$48 = $99 with borrow.
    let (mut cpu, mut bus) = setup(&[0xf8, 0x18, 0xa9, 0x19, 0x69, 0x28, 0x38, 0xe9, 0x48]);
    run(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.a, 0x47);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.a, 0x99);
    assert_eq!(cpu.p & C, 0);
    assert_ne!(cpu.p & N, 0, "N valid in decimal mode");
}

#[test]
fn rmw_on_d019_writes_the_old_value_first() {
    struct Log(FlatBus, Vec<(u32, u32)>);
    impl Bus for Log {
        fn read(&mut self, a: u32, s: Size) -> u32 {
            self.0.read(a, s)
        }
        fn peek(&self, a: u32, s: Size) -> u32 {
            self.0.peek(a, s)
        }
        fn write(&mut self, a: u32, s: Size, v: u32) {
            self.1.push((a, v));
            self.0.write(a, s, v);
        }
        fn addr_bits(&self) -> u8 {
            28
        }
    }
    let (mut cpu, bus) = setup(&[0xee, 0x19, 0xd0, 0xee, 0x20, 0xd0]);
    let mut log = Log(bus, Vec::new());
    log.0.mem[0xd019] = 0x81;
    while !matches!(cpu.tick(&mut log), CycleResult::InstrDone { .. }) {}
    assert_eq!(
        log.1,
        vec![(UNMAPPED | 0xd019, 0x81), (UNMAPPED | 0xd019, 0x82)]
    );
    log.1.clear();
    while !matches!(cpu.tick(&mut log), CycleResult::InstrDone { .. }) {}
    assert_eq!(log.1.len(), 1, "elsewhere one write");
}

#[test]
fn cycle_counts_follow_the_speed() {
    // LDA $1234 (4r), INC $1234 (6dmr), BNE +0 (2b taken).
    let code = [0xad, 0x34, 0x12, 0xee, 0x34, 0x12, 0xd0, 0x00];
    let counts = |speed| {
        let (mut cpu, mut bus) = setup(&code);
        cpu.set_speed(speed);
        (0..3).map(|_| step(&mut cpu, &mut bus)).collect::<Vec<_>>()
    };
    assert_eq!(counts(Speed::Mhz1), vec![4, 6, 3], "the 6502's");
    assert_eq!(counts(Speed::Mhz3_5), vec![4, 5, 3]);
    assert_eq!(counts(Speed::Mhz40), vec![5, 7, 3]);
    // Implied instructions take two cycles at 1 MHz, one fast.
    let (mut cpu, mut bus) = setup(&[0x4b, 0x4b]);
    cpu.set_speed(Speed::Mhz1);
    assert_eq!(step(&mut cpu, &mut bus), 2);
    cpu.set_speed(Speed::Mhz3_5);
    assert_eq!(step(&mut cpu, &mut bus), 1);
}

#[test]
fn the_bus_access_comes_first_then_the_cpu_idles() {
    let (mut cpu, mut bus) = setup(&[0x8d, 0x00, 0x20]);
    cpu.a = 0x5a;
    cpu.set_speed(Speed::Mhz1);
    assert_eq!(cpu.tick(&mut bus), CycleResult::Busy);
    assert_eq!(bus.mem[0x2000], 0x5a);
    assert!(!cpu.at_boundary());
    let mut n = 1;
    while !matches!(
        cpu.tick(&mut bus),
        CycleResult::InstrDone { next_pc: 0x1003 }
    ) {
        n += 1;
    }
    assert_eq!(n + 1, 4);
}

#[test]
fn brk_and_rti() {
    let (mut cpu, mut bus) = setup(&[0x00, 0xff, 0xea]);
    bus.load(0xfffe, &[0x00, 0x30]);
    bus.load(0x3000, &[0x40]);
    cpu.p &= !I;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x3000);
    assert_eq!(bus.mem[0x01fd] & 0x10, 0x10, "B pushed set");
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x1002);
    assert_eq!(cpu.p & I, 0);
    assert_ne!(cpu.p & E, 0, "RTI keeps E");
}

#[test]
fn disassembly_knows_the_prefixes() {
    let mut bus = FlatBus::new(24);
    bus.load(
        0x1000,
        &[
            0x42, 0x42, 0xad, 0x34, 0x12, 0xea, 0xb2, 0x45, 0x42, 0x42, 0xea, 0x92, 0x40, 0x63,
            0x00, 0x01, 0x42,
        ],
    );
    let cpu = Cpu45gs02::new();
    let d = |a| {
        let i = cpu.disassemble(&bus, a);
        (i.to_string(), i.bytes.len())
    };
    assert_eq!(d(0x1000), ("LDQ $1234".into(), 5));
    assert_eq!(d(0x1005), ("LDA [$45],Z".into(), 3));
    assert_eq!(d(0x1008), ("STQ [$40]".into(), 5));
    assert_eq!(d(0x100d), ("BSR $110F".into(), 3));
    assert_eq!(cpu.disassemble(&bus, 0x100d).flow, Flow::Call);
    assert_eq!(d(0x1010), ("NEG A".into(), 1));
}

#[test]
fn savestates_round_trip_mid_instruction() {
    use emu_core::savestate::{Savestate, StateReader, StateWriter};
    let (mut cpu, mut bus) = setup(&[0xee, 0x00, 0x20, 0xea]);
    cpu.set_speed(Speed::Mhz1);
    cpu.map.set(0x00, 0x10, 0x00, 0x00);
    cpu.tick(&mut bus);
    let mut w = StateWriter::default();
    cpu.save(&mut w);
    let bytes = w.into_bytes();
    let mut other = Cpu45gs02::new();
    other.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(other, cpu);
    assert!(Cpu45gs02::new()
        .load(&mut StateReader::new(&bytes[..bytes.len() - 3]))
        .is_err());
}
