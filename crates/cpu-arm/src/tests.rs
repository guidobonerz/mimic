//! Own per-instruction tests, from the data manual's descriptions.

use super::*;
use emu_core::Flow;

/// RAM with abort windows, counting cycles by kind.
struct Ram {
    m: Vec<u8>,
    /// Addresses from here on abort.
    abort_from: u32,
    s: u32,
    n: u32,
    i: u32,
    user_seen: bool,
}

impl Ram {
    fn with(code: &[u32]) -> Self {
        let mut r = Self {
            m: vec![0; 0x10000],
            abort_from: u32::MAX,
            s: 0,
            n: 0,
            i: 0,
            user_seen: false,
        };
        for (i, w) in code.iter().enumerate() {
            r.poke(i as u32 * 4, *w);
        }
        r
    }
    fn poke(&mut self, a: u32, w: u32) {
        self.m[a as usize..a as usize + 4].copy_from_slice(&w.to_le_bytes());
    }
    fn word(&self, a: u32) -> u32 {
        let a = a as usize & 0xfffc;
        u32::from_le_bytes([self.m[a], self.m[a + 1], self.m[a + 2], self.m[a + 3]])
    }
    fn count(&mut self, seq: bool, privileged: bool) {
        if seq {
            self.s += 1;
        } else {
            self.n += 1;
        }
        self.user_seen |= !privileged;
    }
    fn reset_counts(&mut self) {
        (self.s, self.n, self.i) = (0, 0, 0);
    }
}

impl ArmBus for Ram {
    fn fetch(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32> {
        self.read_word(addr, seq, privileged)
    }
    fn read_word(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32> {
        self.count(seq, privileged);
        (addr < self.abort_from).then(|| self.word(addr))
    }
    fn read_byte(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u8> {
        self.count(seq, privileged);
        (addr < self.abort_from).then(|| self.m[addr as usize & 0xffff])
    }
    fn write_word(&mut self, addr: u32, v: u32, seq: bool, privileged: bool) -> bool {
        self.count(seq, privileged);
        if addr >= self.abort_from {
            return false;
        }
        self.poke(addr & 0xfffc, v);
        true
    }
    fn write_byte(&mut self, addr: u32, v: u8, seq: bool, privileged: bool) -> bool {
        self.count(seq, privileged);
        if addr >= self.abort_from {
            return false;
        }
        self.m[addr as usize & 0xffff] = v;
        true
    }
    fn idle(&mut self, n: u32) {
        self.i += n;
    }
}

fn start(code: &[u32]) -> (Arm2, Ram) {
    let mut bus = Ram::with(code);
    let mut cpu = Arm2::new();
    cpu.reset(&mut bus);
    bus.reset_counts();
    (cpu, bus)
}

const MOV_R0_1: u32 = 0xe3a0_0001; // MOV R0,#1
const NOP: u32 = 0xe1a0_0000; // MOV R0,R0

#[test]
fn reset_enters_svc_with_interrupts_off() {
    let (cpu, _) = start(&[NOP]);
    assert_eq!(cpu.mode(), MODE_SVC);
    assert_eq!(cpu.psr & (F_I | F_F), F_I | F_F);
    assert_eq!(cpu.pc, 8);
    assert_eq!(cpu.next_pc(), 0);
}

#[test]
fn data_processing_flags_and_conditions() {
    let (mut cpu, mut bus) = start(&[
        0xe3b0_0000, // MOVS R0,#0
        0x03a0_1007, // MOVEQ R1,#7
        0x13a0_2007, // MOVNE R2,#7
        0xe3e0_3000, // MVN R3,#0
        0xe293_4001, // ADDS R4,R3,#1
        0xe274_5000, // RSBS R5,R4,#0
    ]);
    cpu.step(&mut bus);
    assert_ne!(cpu.psr & F_Z, 0);
    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!((cpu.r[1], cpu.r[2]), (7, 0));
    cpu.step(&mut bus);
    assert_eq!(cpu.r[3], 0xffff_ffff);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[4], 0);
    assert_eq!(cpu.psr & (F_Z | F_C | F_V), F_Z | F_C, "ADDS carries");
    cpu.step(&mut bus);
    assert_eq!(cpu.psr & F_C, F_C, "0 - 0: no borrow");
}

#[test]
fn subtraction_overflow() {
    let (mut cpu, mut bus) = start(&[0xe250_1001]); // SUBS R1,R0,#1
    cpu.r[0] = 0x8000_0000;
    cpu.step(&mut bus);
    assert_eq!(cpu.r[1], 0x7fff_ffff);
    assert_eq!(cpu.psr & (F_V | F_C | F_N), F_V | F_C);
}

#[test]
fn barrel_shifter_special_cases() {
    let (mut cpu, mut bus) = start(&[
        0xe1b0_1020, // MOVS R1,R0,LSR #32
        0xe1b0_2040, // MOVS R2,R0,ASR #32
        0xe1b0_3060, // MOVS R3,R0,RRX
        0xe1b0_4518, // MOVS R4,R8,LSL R5
    ]);
    cpu.r[0] = 0x8000_0001;
    cpu.r[5] = 0; // amount 0 from a register: carry unchanged
    cpu.r[8] = 5;
    cpu.step(&mut bus);
    assert_eq!(cpu.r[1], 0);
    assert_ne!(cpu.psr & F_C, 0);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[2], 0xffff_ffff);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[3], 0xc000_0000, "RRX takes the carry in");
    assert_ne!(cpu.psr & F_C, 0, "and gives bit 0 out");
    cpu.step(&mut bus);
    assert_eq!(cpu.r[4], 5);
    assert_ne!(cpu.psr & F_C, 0);
}

#[test]
fn r15_as_operand_follows_the_manual() {
    let (mut cpu, mut bus) = start(&[
        0xe28f_0000, // ADD R0,PC,#0      Rn: PC alone
        0xe1a0_100f, // MOV R1,PC         Rm: with status
        0xe1a0_2f1f, // MOV R2,PC,LSL PC? shift by PC: register form, +12
    ]);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[0], 8);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[1], 12 | F_I | F_F | MODE_SVC);
    cpu.step(&mut bus);
    // Rm with +12 and status, shifted by bits 7-0 of PC + 12 (no status).
    assert_eq!(cpu.r[2], (20 | F_I | F_F | MODE_SVC) << 20);
}

#[test]
fn writing_r15_with_and_without_s() {
    let (mut cpu, mut bus) = start(&[
        0xe3a0_f020, // MOV PC,#&20
    ]);
    bus.poke(0x20, 0xe3b0_f103); // MOVS PC,#&C0000000: N Z, USR, PC 0
    let old = cpu.psr;
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 0x20);
    assert_eq!(cpu.psr, old, "no S: status stays");
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_USR, "SVC may change the mode");
    assert_eq!(cpu.psr & (F_I | F_F), 0);
    assert_eq!(cpu.psr & (F_N | F_Z), F_N | F_Z);
}

#[test]
fn user_mode_cannot_change_mode_or_interrupts() {
    let (mut cpu, mut bus) = start(&[0xe33f_f103]); // TEQP PC,#&C0000000
    cpu.set_psr(0);
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_USR);
    assert_eq!(cpu.psr, F_N | F_Z, "flags change, I, F and the mode do not");
}

#[test]
fn teqp_sets_status_in_svc() {
    let (mut cpu, mut bus) = start(&[0xe33f_f002]); // TEQP PC,#2: IRQ mode
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_IRQ);
    assert_eq!(cpu.psr & (F_I | F_F), 0);
}

#[test]
fn banked_registers() {
    let (mut cpu, mut bus) = start(&[NOP]);
    cpu.r[8] = 8;
    cpu.r[13] = 0x13;
    cpu.r[14] = 0x14;
    cpu.set_psr(MODE_FIQ);
    assert_eq!(cpu.r[8], 0, "FIQ has its own R8");
    cpu.r[8] = 0x88;
    cpu.r[13] = 0xd0;
    cpu.set_psr(MODE_IRQ);
    assert_eq!(cpu.r[8], 8, "IRQ shares the user R8");
    cpu.r[13] = 0xd1;
    cpu.set_psr(MODE_USR);
    assert_eq!(cpu.r[8], 8);
    cpu.r[13] = 0xd2;
    cpu.set_psr(MODE_SVC);
    assert_eq!((cpu.r[13], cpu.r[14]), (0x13, 0x14));
    cpu.set_psr(MODE_FIQ);
    assert_eq!((cpu.r[8], cpu.r[13]), (0x88, 0xd0));
    cpu.set_psr(MODE_IRQ);
    assert_eq!(cpu.r[13], 0xd1);
    cpu.set_psr(MODE_USR);
    assert_eq!(cpu.r[13], 0xd2);
    let _ = cpu.step(&mut bus);
}

#[test]
fn multiply_and_timing() {
    let (mut cpu, mut bus) = start(&[
        0xe000_0291, // MUL R0,R1,R2
        0xe023_0291, // MLA R3,R1,R2,R0
        0xe000_0491, // MUL R0,R1,R4
    ]);
    cpu.r[1] = 7;
    cpu.r[2] = 6;
    cpu.r[4] = 0xffff_ffff;
    cpu.step(&mut bus);
    assert_eq!(cpu.r[0], 42);
    assert_eq!(bus.i, 2, "2^1 ≤ 6 < 2^3: m = 2");
    bus.reset_counts();
    cpu.step(&mut bus);
    assert_eq!(cpu.r[3], 84);
    bus.reset_counts();
    cpu.step(&mut bus);
    assert_eq!(bus.i, 16, "m tops out at 16");
}

#[test]
fn multiply_m_from_table_21() {
    // m cycles for 2^(2m-3) ≤ Rs ≤ 2^(2m-1) - 1; 0 and 1 take one.
    for (rs, want) in [(0u32, 1), (1, 1), (2, 2), (7, 2), (8, 3), (31, 3), (32, 4)] {
        let (mut cpu, mut bus) = start(&[0xe000_0291]);
        cpu.r[2] = rs;
        cpu.step(&mut bus);
        assert_eq!(bus.i, want, "Rs = {rs}");
    }
}

#[test]
fn ldr_str_addressing_and_rotation() {
    let (mut cpu, mut bus) = start(&[
        0xe591_0004, // LDR R0,[R1,#4]
        0xe5b1_2004, // LDR R2,[R1,#4]!
        0xe491_3004, // LDR R3,[R1],#4
        0xe5d4_5001, // LDRB R5,[R4,#1]
        0xe594_6001, // LDR R6,[R4,#1]   rotated
        0xe5c7_8000, // STRB R8,[R7]
    ]);
    bus.poke(0x104, 0x1122_3344);
    bus.poke(0x200, 0xaabb_ccdd);
    cpu.r[1] = 0x100;
    cpu.r[4] = 0x200;
    cpu.r[7] = 0x303;
    cpu.r[8] = 0x1234_56ab;
    cpu.step(&mut bus);
    assert_eq!(cpu.r[0], 0x1122_3344);
    assert_eq!(cpu.r[1], 0x100);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[1], 0x104);
    cpu.step(&mut bus);
    assert_eq!((cpu.r[3], cpu.r[1]), (0x1122_3344, 0x108));
    cpu.step(&mut bus);
    assert_eq!(cpu.r[5], 0xcc);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[6], 0xddaa_bbcc);
    cpu.step(&mut bus);
    assert_eq!(bus.m[0x303], 0xab);
}

#[test]
fn str_of_r15_stores_plus_12_with_status() {
    let (mut cpu, mut bus) = start(&[0xe581_f000]); // STR PC,[R1]
    cpu.r[1] = 0x100;
    cpu.step(&mut bus);
    assert_eq!(bus.word(0x100), 12 | cpu.psr);
}

#[test]
fn ldrt_goes_out_as_user() {
    let (mut cpu, mut bus) = start(&[0xe4b1_0000]); // LDRT R0,[R1]
    cpu.r[1] = 0x100;
    cpu.step(&mut bus);
    assert!(bus.user_seen);
}

#[test]
fn load_and_store_multiple() {
    let (mut cpu, mut bus) = start(&[
        0xe92d_400f, // STMDB R13!,{R0-R3,R14}
        0xe8bd_40f0, // LDMIA R13!,{R4-R7,R14}
    ]);
    cpu.r[..4].copy_from_slice(&[1, 2, 3, 4]);
    cpu.r[13] = 0x1000;
    cpu.r[14] = 0x77;
    cpu.step(&mut bus);
    assert_eq!(cpu.r[13], 0x1000 - 20);
    assert_eq!(bus.word(0xfec), 1);
    assert_eq!(bus.word(0xffc), 0x77);
    cpu.r[14] = 0;
    cpu.step(&mut bus);
    assert_eq!(&cpu.r[4..8], &[1, 2, 3, 4]);
    assert_eq!((cpu.r[13], cpu.r[14]), (0x1000, 0x77));
}

#[test]
fn stm_with_base_stores_old_value_only_when_first() {
    let (mut cpu, mut bus) = start(&[
        0xe8a1_0003, // STMIA R1!,{R0,R1}: R1 second: new value
    ]);
    cpu.r[1] = 0x100;
    cpu.step(&mut bus);
    assert_eq!(bus.word(0x104), 0x108);
    let (mut cpu, mut bus) = start(&[0xe8a0_0003]); // STMIA R0!,{R0,R1}
    cpu.r[0] = 0x100;
    cpu.step(&mut bus);
    assert_eq!(bus.word(0x100), 0x100);
}

#[test]
fn ldm_caret_with_r15_restores_status() {
    let (mut cpu, mut bus) = start(&[0xe8d0_8000]); // LDMIA R0,{PC}^
    cpu.r[0] = 0x100;
    bus.poke(0x100, 0x2000_0040); // C, USR, PC &40
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_USR);
    assert_eq!(cpu.psr, F_C);
    assert_eq!(cpu.next_pc(), 0x40);
}

#[test]
fn ldm_caret_without_r15_loads_the_user_bank() {
    let (mut cpu, mut bus) = start(&[0xe8d0_6000]); // LDMIA R0,{R13,R14}^
    cpu.r[0] = 0x100;
    bus.poke(0x100, 0x11);
    bus.poke(0x104, 0x22);
    cpu.r[13] = 0x5;
    cpu.step(&mut bus);
    assert_eq!(cpu.r[13], 0x5, "SVC's R13 stays");
    cpu.set_psr(MODE_USR);
    assert_eq!((cpu.r[13], cpu.r[14]), (0x11, 0x22));
}

#[test]
fn branch_and_link() {
    let (mut cpu, mut bus) = start(&[0xeb00_0002]); // BL &10
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 0x10);
    assert_eq!(cpu.r[14], 4 | cpu.psr);
    assert_eq!((bus.s, bus.n), (2, 1), "2S + 1N");
}

#[test]
fn cycle_counts_from_table_21() {
    let check = |code: u32, s, n, i| {
        let (mut cpu, mut bus) = start(&[code, NOP, NOP]);
        cpu.r[1] = 0x100;
        cpu.step(&mut bus);
        // The fetch after a data access is the N of the formula.
        cpu.step(&mut bus);
        let (fs, fn_, fi) = (bus.s, bus.n, bus.i);
        assert_eq!(
            (fs - 1 + u32::from(fn_ > n), fn_.min(n), fi),
            (s, n, i),
            "{code:08x}"
        );
    };
    check(MOV_R0_1, 1, 0, 0);
    check(0xe1a0_0312, 1, 0, 1); // MOV R0,R2,LSL R3: register shift
    check(0xe591_0000, 1, 1, 1); // LDR: 1S + 1N + 1I
    check(0xe581_0000, 0, 2, 0); // STR: 2N
    check(0xe891_001c, 3, 1, 1); // LDM of 3: nS + 1N + 1I
}

#[test]
fn undefined_swi_and_coprocessor_trap() {
    let (mut cpu, mut bus) = start(&[0xef00_0005]); // SWI 5
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 8);
    assert_eq!(cpu.r[14] & PC_MASK, 4);
    let (mut cpu, mut bus) = start(&[0xee00_0000]); // CDP: no coprocessor
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 4);
    let (mut cpu, mut bus) = start(&[0xe600_0010]); // LDR form with bit 4
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 4);
}

#[test]
fn interrupts_save_the_instruction_after_plus_four() {
    let (mut cpu, mut bus) = start(&[NOP, NOP]);
    cpu.set_psr(MODE_USR);
    cpu.step(&mut bus);
    cpu.set_irq(true);
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_IRQ);
    assert_eq!(cpu.next_pc(), 0x18);
    assert_eq!(cpu.r[14] & PC_MASK, 8, "SUBS PC,R14,#4 returns to 4");
    assert_ne!(cpu.psr & F_I, 0);
    let (mut cpu, mut bus) = start(&[NOP]);
    cpu.set_psr(MODE_USR);
    cpu.set_firq(true);
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_FIQ);
    assert_eq!(cpu.next_pc(), 0x1c);
    assert_eq!(cpu.psr & (F_I | F_F), F_I | F_F);
}

#[test]
fn masked_interrupts_wait() {
    let (mut cpu, mut bus) = start(&[NOP, NOP]);
    cpu.set_irq(true);
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_SVC);
}

#[test]
fn data_abort_on_ldr_leaves_registers_alone() {
    let (mut cpu, mut bus) = start(&[0xe5b1_0004]); // LDR R0,[R1,#4]!
    bus.abort_from = 0x1000;
    cpu.r[0] = 9;
    cpu.r[1] = 0x2000;
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 0x10);
    assert_eq!((cpu.r[0], cpu.r[1]), (9, 0x2000));
    assert_eq!(cpu.r[14] & PC_MASK, 8, "SUBS PC,R14,#8 retries");
}

#[test]
fn data_abort_on_ldm_updates_the_base_and_nothing_after() {
    let (mut cpu, mut bus) = start(&[0xe8b1_0005]); // LDMIA R1!,{R0,R2}
    bus.abort_from = 0x0ffc + 4; // first word loads, second aborts
    bus.poke(0xffc, 0x55);
    cpu.r[1] = 0xffc;
    cpu.r[2] = 7;
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 0x10);
    assert_eq!(cpu.r[2], 7);
    assert_eq!(cpu.r[1], 0x1004);
}

#[test]
fn address_exception_above_26_bits() {
    let (mut cpu, mut bus) = start(&[0xe591_0000]); // LDR R0,[R1]
    cpu.r[1] = 0x0400_0000;
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 0x14);
}

#[test]
fn prefetch_abort_counts_only_when_reached() {
    // A branch away from the aborted words never takes the trap.
    let (mut cpu, mut bus) = start(&[NOP, 0xea00_0006]); // B &28
    bus.abort_from = 8;
    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!(cpu.mode(), MODE_SVC);
    // Going on into them does.
    let (mut cpu, mut bus) = start(&[NOP, NOP]);
    bus.abort_from = 8;
    cpu.step(&mut bus);
    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!(cpu.next_pc(), 0x0c);
    assert_eq!(cpu.r[14] & PC_MASK, 12, "SUBS PC,R14,#4 retries");
}

#[test]
fn pipeline_hides_a_write_over_the_next_instruction() {
    let (mut cpu, mut bus) = start(&[
        0xe581_0000, // STR R0,[R1]: overwrites the word at 8 (R1 = 8)
        NOP,
        MOV_R0_1,
    ]);
    cpu.r[0] = 0xe3a0_0002; // MOV R0,#2
    cpu.r[1] = 8;
    cpu.step(&mut bus);
    assert_eq!(bus.word(8), 0xe3a0_0002);
    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!(cpu.r[0], 1, "the old word was already in the pipeline");
}

#[test]
fn disassembly() {
    struct M(Ram);
    impl Bus for M {
        fn read(&mut self, a: u32, s: Size) -> u32 {
            self.peek(a, s)
        }
        fn peek(&self, a: u32, _s: Size) -> u32 {
            self.0.word(a)
        }
        fn write(&mut self, _a: u32, _s: Size, _v: u32) {}
        fn addr_bits(&self) -> u8 {
            26
        }
    }
    let bus = M(Ram::with(&[
        0xe3a0_0001,
        0xe591_0004,
        0xeb00_0002,
        0xe8bd_8010,
        0xef00_0002,
        0x1a00_0000,
        0xe1a0_f00e,
        0xe000_0291,
        0xe33f_f002,
    ]));
    let cpu = Arm2::new();
    let show = |a| cpu.disassemble(&bus, a).to_string();
    assert_eq!(show(0), "MOV R0,#&1");
    assert_eq!(show(4), "LDR R0,[R1,#&4]");
    assert_eq!(show(8), "BL &18");
    assert_eq!(cpu.disassemble(&bus, 8).flow, Flow::Call);
    assert_eq!(show(12), "LDMIA R13!,{R4,PC}");
    assert_eq!(cpu.disassemble(&bus, 12).flow, Flow::Return);
    assert_eq!(show(16), "SWI &2");
    assert_eq!(show(20), "BNE &1C");
    assert_eq!(cpu.disassemble(&bus, 20).flow, Flow::Branch);
    assert_eq!(cpu.disassemble(&bus, 24).flow, Flow::Return);
    assert_eq!(show(28), "MUL R0,R1,R2");
    assert_eq!(show(32), "TEQP PC,#&2");
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, mut bus) = start(&[NOP, NOP]);
    cpu.r[3] = 0x1234;
    cpu.step(&mut bus);
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();
    let mut other = Arm2::new();
    other.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(other, cpu);
}
