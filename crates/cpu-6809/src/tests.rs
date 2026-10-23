use emu_core::savestate::{StateReader, StateWriter};
use emu_core::Flow;

use super::*;

struct Ram {
    mem: Vec<u8>,
    log: Vec<char>,
}

impl M6809Bus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.log.push('r');
        self.mem[usize::from(addr)]
    }
    fn write(&mut self, addr: u16, v: u8) {
        self.log.push('w');
        self.mem[usize::from(addr)] = v;
    }
    fn idle(&mut self) {
        self.log.push('i');
    }
    fn peek(&self, addr: u16) -> u8 {
        self.mem[usize::from(addr)]
    }
}

/// A CPU with S = $8000, PC = $1000 and `code` there.
fn setup(code: &[u8]) -> (M6809, Ram) {
    let mut ram = Ram {
        mem: vec![0; 0x10000],
        log: Vec::new(),
    };
    ram.mem[0x1000..0x1000 + code.len()].copy_from_slice(code);
    let mut cpu = M6809::new();
    cpu.pc = 0x1000;
    cpu.set_s(0x8000);
    cpu.cc = 0;
    (cpu, ram)
}

fn run(code: &[u8]) -> (M6809, Ram, u32) {
    let (mut cpu, mut ram) = setup(code);
    let n = cpu.step(&mut ram);
    (cpu, ram, n)
}

#[test]
fn reset_loads_the_vector_and_masks_interrupts() {
    let (mut cpu, mut ram) = setup(&[]);
    ram.mem[0xfffe] = 0xf0;
    ram.mem[0xffff] = 0x00;
    cpu.dp = 5;
    cpu.cc = 0;
    cpu.reset(&mut ram);
    assert_eq!(cpu.pc, 0xf000);
    assert_eq!(cpu.dp, 0);
    assert_eq!(cpu.cc & (I | F), I | F);
}

#[test]
fn cycle_counts_follow_the_datasheet() {
    // (code, cycles) from the MC6809 datasheet tables.
    let cases: &[(&[u8], u32)] = &[
        (&[0x12], 2),                   // NOP
        (&[0x86, 0x12], 2),             // LDA #
        (&[0x96, 0x12], 4),             // LDA <
        (&[0xb6, 0x12, 0x34], 5),       // LDA ext
        (&[0xa6, 0x84], 4),             // LDA ,X
        (&[0xa6, 0x01], 5),             // LDA 1,X
        (&[0xa6, 0x88, 0x10], 5),       // LDA 16,X
        (&[0xa6, 0x89, 0x01, 0x00], 8), // LDA 256,X
        (&[0xa6, 0x80], 6),             // LDA ,X+
        (&[0xa6, 0x81], 7),             // LDA ,X++
        (&[0xa6, 0x94], 7),             // LDA [,X]
        (&[0xa6, 0x9f, 0x20, 0x00], 9), // LDA [$2000]
        (&[0xa6, 0x8d, 0x00, 0x10], 9), // LDA $10,PCR (16-bit)
        (&[0xa6, 0x8b], 8),             // LDA D,X
        (&[0xcc, 0x12, 0x34], 3),       // LDD #
        (&[0xfc, 0x12, 0x34], 6),       // LDD ext
        (&[0xc3, 0x00, 0x01], 4),       // ADDD #
        (&[0xd3, 0x10], 6),             // ADDD <
        (&[0x8c, 0x00, 0x01], 4),       // CMPX #
        (&[0x10, 0x83, 0x00, 0x01], 5), // CMPD #
        (&[0x10, 0xbc, 0x20, 0x00], 8), // CMPY ext
        (&[0x10, 0x8e, 0x00, 0x01], 4), // LDY #
        (&[0x10, 0xce, 0x90, 0x00], 4), // LDS #
        (&[0x11, 0x83, 0x00, 0x01], 5), // CMPU #
        (&[0x97, 0x10], 4),             // STA <
        (&[0xfd, 0x20, 0x00], 6),       // STD ext
        (&[0x0c, 0x10], 6),             // INC <
        (&[0x7c, 0x20, 0x00], 7),       // INC ext
        (&[0x6c, 0x84], 6),             // INC ,X
        (&[0x0e, 0x10], 3),             // JMP <
        (&[0x7e, 0x20, 0x00], 4),       // JMP ext
        (&[0x6e, 0x84], 3),             // JMP ,X
        (&[0xbd, 0x20, 0x00], 8),       // JSR ext
        (&[0x9d, 0x10], 7),             // JSR <
        (&[0xad, 0x84], 7),             // JSR ,X
        (&[0x8d, 0x00], 7),             // BSR
        (&[0x17, 0x00, 0x00], 9),       // LBSR
        (&[0x16, 0x00, 0x00], 5),       // LBRA
        (&[0x20, 0x00], 3),             // BRA
        (&[0x27, 0x00], 3),             // BEQ (not taken)
        (&[0x10, 0x27, 0x00, 0x00], 5), // LBEQ not taken
        (&[0x10, 0x26, 0x00, 0x00], 6), // LBNE taken
        (&[0x30, 0x01], 5),             // LEAX 1,X
        (&[0x34, 0xff], 17),            // PSHS all
        (&[0x34, 0x06], 7),             // PSHS A,B
        (&[0x39], 5),                   // RTS
        (&[0x3a], 3),                   // ABX
        (&[0x3d], 11),                  // MUL
        (&[0x3f], 19),                  // SWI
        (&[0x10, 0x3f], 20),            // SWI2
        (&[0x11, 0x3f], 20),            // SWI3
        (&[0x1e, 0x89], 8),             // EXG A,B
        (&[0x1f, 0x12], 6),             // TFR X,Y
        (&[0x1a, 0x50], 3),             // ORCC
        (&[0x19], 2),                   // DAA
        (&[0x1d], 2),                   // SEX
        (&[0x4f], 2),                   // CLRA
    ];
    for (code, want) in cases {
        let (_, ram, n) = run(code);
        assert_eq!(n, *want, "{code:02x?}");
        assert_eq!(ram.log.len() as u32, n, "{code:02x?}");
    }
}

#[test]
fn rti_takes_six_or_fifteen_cycles() {
    let (mut cpu, mut ram) = setup(&[0x3b]);
    ram.mem[0x8000] = 0; // CC with E clear
    assert_eq!(cpu.step(&mut ram), 6);
    let (mut cpu, mut ram) = setup(&[0x3b]);
    ram.mem[0x8000] = E;
    assert_eq!(cpu.step(&mut ram), 15);
    assert_eq!(cpu.s, 0x800c);
}

#[test]
fn pull_all_takes_seventeen_cycles() {
    let (mut cpu, mut ram) = setup(&[0x35, 0xff]);
    assert_eq!(cpu.step(&mut ram), 17);
}

#[test]
fn dead_cycles_sit_before_the_access() {
    // LDA <$10: opcode, address, dead, read.
    let (_, ram, _) = run(&[0x96, 0x10]);
    assert_eq!(ram.log, ['r', 'r', 'i', 'r']);
    // INC <$10: opcode, address, dead, read, dead, write.
    let (_, ram, _) = run(&[0x0c, 0x10]);
    assert_eq!(ram.log, ['r', 'r', 'i', 'r', 'i', 'w']);
    // STA $2000: opcode, address (2), dead, write.
    let (_, ram, _) = run(&[0xb7, 0x20, 0x00]);
    assert_eq!(ram.log, ['r', 'r', 'r', 'i', 'w']);
}

#[test]
fn eight_bit_arithmetic_sets_flags() {
    // ADDA #$01 with A = $7F: overflow, negative, half carry.
    let (mut cpu, mut ram) = setup(&[0x8b, 0x01]);
    cpu.a = 0x7f;
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0x80);
    assert_eq!(cpu.cc & (N | Z | V | C | H), N | V | H);
    // SUBA #$01 with A = 0: borrow, negative.
    let (mut cpu, mut ram) = setup(&[0x80, 0x01]);
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0xff);
    assert_eq!(cpu.cc & (N | Z | V | C), N | C);
    // ADCA #$00 with C set and A = $FF: carry, zero.
    let (mut cpu, mut ram) = setup(&[0x89, 0x00]);
    cpu.a = 0xff;
    cpu.cc = C;
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0);
    assert_eq!(cpu.cc & (Z | C), Z | C);
    // SBCB #$7F with C set and B = $80: $00 and overflow.
    let (mut cpu, mut ram) = setup(&[0xc2, 0x7f]);
    cpu.b = 0x80;
    cpu.cc = C;
    cpu.step(&mut ram);
    assert_eq!(cpu.b, 0);
    assert_eq!(cpu.cc & (Z | V | C), Z | V);
    // CMPA leaves A alone.
    let (mut cpu, mut ram) = setup(&[0x81, 0x10]);
    cpu.a = 0x10;
    cpu.step(&mut ram);
    assert_eq!((cpu.a, cpu.cc & Z), (0x10, Z));
}

#[test]
fn read_modify_write_flags() {
    let op = |code: u8, a: u8, cc: u8| {
        let (mut cpu, mut ram) = setup(&[code]);
        cpu.a = a;
        cpu.cc = cc;
        cpu.step(&mut ram);
        (cpu.a, cpu.cc & (N | Z | V | C))
    };
    assert_eq!(op(0x40, 0x80, 0), (0x80, N | V | C)); // NEGA $80
    assert_eq!(op(0x40, 0x00, 0), (0x00, Z)); // NEGA 0: no carry
    assert_eq!(op(0x43, 0x0f, 0), (0xf0, N | C)); // COMA
    assert_eq!(op(0x44, 0x81, 0), (0x40, C)); // LSRA
    assert_eq!(op(0x46, 0x01, C), (0x80, N | C)); // RORA
    assert_eq!(op(0x47, 0x81, 0), (0xc0, N | C)); // ASRA
    assert_eq!(op(0x48, 0x40, 0), (0x80, N | V)); // ASLA
    assert_eq!(op(0x49, 0x80, C), (0x01, V | C)); // ROLA
    assert_eq!(op(0x4a, 0x80, 0), (0x7f, V)); // DECA
    assert_eq!(op(0x4c, 0x7f, 0), (0x80, N | V)); // INCA
    assert_eq!(op(0x4d, 0x00, V | C), (0x00, Z | C)); // TSTA keeps C
    assert_eq!(op(0x4f, 0x55, N | V | C), (0x00, Z)); // CLRA
                                                      // Undocumented aliases.
    assert_eq!(op(0x41, 0x01, 0), (0xff, N | C)); // NEGA
    assert_eq!(op(0x4e, 0x55, 0), (0x00, Z)); // CLRA
}

#[test]
fn memory_read_modify_write() {
    let (mut cpu, mut ram) = setup(&[0x7a, 0x20, 0x00, 0x0d, 0x30]);
    ram.mem[0x2000] = 1;
    cpu.dp = 0x20;
    ram.mem[0x2030] = 0x80;
    cpu.step(&mut ram);
    assert_eq!(ram.mem[0x2000], 0);
    assert_ne!(cpu.cc & Z, 0);
    cpu.step(&mut ram); // TST <$30
    assert_eq!(ram.mem[0x2030], 0x80);
    assert_ne!(cpu.cc & N, 0);
}

#[test]
fn sixteen_bit_operations() {
    // ADDD #$0001 with D = $7FFF.
    let (mut cpu, mut ram) = setup(&[0xc3, 0x00, 0x01]);
    cpu.set_d(0x7fff);
    cpu.step(&mut ram);
    assert_eq!(cpu.d(), 0x8000);
    assert_eq!(cpu.cc & (N | V | C), N | V);
    // SUBD #$0002 with D = 1.
    let (mut cpu, mut ram) = setup(&[0x83, 0x00, 0x02]);
    cpu.set_d(1);
    cpu.step(&mut ram);
    assert_eq!(cpu.d(), 0xffff);
    assert_eq!(cpu.cc & (N | C), N | C);
    // CMPX, LDY, STU.
    let (mut cpu, mut ram) = setup(&[
        0x8c, 0x12, 0x34, 0x10, 0x8e, 0xab, 0xcd, 0xce, 0x55, 0xaa, 0xff, 0x20, 0x00,
    ]);
    cpu.x = 0x1234;
    cpu.step(&mut ram);
    assert_ne!(cpu.cc & Z, 0);
    cpu.step(&mut ram);
    assert_eq!(cpu.y, 0xabcd);
    assert_ne!(cpu.cc & N, 0);
    cpu.step(&mut ram);
    cpu.step(&mut ram);
    assert_eq!(&ram.mem[0x2000..0x2002], &[0x55, 0xaa]);
}

#[test]
fn mul_sex_daa_abx() {
    let (mut cpu, mut ram) = setup(&[0x3d]);
    cpu.a = 0x80;
    cpu.b = 0x81;
    cpu.step(&mut ram);
    assert_eq!(cpu.d(), 0x4080);
    assert_eq!(cpu.cc & (Z | C), C);
    let (mut cpu, mut ram) = setup(&[0x1d]);
    cpu.b = 0x90;
    cpu.step(&mut ram);
    assert_eq!(cpu.d(), 0xff90);
    assert_ne!(cpu.cc & N, 0);
    // $19 + $28 = $41 binary, DAA makes BCD $47.
    let (mut cpu, mut ram) = setup(&[0x8b, 0x28, 0x19]);
    cpu.a = 0x19;
    cpu.step(&mut ram);
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0x47);
    // $99 + $01 = $00 with carry.
    let (mut cpu, mut ram) = setup(&[0x8b, 0x01, 0x19]);
    cpu.a = 0x99;
    cpu.step(&mut ram);
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0x00);
    assert_ne!(cpu.cc & C, 0);
    let (mut cpu, mut ram) = setup(&[0x3a]);
    cpu.x = 0x10ff;
    cpu.b = 0xff;
    cpu.step(&mut ram);
    assert_eq!(cpu.x, 0x11fe);
}

#[test]
fn indexed_modes() {
    let (mut cpu, mut ram) = setup(&[
        0xa6, 0x80, // LDA ,X+
        0xe6, 0xa1, // LDB ,Y++
        0xa6, 0xc2, // LDA ,-U
        0xec, 0xe3, // LDD ,--S
        0xa6, 0x1f, // LDA -1,X
        0xa6, 0x8c, 0x04, // LDA 4,PCR
        0xa6, 0x9f, 0x30, 0x00, // LDA [$3000]
    ]);
    for (i, b) in ram.mem[0x2000..0x2010].iter_mut().enumerate() {
        *b = 0x40 + i as u8;
    }
    cpu.x = 0x2000;
    cpu.y = 0x2004;
    cpu.u = 0x2008;
    cpu.s = 0x200c;
    cpu.step(&mut ram);
    assert_eq!((cpu.a, cpu.x), (0x40, 0x2001));
    cpu.step(&mut ram);
    assert_eq!((cpu.b, cpu.y), (0x44, 0x2006));
    cpu.step(&mut ram);
    assert_eq!((cpu.a, cpu.u), (0x47, 0x2007));
    cpu.step(&mut ram);
    assert_eq!((cpu.d(), cpu.s), (0x4a4b, 0x200a));
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0x40);
    ram.mem[0x1011] = 0x99;
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0x99);
    ram.mem[0x3000] = 0x20;
    ram.mem[0x3001] = 0x0f;
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0x4f);
    // Accumulator offsets are signed: LDA B,X with B = $FF.
    let (mut cpu, mut ram) = setup(&[0xa6, 0x85]);
    cpu.x = 0x2001;
    cpu.b = 0xff;
    ram.mem[0x2000] = 0x77;
    cpu.step(&mut ram);
    assert_eq!(cpu.a, 0x77);
}

#[test]
fn lea_sets_z_for_x_and_y_only() {
    let (mut cpu, mut ram) = setup(&[0x30, 0x1f, 0x33, 0x5f]);
    cpu.x = 1;
    cpu.u = 1;
    cpu.step(&mut ram);
    assert_eq!(cpu.x, 0);
    assert_ne!(cpu.cc & Z, 0);
    cpu.cc = 0;
    cpu.step(&mut ram);
    assert_eq!(cpu.u, 0);
    assert_eq!(cpu.cc & Z, 0);
}

#[test]
fn push_and_pull_order() {
    let (mut cpu, mut ram) = setup(&[0x34, 0xff, 0x35, 0x7f]);
    cpu.a = 1;
    cpu.b = 2;
    cpu.dp = 3;
    cpu.x = 0x0405;
    cpu.y = 0x0607;
    cpu.u = 0x0809;
    cpu.cc = 0x0a;
    cpu.step(&mut ram);
    assert_eq!(cpu.s, 0x8000 - 12);
    assert_eq!(
        &ram.mem[0x7ff4..0x8000],
        &[0x0a, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x10, 0x02]
    );
    cpu.a = 0;
    cpu.x = 0;
    cpu.u = 0;
    cpu.step(&mut ram);
    assert_eq!((cpu.a, cpu.x, cpu.u, cpu.s), (1, 0x0405, 0x0809, 0x7ffe));
}

#[test]
fn subroutines_and_branches() {
    // $1000 BSR $1004; $1002 BRA $1002; $1004 LDA #1; RTS
    let (mut cpu, mut ram) = setup(&[0x8d, 0x02, 0x20, 0xfe, 0x86, 0x01, 0x39]);
    cpu.step(&mut ram);
    assert_eq!((cpu.pc, cpu.s), (0x1004, 0x7ffe));
    assert_eq!(&ram.mem[0x7ffe..0x8000], &[0x10, 0x02]);
    cpu.step(&mut ram);
    cpu.step(&mut ram);
    assert_eq!((cpu.pc, cpu.s), (0x1002, 0x8000));
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x1002);
    // Signed conditions: BGT with N = V, Z clear; BLE otherwise.
    for (cc, op, taken) in [
        (0, 0x2e, true),
        (N, 0x2e, false),
        (N | V, 0x2e, true),
        (Z, 0x2f, true),
        (N, 0x2d, true),
        (C, 0x22, false),
        (0, 0x22, true),
    ] {
        let (mut cpu, mut ram) = setup(&[op, 0x10]);
        cpu.cc = cc;
        cpu.step(&mut ram);
        assert_eq!(cpu.pc == 0x1012, taken, "{op:02x} cc={cc:02x}");
    }
    // LBRA with a negative offset.
    let (mut cpu, mut ram) = setup(&[0x16, 0xff, 0xfd]);
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x1000);
}

#[test]
fn tfr_and_exg_across_sizes() {
    let (mut cpu, mut ram) = setup(&[0x1f, 0x89, 0x1e, 0x01, 0x1f, 0x81, 0x1f, 0x18]);
    cpu.a = 0x12;
    cpu.step(&mut ram); // TFR A,B
    assert_eq!(cpu.b, 0x12);
    cpu.x = 0xbeef;
    cpu.step(&mut ram); // EXG D,X
    assert_eq!((cpu.d(), cpu.x), (0xbeef, 0x1212));
    cpu.step(&mut ram); // TFR A,X: $FF above
    assert_eq!(cpu.x, 0xffbe);
    cpu.step(&mut ram); // TFR X,A: low byte
    assert_eq!(cpu.a, 0xbe);
}

#[test]
fn swi_stacks_everything_and_rti_returns() {
    let (mut cpu, mut ram) = setup(&[0x3f, 0x12]);
    ram.mem[0xfffa] = 0x20;
    ram.mem[0xfffb] = 0x00;
    ram.mem[0x2000] = 0x3b; // RTI
    cpu.a = 0x55;
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x2000);
    assert_eq!(cpu.cc & (E | I | F), E | I | F);
    assert_eq!(cpu.s, 0x8000 - 12);
    cpu.a = 0;
    assert_eq!(cpu.step(&mut ram), 15);
    assert_eq!((cpu.pc, cpu.a, cpu.s), (0x1001, 0x55, 0x8000));
    assert_eq!(cpu.cc & (I | F), 0);
}

#[test]
fn irq_and_firq_entry() {
    let (mut cpu, mut ram) = setup(&[0x12, 0x12]);
    ram.mem[0xfff8] = 0x30;
    ram.mem[0xfff9] = 0x00;
    ram.mem[0xfff6] = 0x40;
    ram.mem[0xfff7] = 0x00;
    // Masked: nothing happens.
    cpu.cc = I | F;
    cpu.irq = true;
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x1001);
    // IRQ: all registers, 19 cycles, I set, F unchanged.
    cpu.cc = F;
    assert_eq!(cpu.step(&mut ram), 19);
    assert_eq!(cpu.pc, 0x3000);
    assert_eq!(cpu.s, 0x8000 - 12);
    assert_eq!(cpu.cc & (E | I | F), E | I | F);
    // FIRQ: PC and CC only, 10 cycles, E clear.
    let (mut cpu, mut ram) = setup(&[0x12]);
    ram.mem[0xfff6] = 0x40;
    ram.mem[0xfff7] = 0x00;
    cpu.firq = true;
    cpu.irq = true;
    assert_eq!(cpu.step(&mut ram), 10);
    assert_eq!((cpu.pc, cpu.s), (0x4000, 0x8000 - 3));
    assert_eq!(cpu.cc & (E | I | F), I | F);
}

#[test]
fn nmi_waits_for_s_and_is_edge_triggered() {
    let (mut cpu, mut ram) = setup(&[0x12, 0x12, 0x12, 0x12]);
    ram.mem[0xfffc] = 0x50;
    ram.mem[0xfffd] = 0x00;
    cpu.nmi_armed = false;
    cpu.set_nmi(true);
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x1001, "disarmed");
    cpu.set_s(0x8000);
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x5000);
    // The line stays low: no second NMI.
    ram.mem[0x5000] = 0x12;
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x5001);
}

#[test]
fn cwai_and_sync_wait_for_interrupts() {
    let (mut cpu, mut ram) = setup(&[0x3c, 0xef, 0x13, 0x12]);
    ram.mem[0xfff8] = 0x30;
    ram.mem[0xfff9] = 0x00;
    cpu.cc = I;
    let a = cpu.step(&mut ram);
    assert_eq!(cpu.wait, Wait::Cwai);
    assert_eq!(cpu.s, 0x8000 - 12);
    assert_eq!(cpu.step(&mut ram), 1);
    cpu.irq = true;
    let b = cpu.step(&mut ram);
    assert_eq!(a + b, 20);
    assert_eq!((cpu.pc, cpu.s), (0x3000, 0x8000 - 12));
    // SYNC ends on a masked interrupt and goes on.
    let (mut cpu, mut ram) = setup(&[0x13, 0x12]);
    cpu.cc = I;
    cpu.step(&mut ram);
    assert_eq!(cpu.wait, Wait::Sync);
    cpu.step(&mut ram);
    assert_eq!(cpu.wait, Wait::Sync);
    cpu.irq = true;
    cpu.step(&mut ram);
    assert_eq!((cpu.wait, cpu.pc), (Wait::None, 0x1001));
}

#[test]
fn hcf_halts_until_reset() {
    let (mut cpu, mut ram) = setup(&[0x14]);
    cpu.step(&mut ram);
    assert_eq!(cpu.wait, Wait::Halt);
    let pc = cpu.pc;
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, pc);
}

#[test]
fn disassembly() {
    let (_, mut ram) = setup(&[]);
    let mut put = |code: &[u8]| {
        ram.mem[0x1000..0x1000 + code.len()].copy_from_slice(code);
        let i = disassemble(&ram, 0x1000);
        (i.to_string(), i.bytes.len())
    };
    assert_eq!(put(&[0x86, 0x12]).0, "LDA #$12");
    assert_eq!(put(&[0x10, 0x8e, 0x12, 0x34]), ("LDY #$1234".into(), 4));
    assert_eq!(put(&[0x96, 0x10]).0, "LDA <$10");
    assert_eq!(put(&[0xbd, 0xf1, 0x92]).0, "JSR $F192");
    assert_eq!(put(&[0xa6, 0x1f]).0, "LDA -$1,X");
    assert_eq!(put(&[0xe6, 0xa1]).0, "LDB ,Y++");
    assert_eq!(put(&[0xa6, 0x94]).0, "LDA [,X]");
    assert_eq!(put(&[0xa6, 0x9f, 0x30, 0x00]).0, "LDA [$3000]");
    assert_eq!(put(&[0x30, 0x8c, 0x02]).0, "LEAX $1005,PCR");
    assert_eq!(put(&[0x34, 0x56]).0, "PSHS A,B,X,U");
    assert_eq!(put(&[0x37, 0x46]).0, "PULU A,B,S");
    assert_eq!(put(&[0x1f, 0x8b]).0, "TFR A,DP");
    assert_eq!(put(&[0x26, 0xfe]).0, "BNE $1000");
    assert_eq!(put(&[0x10, 0x27, 0x00, 0x10]).0, "LBEQ $1014");
    assert_eq!(put(&[0x11, 0x3f]).0, "SWI3");
    assert_eq!(put(&[0x5f]).0, "CLRB");
    let i = disassemble(&ram, 0x1000);
    assert_eq!(i.flow, Flow::Normal);
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, mut ram) = setup(&[0x86, 0x42]);
    cpu.step(&mut ram);
    cpu.irq = true;
    cpu.wait = Wait::Cwai;
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();
    let mut other = M6809::new();
    other.load(&mut StateReader::new(&bytes)).unwrap();
    cpu.cycles = 0;
    cpu.target = 0;
    assert_eq!(other, cpu);
}
