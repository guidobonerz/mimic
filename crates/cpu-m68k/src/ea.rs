//! Effective addresses: calculation (with its bus cycles: extension words
//! and internal clocks, as in the "effective address calculation" table of
//! the 68000 User's Manual) and operand access.

use emu_core::Size;

use crate::{Res, TimedBus68, M68000};

/// Operand size of an instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sz {
    B,
    W,
    L,
}

impl Sz {
    pub(crate) fn bytes(self) -> u32 {
        match self {
            Sz::B => 1,
            Sz::W => 2,
            Sz::L => 4,
        }
    }

    pub(crate) fn mask(self) -> u32 {
        match self {
            Sz::B => 0xff,
            Sz::W => 0xffff,
            Sz::L => 0xffff_ffff,
        }
    }

    pub(crate) fn msb(self) -> u32 {
        match self {
            Sz::B => 0x80,
            Sz::W => 0x8000,
            Sz::L => 0x8000_0000,
        }
    }

    /// Sign-extend a value of this size to 32 bits.
    pub(crate) fn sext(self, v: u32) -> u32 {
        match self {
            Sz::B => v as u8 as i8 as i32 as u32,
            Sz::W => v as u16 as i16 as i32 as u32,
            Sz::L => v,
        }
    }

    /// The common size field (bits 7–6: 0 byte, 1 word, 2 long).
    pub(crate) fn from_bits(bits: u16) -> Option<Sz> {
        match bits & 3 {
            0 => Some(Sz::B),
            1 => Some(Sz::W),
            2 => Some(Sz::L),
            _ => None,
        }
    }
}

/// A resolved effective address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ea {
    D(usize),
    A(usize),
    Mem(u32),
    Imm(u32),
}

/// Mode/register of an effective address field.
pub(crate) fn mode(op: u16) -> (u16, usize) {
    ((op >> 3) & 7, usize::from(op & 7))
}

impl M68000 {
    /// (An)+ and -(An) step: the operand size, but A7 stays word aligned.
    fn step(r: usize, sz: Sz) -> u32 {
        if r == 7 && sz == Sz::B {
            2
        } else {
            sz.bytes()
        }
    }

    /// Brief extension word (d8,An,Xn): base + index + displacement.
    pub(crate) fn index(&self, base: u32, ext: u16) -> u32 {
        let r = usize::from((ext >> 12) & 7);
        let xn = if ext & 0x8000 != 0 {
            self.a[r]
        } else {
            self.d[r]
        };
        let xn = if ext & 0x0800 != 0 {
            xn
        } else {
            xn as u16 as i16 as i32 as u32
        };
        base.wrapping_add(xn)
            .wrapping_add(ext as u8 as i8 as i32 as u32)
    }

    /// Calculate the effective address of mode `m`, register `r`: the
    /// extension words and internal clocks, (An)+/-(An) register updates;
    /// immediates are read here. Returns `None` for an invalid mode.
    pub(crate) fn ea(
        &mut self,
        bus: &mut dyn TimedBus68,
        m: u16,
        r: usize,
        sz: Sz,
    ) -> Res<Option<Ea>> {
        Ok(Some(match m {
            0 => Ea::D(r),
            1 => Ea::A(r),
            2 => Ea::Mem(self.a[r]),
            3 => {
                let a = self.a[r];
                self.a[r] = a.wrapping_add(Self::step(r, sz));
                Ea::Mem(a)
            }
            4 => {
                self.idle(bus, 2);
                self.a[r] = self.a[r].wrapping_sub(Self::step(r, sz));
                Ea::Mem(self.a[r])
            }
            5 => {
                let d = self.ext(bus)? as i16 as i32 as u32;
                Ea::Mem(self.a[r].wrapping_add(d))
            }
            6 => {
                self.idle(bus, 2);
                let ext = self.ext(bus)?;
                Ea::Mem(self.index(self.a[r], ext))
            }
            7 => match r {
                0 => Ea::Mem(self.ext(bus)? as i16 as i32 as u32),
                1 => Ea::Mem(self.ext_long(bus)?),
                2 => {
                    let base = self.ext_pc();
                    let d = self.ext(bus)? as i16 as i32 as u32;
                    Ea::Mem(base.wrapping_add(d))
                }
                3 => {
                    self.idle(bus, 2);
                    let base = self.ext_pc();
                    let ext = self.ext(bus)?;
                    Ea::Mem(self.index(base, ext))
                }
                4 => Ea::Imm(match sz {
                    Sz::B => u32::from(self.ext(bus)?) & 0xff,
                    Sz::W => u32::from(self.ext(bus)?),
                    Sz::L => self.ext_long(bus)?,
                }),
                _ => return Ok(None),
            },
            _ => return Ok(None),
        }))
    }

    /// Read the operand at `ea` (longs: high word, then low word).
    pub(crate) fn read_ea(&mut self, bus: &mut dyn TimedBus68, ea: Ea, sz: Sz) -> Res<u32> {
        Ok(match ea {
            Ea::D(r) => self.d[r] & sz.mask(),
            Ea::A(r) => self.a[r] & sz.mask(),
            Ea::Imm(v) => v & sz.mask(),
            Ea::Mem(addr) => self.read_mem(bus, addr, sz)?,
        })
    }

    pub(crate) fn read_mem(&mut self, bus: &mut dyn TimedBus68, addr: u32, sz: Sz) -> Res<u32> {
        match sz {
            Sz::B => self.read(bus, addr, Size::Byte),
            Sz::W => self.read(bus, addr, Size::Word),
            Sz::L => {
                let hi = self.read(bus, addr, Size::Word)?;
                let lo = self.read(bus, addr.wrapping_add(2), Size::Word)?;
                Ok(hi << 16 | lo)
            }
        }
    }

    /// Write a memory operand; `low_first` writes the low word of a long
    /// first (-(An) destinations and read-modify-write instructions).
    pub(crate) fn write_mem(
        &mut self,
        bus: &mut dyn TimedBus68,
        addr: u32,
        sz: Sz,
        v: u32,
        low_first: bool,
    ) -> Res {
        match sz {
            Sz::B => self.write(bus, addr, Size::Byte, v & 0xff),
            Sz::W => self.write(bus, addr, Size::Word, v & 0xffff),
            Sz::L if low_first => {
                self.write(bus, addr.wrapping_add(2), Size::Word, v & 0xffff)?;
                self.write(bus, addr, Size::Word, v >> 16)
            }
            Sz::L => {
                self.write(bus, addr, Size::Word, v >> 16)?;
                self.write(bus, addr.wrapping_add(2), Size::Word, v & 0xffff)
            }
        }
    }

    /// Store into a data register, keeping the bits above the size.
    pub(crate) fn set_d(&mut self, r: usize, sz: Sz, v: u32) {
        let m = sz.mask();
        self.d[r] = (self.d[r] & !m) | (v & m);
    }

    /// N and Z from a result; V and C cleared (logical operations, MOVE).
    pub(crate) fn logic_flags(&mut self, sz: Sz, v: u32) {
        use crate::sr;
        let v = v & sz.mask();
        self.set_flag(sr::N, v & sz.msb() != 0);
        self.set_flag(sr::Z, v == 0);
        self.set_flag(sr::V, false);
        self.set_flag(sr::C, false);
    }
}
