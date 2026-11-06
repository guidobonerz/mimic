//! Ada: the 16-bit load/store RISC CPU of the Logophor 16, a fictional
//! home computer of this project (13 registers, four segment registers,
//! 16-bit instructions, 12 MHz).
//!
//! Specification: `docs/logophor16/SPEC.md` section 3, the single source
//! of truth; the instruction table is [`isa`], shared with the assembler.
//!
//! Execution is instruction-exact: [`Ada::tick`] runs a whole instruction
//! in the first cycle of its time and then reports the remaining cycles
//! as busy (SPEC.md 3.4 gives the counts; chip-bus wait states are the
//! machine's, which holds the CPU while it waits for a slot). Memory goes
//! through [`emu_core::Bus`] with linear addresses (segment × 65 536 +
//! offset); words are little-endian (the bus's `Size::Word` access at an
//! even address).
//!
//! Interrupts: [`Ada::set_irq_level`] gives the highest pending level from
//! the interrupt controller; level 7 is the NMI, taken on its rising edge.
//! Interrupts and exceptions save PC, S0 and SR in shadow registers and
//! continue at the vector in the ROM's table (FFFB:0040).
#![forbid(unsafe_code)]

pub mod isa;

#[cfg(test)]
mod tests;

use emu_core::{
    Bus, CycleResult, Flow, Instr, Register, RegisterSet, Savestate, Size, StateError, StateReader,
    StateWriter,
};
use isa::{Alu, Imm, Md, Mem, Op, Xop};

/// SR bits (SPEC.md 3.1).
pub const C: u16 = 1 << 0;
pub const V: u16 = 1 << 1;
pub const Z: u16 = 1 << 2;
pub const N: u16 = 1 << 3;
pub const IPL_SHIFT: u16 = 8;
pub const IPL: u16 = 7 << IPL_SHIFT;
pub const S: u16 = 1 << 13;
/// The SR bits that exist.
pub const SR_MASK: u16 = C | V | Z | N | IPL | S;

/// Exception vectors (SPEC.md 3.11).
pub const VEC_ILLEGAL: u8 = 1;
pub const VEC_PRIVILEGE: u8 = 2;
pub const VEC_DIVIDE: u8 = 3;
pub const VEC_TRAP: u8 = 4;
pub const VEC_BRK: u8 = 5;
/// Interrupt level L uses vector 8 + L.
pub const VEC_IRQ_BASE: u8 = 8;

/// Segment of the ROM and the vector table's linear address.
pub const ROM_SEGMENT: u16 = 0xfffb;
pub const VECTORS: u32 = 0xfffb_0040;

/// System control registers (MFSC/MTSC).
pub const SC_SPC: u8 = 0;
pub const SC_SS0: u8 = 1;
pub const SC_SSR: u8 = 2;
pub const SC_CAUSE: u8 = 3;
pub const SC_CYCLE: u8 = 4;
pub const SC_CYCLEH: u8 = 5;

/// Cycles of interrupt and exception entry.
const ENTRY_CYCLES: u8 = 2;

/// The CPU's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ada {
    /// R0–R12 (R0 always reads 0).
    pub r: [u16; 13],
    /// S0–S3.
    pub s: [u16; 4],
    pub pc: u16,
    pub sr: u16,
    pub spc: u16,
    pub ss0: u16,
    pub ssr: u16,
    pub cause: u16,
    /// Cycles since reset (CYCLE/CYCLEH) and CYCLEH's latch.
    pub cycles: u64,
    cycleh: u16,
    /// Cycles of the current instruction still to run.
    busy: u8,
    /// HALT: waiting for an interrupt.
    pub halted: bool,
    /// Interrupt input: highest pending level (0 = none), NMI edge state.
    irq_level: u8,
    nmi_pending: bool,
    /// A debugger is attached: BRK stops (sets `brk_hit`) instead of
    /// raising exception 5.
    pub brk_stops: bool,
    pub brk_hit: bool,
}

impl Ada {
    pub fn new() -> Self {
        let mut a = Self::default();
        a.reset();
        a
    }

    /// Reset (SPEC.md 3.1): FFFB:0000, supervisor, IPL 7.
    pub fn reset(&mut self) {
        let brk_stops = self.brk_stops;
        *self = Self {
            brk_stops,
            ..Self::default()
        };
        self.s[0] = ROM_SEGMENT;
        self.sr = S | IPL;
    }

    /// The highest pending interrupt level (0–7) from the controller.
    pub fn set_irq_level(&mut self, level: u8) {
        let level = level.min(7);
        if level == 7 && self.irq_level != 7 {
            self.nmi_pending = true;
        }
        self.irq_level = level;
    }

    /// Linear address of `seg:off`.
    pub fn linear(seg: u16, off: u16) -> u32 {
        u32::from(seg) << 16 | u32::from(off)
    }

    fn reg(&self, r: u8) -> u16 {
        self.r[usize::from(r)]
    }

    fn set(&mut self, r: u8, v: u16) {
        if r != 0 {
            self.r[usize::from(r)] = v;
        }
    }

    fn flag(&self, f: u16) -> bool {
        self.sr & f != 0
    }

    fn put(&mut self, f: u16, on: bool) {
        if on {
            self.sr |= f;
        } else {
            self.sr &= !f;
        }
    }

    fn nz(&mut self, v: u16) {
        self.put(Z, v == 0);
        self.put(N, v & 0x8000 != 0);
    }

    fn supervisor(&self) -> bool {
        self.flag(S)
    }

    fn ipl(&self) -> u8 {
        ((self.sr & IPL) >> IPL_SHIFT) as u8
    }

    /// a + b + carry with all four flags.
    fn add(&mut self, a: u16, b: u16, carry: bool) -> u16 {
        let sum = u32::from(a) + u32::from(b) + u32::from(carry);
        let r = sum as u16;
        self.nz(r);
        self.put(C, sum > 0xffff);
        self.put(V, (!(a ^ b) & (a ^ r)) & 0x8000 != 0);
        r
    }

    /// a − b − borrow; C = no borrow.
    fn sub(&mut self, a: u16, b: u16, borrow: bool) -> u16 {
        let r = a.wrapping_sub(b).wrapping_sub(u16::from(borrow));
        self.nz(r);
        self.put(C, u32::from(a) >= u32::from(b) + u32::from(borrow));
        self.put(V, ((a ^ b) & (a ^ r)) & 0x8000 != 0);
        r
    }

    fn read_word(bus: &mut dyn Bus, seg: u16, off: u16) -> u16 {
        bus.read(Self::linear(seg, off & !1), Size::Word) as u16
    }

    fn write_word(bus: &mut dyn Bus, seg: u16, off: u16, v: u16) {
        bus.write(Self::linear(seg, off & !1), Size::Word, u32::from(v));
    }

    /// Segment for a load/store with base register `rb` (SPEC.md 3.2).
    fn data_segment(&self, rb: u8) -> u16 {
        if rb == 12 {
            self.s[2]
        } else {
            self.s[1]
        }
    }

    /// Enter exception or interrupt `vector`, returning to `ret`.
    fn enter(&mut self, bus: &mut dyn Bus, vector: u8, ret: u16, cause: u16, level: Option<u8>) {
        self.spc = ret;
        self.ss0 = self.s[0];
        self.ssr = self.sr;
        // Supervisor, all interrupts but the NMI blocked (SPEC.md 3.11):
        // the one set of shadow registers is safe until the handler has
        // saved it and lowers IPL itself.
        let _ = level;
        self.sr |= S | IPL;
        self.cause = cause;
        let at = VECTORS + 4 * u32::from(vector);
        let seg = bus.read(at, Size::Word) as u16;
        let off = bus.read(at + 2, Size::Word) as u16;
        self.s[0] = seg;
        self.pc = off;
    }

    /// Take a pending interrupt before the next instruction, if any.
    fn interrupt(&mut self, bus: &mut dyn Bus) -> bool {
        let level = if self.nmi_pending {
            self.nmi_pending = false;
            7
        } else if self.irq_level > self.ipl() && self.irq_level < 7 {
            self.irq_level
        } else {
            return false;
        };
        self.halted = false;
        let vector = VEC_IRQ_BASE + level;
        self.enter(bus, vector, self.pc, u16::from(vector), Some(level));
        true
    }

    /// Run one whole instruction (or take an interrupt); returns its
    /// cycles.
    pub fn step(&mut self, bus: &mut dyn Bus) -> u8 {
        if self.interrupt(bus) {
            return ENTRY_CYCLES;
        }
        if self.halted {
            return 1;
        }
        let at = self.pc;
        let w = Self::read_word(bus, self.s[0], at);
        let next = at.wrapping_add(2);
        self.pc = next;
        self.execute(bus, isa::decode(w), at)
    }

    fn exception(&mut self, bus: &mut dyn Bus, vector: u8, ret: u16) -> u8 {
        self.enter(bus, vector, ret, u16::from(vector), None);
        ENTRY_CYCLES
    }

    fn execute(&mut self, bus: &mut dyn Bus, op: Op, at: u16) -> u8 {
        let next = self.pc;
        match op {
            Op::Alu { f, rd, rs } => {
                let (a, b) = (self.reg(rd), self.reg(rs));
                let count = u32::from(b & 15);
                let r = match f {
                    Alu::Add => self.add(a, b, false),
                    Alu::Sub => self.sub(a, b, false),
                    Alu::Adc => self.add(a, b, self.flag(C)),
                    Alu::Sbc => self.sub(a, b, !self.flag(C)),
                    Alu::And => a & b,
                    Alu::Or => a | b,
                    Alu::Xor => a ^ b,
                    Alu::Not => !b,
                    Alu::Mov => b,
                    Alu::Mul => a.wrapping_mul(b),
                    Alu::Shl => {
                        if count > 0 {
                            self.put(C, (a >> (16 - count)) & 1 != 0);
                        }
                        a.wrapping_shl(count)
                    }
                    Alu::Shr => {
                        if count > 0 {
                            self.put(C, (a >> (count - 1)) & 1 != 0);
                        }
                        a >> count
                    }
                    Alu::Asr => {
                        if count > 0 {
                            self.put(C, (a >> (count - 1)) & 1 != 0);
                        }
                        ((a as i16) >> count) as u16
                    }
                    Alu::Rol => {
                        let r = a.rotate_left(count);
                        if count > 0 {
                            self.put(C, r & 1 != 0);
                        }
                        r
                    }
                    Alu::Cmp => {
                        self.sub(a, b, false);
                        return 1;
                    }
                    Alu::Tst => {
                        self.nz(a & b);
                        return 1;
                    }
                };
                if !matches!(f, Alu::Add | Alu::Sub | Alu::Adc | Alu::Sbc) {
                    self.nz(r);
                }
                self.set(rd, r);
                if f == Alu::Mul {
                    2
                } else {
                    1
                }
            }
            Op::Imm { f, rd, imm } => {
                let a = self.reg(rd);
                match f {
                    Imm::Addi => {
                        let r = self.add(a, i16::from(imm as i8) as u16, false);
                        self.set(rd, r);
                    }
                    Imm::Movi => self.set(rd, u16::from(imm)),
                    Imm::Lui => self.set(rd, u16::from(imm) << 8 | (a & 0xff)),
                    Imm::Cmpi => {
                        self.sub(a, u16::from(imm), false);
                    }
                }
                1
            }
            Op::Mem { f, rd, rb, off } => {
                let seg = self.data_segment(rb);
                let addr = self.reg(rb).wrapping_add(u16::from(off));
                match f {
                    Mem::Lw => {
                        let v = Self::read_word(bus, seg, addr);
                        self.set(rd, v);
                    }
                    Mem::Sw => Self::write_word(bus, seg, addr, self.reg(rd)),
                    Mem::Lb => {
                        let v = bus.read(Self::linear(seg, addr), Size::Byte) as u8;
                        self.set(rd, u16::from(v));
                    }
                    Mem::Sb => bus.write(
                        Self::linear(seg, addr),
                        Size::Byte,
                        u32::from(self.reg(rd) & 0xff),
                    ),
                }
                2
            }
            Op::X { f, post, rd, rs } => {
                let (seg, addr) = (self.s[3], self.reg(rs));
                let word = matches!(f, Xop::Lwx | Xop::Swx);
                let stored = self.reg(rd);
                if post {
                    let step = if word { 2 } else { 1 };
                    let new = addr.wrapping_add(step);
                    if new < addr {
                        self.s[3] = self.s[3].wrapping_add(1);
                    }
                    self.set(rs, new);
                }
                match f {
                    Xop::Lwx => {
                        let v = Self::read_word(bus, seg, addr);
                        self.set(rd, v);
                    }
                    Xop::Swx => Self::write_word(bus, seg, addr, stored),
                    Xop::Lbx => {
                        let v = bus.read(Self::linear(seg, addr), Size::Byte) as u8;
                        self.set(rd, u16::from(v));
                    }
                    Xop::Sbx => bus.write(
                        Self::linear(seg, addr),
                        Size::Byte,
                        u32::from(stored & 0xff),
                    ),
                }
                2
            }
            Op::Branch { cond, off } => {
                if self.condition(cond) {
                    self.pc = next.wrapping_add((i16::from(off) * 2) as u16);
                    2
                } else {
                    1
                }
            }
            Op::Bl { off } => {
                self.set(11, next);
                self.pc = next.wrapping_add((off * 2) as u16);
                2
            }
            Op::Jr { rs } => {
                self.pc = self.reg(rs);
                2
            }
            Op::Jalr { rd, rs } => {
                let t = self.reg(rs);
                self.set(rd, next);
                self.pc = t;
                2
            }
            Op::Mts { sn, rs } => {
                self.s[usize::from(sn)] = self.reg(rs);
                1
            }
            Op::Mfs { rd, sn } => {
                self.set(rd, self.s[usize::from(sn)]);
                1
            }
            Op::Jf { rd, rs } => {
                let (seg, off) = (self.reg(rd), self.reg(rs));
                self.s[0] = seg;
                self.pc = off;
                2
            }
            Op::Callf { rd, rs } => {
                let (seg, off) = (self.reg(rd), self.reg(rs));
                self.set(10, self.s[0]);
                self.set(11, next);
                self.s[0] = seg;
                self.pc = off;
                2
            }
            Op::Md { f, rd, rs } => {
                let (a, b) = (self.reg(rd), self.reg(rs));
                let divide = matches!(f, Md::Divu | Md::Divs | Md::Remu | Md::Rems);
                if divide && b == 0 {
                    return self.exception(bus, VEC_DIVIDE, at);
                }
                let r = match f {
                    Md::Mulhu => ((u32::from(a) * u32::from(b)) >> 16) as u16,
                    Md::Mulhs => ((i32::from(a as i16) * i32::from(b as i16)) >> 16) as u16,
                    Md::Divu => a / b,
                    Md::Divs => {
                        let overflow = a == 0x8000 && b == 0xffff;
                        self.put(V, overflow);
                        (a as i16).wrapping_div(b as i16) as u16
                    }
                    Md::Remu => a % b,
                    Md::Rems => (a as i16).wrapping_rem(b as i16) as u16,
                    Md::Neg => {
                        let r = self.sub(0, b, false);
                        self.set(rd, r);
                        return 2;
                    }
                    Md::Sxb => i16::from(b as u8 as i8) as u16,
                    Md::Zxb => b & 0xff,
                    Md::Swab => b.swap_bytes(),
                };
                self.nz(r);
                self.set(rd, r);
                if divide {
                    18
                } else {
                    2
                }
            }
            Op::Trap { n } => {
                self.enter(
                    bus,
                    VEC_TRAP,
                    next,
                    u16::from(n) << 8 | u16::from(VEC_TRAP),
                    None,
                );
                ENTRY_CYCLES
            }
            Op::Brk => {
                if self.brk_stops {
                    self.brk_hit = true;
                    1
                } else {
                    self.exception(bus, VEC_BRK, next)
                }
            }
            Op::Nop => 1,
            Op::Mfsr { rd } => {
                self.set(rd, self.sr);
                1
            }
            Op::Rti
            | Op::Ei
            | Op::Di
            | Op::Halt
            | Op::Mtsr { .. }
            | Op::Mfsc { .. }
            | Op::Mtsc { .. }
                if !self.supervisor() =>
            {
                self.exception(bus, VEC_PRIVILEGE, at)
            }
            Op::Rti => {
                self.pc = self.spc;
                self.s[0] = self.ss0;
                self.sr = self.ssr & SR_MASK;
                2
            }
            Op::Ei => {
                self.sr &= !IPL;
                1
            }
            Op::Di => {
                self.sr |= IPL;
                1
            }
            Op::Halt => {
                self.halted = true;
                1
            }
            Op::Mtsr { rs } => {
                self.sr = self.reg(rs) & SR_MASK;
                1
            }
            Op::Mfsc { rd, c } => {
                let v = match c {
                    SC_SPC => self.spc,
                    SC_SS0 => self.ss0,
                    SC_SSR => self.ssr,
                    SC_CAUSE => self.cause,
                    SC_CYCLE => {
                        self.cycleh = (self.cycles >> 16) as u16;
                        self.cycles as u16
                    }
                    SC_CYCLEH => self.cycleh,
                    _ => 0,
                };
                self.set(rd, v);
                1
            }
            Op::Mtsc { c, rs } => {
                let v = self.reg(rs);
                match c {
                    SC_SPC => self.spc = v,
                    SC_SS0 => self.ss0 = v,
                    SC_SSR => self.ssr = v & SR_MASK,
                    SC_CAUSE => self.cause = v,
                    _ => {}
                }
                1
            }
            Op::Illegal(_) => self.exception(bus, VEC_ILLEGAL, at),
        }
    }

    fn condition(&self, cond: u8) -> bool {
        let (c, v, z, n) = (self.flag(C), self.flag(V), self.flag(Z), self.flag(N));
        match cond {
            0 => z,
            1 => !z,
            2 => c,
            3 => !c,
            4 => n,
            5 => !n,
            6 => v,
            7 => !v,
            8 => c && !z,
            9 => !c || z,
            10 => n == v,
            11 => n != v,
            12 => !z && n == v,
            13 => z || n != v,
            14 => true,
            _ => false,
        }
    }

    /// The next cycle starts an instruction (or an interrupt entry, or a
    /// HALT wait).
    pub fn at_boundary(&self) -> bool {
        self.busy == 0
    }

    /// The linear addresses the next instruction will access: its fetch
    /// and, for a load or store, its data (read with `peek`). For the
    /// machine's bus arbitration.
    pub fn next_accesses(&self, bus: &dyn Bus) -> (u32, Option<u32>) {
        let fetch = Self::linear(self.s[0], self.pc & !1);
        let op = isa::decode(bus.peek(fetch, Size::Word) as u16);
        let data = match op {
            Op::Mem { rb, off, .. } => Some(Self::linear(
                self.data_segment(rb),
                self.reg(rb).wrapping_add(u16::from(off)),
            )),
            Op::X { rs, .. } => Some(Self::linear(self.s[3], self.reg(rs))),
            _ => None,
        };
        (fetch, data)
    }

    /// One cycle: the first cycle of an instruction runs it whole.
    pub fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        self.cycles += 1;
        if self.busy == 0 {
            self.busy = self.step(bus);
        }
        self.busy -= 1;
        if self.busy == 0 && !self.halted {
            CycleResult::InstrDone {
                next_pc: Self::linear(self.s[0], self.pc),
            }
        } else {
            CycleResult::Busy
        }
    }

    /// The instruction at linear `addr`, read with `peek`.
    pub fn disassemble_at(bus: &dyn Bus, addr: u32) -> Instr {
        let w = bus.peek(addr & !1, Size::Word) as u16;
        let op = isa::decode(w);
        let pc = addr as u16;
        let (mnemonic, operand) = op.text(pc);
        let seg = addr & 0xffff_0000;
        let flow = match op {
            Op::Jr { rs: 11 } | Op::Jf { rd: 10, rs: 11 } | Op::Rti => Flow::Return,
            Op::Branch { cond: 14, .. } | Op::Jr { .. } | Op::Jf { .. } => Flow::Jump,
            Op::Branch { cond: 15, .. } => Flow::Normal,
            Op::Branch { .. } => Flow::Branch,
            Op::Bl { .. } | Op::Jalr { .. } | Op::Callf { .. } | Op::Trap { .. } => Flow::Call,
            _ => Flow::Normal,
        };
        Instr {
            addr,
            bytes: w.to_le_bytes().to_vec(),
            mnemonic,
            operand,
            target: op.target(pc).map(|t| seg | u32::from(t)),
            flow,
        }
    }
}

impl emu_core::Cpu for Ada {
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        Ada::tick(self, bus)
    }

    fn regs(&self) -> RegisterSet {
        const NAMES: [&str; 13] = [
            "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12",
        ];
        let mut v: Vec<Register> = NAMES
            .iter()
            .zip(self.r)
            .map(|(&name, value)| Register {
                name,
                value: u32::from(value),
                bits: 16,
            })
            .collect();
        for (name, value) in [
            ("s0", self.s[0]),
            ("s1", self.s[1]),
            ("s2", self.s[2]),
            ("s3", self.s[3]),
            ("pc", self.pc),
            ("sr", self.sr),
            ("spc", self.spc),
            ("ss0", self.ss0),
            ("ssr", self.ssr),
            ("cause", self.cause),
        ] {
            v.push(Register {
                name,
                value: u32::from(value),
                bits: 16,
            });
        }
        RegisterSet(v)
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        let v = v as u16;
        let name = name.to_ascii_lowercase();
        if let Some(n) = name.strip_prefix('r').and_then(|n| n.parse::<u8>().ok()) {
            if (1..13).contains(&n) {
                self.r[usize::from(n)] = v;
            }
            return;
        }
        match name.as_str() {
            "s0" => self.s[0] = v,
            "s1" => self.s[1] = v,
            "s2" => self.s[2] = v,
            "s3" => self.s[3] = v,
            "pc" => self.pc = v,
            "sr" => self.sr = v & SR_MASK,
            "spc" => self.spc = v,
            "ss0" => self.ss0 = v,
            "ssr" => self.ssr = v & SR_MASK,
            "cause" => self.cause = v,
            _ => {}
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        Ada::disassemble_at(bus, addr)
    }
}

const TAG: [u8; 4] = *b"ADA0";
const VERSION: u16 = 1;

impl Savestate for Ada {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for r in self.r {
                w.u16(r);
            }
            for s in self.s {
                w.u16(s);
            }
            for v in [
                self.pc,
                self.sr,
                self.spc,
                self.ss0,
                self.ssr,
                self.cause,
                self.cycleh,
            ] {
                w.u16(v);
            }
            w.u64(self.cycles);
            w.u8(self.busy);
            w.bool(self.halted);
            w.u8(self.irq_level);
            w.bool(self.nmi_pending);
            w.bool(self.brk_hit);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut a = Ada {
            brk_stops: self.brk_stops,
            ..Ada::default()
        };
        for r in &mut a.r {
            *r = c.u16()?;
        }
        a.r[0] = 0;
        for s in &mut a.s {
            *s = c.u16()?;
        }
        a.pc = c.u16()?;
        a.sr = c.u16()? & SR_MASK;
        a.spc = c.u16()?;
        a.ss0 = c.u16()?;
        a.ssr = c.u16()? & SR_MASK;
        a.cause = c.u16()?;
        a.cycleh = c.u16()?;
        a.cycles = c.u64()?;
        a.busy = c.u8()?.min(18);
        a.halted = c.bool()?;
        a.irq_level = c.u8()?.min(7);
        a.nmi_pending = c.bool()?;
        a.brk_hit = c.bool()?;
        *self = a;
        Ok(())
    }
}
