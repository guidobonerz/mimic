//! The 9900 instruction by instruction: results, status bits and clock
//! cycles as the Data Manual gives them (Table 3 and the address
//! modification times), on a flat 64 KB memory.

use super::*;

/// 64 KB of memory, 4096 CRU bits, and wait states at `>A000` and up.
struct TestBus {
    mem: Vec<u8>,
    cru_in: [bool; 4096],
    cru_out: Vec<(u16, bool)>,
    waits: u32,
    externals: Vec<External>,
}

impl TestBus {
    fn new() -> Self {
        let mut b = Self {
            mem: vec![0; 0x10000],
            cru_in: [false; 4096],
            cru_out: Vec::new(),
            waits: 0,
            externals: Vec::new(),
        };
        // RESET vector: WP >8300, PC >0100.
        b.put(0x0000, 0x8300);
        b.put(0x0002, 0x0100);
        b
    }

    fn put(&mut self, addr: u16, v: u16) {
        let a = usize::from(addr & 0xfffe);
        self.mem[a..a + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn get(&self, addr: u16) -> u16 {
        let a = usize::from(addr & 0xfffe);
        u16::from_be_bytes([self.mem[a], self.mem[a + 1]])
    }

    /// The program at >0100.
    fn program(&mut self, words: &[u16]) {
        for (i, w) in words.iter().enumerate() {
            self.put(0x0100 + 2 * i as u16, *w);
        }
    }

    fn reg(&self, r: u16) -> u16 {
        self.get(0x8300 + 2 * r)
    }

    fn set_reg(&mut self, r: u16, v: u16) {
        self.put(0x8300 + 2 * r, v);
    }
}

impl Bus for TestBus {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        u32::from(self.get(addr as u16))
    }
    fn peek(&self, addr: u32, size: Size) -> u32 {
        match size {
            Size::Byte => u32::from(self.mem[addr as usize & 0xffff]),
            _ => u32::from(self.get(addr as u16)),
        }
    }
    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        self.put(addr as u16, v as u16);
    }
    fn addr_bits(&self) -> u8 {
        16
    }
}

impl Tms9900Bus for TestBus {
    fn wait_states(&mut self, addr: u16) -> u32 {
        if addr >= 0xa000 {
            self.waits
        } else {
            0
        }
    }
    fn cru_read(&mut self, bit: u16) -> bool {
        self.cru_in[usize::from(bit)]
    }
    fn cru_write(&mut self, bit: u16, value: bool) {
        self.cru_out.push((bit, value));
    }
    fn external(&mut self, op: External) {
        self.externals.push(op);
    }
}

/// A CPU after RESET with `program` at >0100.
fn setup(program: &[u16]) -> (Tms9900, TestBus) {
    let mut bus = TestBus::new();
    bus.program(program);
    let mut cpu = Tms9900::new();
    assert_eq!(cpu.step(&mut bus), 26, "RESET");
    assert_eq!((cpu.wp, cpu.pc, cpu.st), (0x8300, 0x0100, 0));
    (cpu, bus)
}

const LGT: u16 = st::LGT;
const AGT: u16 = st::AGT;
const EQ: u16 = st::EQ;
const C: u16 = st::C;
const OV: u16 = st::OV;
const OP: u16 = st::OP;

fn flags(cpu: &Tms9900) -> u16 {
    cpu.st & 0xfc00
}

#[test]
fn li_sets_the_register_and_compares_with_zero() {
    let (mut cpu, mut bus) = setup(&[0x0201, 0x8000, 0x0202, 0x0000, 0x0203, 0x0005]);
    assert_eq!(cpu.step(&mut bus), 12);
    assert_eq!(bus.reg(1), 0x8000);
    assert_eq!(flags(&cpu), LGT);
    cpu.step(&mut bus);
    assert_eq!(flags(&cpu), EQ);
    cpu.step(&mut bus);
    assert_eq!(flags(&cpu), LGT | AGT);
    assert_eq!(cpu.pc, 0x010c);
}

#[test]
fn mov_with_every_addressing_mode_and_its_time() {
    // MOV R1,R2; MOV *R1,R3; MOV *R1+,R4; MOV @>A000,R5;
    // MOV @>0002(R1),@>0004(R6)
    let (mut cpu, mut bus) = setup(&[
        0xc081, 0xc0d1, 0xc131, 0xc160, 0xa000, 0xc9a1, 0x0002, 0x0004,
    ]);
    bus.set_reg(1, 0xa000);
    bus.set_reg(6, 0xb000);
    bus.put(0xa000, 0x1234);
    bus.put(0xa002, 0x5678);
    bus.put(0xa004, 0x9abc);
    assert_eq!(cpu.step(&mut bus), 14);
    assert_eq!(bus.reg(2), 0xa000);
    assert_eq!(cpu.step(&mut bus), 18, "*R: +4");
    assert_eq!(bus.reg(3), 0x1234);
    assert_eq!(cpu.step(&mut bus), 22, "*R+ word: +8");
    assert_eq!((bus.reg(4), bus.reg(1)), (0x1234, 0xa002));
    assert_eq!(cpu.step(&mut bus), 22, "@sym: +8");
    assert_eq!(bus.reg(5), 0x1234);
    assert_eq!(cpu.step(&mut bus), 30, "@sym(R) on both: +16");
    assert_eq!(bus.get(0xb004), 0x9abc);
    assert_eq!(flags(&cpu), LGT);
}

#[test]
fn wait_states_count_per_memory_access() {
    // MOV *R1,*R2 with both in waited memory: 4 accesses there
    // (source read, destination read before write, write; R1/R2 in pad).
    let (mut cpu, mut bus) = setup(&[0xc491]);
    bus.waits = 4;
    bus.set_reg(1, 0xa000);
    bus.set_reg(2, 0xa002);
    assert_eq!(cpu.step(&mut bus), 14 + 4 + 4 + 3 * 4);
}

#[test]
fn movb_moves_one_byte_and_sets_parity() {
    // MOVB R1,@>A001; MOVB @>A001,R2; MOVB *R3+,R4 (twice)
    let (mut cpu, mut bus) = setup(&[0xd801, 0xa001, 0xd0a0, 0xa001, 0xd133, 0xd133]);
    bus.set_reg(1, 0x07ff);
    bus.put(0xa000, 0xaabb);
    bus.set_reg(2, 0x1122);
    bus.set_reg(3, 0xa000);
    cpu.step(&mut bus);
    assert_eq!(bus.get(0xa000), 0xaa07, "the low byte only");
    assert_eq!(flags(&cpu), LGT | AGT | OP, "07 has three ones");
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 0x0722, "the register's high byte");
    assert_eq!(cpu.step(&mut bus), 20, "*R+ byte: +6");
    assert_eq!((bus.reg(4) >> 8, bus.reg(3)), (0xaa, 0xa001));
    cpu.step(&mut bus);
    assert_eq!(bus.reg(4) >> 8, 0x07);
}

#[test]
fn add_and_subtract_set_carry_and_overflow() {
    // A R1,R2 / S R1,R2 / AB R1,R2
    let (mut cpu, mut bus) = setup(&[0xa081, 0xa081, 0x6081, 0x6081, 0xb081]);
    bus.set_reg(1, 0x0001);
    bus.set_reg(2, 0x7fff);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 0x8000);
    assert_eq!(flags(&cpu), LGT | OV);
    bus.set_reg(1, 0x8000);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 0x0000);
    assert_eq!(flags(&cpu), EQ | C | OV);
    bus.set_reg(1, 3);
    bus.set_reg(2, 5);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 2);
    assert_eq!(flags(&cpu), LGT | AGT | C, "no borrow");
    bus.set_reg(1, 5);
    bus.set_reg(2, 3);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 0xfffe);
    assert_eq!(flags(&cpu), LGT, "borrow: carry clear");
    bus.set_reg(1, 0x8001);
    bus.set_reg(2, 0x80ff);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 0x00ff, "high bytes 80 + 80, low byte kept");
    assert_eq!(flags(&cpu), EQ | C | OV);
}

#[test]
fn compare_words_and_bytes() {
    // C R1,R2; CB R1,R2; CI R1,>0005
    let (mut cpu, mut bus) = setup(&[0x8081, 0x9081, 0x0281, 0x0005, 0x8081]);
    bus.set_reg(1, 0xffff);
    bus.set_reg(2, 0x0001);
    assert_eq!(cpu.step(&mut bus), 14);
    assert_eq!(flags(&cpu), LGT, "logically greater, arithmetically less");
    assert_eq!(bus.reg(2), 1, "no store");
    cpu.step(&mut bus);
    assert_eq!(flags(&cpu), LGT, "bytes ff and 00; ff has even parity");
    bus.set_reg(1, 0x0007);
    assert_eq!(cpu.step(&mut bus), 14);
    assert_eq!(flags(&cpu), LGT | AGT, "register 7 > 5");
    bus.set_reg(1, 1);
    cpu.step(&mut bus);
    assert_eq!(flags(&cpu), EQ);
}

#[test]
fn logic_instructions() {
    // SZC R1,R2; SOC R1,R3; COC R1,R4; CZC R1,R5; XOR R1,R6; ANDI R7,>0F0F; ORI R7,>F000
    let (mut cpu, mut bus) = setup(&[
        0x4081, 0xe0c1, 0x2101, 0x2541, 0x2981, 0x0247, 0x0f0f, 0x0267, 0xf000,
    ]);
    bus.set_reg(1, 0x00f0);
    bus.set_reg(2, 0x0ff0);
    bus.set_reg(3, 0x0001);
    bus.set_reg(4, 0x01f1);
    bus.set_reg(5, 0x0f0f);
    bus.set_reg(6, 0x00ff);
    bus.set_reg(7, 0x1234);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 0x0f00);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(3), 0x00f1);
    cpu.step(&mut bus);
    assert_eq!(flags(&cpu) & EQ, EQ, "COC: all ones of R1 in R4");
    cpu.step(&mut bus);
    assert_eq!(flags(&cpu) & EQ, EQ, "CZC: none of R1 in R5");
    cpu.step(&mut bus);
    assert_eq!(bus.reg(6), 0x000f);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(7), 0x0204);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(7), 0xf204);
    assert_eq!(flags(&cpu), LGT);
}

#[test]
fn jumps_follow_the_status_bits() {
    // Each case: ST, opcode (displacement 2), taken.
    let cases: [(u16, u16, bool); 16] = [
        (0, 0x1002, true),         // JMP
        (0, 0x1102, true),         // JLT: not A> and not EQ
        (AGT, 0x1102, false),      //
        (EQ, 0x1202, true),        // JLE: not L> or EQ
        (LGT, 0x1202, false),      //
        (EQ, 0x1302, true),        // JEQ
        (LGT, 0x1402, true),       // JHE
        (0, 0x1402, false),        //
        (AGT, 0x1502, true),       // JGT
        (EQ, 0x1602, false),       // JNE
        (C, 0x1702, false),        // JNC
        (C, 0x1802, true),         // JOC
        (OV, 0x1902, false),       // JNO
        (0, 0x1a02, true),         // JL
        (LGT | EQ, 0x1b02, false), // JH
        (OP, 0x1c02, true),        // JOP
    ];
    for (status, op, taken) in cases {
        let (mut cpu, mut bus) = setup(&[op]);
        cpu.st = status;
        let t = cpu.step(&mut bus);
        let want = if taken { (10, 0x0106) } else { (8, 0x0102) };
        assert_eq!((t, cpu.pc), want, "{op:04X} with ST {status:04X}");
    }
    // Backwards: JMP $ (displacement -1) loops on itself.
    let (mut cpu, mut bus) = setup(&[0x10ff]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x0100);
}

#[test]
fn shifts_and_their_counts() {
    // SLA R1,1; SRA R2,4; SRL R3,4; SRC R4,4; SLA R5,0 (count from R0)
    let (mut cpu, mut bus) = setup(&[0x0a11, 0x0842, 0x0943, 0x0b44, 0x0a05]);
    bus.set_reg(1, 0x4000);
    bus.set_reg(2, 0x8010);
    bus.set_reg(3, 0x8010);
    bus.set_reg(4, 0x1234);
    bus.set_reg(5, 0x0001);
    bus.set_reg(0, 0x0003);
    assert_eq!(cpu.step(&mut bus), 14);
    assert_eq!(bus.reg(1), 0x8000);
    assert_eq!(flags(&cpu), LGT | OV, "the sign changed");
    assert_eq!(cpu.step(&mut bus), 20);
    assert_eq!(bus.reg(2), 0xf801);
    assert_eq!(flags(&cpu) & C, 0);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(3), 0x0801);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(4), 0x4123);
    assert_eq!(cpu.step(&mut bus), 20 + 2 * 3, "count from R0: +8");
    assert_eq!(bus.reg(5), 0x0008);
}

#[test]
fn multiply_and_divide() {
    // MPY R1,R2 (R2:R3 = R2 * R1); DIV R4,R5 (R5:R6 / R4); DIV R7,R8 (overflow)
    let (mut cpu, mut bus) = setup(&[0x3881, 0x3d44, 0x3e07]);
    bus.set_reg(1, 0x1234);
    bus.set_reg(2, 0x5678);
    assert_eq!(cpu.step(&mut bus), 52);
    let p = 0x1234u32 * 0x5678;
    assert_eq!((bus.reg(2), bus.reg(3)), ((p >> 16) as u16, p as u16));
    bus.set_reg(4, 10);
    bus.set_reg(5, 0);
    bus.set_reg(6, 1234);
    assert_eq!(cpu.step(&mut bus), 108);
    assert_eq!((bus.reg(5), bus.reg(6)), (123, 4));
    assert_eq!(flags(&cpu) & OV, 0);
    bus.set_reg(7, 2);
    bus.set_reg(8, 2);
    bus.set_reg(9, 0);
    assert_eq!(cpu.step(&mut bus), 16);
    assert_eq!(flags(&cpu) & OV, OV);
    assert_eq!(bus.reg(8), 2, "unchanged");
}

#[test]
fn single_operand_instructions() {
    // CLR R1; SETO R2; INV R3; NEG R4; ABS R5; ABS R6; INC R7; INCT R8; DEC R9; DECT R10; SWPB R11
    let (mut cpu, mut bus) = setup(&[
        0x04c1, 0x0702, 0x0543, 0x0504, 0x0745, 0x0746, 0x0587, 0x05c8, 0x0609, 0x064a, 0x06cb,
    ]);
    let init = [
        (1, 0x1234),
        (3, 0x00ff),
        (4, 0x0000),
        (5, 0xfffe),
        (6, 0x0005),
        (7, 0x7fff),
        (8, 0xffff),
        (9, 0x0001),
        (10, 0x0001),
        (11, 0x1234),
    ];
    for (r, v) in init {
        bus.set_reg(r, v);
    }
    assert_eq!(cpu.step(&mut bus), 10);
    assert_eq!(bus.reg(1), 0);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(2), 0xffff);
    cpu.step(&mut bus);
    assert_eq!(bus.reg(3), 0xff00);
    assert_eq!(cpu.step(&mut bus), 12);
    assert_eq!(flags(&cpu), EQ | C, "NEG 0 carries");
    assert_eq!(cpu.step(&mut bus), 14, "ABS of a negative value");
    assert_eq!(bus.reg(5), 2);
    assert_eq!(flags(&cpu), LGT, "flags from the operand before");
    assert_eq!(cpu.step(&mut bus), 12, "ABS of a positive value");
    assert_eq!(bus.reg(6), 5);
    cpu.step(&mut bus);
    assert_eq!((bus.reg(7), flags(&cpu)), (0x8000, LGT | OV));
    cpu.step(&mut bus);
    assert_eq!((bus.reg(8), flags(&cpu) & C), (0x0001, C));
    cpu.step(&mut bus);
    assert_eq!((bus.reg(9), flags(&cpu)), (0, EQ | C));
    cpu.step(&mut bus);
    assert_eq!((bus.reg(10), flags(&cpu)), (0xffff, LGT));
    cpu.step(&mut bus);
    assert_eq!(bus.reg(11), 0x3412);
}

#[test]
fn branches_and_context_switches() {
    // BL @>0200; at >0200: BLWP @>0300; vector >0300: WP >8320, PC >0400;
    // at >0400: RTWP; then B *R11 back to >0104.
    let (mut cpu, mut bus) = setup(&[0x06a0, 0x0200]);
    bus.put(0x0200, 0x0420);
    bus.put(0x0202, 0x0300);
    bus.put(0x0204, 0x045b);
    bus.put(0x0300, 0x8320);
    bus.put(0x0302, 0x0400);
    bus.put(0x0400, 0x0380);
    assert_eq!(cpu.step(&mut bus), 20, "BL @sym");
    assert_eq!((cpu.pc, bus.reg(11)), (0x0200, 0x0104));
    cpu.st = 0x1234;
    assert_eq!(cpu.step(&mut bus), 34, "BLWP @sym");
    assert_eq!((cpu.wp, cpu.pc), (0x8320, 0x0400));
    assert_eq!(
        (bus.get(0x833a), bus.get(0x833c), bus.get(0x833e)),
        (0x8300, 0x0204, 0x1234)
    );
    assert_eq!(cpu.step(&mut bus), 14, "RTWP");
    assert_eq!((cpu.wp, cpu.pc, cpu.st), (0x8300, 0x0204, 0x1234));
    assert_eq!(cpu.step(&mut bus), 12, "B *R11");
    assert_eq!(cpu.pc, 0x0104);
}

#[test]
fn xop_passes_the_operand_address() {
    // XOP @>1234,1: vector >0044: WP >8340, PC >0500.
    let (mut cpu, mut bus) = setup(&[0x2c60, 0x1234]);
    bus.put(0x0044, 0x8340);
    bus.put(0x0046, 0x0500);
    assert_eq!(cpu.step(&mut bus), 36 + 8);
    assert_eq!((cpu.wp, cpu.pc), (0x8340, 0x0500));
    assert_eq!(bus.get(0x8340 + 22), 0x1234, "R11");
    assert_eq!(cpu.st & st::X, st::X);
}

#[test]
fn x_runs_its_operand_as_an_instruction() {
    // X R1 with R1 = LI R2,>xxxx: the immediate follows the X.
    let (mut cpu, mut bus) = setup(&[0x0481, 0xbeef]);
    bus.set_reg(1, 0x0202);
    assert_eq!(cpu.step(&mut bus), 8 + 12 - 4);
    assert_eq!(bus.reg(2), 0xbeef);
    assert_eq!(cpu.pc, 0x0104);
}

#[test]
fn cru_bits_and_fields() {
    // R12 = >0040 (bit >20). SBO 1; SBZ 2; TB 3; LDCR R1,4; STCR R2,8; STCR R3,0 (16 bits)
    let (mut cpu, mut bus) = setup(&[0x1d01, 0x1e02, 0x1f03, 0x3101, 0x3602, 0x3403]);
    bus.set_reg(12, 0x0040);
    // With 8 bits or fewer the operand is a byte: R1's high byte.
    bus.set_reg(1, 0x0500);
    bus.cru_in[0x23] = true;
    bus.cru_in[0x20] = true;
    bus.cru_in[0x27] = true;
    bus.cru_in[0x2f] = true;
    assert_eq!(cpu.step(&mut bus), 12);
    cpu.step(&mut bus);
    assert_eq!(bus.cru_out, [(0x21, true), (0x22, false)]);
    cpu.step(&mut bus);
    assert_eq!(flags(&cpu) & EQ, EQ);
    bus.cru_out.clear();
    assert_eq!(cpu.step(&mut bus), 20 + 8, "LDCR 4 bits");
    assert_eq!(
        bus.cru_out,
        [(0x20, true), (0x21, false), (0x22, true), (0x23, false)],
        "the low bits, first bit first"
    );
    assert_eq!(cpu.step(&mut bus), 44, "STCR 8 bits");
    assert_eq!(bus.reg(2) >> 8, 0x89, "into the high byte: bits 0, 3, 7");
    assert_eq!(cpu.step(&mut bus), 60, "STCR 16 bits");
    assert_eq!(bus.reg(3), 0x8089);
}

#[test]
fn interrupts_respect_the_mask_and_run_one_instruction_first() {
    // LIMI 0; LIMI 2; NOP. Level 1 vector: WP >83C0, PC >0900.
    let (mut cpu, mut bus) = setup(&[0x0300, 0x0000, 0x0300, 0x0002, 0x1000]);
    bus.put(0x0004, 0x83c0);
    bus.put(0x0006, 0x0900);
    cpu.set_intreq(Some(1));
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x0104, "masked");
    cpu.step(&mut bus);
    assert_eq!(cpu.step(&mut bus), 22, "taken after LIMI 2");
    assert_eq!((cpu.wp, cpu.pc, cpu.st & st::MASK), (0x83c0, 0x0900, 0));
    assert_eq!(bus.get(0x83c0 + 28), 0x0108, "R14: the return address");
    // The first instruction of the routine runs before the next one.
    bus.put(0x0900, 0x0300);
    bus.put(0x0902, 0x0002);
    bus.put(0x0904, 0x1000);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x0904);
    // A pending interrupt wakes IDLE.
    let (mut cpu, mut bus) = setup(&[0x0340]);
    bus.put(0x0004, 0x83c0);
    bus.put(0x0006, 0x0900);
    cpu.step(&mut bus);
    assert!(cpu.idle);
    assert_eq!(cpu.step(&mut bus), 2);
    cpu.st |= 1;
    cpu.set_intreq(Some(1));
    cpu.step(&mut bus);
    assert!(!cpu.idle);
    assert_eq!(cpu.pc, 0x0900);
    assert!(bus.externals.contains(&External::Idle));
}

#[test]
fn load_switches_through_fffc() {
    let (mut cpu, mut bus) = setup(&[0x1000]);
    bus.put(0xfffc, 0x83e0);
    bus.put(0xfffe, 0x0a00);
    cpu.load_interrupt();
    assert_eq!(cpu.step(&mut bus), 22);
    assert_eq!((cpu.wp, cpu.pc), (0x83e0, 0x0a00));
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, mut bus) = setup(&[0x0201, 0x1234, 0x0340]);
    cpu.step(&mut bus);
    cpu.set_intreq(Some(3));
    // The running instruction's cycle count is not state.
    cpu.cycles = 0;
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let state = w.into_bytes();
    let mut other = Tms9900::new();
    Savestate::load(&mut other, &mut StateReader::new(&state)).unwrap();
    assert_eq!(other, cpu);
    assert!(Savestate::load(&mut other, &mut StateReader::new(&state[..6])).is_err());
}

#[test]
fn disassembles_in_ti_syntax() {
    let mut bus = TestBus::new();
    bus.program(&[
        0xc9a1, 0x0002, 0x0004, 0x0201, 0x8c00, 0x045b, 0x13fd, 0x1d03, 0x0380,
    ]);
    let d = |a: u32| {
        let i = disasm::disassemble(&bus, a);
        (format!("{i}"), i.bytes.len())
    };
    assert_eq!(d(0x100), ("MOV @>0002(R1),@>0004(R6)".into(), 6));
    assert_eq!(d(0x106), ("LI R1,>8C00".into(), 4));
    assert_eq!(d(0x10a).0, "B *R11");
    assert_eq!(d(0x10c).0, "JEQ >0108");
    assert_eq!(d(0x10e).0, "SBO 3");
    assert_eq!(d(0x110).0, "RTWP");
}
