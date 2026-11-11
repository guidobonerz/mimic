//! The MEGA65's 45GS02: the CSG 4510 / 65CE02 core (registers A, X, Y, Z,
//! the base page register B, a 16-bit stack pointer with the E flag, long
//! branches, word instructions) with the MEGA65's extensions: the MAP
//! megabyte registers for its 28-bit address space, flat 32-bit pointers
//! (`EA` before a `(zp),Z` instruction: `[zp],Z`) and the 32-bit Q
//! instructions (`42 42` before an instruction; Q is Z:Y:X:A).
//!
//! **Bus addresses.** Every access goes through the MAP translation
//! ([`Map`]). A mapped access carries its 28-bit address. An access MAP
//! leaves alone carries the CPU's 16-bit address with [`UNMAPPED`] set: the
//! machine decodes it (the C64 port's ROM banking, the VIC-III's `$D030`
//! ROM bits, I/O at `$D000`), as the MAP translation takes precedence over
//! every other banking. Flat accesses carry their 28-bit address.
//!
//! **Timing.** A [`Cpu::tick`] is one CPU cycle at the speed the machine
//! set ([`Speed`]). An instruction does all its bus accesses in its first
//! cycle and then idles for the rest of its count, which is what the
//! hardware does at 1–3.5 MHz ("instructions complete faster, and the
//! processor idles until the correct number of cycles have passed", the
//! MEGA65 user guide, 45GS02 appendix). The counts: at 1 and 2 MHz the
//! 6502's for its own opcodes (the MEGA65 re-inserts the dummy cycles), at
//! 3.5 and 40 MHz the 4510 table with its flags ([`table`]).
//!
//! **Interrupts.** IRQ is level-sensitive and masked by I; NMI is taken on
//! a rising edge. After MAP both are held off until the next EOM (`EA`).
//! An interrupt takes 7 cycles. The stack: with E set (after reset) SP's
//! low byte wraps in the page SPH, with E clear SP is 16 bits.
//!
//! Decisions of this project where the documentation is silent (to be
//! measured on a real MEGA65): decimal ADC/SBC give valid N and Z (as the
//! CMOS parts); interrupts leave D alone; a long branch is relative to the
//! address of its last byte (opcode address + 2); a `42 42` prefix before
//! an opcode without a Q form is two ordinary NEGs.
//!
//! The RMW instructions write the unchanged value first when their target
//! is `$D019` (the VIC-II's interrupt flags), the 45GS02's C64
//! compatibility rule.
//!
//! Sources: the MEGA65 user guide (github.com/MEGA65/mega65-user-guide):
//! `appendix-45gs02-registers.tex` (MAP, flat memory, the Q register,
//! speeds, the `$D019` rule), `appendix-instructionset.tex` and
//! `instruction_sets/*` (opcodes, cycles, per-instruction semantics). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;
pub mod table;
#[cfg(test)]
mod tests;

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};
use emu_core::{Bus, Cpu, CycleResult, Instr, Register, RegisterSet, Size};

/// Set on a bus address the MAP translation left alone: the low 16 bits are
/// the CPU's address.
pub const UNMAPPED: u32 = 0x8000_0000;
/// The 28 address bits of the MEGA65.
pub const ADDR_MASK: u32 = 0x0fff_ffff;

/// Processor status bits.
pub mod flag {
    pub const C: u8 = 0x01;
    pub const Z: u8 = 0x02;
    pub const I: u8 = 0x04;
    pub const D: u8 = 0x08;
    /// Pushed set by BRK and PHP, not a stored flag.
    pub const B: u8 = 0x10;
    /// Extended stack disable: SP is 8 bits in page SPH.
    pub const E: u8 = 0x20;
    pub const V: u8 = 0x40;
    pub const N: u8 = 0x80;
}

/// Addressing modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Imp,
    Acc,
    Imm,
    /// PHW #$nnnn.
    Imm16,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Absx,
    Absy,
    /// JMP/JSR ($nnnn).
    Ind,
    /// JMP/JSR ($nnnn,X).
    Iax,
    /// ($nn,X).
    Izx,
    /// ($nn),Y.
    Izy,
    /// ($nn),Z; with the `EA` prefix [$nn],Z.
    Izz,
    /// ($nn,SP),Y.
    Isy,
    Rel,
    Rel16,
    /// BBR/BBS $nn,$rr.
    Zrel,
}

/// The CPU's speed (`$D031`/`$D054` and the C64 `$D030` bit, set by the
/// machine).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Speed {
    /// The C64's ~1 MHz.
    #[default]
    Mhz1,
    /// The C128's 2 MHz.
    Mhz2,
    /// The C65's 3.5 MHz.
    Mhz3_5,
    /// Full speed, 40.5 MHz.
    Mhz40,
}

/// The MAP registers: two halves of the 16-bit space, each with a 20-bit
/// offset (a multiple of 256), a mask of which 8 KB blocks it applies to,
/// and the MEGA65's megabyte number (bits 20–27).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Map {
    pub lo_offset: u32,
    pub hi_offset: u32,
    /// Bit n: block n (`$0000`/`$2000`/`$4000`/`$6000`; `$8000`… for hi).
    pub lo_mask: u8,
    pub hi_mask: u8,
    pub lo_mb: u8,
    pub hi_mb: u8,
}

impl Map {
    /// The bus address of CPU address `a`.
    pub fn translate(&self, a: u16) -> u32 {
        let block = a >> 13;
        let (mask, offset, mb, bit) = if block < 4 {
            (self.lo_mask, self.lo_offset, self.lo_mb, block)
        } else {
            (self.hi_mask, self.hi_offset, self.hi_mb, block - 4)
        };
        if mask >> bit & 1 != 0 {
            (u32::from(mb) << 20) | (offset.wrapping_add(u32::from(a)) & 0xf_ffff)
        } else {
            UNMAPPED | u32::from(a)
        }
    }

    /// The MAP instruction with A, X, Y, Z. X = `$0F` (no block mapped,
    /// offset bits 19–16 all set) also takes A as the lower half's
    /// megabyte; Z = `$0F` takes Y as the upper half's.
    pub fn set(&mut self, a: u8, x: u8, y: u8, z: u8) {
        if x == 0x0f {
            self.lo_mb = a;
        }
        if z == 0x0f {
            self.hi_mb = y;
        }
        self.lo_offset = (u32::from(x & 0x0f) << 16) | (u32::from(a) << 8);
        self.lo_mask = x >> 4;
        self.hi_offset = (u32::from(z & 0x0f) << 16) | (u32::from(y) << 8);
        self.hi_mask = z >> 4;
    }
}

const STATE_TAG: [u8; 4] = *b"45GS";
const STATE_VERSION: u16 = 1;

/// The 45GS02.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu45gs02 {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub z: u8,
    /// Base page: zero-page addresses are `B:nn`.
    pub b: u8,
    pub sp: u16,
    pub p: u8,
    pub pc: u16,
    pub map: Map,
    /// MAP ran and no EOM yet: interrupts are held off.
    pub map_lock: bool,
    speed: Speed,
    /// Idle cycles left of the instruction running.
    wait: u32,
    irq: bool,
    nmi: bool,
    nmi_pending: bool,
    /// Cycles of the last instruction or interrupt.
    pub last_cycles: u32,
}

impl Default for Cpu45gs02 {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu45gs02 {
    /// A CPU at PC = 0 with E and I set; machines call
    /// [`Cpu45gs02::reset`].
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            z: 0,
            b: 0,
            sp: 0x01ff,
            p: flag::E | flag::I,
            pc: 0,
            map: Map::default(),
            map_lock: false,
            speed: Speed::default(),
            wait: 0,
            irq: false,
            nmi: false,
            nmi_pending: false,
            last_cycles: 0,
        }
    }

    /// Reset: registers as at power-on (Z and B 0, SP `$01FF` with E set),
    /// MAP cleared, PC from the vector at `$FFFC`.
    pub fn reset(&mut self, bus: &mut dyn Bus) {
        let speed = self.speed;
        *self = Self::new();
        self.speed = speed;
        let lo = self.rd(bus, 0xfffc);
        let hi = self.rd(bus, 0xfffd);
        self.pc = u16::from_le_bytes([lo, hi]);
    }

    pub fn speed(&self) -> Speed {
        self.speed
    }

    /// The machine changed the speed; the instruction running keeps its
    /// count.
    pub fn set_speed(&mut self, speed: Speed) {
        self.speed = speed;
    }

    /// The IRQ line (level, active high here).
    pub fn set_irq(&mut self, active: bool) {
        self.irq = active;
    }

    /// The NMI line; a rising edge requests an NMI.
    pub fn set_nmi(&mut self, active: bool) {
        if active && !self.nmi {
            self.nmi_pending = true;
        }
        self.nmi = active;
    }

    /// The CPU is between instructions (the next tick starts one).
    pub fn at_boundary(&self) -> bool {
        self.wait == 0
    }

    /// Idle cycles of the running instruction before its last one (they
    /// touch nothing, see the crate's timing).
    pub fn idle(&self) -> u32 {
        self.wait.saturating_sub(1)
    }

    /// Spend `n` of the [`Cpu45gs02::idle`] cycles at once (machines that
    /// skip ahead without a debugger).
    pub fn skip(&mut self, n: u32) {
        self.wait -= n.min(self.idle());
    }

    /// Q: Z:Y:X:A.
    pub fn q(&self) -> u32 {
        u32::from_le_bytes([self.a, self.x, self.y, self.z])
    }

    pub fn set_q(&mut self, v: u32) {
        [self.a, self.x, self.y, self.z] = v.to_le_bytes();
    }

    pub(crate) fn rd(&self, bus: &mut dyn Bus, a: u16) -> u8 {
        bus.read(self.map.translate(a), Size::Byte) as u8
    }

    pub(crate) fn wr(&self, bus: &mut dyn Bus, a: u16, v: u8) {
        bus.write(self.map.translate(a), Size::Byte, u32::from(v));
    }

    /// The instruction or interrupt starting now; its cycle count.
    fn start(&mut self, bus: &mut dyn Bus) -> u32 {
        if self.nmi_pending && !self.map_lock {
            self.nmi_pending = false;
            self.interrupt(bus, 0xfffa, false);
            return 7;
        }
        if self.irq && self.p & flag::I == 0 && !self.map_lock {
            self.interrupt(bus, 0xfffe, false);
            return 7;
        }
        self.execute(bus)
    }

    /// Push PC and P, set I, jump through `vector`.
    pub(crate) fn interrupt(&mut self, bus: &mut dyn Bus, vector: u16, brk: bool) {
        let pc = self.pc;
        self.push(bus, (pc >> 8) as u8);
        self.push(bus, pc as u8);
        let p = (self.p & !flag::B) | if brk { flag::B } else { 0 };
        self.push(bus, p);
        self.p |= flag::I;
        let lo = self.rd(bus, vector);
        let hi = self.rd(bus, vector.wrapping_add(1));
        self.pc = u16::from_le_bytes([lo, hi]);
    }

    pub(crate) fn push(&mut self, bus: &mut dyn Bus, v: u8) {
        self.wr(bus, self.sp, v);
        self.sp = self.sp_step(-1);
    }

    pub(crate) fn pull(&mut self, bus: &mut dyn Bus) -> u8 {
        self.sp = self.sp_step(1);
        self.rd(bus, self.sp)
    }

    /// SP moved by `d`: within its page while E is set.
    pub(crate) fn sp_step(&self, d: i32) -> u16 {
        let moved = (i32::from(self.sp) + d) as u16;
        if self.p & flag::E != 0 {
            (self.sp & 0xff00) | (moved & 0x00ff)
        } else {
            moved
        }
    }
}

impl Cpu for Cpu45gs02 {
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.wait == 0 {
            let n = self.start(bus).max(1);
            self.last_cycles = n;
            self.wait = n;
        }
        self.wait -= 1;
        if self.wait == 0 {
            CycleResult::InstrDone {
                next_pc: u32::from(self.pc),
            }
        } else {
            CycleResult::Busy
        }
    }

    fn regs(&self) -> RegisterSet {
        let r = |name, value: u32, bits| Register { name, value, bits };
        RegisterSet(vec![
            r("PC", u32::from(self.pc), 16),
            r("A", u32::from(self.a), 8),
            r("X", u32::from(self.x), 8),
            r("Y", u32::from(self.y), 8),
            r("Z", u32::from(self.z), 8),
            r("B", u32::from(self.b), 8),
            r("SP", u32::from(self.sp), 16),
            r("P", u32::from(self.p), 8),
            r(
                "MAPLO",
                u32::from(self.map.lo_mask) << 20 | self.map.lo_offset,
                24,
            ),
            r(
                "MAPHI",
                u32::from(self.map.hi_mask) << 20 | self.map.hi_offset,
                24,
            ),
            r("MBLO", u32::from(self.map.lo_mb), 8),
            r("MBHI", u32::from(self.map.hi_mb), 8),
        ])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = v as u16,
            "A" => self.a = v as u8,
            "X" => self.x = v as u8,
            "Y" => self.y = v as u8,
            "Z" => self.z = v as u8,
            "B" => self.b = v as u8,
            "SP" => self.sp = v as u16,
            "P" => self.p = v as u8 & !flag::B,
            "Q" => self.set_q(v),
            "MAPLO" => {
                self.map.lo_mask = (v >> 20 & 15) as u8;
                self.map.lo_offset = v & 0xf_ff00;
            }
            "MAPHI" => {
                self.map.hi_mask = (v >> 20 & 15) as u8;
                self.map.hi_offset = v & 0xf_ff00;
            }
            "MBLO" => self.map.lo_mb = v as u8,
            "MBHI" => self.map.hi_mb = v as u8,
            _ => {}
        }
    }

    /// `addr` is a CPU address, read through the current MAP.
    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        let map = self.map;
        disasm::disassemble(
            &|a: u16| bus.peek(map.translate(a), Size::Byte) as u8,
            addr as u16,
        )
    }
}

impl Savestate for Cpu45gs02 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, STATE_VERSION, |w| {
            for v in [self.a, self.x, self.y, self.z, self.b, self.p] {
                w.u8(v);
            }
            w.u16(self.sp);
            w.u16(self.pc);
            let m = &self.map;
            w.u32(m.lo_offset);
            w.u32(m.hi_offset);
            for v in [m.lo_mask, m.hi_mask, m.lo_mb, m.hi_mb] {
                w.u8(v);
            }
            w.bool(self.map_lock);
            w.u8(match self.speed {
                Speed::Mhz1 => 0,
                Speed::Mhz2 => 1,
                Speed::Mhz3_5 => 2,
                Speed::Mhz40 => 3,
            });
            w.u32(self.wait);
            w.bool(self.irq);
            w.bool(self.nmi);
            w.bool(self.nmi_pending);
            w.u32(self.last_cycles);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut s) = r.chunk(STATE_TAG, STATE_VERSION)?;
        let mut c = Self::new();
        c.a = s.u8()?;
        c.x = s.u8()?;
        c.y = s.u8()?;
        c.z = s.u8()?;
        c.b = s.u8()?;
        c.p = s.u8()? & !flag::B;
        c.sp = s.u16()?;
        c.pc = s.u16()?;
        c.map.lo_offset = s.u32()? & 0xf_ff00;
        c.map.hi_offset = s.u32()? & 0xf_ff00;
        c.map.lo_mask = s.u8()? & 15;
        c.map.hi_mask = s.u8()? & 15;
        c.map.lo_mb = s.u8()?;
        c.map.hi_mb = s.u8()?;
        c.map_lock = s.bool()?;
        c.speed = match s.u8()? {
            0 => Speed::Mhz1,
            1 => Speed::Mhz2,
            2 => Speed::Mhz3_5,
            3 => Speed::Mhz40,
            _ => return Err(StateError::Invalid("45GS02 speed")),
        };
        // An instruction lasts at most a few dozen cycles.
        c.wait = s.u32()?.min(64);
        c.irq = s.bool()?;
        c.nmi = s.bool()?;
        c.nmi_pending = s.bool()?;
        c.last_cycles = s.u32()?.min(64);
        *self = c;
        Ok(())
    }
}
