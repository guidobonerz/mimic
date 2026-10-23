//! 68020 unit tests: the new instructions and addressing modes against
//! the M68000 Family Programmer's Reference Manual (no bus-trace suite
//! exists for the 68020, see the crate doc).

use emu_core::{Bus, Savestate, Size, StateReader, StateWriter};

use crate::{sr, Model, NoWait, Step, M68000};

/// 64 KB of RAM without wait states.
struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }
    fn peek(&self, addr: u32, size: Size) -> u32 {
        let b = |a: u32| u32::from(self.0[(a & 0xffff) as usize]);
        match size {
            Size::Byte => b(addr),
            Size::Word => b(addr) << 8 | b(addr + 1),
            Size::Long => b(addr) << 24 | b(addr + 1) << 16 | b(addr + 2) << 8 | b(addr + 3),
        }
    }
    fn write(&mut self, addr: u32, size: Size, v: u32) {
        let n = match size {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        };
        for i in 0..n {
            self.0[((addr + i) & 0xffff) as usize] = (v >> (8 * (n - 1 - i))) as u8;
        }
    }
    fn addr_bits(&self) -> u8 {
        24
    }
}

/// A 68020 after reset with `program` at $400 (SSP $8000, every exception
/// vector pointing at a STOP at $600).
fn setup(program: &[u16]) -> (M68000, Ram) {
    let mut ram = Ram(vec![0; 0x10000]);
    ram.write(0, Size::Long, 0x8000);
    ram.write(4, Size::Long, 0x400);
    for v in 2..64u32 {
        ram.write(v * 4, Size::Long, 0x600);
    }
    // $600: STOP #$2700 (the handler); $604: NOP.
    ram.write(0x600, Size::Word, 0x4e72);
    ram.write(0x602, Size::Word, 0x2700);
    for (i, w) in program.iter().enumerate() {
        ram.write(0x400 + 2 * i as u32, Size::Word, u32::from(*w));
    }
    // NOPs after the program.
    for i in program.len()..program.len() + 8 {
        ram.write(0x400 + 2 * i as u32, Size::Word, 0x4e71);
    }
    let mut cpu = M68000::with_model(Model::Mc68020);
    cpu.reset_timed(&mut NoWait(&mut ram));
    (cpu, ram)
}

fn step(cpu: &mut M68000, ram: &mut Ram) -> Step {
    cpu.step_timed(&mut NoWait(ram))
}

fn steps(cpu: &mut M68000, ram: &mut Ram, n: usize) {
    for _ in 0..n {
        step(cpu, ram);
    }
}

fn flags(cpu: &M68000) -> u16 {
    cpu.sr() & 0x1f
}

#[test]
fn extb_and_bcc_long() {
    // EXTB.L D0; BRA.L +$10000 (lands at $1_0406 & $FFFF = $406 in this
    // 64 KB RAM, so check the PC instead).
    let (mut cpu, mut ram) = setup(&[0x49c0, 0x60ff, 0x0001, 0x0000]);
    cpu.d[0] = 0x1234_56f0;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[0], 0xffff_fff0);
    assert_eq!(flags(&cpu) & (sr::N | sr::Z), sr::N);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x404 + 0x1_0000);
}

#[test]
fn long_multiply_and_divide() {
    let (mut cpu, mut ram) = setup(&[
        0x4c3c, 0x1000, 0x0000, 0x0003, // MULU.L #3,D1
        0x4c00, 0x3c02, // MULS.L D0,D2:D3
        0x4c7c, 0x1000, 0x0000, 0x0007, // DIVU.L #7,D1
        0x4c7c, 0x3c02, 0xffff, 0xfffd, // DIVS.L #-3,D2:D3 (64-bit dividend)
        0x4c7c, 0x4c04, 0x0000, 0x0002, // DIVU.L #2,D4:D4 (64-bit, overflows)
        0x4c7c, 0x5005, 0x0000, 0x0000, // DIVU.L #0,D5
    ]);
    cpu.d[1] = 0x8000_0000;
    cpu.d[0] = 4;
    cpu.d[3] = 0xffff_fffd;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[1], 0x8000_0000, "low 32 bits of 3 × 2^31");
    assert_eq!(flags(&cpu) & (sr::V | sr::N | sr::Z), sr::V | sr::N);
    step(&mut cpu, &mut ram);
    assert_eq!((cpu.d[2], cpu.d[3]), (0xffff_ffff, 0xffff_fff4), "-3 × 4");
    assert_eq!(flags(&cpu) & (sr::V | sr::N | sr::Z), sr::N);
    cpu.d[1] = 100;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[1], 14, "100 / 7, remainder discarded");
    assert_eq!(flags(&cpu) & (sr::V | sr::N | sr::Z | sr::C), 0);
    cpu.d[2] = 0;
    cpu.d[3] = 10;
    step(&mut cpu, &mut ram);
    assert_eq!((cpu.d[2], cpu.d[3]), (1, 0xffff_fffd), "10 / -3 = -3 rem 1");
    assert_eq!(flags(&cpu) & sr::N, sr::N);
    cpu.d[4] = 0x0000_0001;
    cpu.d[4] = 0x8000_0000;
    step(&mut cpu, &mut ram);
    assert_eq!(flags(&cpu) & sr::V, sr::V, "2^63 / 2 does not fit");
    assert_eq!(cpu.d[4], 0x8000_0000, "unchanged");
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x600, "division by zero trapped");
    // Format 2 frame: SR, PC of the next instruction, $2014, the
    // instruction's address.
    let sp = cpu.a[7];
    assert_eq!(ram.peek(sp + 6, Size::Word), 0x2014);
    assert_eq!(ram.peek(sp + 2, Size::Long), 0x400 + 2 * 22);
    assert_eq!(ram.peek(sp + 8, Size::Long), 0x400 + 2 * 18);
}

#[test]
fn full_format_extension_words() {
    let (mut cpu, mut ram) = setup(&[
        0x45f0, 0x1d20, 0x1234, // LEA ($1234,A0,D1.L*4),A2
        0x2030, 0x1326, 0x0100, 0x0020, // MOVE.L ([$100,A0],D1.W*2,$20),D0
        0x41f0, 0x01e1, 0x2000, // LEA ([$2000]),A0 (base and index suppressed)
        0x4eb0, 0x01a0, 0x2000, // JSR ($2000,ZA0,D0.W)
    ]);
    cpu.a[0] = 0x1000;
    cpu.d[1] = 3;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.a[2], 0x1000 + 0x1234 + 12);
    cpu.d[1] = 2;
    ram.write(0x1100, Size::Long, 0x3000);
    ram.write(0x3024, Size::Long, 0xdead_beef);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[0], 0xdead_beef);
    ram.write(0x2000, Size::Long, 0x5000);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.a[0], 0x5000);
    cpu.d[0] = 0x10;
    ram.write(0x2010, Size::Word, 0x4e75); // RTS
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x2010);
    assert_eq!(
        ram.peek(cpu.a[7], Size::Long),
        0x400 + 2 * 13,
        "return address"
    );
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x400 + 2 * 13);
}

#[test]
fn bit_fields() {
    let (mut cpu, mut ram) = setup(&[
        0xe9c0, 0x1108, // BFEXTU D0{4:8},D1
        0xeed0, 0x0083, // BFSET (A0){2:3}
        0xedc0, 0x1000, // BFFFO D0{0:32},D1
        0xefc0, 0x1708, // BFINS D1,D0{28:8}
        0xebd0, 0x27c4, // BFEXTS (A0){31:4},D2
        0xe8d0, 0x0000, // BFTST (A0){0:32}
    ]);
    cpu.d[0] = 0x1234_5678;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[1], 0x23);
    cpu.a[0] = 0x2000;
    step(&mut cpu, &mut ram);
    assert_eq!(ram.peek(0x2000, Size::Byte), 0x38);
    assert_eq!(flags(&cpu) & sr::Z, sr::Z, "the field was zero before");
    cpu.d[0] = 0x0010_0000;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[1], 11);
    cpu.d[0] = 0;
    cpu.d[1] = 0xab;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[0], 0xb000_000a, "the field wraps around the register");
    assert_eq!(flags(&cpu) & sr::N, sr::N);
    // Bits 31–34 of the bytes at $2000: byte 3 bit 0 and byte 4 bits 7–5.
    ram.write(0x2003, Size::Byte, 0x01);
    ram.write(0x2004, Size::Byte, 0x40);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[2], 0xffff_fffa, "1010 sign-extended");
    step(&mut cpu, &mut ram);
    assert_eq!(flags(&cpu) & (sr::N | sr::Z), 0);
}

#[test]
fn cas_cas2_chk2_cmp2() {
    let (mut cpu, mut ram) = setup(&[
        0x0cd0, 0x0040, // CAS.W D0,D1,(A0)
        0x0cd0, 0x0040, // CAS.W D0,D1,(A0)
        0x0efc, 0x8080, 0x90c1, // CAS2.L D0:D1,D2:D3,(A0):(A1)
        0x00d0, 0x0000, // CMP2.B (A0),D0
        0x00d0, 0x0000, // CMP2.B (A0),D0
        0x00d0, 0x0800, // CHK2.B (A0),D0
    ]);
    cpu.a[0] = 0x2000;
    ram.write(0x2000, Size::Word, 0x1234);
    cpu.d[0] = 0x1234;
    cpu.d[1] = 0x5678;
    step(&mut cpu, &mut ram);
    assert_eq!(ram.peek(0x2000, Size::Word), 0x5678, "equal: Du stored");
    assert_eq!(flags(&cpu) & sr::Z, sr::Z);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[0], 0x5678, "not equal: Dc takes the operand");
    assert_eq!(flags(&cpu) & sr::Z, 0);
    cpu.a[1] = 0x2010;
    ram.write(0x2000, Size::Long, 0x1111_1111);
    ram.write(0x2010, Size::Long, 0x2222_2222);
    cpu.d[0] = 0x1111_1111;
    cpu.d[1] = 0x2222_2222;
    cpu.d[2] = 0xaaaa_aaaa;
    cpu.d[3] = 0xbbbb_bbbb;
    step(&mut cpu, &mut ram);
    assert_eq!(ram.peek(0x2000, Size::Long), 0xaaaa_aaaa);
    assert_eq!(ram.peek(0x2010, Size::Long), 0xbbbb_bbbb);
    // Bounds 10 and 20.
    ram.write(0x2000, Size::Byte, 10);
    ram.write(0x2001, Size::Byte, 20);
    cpu.d[0] = 15;
    step(&mut cpu, &mut ram);
    assert_eq!(flags(&cpu) & (sr::C | sr::Z), 0, "inside");
    cpu.d[0] = 20;
    step(&mut cpu, &mut ram);
    assert_eq!(flags(&cpu) & (sr::C | sr::Z), sr::Z, "on the bound");
    cpu.d[0] = 25;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x600, "CHK2 trapped");
    assert_eq!(
        ram.peek(cpu.a[7] + 6, Size::Word),
        0x2018,
        "format 2, vector 6"
    );
}

#[test]
fn pack_unpk_rtd_link_long() {
    let (mut cpu, mut ram) = setup(&[
        0x8340, 0x0000, // PACK D0,D1,#0
        0x8380, 0x3030, // UNPK D0,D1,#$3030
        0x8b4c, 0x0000, // PACK -(A4),-(A5),#0
        0x4808, 0xffff, 0xff00, // LINK.L A0,#-$100
        0x4e74, 0x0008, // RTD #8
    ]);
    cpu.d[0] = 0x0102;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[1] & 0xff, 0x12);
    cpu.d[0] = 0x12;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[1] & 0xffff, 0x3132);
    cpu.a[4] = 0x2002;
    cpu.a[5] = 0x2100;
    ram.write(0x2000, Size::Word, 0x0304);
    step(&mut cpu, &mut ram);
    assert_eq!((cpu.a[4], cpu.a[5]), (0x2000, 0x20ff));
    assert_eq!(ram.peek(0x20ff, Size::Byte), 0x34);
    cpu.a[0] = 0x1111_1111;
    let sp = cpu.a[7];
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.a[0], sp - 4);
    assert_eq!(cpu.a[7], sp - 4 - 0x100);
    assert_eq!(ram.peek(sp - 4, Size::Long), 0x1111_1111);
    cpu.a[7] = 0x7000;
    ram.write(0x7000, Size::Long, 0x500);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x500);
    assert_eq!(cpu.a[7], 0x700c);
}

#[test]
fn movec_vbr_frames_rte_and_trapcc() {
    let (mut cpu, mut ram) = setup(&[
        0x4e7b, 0x0801, // MOVEC D0,VBR
        0x4e40, // TRAP #0
        0x4e71, // NOP
        0x57fc, // TRAPEQ (Z set)
        0x4e73, // RTE on a format 9 frame
    ]);
    // The relocated table at $1000: TRAP #0 → $700 (RTE), TRAPcc → $600.
    for v in 0..64u32 {
        ram.write(0x1000 + v * 4, Size::Long, 0x600);
    }
    ram.write(0x1000 + 32 * 4, Size::Long, 0x700);
    ram.write(0x700, Size::Word, 0x4e73);
    cpu.d[0] = 0x1000;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.vbr, 0x1000);
    let sp = cpu.a[7];
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x700);
    assert_eq!(cpu.a[7], sp - 8, "format 0 frame");
    assert_eq!(ram.peek(sp - 2, Size::Word), 0x0080, "vector offset $80");
    assert_eq!(ram.peek(sp - 6, Size::Long), 0x406);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x406, "RTE");
    assert_eq!(cpu.a[7], sp);
    step(&mut cpu, &mut ram); // NOP
    cpu.set_sr(cpu.sr() | sr::Z);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x600, "TRAPEQ taken");
    assert_eq!(ram.peek(cpu.a[7] + 6, Size::Word), 0x201c);
    assert_eq!(
        ram.peek(cpu.a[7] + 8, Size::Long),
        0x408,
        "instruction address"
    );
    // Back at $40A with a bogus frame: format error (vector 14).
    cpu.set_pc_and_queue(0x40a, [0x4e73, 0x4e71]);
    cpu.a[7] = 0x7000;
    ram.write(0x7000, Size::Word, 0x2000);
    ram.write(0x7002, Size::Long, 0x500);
    ram.write(0x7006, Size::Word, 0x9000);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x600);
    assert_eq!(ram.peek(cpu.a[7] + 6, Size::Word), 14 * 4);
    assert_eq!(ram.peek(cpu.a[7] + 2, Size::Long), 0x40a, "the RTE itself");
}

#[test]
fn master_and_interrupt_stacks() {
    let (mut cpu, mut ram) = setup(&[
        0x4e7b, 0x0803, // MOVEC D0,MSP
        0x46fc, 0x3000, // MOVE #$3000,SR (M set)
        0x4e71, 0x4e71,
    ]);
    ram.write(0x1000 + 26 * 4, Size::Long, 0x600);
    cpu.d[0] = 0x6000;
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.msp(), 0x6000);
    assert_eq!(cpu.isp(), 0x8000);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.a[7], 0x6000, "A7 is the MSP now");
    // An interrupt: the frame goes to the master stack, then a throwaway
    // frame to the interrupt stack and M is cleared.
    cpu.set_ipl(2);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x600);
    assert_eq!(cpu.sr() & sr::M, 0);
    assert_eq!(cpu.a[7], 0x8000 - 8);
    assert_eq!(cpu.msp(), 0x6000 - 8);
    assert_eq!(
        ram.peek(0x8000 - 2, Size::Word),
        0x1000 | (26 * 4),
        "format 1"
    );
    assert_eq!(ram.peek(0x6000 - 2, Size::Word), 26 * 4, "format 0");
    // RTE drops the throwaway frame and returns through the master stack.
    cpu.set_pc_and_queue(0x600, [0x4e73, 0x4e71]);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.pc(), 0x408);
    assert_eq!(cpu.sr() & sr::M, sr::M);
    assert_eq!(cpu.a[7], 0x6000);
}

#[test]
fn misaligned_accesses_and_privilege() {
    let (mut cpu, mut ram) = setup(&[
        0x2028, 0x0001, // MOVE.L 1(A0),D0
        0x2140, 0x0003, // MOVE.L D0,3(A0)
        0x46fc, 0x0000, // MOVE #0,SR (user mode)
        0x40c1, // MOVE SR,D1
    ]);
    cpu.a[0] = 0x2000;
    ram.write(0x2000, Size::Long, 0x0011_2233);
    ram.write(0x2004, Size::Long, 0x4455_6677);
    step(&mut cpu, &mut ram);
    assert_eq!(cpu.d[0], 0x1122_3344, "no address error");
    step(&mut cpu, &mut ram);
    assert_eq!(ram.peek(0x2000, Size::Long), 0x0011_2211);
    assert_eq!(ram.peek(0x2004, Size::Long), 0x2233_4477);
    step(&mut cpu, &mut ram);
    assert!(!cpu.supervisor());
    assert_eq!(cpu.a[7], 0, "USP");
    cpu.a[7] = 0x7000;
    let mut cpu2 = cpu.clone();
    step(&mut cpu, &mut ram);
    assert!(cpu.supervisor(), "privilege violation");
    assert_eq!(ram.peek(cpu.a[7] + 6, Size::Word), 8 * 4);
    // The same on a 68000 is allowed.
    cpu2.model = Model::Mc68000;
    let mut ram2 = Ram(ram.0.clone());
    cpu2.step_timed(&mut NoWait(&mut ram2));
    assert!(!cpu2.supervisor());
    assert_eq!(cpu2.d[1] & 0xffff, u32::from(cpu2.sr()));
}

#[test]
fn savestate_keeps_the_control_registers() {
    let (mut cpu, mut ram) = setup(&[0x4e7b, 0x0801, 0x4e7b, 0x1002]);
    cpu.d[0] = 0x1234;
    cpu.d[1] = 1;
    steps(&mut cpu, &mut ram, 2);
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let data = w.into_bytes();
    let mut other = M68000::with_model(Model::Mc68020);
    other.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!((other.vbr, other.cacr), (0x1234, 1));
    use emu_core::Cpu;
    assert_eq!(other.regs(), cpu.regs());
}
