//! 68020 unit tests: the new instructions and addressing modes against
//! the M68000 Family Programmer's Reference Manual (no bus-trace suite
//! exists for the 68020, see the crate doc).

use emu_core::{Bus, Savestate, Size, StateReader, StateWriter};

use crate::{cacr, fc, sr, Access, Model, NoWait, Step, TimedBus68, M68000};

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

// ---------------------------------------------------------------- cache and timing

/// RAM with a fixed number of wait states per bus cycle that records the
/// cycles (kind, function code, address, starting clock).
struct TBus {
    ram: Ram,
    waits: u32,
    cycles: Vec<(Access, u8, u32, u32)>,
}

impl Bus for TBus {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.ram.read(addr, size)
    }
    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.ram.peek(addr, size)
    }
    fn write(&mut self, addr: u32, size: Size, v: u32) {
        self.ram.write(addr, size, v);
    }
    fn addr_bits(&self) -> u8 {
        24
    }
}

impl TimedBus68 for TBus {
    fn bus_cycle(&mut self, access: Access, fc: u8, addr: u32, _: Size, t: u32) -> u32 {
        self.cycles.push((access, fc, addr, t));
        self.waits
    }
}

impl TBus {
    /// Instruction fetches recorded since the last call.
    fn take_fetches(&mut self) -> usize {
        let n = self
            .cycles
            .iter()
            .filter(|c| c.1 & 3 == 2 && c.0 == Access::Read)
            .count();
        self.cycles.clear();
        n
    }
}

/// A 68020 with `program` at $400 on a bus with `waits` wait states; with
/// `cached`, the cache is on and holds the program ($400–$4FF; the
/// handler at $600 shares its entries).
fn timed_setup(program: &[u16], cached: bool, waits: u32) -> (M68000, TBus) {
    let (mut cpu, ram) = setup(program);
    let bus = TBus {
        ram,
        waits,
        cycles: Vec::new(),
    };
    if cached {
        cpu.cacr = cacr::E;
        cpu.preload_cache(&bus, 0x400, 0x500, fc::SUPERVISOR_PROGRAM);
    }
    (cpu, bus)
}

/// Clocks of the next instruction on its own: from an idle bus controller
/// until the following instruction can start and the bus is idle again
/// (the manual's "no overlap" cases).
fn alone(cpu: &mut M68000, bus: &mut TBus) -> u32 {
    cpu.settle_020();
    match cpu.step_timed(bus) {
        Step::Done { clocks } => clocks + cpu.settle_020(),
        s => panic!("{s:?}"),
    }
}

/// Registers the timing programs use: A0/A1 point at long-aligned data,
/// D1 = 4 (an aligned index), A7 at a long-aligned stack whose top holds
/// a return address in the cached program area.
fn timing_regs(cpu: &mut M68000, bus: &mut TBus) {
    cpu.a[0] = 0x2000;
    cpu.a[1] = 0x2100;
    cpu.d[0] = 100;
    cpu.d[1] = 4;
    cpu.a[7] = 0x7ffc;
    bus.ram.write(0x7ffc, Size::Long, 0x480);
    bus.ram.write(0x2000, Size::Long, 0x2100);
    bus.ram.write(0x2104, Size::Long, 0x1234_5678);
}

/// One instruction alone, cached or not, with `waits` wait states.
fn time_of(program: &[u16], cached: bool, waits: u32, prep: fn(&mut M68000)) -> u32 {
    let (mut cpu, mut bus) = timed_setup(program, cached, waits);
    timing_regs(&mut cpu, &mut bus);
    prep(&mut cpu);
    alone(&mut cpu, &mut bus)
}

fn no_prep(_: &mut M68000) {}

/// (name, program, clocks, the table the number comes from).
type Case = (&'static str, &'static [u16], u32, &'static str);

/// Instructions in the cache: the "cache case" column of MC68020UM
/// section 8.2 (zero wait states, no overlap), the operation's entry plus
/// the effective address entry its footnote names.
const CACHE_CASES: &[Case] = &[
    ("NOP", &[0x4e71], 2, "8.2.16 NOP 2(0/0/0)"),
    ("MOVEQ #1,D0", &[0x7001], 2, "8.2.9 MOVEQ 2(0/0/0)"),
    (
        "ADD.L D1,D0",
        &[0xd081],
        2,
        "8.2.8 ADD EA,Dn 2 + 8.2.1 Dn 0",
    ),
    (
        "ADD.L (A0),D0",
        &[0xd090],
        6,
        "8.2.8 ADD EA,Dn 2 + 8.2.1 (An) 4(1/0/0)",
    ),
    (
        "ADD.L D0,(A0)",
        &[0xd190],
        8,
        "8.2.8 ADD Dn,EA 4(0/0/1) + 8.2.1 (An) 4(1/0/0)",
    ),
    (
        "ADD.L (0,A0,D1.W),D0",
        &[0xd0b0, 0x1000],
        9,
        "8.2.8 ADD EA,Dn 2 + 8.2.1 (d8,An,Xn) 7(1/0/0)",
    ),
    ("MOVE.L D0,(A0)", &[0x2080], 4, "8.2.6 Rn -> (An) 4(0/0/1)"),
    (
        "MOVE.L (A0),(A1)",
        &[0x2290],
        7,
        "8.2.6 (An) -> (An) 7(1/0/1)",
    ),
    (
        "MOVE.L (0,A0),D1",
        &[0x2228, 0x0000],
        7,
        "8.2.6 (d16,An) -> Dn 7(1/0/0)",
    ),
    (
        "MOVE.L #imm,D0",
        &[0x203c, 0x1234, 0x5678],
        6,
        "8.2.6 #<data>.L -> Dn 6(0/0/0)",
    ),
    (
        "MOVE.L ([0,A0],4),D0",
        &[0x2030, 0x0162, 0x0000, 0x0004],
        18,
        "8.2.6 ([d16,B],I,d16) -> Dn 18(2/0/0)",
    ),
    (
        "ADDI.L #imm,D0",
        &[0x0680, 0x1234, 0x5678],
        6,
        "8.2.9 ADDI #,Dn 2 + 8.2.2 #<data>.L,Dn 4",
    ),
    (
        "LEA (16,A0),A1",
        &[0x43e8, 0x0010],
        4,
        "8.2.16 LEA 2 + 8.2.3 (d16,An) 2",
    ),
    (
        "MULU.L D1,D0",
        &[0x4c01, 0x0000],
        45,
        "8.2.8 MUL.L 43 + 8.2.2 #<data>.W,Dn 2",
    ),
    (
        "DIVU.W D1,D0",
        &[0x80c1],
        44,
        "8.2.8 DIVU.W 44 + 8.2.1 Dn 0",
    ),
    ("LSL.L #2,D0", &[0xe588], 4, "8.2.12 LSL Dn (static) 4"),
    ("ROXL.L #2,D0", &[0xe590], 12, "8.2.12 ROXL Dn 12"),
    (
        "BFEXTU D0{4:8},D1",
        &[0xe9c0, 0x1108],
        8,
        "8.2.14 BFEXTU Dn 8",
    ),
    ("BRA.S", &[0x600e], 6, "8.2.15 Bcc (taken) 6"),
    (
        "BEQ.S (not taken)",
        &[0x670e],
        4,
        "8.2.15 Bcc.B (not taken) 4",
    ),
    (
        "DBRA D0 (loops)",
        &[0x51c8, 0x000e],
        6,
        "8.2.15 DBcc (false, not expired) 6",
    ),
    ("JMP (A0)", &[0x4ed0], 6, "8.2.16 JMP 4 + 8.2.5 (An) 2"),
    (
        "JSR (A0)",
        &[0x4e90],
        7,
        "8.2.16 JSR 5(0/0/1) + 8.2.5 (An) 2",
    ),
    ("RTS", &[0x4e75], 10, "8.2.16 RTS 10(1/0/0)"),
    (
        "MOVEM.L D0-D3,-(A7)",
        &[0x48e7, 0xf000],
        20,
        "8.2.7 MOVEM RL,EA 4+3n + 8.2.4 #<data>.W,(An)+ 4",
    ),
    (
        "MOVEM.L (A0),D0-D3",
        &[0x4cd0, 0x000f],
        26,
        "8.2.7 MOVEM EA,RL 8+4n + 8.2.4 #<data>.W,(An) 2",
    ),
    (
        "MOVEC D0,VBR",
        &[0x4e7b, 0x0801],
        12,
        "8.2.7 MOVEC Rn,Cr 12",
    ),
    (
        "CAS.W D2,D3,(A0) (fails)",
        &[0x0cd0, 0x00c2],
        14,
        "8.2.16 CAS unsuccessful 12(1/0/0) + 8.2.4 #<data>.W,(An) 2",
    ),
    ("TRAP #0", &[0x4e40], 20, "8.2.17 TRAP #n 20(1/0/4)"),
];

#[test]
fn cache_case_matches_the_timing_tables() {
    for &(name, program, expected, table) in CACHE_CASES {
        let (mut cpu, mut bus) = timed_setup(program, true, 0);
        timing_regs(&mut cpu, &mut bus);
        if name.starts_with("JMP") || name.starts_with("JSR") {
            cpu.a[0] = 0x440;
        }
        if name.starts_with("CAS") {
            cpu.d[2] = 0x5555;
        }
        if name.starts_with("TRAP") {
            // The handler at $600 shares the program's cache entries.
            cpu.preload_cache(&bus, 0x600, 0x608, fc::SUPERVISOR_PROGRAM);
        }
        assert_eq!(alone(&mut cpu, &mut bus), expected, "{name}: {table}");
    }
}

#[test]
fn cas_success_takes_the_successful_row() {
    // CAS.W D2,D3,(A0) with D2 equal to the operand (0): 8.2.16 CAS
    // (successful compare) 15(1/0/1) + 8.2.4 #<data>.W,(An) 2.
    let t = time_of(&[0x0cd0, 0x00c2], true, 0, |cpu| cpu.d[2] = 0);
    assert_eq!(t, 17);
}

/// Without the cache the prefetches add their bus cycles where they do
/// not overlap the internal clocks; the flow changes and the simplest
/// instructions reach the manual's worst case.
#[test]
fn no_cache_adds_the_prefetches() {
    let cases: &[Case] = &[
        ("NOP", &[0x4e71], 3, "8.2.16 NOP worst case 3(0/1/0)"),
        ("ADD.L D1,D0", &[0xd081], 3, "8.2.8 ADD EA,Dn 3(0/1/0)"),
        (
            "ADD.L (A0),D0",
            &[0xd090],
            7,
            "8.2.8 ADD EA,Dn 3(0/1/0) + 8.2.1 (An) 4(1/0/0)",
        ),
        (
            "ADD.L D0,(A0)",
            &[0xd190],
            10,
            "8.2.8 ADD Dn,EA 6(0/1/1) + 8.2.1 (An) 4(1/0/0)",
        ),
        ("DIVU.W D1,D0", &[0x80c1], 44, "8.2.8 DIVU.W 44(0/1/0)"),
        ("BRA.S", &[0x600e], 9, "8.2.15 Bcc (taken) 9(0/2/0)"),
        (
            "DBRA D0 (loops)",
            &[0x51c8, 0x000e],
            9,
            "8.2.15 DBcc (false, not expired) 9(0/2/0)",
        ),
        (
            "JMP (A0)",
            &[0x4ed0],
            9,
            "8.2.16 JMP 7(0/2/0) + 8.2.5 (An) 2(0/0/0)",
        ),
    ];
    for &(name, program, expected, table) in cases {
        let t = time_of(program, false, 0, |cpu| cpu.a[0] = 0x440);
        let t = if name.starts_with("JMP") {
            t
        } else {
            time_of(program, false, 0, no_prep)
        };
        assert_eq!(t, expected, "{name}: {table}");
    }
    // Never faster than in the cache.
    for &(name, program, cc, _) in CACHE_CASES {
        let t = time_of(program, false, 0, |cpu| {
            if cpu.prefetch_queue()[0] & 0xffc0 == 0x4e80 {
                cpu.a[0] = 0x440;
            }
        });
        assert!(t >= cc, "{name}: {t} without the cache, {cc} with");
    }
}

#[test]
fn wait_states_stretch_only_real_bus_cycles() {
    // In the cache, one wait state per cycle costs a NOP nothing and each
    // operand cycle one clock (MC68020UM 8.1.5, example 4).
    assert_eq!(time_of(&[0x4e71], true, 1, no_prep), 2);
    assert_eq!(time_of(&[0xd090], true, 1, no_prep), 7);
    assert_eq!(time_of(&[0x2290], true, 1, no_prep), 9);
    // Without it the prefetch's cycle grows too.
    assert_eq!(time_of(&[0x4e71], false, 1, no_prep), 4);
    assert_eq!(time_of(&[0xd090], false, 1, no_prep), 9);
}

/// MOVEQ #1,D2; MOVEC D2,CACR (enable); loop: ADDQ.L #1,D0; DBRA D1,loop.
const LOOP: &[u16] = &[0x7401, 0x4e7b, 0x2002, 0x5280, 0x51c9, 0xfffc];

fn steps_on(cpu: &mut M68000, bus: &mut TBus, n: usize) {
    for _ in 0..n {
        cpu.step_timed(bus);
    }
}

fn set_cacr(cpu: &mut M68000, v: u32) {
    use emu_core::Cpu;
    cpu.set_reg("CACR", v);
}

#[test]
fn cache_hits_take_no_bus_cycles() {
    let (mut cpu, mut bus) = timed_setup(LOOP, false, 0);
    cpu.d[1] = 100;
    steps_on(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.cacr, cacr::E);
    bus.take_fetches();
    // First pass: the loop's long words come over the bus.
    steps_on(&mut cpu, &mut bus, 2);
    assert!(bus.take_fetches() > 0);
    // Then they come from the cache.
    for _ in 0..5 {
        steps_on(&mut cpu, &mut bus, 2);
        assert_eq!(bus.take_fetches(), 0);
    }
    // Disabling it does not flush it; while it is off every pass fetches.
    set_cacr(&mut cpu, 0);
    for _ in 0..2 {
        steps_on(&mut cpu, &mut bus, 2);
        assert!(bus.take_fetches() > 0);
    }
    set_cacr(&mut cpu, cacr::E);
    steps_on(&mut cpu, &mut bus, 2);
    assert_eq!(bus.take_fetches(), 0, "the entries stayed valid");
}

#[test]
fn the_tag_holds_fc2() {
    // loop: ADDQ.L #1,D0; BRA.S loop.
    let (mut cpu, mut bus) = timed_setup(&[0x5280, 0x60fc], true, 0);
    steps_on(&mut cpu, &mut bus, 4);
    assert_eq!(bus.take_fetches(), 0, "supervisor program space cached");
    // The same addresses in user program space miss, then hit too.
    cpu.set_sr(cpu.sr() & !sr::S);
    steps_on(&mut cpu, &mut bus, 2);
    assert!(bus.cycles.iter().any(|c| c.1 == fc::USER_PROGRAM));
    bus.take_fetches();
    steps_on(&mut cpu, &mut bus, 4);
    assert_eq!(bus.take_fetches(), 0, "both are cached now");
}

#[test]
fn freeze_keeps_the_entries() {
    let (mut cpu, mut bus) = timed_setup(&[0x5280, 0x60fc], false, 0);
    // Frozen before anything was cached: misses are not filled.
    set_cacr(&mut cpu, cacr::E | cacr::F);
    steps_on(&mut cpu, &mut bus, 2);
    bus.take_fetches();
    steps_on(&mut cpu, &mut bus, 2);
    assert!(bus.take_fetches() > 0);
    // Unfrozen they are; frozen again, the cached loop stays.
    set_cacr(&mut cpu, cacr::E);
    steps_on(&mut cpu, &mut bus, 2);
    set_cacr(&mut cpu, cacr::E | cacr::F);
    bus.take_fetches();
    steps_on(&mut cpu, &mut bus, 4);
    assert_eq!(bus.take_fetches(), 0);
}

#[test]
fn clear_entry_and_clear_cache() {
    // MOVE.L #$404,D2; MOVEC D2,CAAR; MOVEQ #5,D3; MOVEC D3,CACR (CE|E);
    // MOVEQ #9,D4; MOVEC D4,CACR (C|E).
    let program = [
        0x243c, 0x0000, 0x0404, 0x4e7b, 0x2802, 0x7605, 0x4e7b, 0x3002, 0x7809, 0x4e7b, 0x4002,
    ];
    let (mut cpu, mut bus) = timed_setup(&program, true, 0);
    let cached = |cpu: &M68000, a: u32| {
        cpu.icache
            .lookup(cpu.cacr, a, fc::SUPERVISOR_PROGRAM)
            .is_some()
    };
    steps_on(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.caar, 0x404);
    assert!(!cached(&cpu, 0x404), "the entry CAAR selects is cleared");
    assert!(cached(&cpu, 0x400) && cached(&cpu, 0x408));
    assert_eq!(cpu.cacr, cacr::E, "CE reads as zero");
    steps_on(&mut cpu, &mut bus, 2);
    assert!((0x440..0x500).step_by(4).all(|a| !cached(&cpu, a)));
    assert_eq!(cpu.cacr, cacr::E, "C reads as zero");
}

/// Enable the cache (or not), JSR $440; MOVE.W #$7002,$440 (MOVEQ #2,D0
/// over MOVEQ #1,D0); JSR $440; MOVEQ #9,D3; MOVEC D3,CACR (C|E); JSR
/// $440. At $440: MOVEQ #1,D0; RTS. D0 after each JSR's return.
fn self_modifying(enable: bool) -> Vec<u32> {
    let (mut cpu, mut bus) = timed_setup(
        &[
            if enable { 0x7401 } else { 0x7400 },
            0x4e7b,
            0x2002,
            0x4eb8,
            0x0440,
            0x31fc,
            0x7002,
            0x0440,
            0x4eb8,
            0x0440,
            0x7609,
            0x4e7b,
            0x3002,
            0x4eb8,
            0x0440,
        ],
        false,
        0,
    );
    bus.ram.write(0x440, Size::Long, 0x7001_4e75);
    steps_on(&mut cpu, &mut bus, 2);
    let mut d0 = Vec::new();
    for n in [3, 4, 5] {
        steps_on(&mut cpu, &mut bus, n);
        d0.push(cpu.d[0]);
    }
    d0
}

#[test]
fn self_modifying_code_needs_a_cache_clear() {
    // In the cache the old instruction runs until the cache is cleared.
    assert_eq!(self_modifying(true), [1, 1, 2]);
    // Without the cache the new one runs at once.
    assert_eq!(self_modifying(false), [1, 2, 2]);
}

#[test]
fn reset_clears_the_cache() {
    let (mut cpu, mut bus) = timed_setup(&[0x4e71], true, 0);
    let held = |cpu: &M68000| {
        cpu.icache
            .lookup(cacr::E, 0x400, fc::SUPERVISOR_PROGRAM)
            .is_some()
    };
    assert!(held(&cpu));
    cpu.reset_timed(&mut bus);
    assert_eq!(cpu.cacr, 0);
    assert!(!held(&cpu));
}

#[test]
fn savestate_keeps_cache_and_pipeline() {
    let (mut cpu, mut bus) = timed_setup(LOOP, false, 1);
    cpu.d[1] = 50;
    steps_on(&mut cpu, &mut bus, 9);
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let data = w.into_bytes();
    let mut other = M68000::with_model(Model::Mc68020);
    other.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(other.icache, cpu.icache);
    assert_eq!(other.pipe.bus_free, cpu.pipe.bus_free);
    assert_eq!(other.prefetch_queue(), cpu.prefetch_queue());
    let mut bus2 = TBus {
        ram: Ram(bus.ram.0.clone()),
        waits: 1,
        cycles: Vec::new(),
    };
    bus.cycles.clear();
    for _ in 0..20 {
        assert_eq!(cpu.step_timed(&mut bus), other.step_timed(&mut bus2));
    }
    assert_eq!(bus.cycles, bus2.cycles);
}
