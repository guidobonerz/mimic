//! Cycle-stepped NMOS 6502 and CMOS 65C02 microcode.
//!
//! Every call to [`Core::cycle`] performs exactly one bus access, including
//! the dummy reads and writes of the real chip. Cycle sequences follow the
//! MCS6500 hardware manual (appendix A) and "64doc" by John West and Marko
//! Mäkelä; interrupt polling follows the NESdev wiki "CPU interrupts" page.
//! The 65C02's differences (fixed JMP (abs), dummy reads instead of dummy
//! writes, the decimal-mode cycle, D cleared on interrupts) follow the WDC
//! W65C02S data sheet ("Table 5-7, Operation, Operational Codes and Cycle
//! Timing") and Tom Harte's `synertek65c02` bus traces.

use emu_core::{Bus, CycleResult, InterruptKind, Size};

use crate::table::{Kind, Mode, Op, CMOS_TABLE, TABLE};

pub(crate) const C: u8 = 0x01;
pub(crate) const Z: u8 = 0x02;
pub(crate) const I: u8 = 0x04;
pub(crate) const D: u8 = 0x08;
pub(crate) const B: u8 = 0x10;
pub(crate) const U: u8 = 0x20;
pub(crate) const V: u8 = 0x40;
pub(crate) const N: u8 = 0x80;

/// What the BRK cycle sequence is currently doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Seq {
    /// Software BRK (B flag pushed set).
    Brk,
    /// Hardware IRQ or NMI; the vector is chosen in cycle 4.
    Interrupt,
    /// Reset: stack writes become reads.
    Reset,
}

#[inline]
fn read(bus: &mut dyn Bus, addr: u16) -> u8 {
    bus.read(u32::from(addr), Size::Byte) as u8
}

#[inline]
fn write(bus: &mut dyn Bus, addr: u16, v: u8) {
    bus.write(u32::from(addr), Size::Byte, u32::from(v));
}

/// Complete CPU state. `Copy` so a cycle can be rolled back while RDY is low
/// and so savestates capture mid-instruction state exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Core {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub s: u8,
    /// Only the six real flags (N V D I Z C); B and bit 5 exist only on the stack.
    pub p: u8,
    pub pc: u16,

    /// Current opcode and cycle within the instruction (0 = opcode fetch).
    pub op: u8,
    pub t: u8,
    /// Effective address, base address before indexing, zero-page pointer, data latch.
    pub ea: u16,
    pub base: u16,
    pub ptr: u8,
    pub data: u8,
    /// Indexing or branch crossed a page.
    pub crossed: bool,
    pub seq: Seq,
    pub vector: u16,

    pub irq_line: bool,
    pub nmi_line: bool,
    /// NMI edge detected and not yet serviced.
    pub nmi_pending: bool,
    /// An NMI edge was dropped as BRK/IRQ chose its vector; the line,
    /// if still held, registers again in the next cycle (see `brk`).
    pub nmi_rearm: bool,
    /// RDY held the cycle before the one that combines the unstable result:
    /// the operand read of ANE, the fix-up of SHA/SHX/SHY/TAS. Set on the
    /// RDY path of `tick`, taken by the instruction.
    pub rdy_stalled: bool,
    /// Flags of the cycle RDY repeats, for the polls while it stands.
    pub stall_p: u8,
    pub reset_pending: bool,
    /// Interrupt decision for the next opcode fetch.
    pub poll: bool,
    /// Set by a cycle that must not update `poll` (branch quirk).
    pub no_poll: bool,
    /// The last cycle polled the interrupt inputs (for late sampling, see
    /// `Cpu6502::sample_interrupts`).
    pub polled: bool,
    pub jammed: bool,
    /// A hardware interrupt sequence just completed (for IRQ/NMI
    /// breakpoints); the machine takes it right after the tick, so it is
    /// not part of the savestate.
    pub entered: Option<InterruptKind>,

    /// Magic constants of the unstable ANE ($8B) and LXA ($AB) opcodes.
    pub ane_magic: u8,
    pub lxa_magic: u8,
    /// The D flag switches ADC/SBC/ARR to BCD (false on the 2A03, whose
    /// decimal adjust is cut from the die; D is still stored and pushed).
    pub decimal: bool,
    /// The vector is chosen as PCL is pushed and an NMI arriving as P is
    /// pushed is lost (true; Acid800), or, for machines sampling the
    /// inputs late in the cycle (`Cpu6502::sample_interrupts`, the 2A03),
    /// the vector is chosen as P is pushed and a later NMI waits for the
    /// first handler instruction (false; blargg's cpu_interrupts_v2: NMI
    /// hijacks BRK for 5 clocks, then follows the handler's first
    /// instruction).
    pub lose_late_nmi: bool,
    /// CMOS 65C02 opcode matrix and timing.
    pub cmos: bool,
    /// The opcode matrix ([`TABLE`] or [`CMOS_TABLE`]).
    table: &'static [(Op, Mode); 256],
}

impl Core {
    pub(crate) fn new(
        ane_magic: u8,
        lxa_magic: u8,
        decimal: bool,
        lose_late_nmi: bool,
        cmos: bool,
    ) -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            s: 0,
            p: I,
            pc: 0,
            op: 0,
            t: 0,
            ea: 0,
            base: 0,
            ptr: 0,
            data: 0,
            crossed: false,
            seq: Seq::Brk,
            vector: 0,
            irq_line: false,
            nmi_line: false,
            nmi_pending: false,
            nmi_rearm: false,
            rdy_stalled: false,
            stall_p: 0,
            reset_pending: false,
            poll: false,
            no_poll: false,
            polled: false,
            jammed: false,
            entered: None,
            ane_magic,
            lxa_magic,
            decimal,
            lose_late_nmi,
            cmos,
            table: if cmos { &CMOS_TABLE } else { &TABLE },
        }
    }

    /// Operation and addressing mode of the current opcode.
    #[inline]
    pub(crate) fn entry(&self) -> (Op, Mode) {
        self.table[usize::from(self.op)]
    }

    /// Whether a stall of the cycle about to run changes the instruction's
    /// result (VICE testprogs `CPU/ane`, `CPU/sha`, `CPU/shs`, `CPU/shxy`).
    pub(crate) fn rdy_sensitive(&self) -> bool {
        match self.entry() {
            (Op::Ane, _) => self.t == 1,
            (Op::Sha | Op::Shx | Op::Shy | Op::Tas, Mode::IndY) => self.t == 4,
            (Op::Sha | Op::Shx | Op::Shy | Op::Tas, _) => self.t == 3,
            _ => false,
        }
    }

    /// Whether interrupts are recognised while RDY repeats this cycle: not
    /// in an opcode fetch (the last poll stands), BRK or an interrupt
    /// sequence, nor in the last cycle of a taken branch
    /// (`interrupts/irqdma` test 3, test 5).
    pub(crate) fn stall_polls(&self) -> bool {
        self.t != 0 && self.op != 0x00 && !self.no_poll && !(self.op & 0x1f == 0x10 && self.t == 2)
    }

    /// Execute one cycle.
    pub(crate) fn cycle(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.jammed {
            // Observed bus pattern of a jammed NMOS 6502 (Tom Harte's tests):
            // $FFFF, $FFFE, $FFFE, then $FFFF forever.
            let addr = if matches!(self.t, 3 | 4) {
                0xfffe
            } else {
                0xffff
            };
            read(bus, addr);
            self.t = self.t.saturating_add(1);
            return CycleResult::Jammed;
        }

        let done = if self.t == 0 {
            self.fetch(bus)
        } else {
            self.exec(bus)
        };
        let skip_poll = std::mem::take(&mut self.no_poll);
        self.polled = false;

        if done {
            if self.t == 0 {
                // A one-cycle 65C02 NOP: its only cycle is also its last.
                self.poll = self.nmi_pending || (self.irq_line && self.p & I == 0);
            }
            self.t = 0;
            return CycleResult::InstrDone {
                next_pc: u32::from(self.pc),
            };
        }
        self.t += 1;
        // Interrupts are polled at the end of every cycle but the last, so the
        // value from the penultimate cycle decides. BRK and the interrupt
        // sequences do not poll: the first handler instruction always runs
        // before the next interrupt (Lorenz nmi).
        if !skip_poll && self.op != 0x00 {
            self.poll = self.nmi_pending || (self.irq_line && self.p & I == 0);
            self.polled = true;
        }
        if self.jammed {
            CycleResult::Jammed
        } else {
            CycleResult::Busy
        }
    }

    /// Cycle 0: opcode fetch, or the first cycle of an interrupt sequence.
    fn fetch(&mut self, bus: &mut dyn Bus) -> bool {
        if self.reset_pending || self.poll {
            // The fetched opcode is discarded and PC is not incremented.
            read(bus, self.pc);
            self.op = 0x00;
            self.poll = false;
            self.seq = if self.reset_pending {
                Seq::Reset
            } else {
                Seq::Interrupt
            };
            self.reset_pending = false;
        } else {
            self.op = self.fetch_pc(bus);
            self.seq = Seq::Brk;
            if self.cmos && self.entry().0 == Op::Nop1 {
                return true;
            }
        }
        false
    }

    fn exec(&mut self, bus: &mut dyn Bus) -> bool {
        let (op, mode) = self.entry();
        match op {
            Op::Brk => self.brk(bus),
            Op::Rti => self.rti(bus),
            Op::Rts => self.rts(bus),
            Op::Jsr => self.jsr(bus),
            Op::Jmp if mode == Mode::Ind && self.cmos => self.jmp_ind_cmos(bus, false),
            Op::Jmp if mode == Mode::AbsXInd => self.jmp_ind_cmos(bus, true),
            Op::Jmp if mode == Mode::Ind => self.jmp_ind(bus),
            Op::Jmp => self.jmp_abs(bus),
            Op::Pha | Op::Php | Op::Phx | Op::Phy => self.push_op(bus, op),
            Op::Pla | Op::Plp | Op::Plx | Op::Ply => self.pull_op(bus, op),
            Op::Bpl
            | Op::Bmi
            | Op::Bvc
            | Op::Bvs
            | Op::Bcc
            | Op::Bcs
            | Op::Bne
            | Op::Beq
            | Op::Bra => self.branch(bus, op),
            Op::NopAbs | Op::NopAbs3 => self.nop_abs(bus, op),
            // Handled in the fetch cycle.
            Op::Nop1 => true,
            Op::Jam => {
                read(bus, self.pc);
                self.jammed = true;
                false
            }
            _ => self.addressed(bus, op, mode),
        }
    }

    // ---- helpers -------------------------------------------------------

    #[inline]
    fn fetch_pc(&mut self, bus: &mut dyn Bus) -> u8 {
        let v = read(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    #[inline]
    fn stack(&self) -> u16 {
        0x0100 | u16::from(self.s)
    }

    fn push(&mut self, bus: &mut dyn Bus, v: u8) {
        if self.seq == Seq::Reset && self.op == 0x00 {
            read(bus, self.stack());
        } else {
            write(bus, self.stack(), v);
        }
        self.s = self.s.wrapping_sub(1);
    }

    #[inline]
    fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.p |= flag;
        } else {
            self.p &= !flag;
        }
    }

    #[inline]
    fn set_nz(&mut self, v: u8) {
        self.set_flag(Z, v == 0);
        self.set_flag(N, v & 0x80 != 0);
    }

    // ---- special sequences ---------------------------------------------

    /// BRK, IRQ, NMI and reset share one 7-cycle sequence.
    /// The vector of a BRK/interrupt sequence: a pending NMI takes over.
    fn choose_vector(&mut self) {
        self.vector = if self.seq == Seq::Reset {
            0xfffc
        } else if self.nmi_pending {
            self.nmi_pending = false;
            0xfffa
        } else {
            0xfffe
        };
    }

    fn brk(&mut self, bus: &mut dyn Bus) -> bool {
        match self.t {
            1 => {
                read(bus, self.pc);
                if self.seq == Seq::Brk {
                    self.pc = self.pc.wrapping_add(1);
                }
                false
            }
            2 => {
                self.push(bus, (self.pc >> 8) as u8);
                false
            }
            3 => {
                self.push(bus, self.pc as u8);
                // An NMI that is pending here hijacks BRK and IRQ. One that
                // arrives later waits for the first handler instruction
                // (Lorenz "nmi").
                if self.lose_late_nmi {
                    self.choose_vector();
                }
                false
            }
            4 => {
                let p = if self.seq == Seq::Brk {
                    self.p | B | U
                } else {
                    self.p | U
                };
                self.push(bus, p);
                // An NMI arriving in this cycle is too late to hijack, and the
                // edge latch is cleared as the vector is chosen. A line that
                // is still held in the next cycle registers again and is
                // taken after the first handler instruction (CIA2 on the
                // C64, Lorenz `nmi`); a pulse that is over by then is lost
                // (ANTIC's one-cycle NMI, Acid800 `antic_blockednmi`).
                if !self.lose_late_nmi {
                    self.choose_vector();
                } else if self.seq != Seq::Reset {
                    self.nmi_rearm = std::mem::take(&mut self.nmi_pending);
                }
                false
            }
            5 => {
                if std::mem::take(&mut self.nmi_rearm) && self.nmi_line {
                    self.nmi_pending = true;
                }
                self.data = read(bus, self.vector);
                self.p |= I;
                if self.cmos {
                    self.p &= !D;
                }
                false
            }
            _ => {
                let hi = read(bus, self.vector.wrapping_add(1));
                self.pc = u16::from_le_bytes([self.data, hi]);
                // BRK hijacked by an NMI counts as the NMI.
                self.entered = match (self.seq, self.vector) {
                    (Seq::Reset, _) => None,
                    (_, 0xfffa) => Some(InterruptKind::Nmi),
                    (Seq::Interrupt, _) => Some(InterruptKind::Irq),
                    _ => None,
                };
                true
            }
        }
    }

    fn rti(&mut self, bus: &mut dyn Bus) -> bool {
        match self.t {
            1 => {
                read(bus, self.pc);
                false
            }
            2 => {
                read(bus, self.stack());
                self.s = self.s.wrapping_add(1);
                false
            }
            3 => {
                self.p = read(bus, self.stack()) & !(B | U);
                self.s = self.s.wrapping_add(1);
                false
            }
            4 => {
                self.data = read(bus, self.stack());
                self.s = self.s.wrapping_add(1);
                false
            }
            _ => {
                let hi = read(bus, self.stack());
                self.pc = u16::from_le_bytes([self.data, hi]);
                true
            }
        }
    }

    fn rts(&mut self, bus: &mut dyn Bus) -> bool {
        match self.t {
            1 => {
                read(bus, self.pc);
                false
            }
            2 => {
                read(bus, self.stack());
                self.s = self.s.wrapping_add(1);
                false
            }
            3 => {
                self.data = read(bus, self.stack());
                self.s = self.s.wrapping_add(1);
                false
            }
            4 => {
                let hi = read(bus, self.stack());
                self.pc = u16::from_le_bytes([self.data, hi]);
                false
            }
            _ => {
                read(bus, self.pc);
                self.pc = self.pc.wrapping_add(1);
                true
            }
        }
    }

    fn jsr(&mut self, bus: &mut dyn Bus) -> bool {
        match self.t {
            1 => {
                self.data = self.fetch_pc(bus);
                false
            }
            2 => {
                read(bus, self.stack());
                false
            }
            3 => {
                self.push(bus, (self.pc >> 8) as u8);
                false
            }
            4 => {
                self.push(bus, self.pc as u8);
                false
            }
            _ => {
                // Read after the pushes: JSR can overwrite its own operand.
                let hi = read(bus, self.pc);
                self.pc = u16::from_le_bytes([self.data, hi]);
                true
            }
        }
    }

    fn jmp_abs(&mut self, bus: &mut dyn Bus) -> bool {
        if self.t == 1 {
            self.data = self.fetch_pc(bus);
            false
        } else {
            let hi = read(bus, self.pc);
            self.pc = u16::from_le_bytes([self.data, hi]);
            true
        }
    }

    fn jmp_ind(&mut self, bus: &mut dyn Bus) -> bool {
        match self.t {
            1 => {
                self.ea = u16::from(self.fetch_pc(bus));
                false
            }
            2 => {
                self.ea |= u16::from(self.fetch_pc(bus)) << 8;
                false
            }
            3 => {
                self.data = read(bus, self.ea);
                false
            }
            _ => {
                // The pointer's high byte is fetched without carry into the page.
                let hi_addr = (self.ea & 0xff00) | (self.ea.wrapping_add(1) & 0x00ff);
                let hi = read(bus, hi_addr);
                self.pc = u16::from_le_bytes([self.data, hi]);
                true
            }
        }
    }

    /// JMP (abs) and JMP (abs,X) of the 65C02: the pointer's high byte
    /// comes from the next address with carry; one more cycle reads it
    /// again (abs) or reads the operand's low byte again before the
    /// pointer (abs,X).
    fn jmp_ind_cmos(&mut self, bus: &mut dyn Bus, indexed: bool) -> bool {
        match (self.t, indexed) {
            (1, _) => {
                self.ea = u16::from(self.fetch_pc(bus));
                false
            }
            (2, _) => {
                self.ea |= u16::from(self.fetch_pc(bus)) << 8;
                if indexed {
                    self.ea = self.ea.wrapping_add(u16::from(self.x));
                }
                false
            }
            (3, true) => {
                read(bus, self.pc.wrapping_sub(2));
                false
            }
            (3, false) | (4, true) => {
                self.data = read(bus, self.ea);
                false
            }
            (4, false) => {
                // First the next address without carry, as the NMOS reads it.
                read(bus, (self.ea & 0xff00) | (self.ea.wrapping_add(1) & 0x00ff));
                false
            }
            (_, true) => {
                let hi = read(bus, self.ea.wrapping_add(1));
                self.pc = u16::from_le_bytes([self.data, hi]);
                true
            }
            _ => {
                let hi = read(bus, self.ea.wrapping_add(1));
                self.pc = u16::from_le_bytes([self.data, hi]);
                true
            }
        }
    }

    /// The 65C02's undefined three-byte NOPs.
    fn nop_abs(&mut self, bus: &mut dyn Bus, op: Op) -> bool {
        match self.t {
            1 => {
                self.fetch_pc(bus);
                false
            }
            2 => {
                self.fetch_pc(bus);
                op == Op::NopAbs3
            }
            _ => {
                read(bus, self.pc.wrapping_sub(1));
                true
            }
        }
    }

    fn push_op(&mut self, bus: &mut dyn Bus, op: Op) -> bool {
        if self.t == 1 {
            read(bus, self.pc);
            false
        } else {
            let v = match op {
                Op::Pha => self.a,
                Op::Phx => self.x,
                Op::Phy => self.y,
                _ => self.p | B | U,
            };
            self.push(bus, v);
            true
        }
    }

    fn pull_op(&mut self, bus: &mut dyn Bus, op: Op) -> bool {
        match self.t {
            1 => {
                read(bus, self.pc);
                false
            }
            2 => {
                read(bus, self.stack());
                self.s = self.s.wrapping_add(1);
                false
            }
            _ => {
                let v = read(bus, self.stack());
                match op {
                    Op::Pla => {
                        self.a = v;
                        self.set_nz(v);
                    }
                    Op::Plx => {
                        self.x = v;
                        self.set_nz(v);
                    }
                    Op::Ply => {
                        self.y = v;
                        self.set_nz(v);
                    }
                    _ => self.p = v & !(B | U),
                }
                true
            }
        }
    }

    fn branch_taken(&self, op: Op) -> bool {
        match op {
            Op::Bpl => self.p & N == 0,
            Op::Bmi => self.p & N != 0,
            Op::Bvc => self.p & V == 0,
            Op::Bvs => self.p & V != 0,
            Op::Bcc => self.p & C == 0,
            Op::Bcs => self.p & C != 0,
            Op::Bne => self.p & Z == 0,
            Op::Beq => self.p & Z != 0,
            Op::Bra => true,
            _ => false,
        }
    }

    fn branch(&mut self, bus: &mut dyn Bus, op: Op) -> bool {
        match self.t {
            1 => {
                self.data = self.fetch_pc(bus);
                if !self.branch_taken(op) {
                    return true;
                }
                // A taken branch does not poll interrupts during this cycle.
                self.no_poll = true;
                false
            }
            2 => {
                read(bus, self.pc);
                let target = self.pc.wrapping_add(self.data as i8 as u16);
                self.crossed = (target ^ self.pc) & 0xff00 != 0;
                self.ea = target;
                self.pc = (self.pc & 0xff00) | (target & 0x00ff);
                !self.crossed
            }
            _ => {
                read(bus, self.pc);
                self.pc = self.ea;
                true
            }
        }
    }

    // ---- addressing modes ----------------------------------------------

    #[inline]
    fn index(&self, mode: Mode) -> u8 {
        if matches!(mode, Mode::ZpY | Mode::AbsY | Mode::IndY) {
            self.y
        } else {
            self.x
        }
    }

    fn addressed(&mut self, bus: &mut dyn Bus, op: Op, mode: Mode) -> bool {
        let kind = op.kind();
        let t = self.t;
        match mode {
            Mode::Imp => {
                read(bus, self.pc);
                self.implied(op);
                true
            }
            Mode::Imm if t == 1 => {
                let v = self.fetch_pc(bus);
                if self.decimal_cycle(op) {
                    self.data = v;
                    return false;
                }
                self.read_op(op, v);
                true
            }
            Mode::Imm => {
                self.decimal_dummy(bus, true);
                self.read_op(op, self.data);
                true
            }
            Mode::Zp => {
                if t == 1 {
                    self.ea = u16::from(self.fetch_pc(bus));
                    false
                } else {
                    self.operand(bus, op, kind, t - 2)
                }
            }
            Mode::ZpX | Mode::ZpY => match t {
                1 => {
                    self.base = u16::from(self.fetch_pc(bus));
                    false
                }
                2 => {
                    read(bus, self.base);
                    self.ea = u16::from((self.base as u8).wrapping_add(self.index(mode)));
                    false
                }
                _ => self.operand(bus, op, kind, t - 3),
            },
            Mode::Abs => match t {
                1 => {
                    self.ea = u16::from(self.fetch_pc(bus));
                    false
                }
                2 => {
                    self.ea |= u16::from(self.fetch_pc(bus)) << 8;
                    false
                }
                _ => self.operand(bus, op, kind, t - 3),
            },
            Mode::AbsX | Mode::AbsY => match t {
                1 => {
                    self.base = u16::from(self.fetch_pc(bus));
                    false
                }
                2 => {
                    self.base |= u16::from(self.fetch_pc(bus)) << 8;
                    self.set_indexed_ea(mode);
                    false
                }
                3 => self.fixup(bus, op, kind),
                _ => {
                    let skipped = u8::from(self.fast_rmw(op, mode) || self.read_no_fixup(kind));
                    self.operand(bus, op, kind, t - 4 + skipped)
                }
            },
            Mode::IndX => match t {
                1 => {
                    self.ptr = self.fetch_pc(bus);
                    false
                }
                2 => {
                    read(bus, u16::from(self.ptr));
                    self.ptr = self.ptr.wrapping_add(self.x);
                    false
                }
                3 => {
                    self.ea = u16::from(read(bus, u16::from(self.ptr)));
                    false
                }
                4 => {
                    let hi = read(bus, u16::from(self.ptr.wrapping_add(1)));
                    self.ea |= u16::from(hi) << 8;
                    false
                }
                _ => self.operand(bus, op, kind, t - 5),
            },
            Mode::IndY => match t {
                1 => {
                    self.ptr = self.fetch_pc(bus);
                    false
                }
                2 => {
                    self.base = u16::from(read(bus, u16::from(self.ptr)));
                    false
                }
                3 => {
                    let hi = read(bus, u16::from(self.ptr.wrapping_add(1)));
                    self.base |= u16::from(hi) << 8;
                    self.set_indexed_ea(mode);
                    false
                }
                4 => self.fixup(bus, op, kind),
                _ => {
                    let skipped = u8::from(self.read_no_fixup(kind));
                    self.operand(bus, op, kind, t - 5 + skipped)
                }
            },
            Mode::Zpi => match t {
                1 => {
                    self.ptr = self.fetch_pc(bus);
                    false
                }
                2 => {
                    self.ea = u16::from(read(bus, u16::from(self.ptr)));
                    false
                }
                3 => {
                    let hi = read(bus, u16::from(self.ptr.wrapping_add(1)));
                    self.ea |= u16::from(hi) << 8;
                    false
                }
                _ => self.operand(bus, op, kind, t - 4),
            },
            // Ind, AbsXInd and Rel only occur with special operations.
            Mode::Ind | Mode::AbsXInd | Mode::Rel => true,
        }
    }

    fn set_indexed_ea(&mut self, mode: Mode) {
        self.ea = self.base.wrapping_add(u16::from(self.index(mode)));
        self.crossed = (self.ea ^ self.base) & 0xff00 != 0;
    }

    /// The cycle that reads from the not-yet-carried address. Reads that did
    /// not cross a page use it as the real operand read.
    fn fixup(&mut self, bus: &mut dyn Bus, op: Op, kind: Kind) -> bool {
        if kind == Kind::Read && !self.crossed {
            return self.operand(bus, op, kind, 0);
        }
        let (_, mode) = self.entry();
        if self.fast_rmw(op, mode) {
            return self.operand(bus, op, kind, 0);
        }
        if self.cmos {
            // The 65C02 reads the last operand byte again instead of the
            // address without carry.
            read(bus, self.pc.wrapping_sub(1));
        } else {
            read(bus, (self.base & 0xff00) | (self.ea & 0x00ff));
        }
        false
    }

    /// A read whose fix-up cycle was its operand read (no page crossing);
    /// only a 65C02 decimal-mode ADC/SBC gets past it.
    fn read_no_fixup(&self, kind: Kind) -> bool {
        kind == Kind::Read && !self.crossed
    }

    /// 65C02 shifts at abs,X without a page crossing skip the fix-up cycle.
    fn fast_rmw(&self, op: Op, mode: Mode) -> bool {
        self.cmos
            && mode == Mode::AbsX
            && !self.crossed
            && matches!(op, Op::Asl | Op::Lsr | Op::Rol | Op::Ror)
    }

    /// The 65C02 spends a cycle more on ADC and SBC in decimal mode.
    fn decimal_cycle(&self, op: Op) -> bool {
        self.cmos && self.p & D != 0 && matches!(op, Op::Adc | Op::Sbc)
    }

    /// That cycle's bus access: the operand again, or for an immediate
    /// operand a fixed zero-page address ($56 for ADC, $00 for SBC; Tom
    /// Harte's `synertek65c02` traces).
    fn decimal_dummy(&mut self, bus: &mut dyn Bus, immediate: bool) {
        let addr = match (immediate, self.entry().0) {
            (false, _) => self.ea,
            (true, Op::Adc) => 0x56,
            (true, _) => 0x00,
        };
        read(bus, addr);
    }

    /// Operand cycles at the effective address; `k` counts from 0.
    fn operand(&mut self, bus: &mut dyn Bus, op: Op, kind: Kind, k: u8) -> bool {
        match (kind, k) {
            (Kind::Read, 0) => {
                let v = read(bus, self.ea);
                if self.decimal_cycle(op) {
                    self.data = v;
                    return false;
                }
                self.read_op(op, v);
                true
            }
            (Kind::Read, _) => {
                self.decimal_dummy(bus, false);
                self.read_op(op, self.data);
                true
            }
            (Kind::Write, _) => {
                self.store(bus, op);
                true
            }
            (Kind::Rmw, 0) => {
                self.data = read(bus, self.ea);
                false
            }
            (Kind::Rmw, 1) => {
                // Dummy write of the unmodified value (a second read on the
                // 65C02).
                if self.cmos {
                    read(bus, self.ea);
                } else {
                    write(bus, self.ea, self.data);
                }
                self.data = self.rmw_op(op, self.data);
                false
            }
            _ => {
                write(bus, self.ea, self.data);
                true
            }
        }
    }

    // ---- operations ----------------------------------------------------

    fn implied(&mut self, op: Op) {
        match op {
            Op::Tax => {
                self.x = self.a;
                self.set_nz(self.x);
            }
            Op::Txa => {
                self.a = self.x;
                self.set_nz(self.a);
            }
            Op::Tay => {
                self.y = self.a;
                self.set_nz(self.y);
            }
            Op::Tya => {
                self.a = self.y;
                self.set_nz(self.a);
            }
            Op::Tsx => {
                self.x = self.s;
                self.set_nz(self.x);
            }
            Op::Txs => self.s = self.x,
            Op::Inx => {
                self.x = self.x.wrapping_add(1);
                self.set_nz(self.x);
            }
            Op::Iny => {
                self.y = self.y.wrapping_add(1);
                self.set_nz(self.y);
            }
            Op::Dex => {
                self.x = self.x.wrapping_sub(1);
                self.set_nz(self.x);
            }
            Op::Dey => {
                self.y = self.y.wrapping_sub(1);
                self.set_nz(self.y);
            }
            Op::Clc => self.p &= !C,
            Op::Sec => self.p |= C,
            Op::Cli => self.p &= !I,
            Op::Sei => self.p |= I,
            Op::Clv => self.p &= !V,
            Op::Cld => self.p &= !D,
            Op::Sed => self.p |= D,
            // Accumulator forms of the shifts (and the 65C02's INC A, DEC A).
            Op::Asl | Op::Lsr | Op::Rol | Op::Ror | Op::Inc | Op::Dec => {
                self.a = self.rmw_op(op, self.a)
            }
            _ => {}
        }
    }

    fn read_op(&mut self, op: Op, v: u8) {
        match op {
            Op::Lda => {
                self.a = v;
                self.set_nz(v);
            }
            Op::Ldx => {
                self.x = v;
                self.set_nz(v);
            }
            Op::Ldy => {
                self.y = v;
                self.set_nz(v);
            }
            Op::Lax => {
                self.a = v;
                self.x = v;
                self.set_nz(v);
            }
            Op::And => {
                self.a &= v;
                self.set_nz(self.a);
            }
            Op::Ora => {
                self.a |= v;
                self.set_nz(self.a);
            }
            Op::Eor => {
                self.a ^= v;
                self.set_nz(self.a);
            }
            Op::Adc => self.adc(v),
            Op::Sbc => self.sbc(v),
            Op::Cmp => self.compare(self.a, v),
            Op::Cpx => self.compare(self.x, v),
            Op::Cpy => self.compare(self.y, v),
            // BIT # (65C02) sets Z only.
            Op::Bit if self.entry().1 == Mode::Imm => self.set_flag(Z, self.a & v == 0),
            Op::Bit => {
                self.set_flag(Z, self.a & v == 0);
                self.set_flag(N, v & 0x80 != 0);
                self.set_flag(V, v & 0x40 != 0);
            }
            Op::Anc => {
                self.a &= v;
                self.set_nz(self.a);
                self.set_flag(C, self.a & 0x80 != 0);
            }
            Op::Alr => {
                let t = self.a & v;
                self.set_flag(C, t & 0x01 != 0);
                self.a = t >> 1;
                self.set_nz(self.a);
            }
            Op::Arr => self.arr(v),
            Op::Ane => {
                // Bits 0 and 4 of the constant may drop in the RDY cycle
                // (VICE testprogs `CPU/ane/readme.txt`).
                let magic = if std::mem::take(&mut self.rdy_stalled) {
                    self.ane_magic & 0xee
                } else {
                    self.ane_magic
                };
                self.a = (self.a | magic) & self.x & v;
                self.set_nz(self.a);
            }
            Op::Lxa => {
                self.a = (self.a | self.lxa_magic) & v;
                self.x = self.a;
                self.set_nz(self.a);
            }
            Op::Sbx => {
                let t = self.a & self.x;
                self.set_flag(C, t >= v);
                self.x = t.wrapping_sub(v);
                self.set_nz(self.x);
            }
            Op::Las => {
                let r = v & self.s;
                self.a = r;
                self.x = r;
                self.s = r;
                self.set_nz(r);
            }
            _ => {} // NOP variants
        }
    }

    fn store(&mut self, bus: &mut dyn Bus, op: Op) {
        let v = match op {
            Op::Sta => self.a,
            Op::Stx => self.x,
            Op::Sty => self.y,
            Op::Sax => self.a & self.x,
            Op::Stz => 0,
            Op::Sha => return self.store_unstable(bus, self.a & self.x),
            Op::Shx => return self.store_unstable(bus, self.x),
            Op::Shy => return self.store_unstable(bus, self.y),
            Op::Tas => {
                self.s = self.a & self.x;
                return self.store_unstable(bus, self.s);
            }
            _ => return,
        };
        write(bus, self.ea, v);
    }

    /// SHA/SHX/SHY/TAS store `v & (base high byte + 1)`; on a page crossing
    /// that value also replaces the high byte of the address. When RDY held
    /// the cycle before the write the AND drops off the stored value, not
    /// the address (VICE testprogs `CPU/sha/readme.txt`, `CPU/shxy/readme.txt`).
    fn store_unstable(&mut self, bus: &mut dyn Bus, v: u8) {
        let masked = v & ((self.base >> 8) as u8).wrapping_add(1);
        let addr = if self.crossed {
            (u16::from(masked) << 8) | (self.ea & 0x00ff)
        } else {
            self.ea
        };
        let stored = if std::mem::take(&mut self.rdy_stalled) {
            v
        } else {
            masked
        };
        write(bus, addr, stored);
    }

    fn rmw_op(&mut self, op: Op, v: u8) -> u8 {
        match op {
            Op::Asl => self.asl(v),
            Op::Lsr => self.lsr(v),
            Op::Rol => self.rol(v),
            Op::Ror => self.ror(v),
            Op::Inc => {
                let r = v.wrapping_add(1);
                self.set_nz(r);
                r
            }
            Op::Dec => {
                let r = v.wrapping_sub(1);
                self.set_nz(r);
                r
            }
            Op::Slo => {
                let r = self.asl(v);
                self.a |= r;
                self.set_nz(self.a);
                r
            }
            Op::Rla => {
                let r = self.rol(v);
                self.a &= r;
                self.set_nz(self.a);
                r
            }
            Op::Sre => {
                let r = self.lsr(v);
                self.a ^= r;
                self.set_nz(self.a);
                r
            }
            Op::Rra => {
                let r = self.ror(v);
                self.adc(r);
                r
            }
            Op::Dcp => {
                let r = v.wrapping_sub(1);
                self.compare(self.a, r);
                r
            }
            Op::Isc => {
                let r = v.wrapping_add(1);
                self.sbc(r);
                r
            }
            Op::Tsb => {
                self.set_flag(Z, self.a & v == 0);
                v | self.a
            }
            Op::Trb => {
                self.set_flag(Z, self.a & v == 0);
                v & !self.a
            }
            _ => v,
        }
    }

    fn asl(&mut self, v: u8) -> u8 {
        self.set_flag(C, v & 0x80 != 0);
        let r = v << 1;
        self.set_nz(r);
        r
    }

    fn lsr(&mut self, v: u8) -> u8 {
        self.set_flag(C, v & 0x01 != 0);
        let r = v >> 1;
        self.set_nz(r);
        r
    }

    fn rol(&mut self, v: u8) -> u8 {
        let r = (v << 1) | (self.p & C);
        self.set_flag(C, v & 0x80 != 0);
        self.set_nz(r);
        r
    }

    fn ror(&mut self, v: u8) -> u8 {
        let r = (v >> 1) | ((self.p & C) << 7);
        self.set_flag(C, v & 0x01 != 0);
        self.set_nz(r);
        r
    }

    fn compare(&mut self, r: u8, v: u8) {
        self.set_flag(C, r >= v);
        self.set_nz(r.wrapping_sub(v));
    }

    /// ADC including NMOS decimal mode (Bruce Clark, "Decimal Mode in NMOS
    /// 6500 series", appendix A): Z from the binary sum, N and V from the
    /// intermediate result.
    fn adc(&mut self, v: u8) {
        let (a, m, c) = (u16::from(self.a), u16::from(v), u16::from(self.p & C));
        let bin = a + m + c;
        if self.p & D == 0 || !self.decimal {
            self.set_flag(C, bin > 0xff);
            self.set_flag(V, !(a ^ m) & (a ^ bin) & 0x80 != 0);
            self.a = bin as u8;
            self.set_nz(self.a);
            return;
        }
        let mut lo = (a & 0x0f) + (m & 0x0f) + c;
        if lo >= 0x0a {
            lo = ((lo + 0x06) & 0x0f) + 0x10;
        }
        let mut r = (a & 0xf0) + (m & 0xf0) + lo;
        self.set_flag(Z, bin & 0xff == 0);
        self.set_flag(N, r & 0x80 != 0);
        self.set_flag(V, !(a ^ m) & (a ^ r) & 0x80 != 0);
        if r >= 0xa0 {
            r += 0x60;
        }
        self.set_flag(C, r >= 0x100);
        self.a = r as u8;
        if self.cmos {
            // The 65C02's N and Z follow the decimal result.
            self.set_nz(self.a);
        }
    }

    /// SBC including NMOS decimal mode; flags always follow the binary result.
    fn sbc(&mut self, v: u8) {
        let borrow = u16::from(self.p & C == 0);
        let bin = u16::from(self.a)
            .wrapping_sub(u16::from(v))
            .wrapping_sub(borrow);
        let r8 = bin as u8;
        self.set_flag(C, bin < 0x100);
        self.set_flag(V, (self.a ^ v) & (self.a ^ r8) & 0x80 != 0);
        self.set_nz(r8);
        if self.p & D == 0 || !self.decimal {
            self.a = r8;
            return;
        }
        let (a, m, b) = (i16::from(self.a), i16::from(v), i16::from(borrow as u8));
        if self.cmos {
            // Bruce Clark, "Decimal Mode", appendix A, 65C02 SBC (steps
            // 3a-3e): the high adjustment first, then the low one.
            let lo = (a & 0x0f) - (m & 0x0f) - b;
            let mut r = a - m - b;
            if r < 0 {
                r -= 0x60;
            }
            if lo < 0 {
                r -= 0x06;
            }
            self.a = r as u8;
            self.set_nz(self.a);
            return;
        }
        let mut lo = (a & 0x0f) - (m & 0x0f) - b;
        if lo < 0 {
            lo = ((lo - 0x06) & 0x0f) - 0x10;
        }
        let mut r = (a & 0xf0) - (m & 0xf0) + lo;
        if r < 0 {
            r -= 0x60;
        }
        self.a = r as u8;
    }

    /// ARR, including its NMOS decimal-mode behaviour ("NMOS 6510 Unintended
    /// Opcodes", section ARR).
    fn arr(&mut self, v: u8) {
        let t = self.a & v;
        let carry_in = self.p & C != 0;
        let mut r = (t >> 1) | if carry_in { 0x80 } else { 0 };
        if self.p & D == 0 || !self.decimal {
            self.a = r;
            self.set_nz(r);
            self.set_flag(C, r & 0x40 != 0);
            self.set_flag(V, ((r >> 6) ^ (r >> 5)) & 0x01 != 0);
            return;
        }
        self.set_flag(N, carry_in);
        self.set_flag(Z, r == 0);
        self.set_flag(V, (t ^ r) & 0x40 != 0);
        if (t & 0x0f) + (t & 0x01) > 5 {
            r = (r & 0xf0) | (r.wrapping_add(6) & 0x0f);
        }
        let carry = u16::from(t & 0xf0) + u16::from(t & 0x10) > 0x50;
        self.set_flag(C, carry);
        if carry {
            r = r.wrapping_add(0x60);
        }
        self.a = r;
    }
}
