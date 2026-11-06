//! The Lithograph in Babbage (SPEC.md 6.3): registers at FFFF:0100.
//!
//! A job works a row at a time (a pixel at a time for a line): every
//! 32-bit chip-RAM access costs one free chip-bus slot, and a row is done
//! when its slots are spent ([`Lithograph::slot`]). Operations: copy with a
//! 4-bit raster operation of sources A and B into D, fill, Bresenham line,
//! shade (D = TABLE[B] where A is not the colour key). Pixels of 4, 8 or
//! 16 bits; flip X/Y mirror the walk over A; the colour key on A skips
//! pixels; 4-bit rows start at the nibble each channel's phase bit says.
//! A channel that is not used reads 0.
//!
//! **[S]** Slot costs: a row costs, for each channel it uses, the 32-bit
//! words its bytes touch; a line pixel costs two slots (read and write).

use emu_core::{StateError, StateReader, StateWriter};

pub const LTHCON0: u16 = 0x0100;
pub const LTHCON1: u16 = 0x0102;
pub const LTHASEG: u16 = 0x0104;
pub const LTHAOFS: u16 = 0x0106;
pub const LTHBSEG: u16 = 0x0108;
pub const LTHBOFS: u16 = 0x010a;
pub const LTHDSEG: u16 = 0x010c;
pub const LTHDOFS: u16 = 0x010e;
pub const LTHAPITCH: u16 = 0x0110;
pub const LTHBPITCH: u16 = 0x0112;
pub const LTHDPITCH: u16 = 0x0114;
pub const LTHW: u16 = 0x0116;
pub const LTHH: u16 = 0x0118;
pub const LTHKEY: u16 = 0x011a;
pub const LTHCOLOR: u16 = 0x011c;
pub const LTHTSEG: u16 = 0x011e;
pub const LTHTOFS: u16 = 0x0120;
pub const LTHLINE: u16 = 0x0122;
pub const LTHSTART: u16 = 0x0124;
pub const LTHSTAT: u16 = 0x0126;

/// LTHCON0: bits 3–0 ROP, 4/5/6 use A/B/D, 9–8 operation.
pub const USE_A: u16 = 1 << 4;
pub const USE_B: u16 = 1 << 5;
pub const USE_D: u16 = 1 << 6;
pub const OP_COPY: u16 = 0 << 8;
pub const OP_FILL: u16 = 1 << 8;
pub const OP_LINE: u16 = 2 << 8;
pub const OP_SHADE: u16 = 3 << 8;
/// LTHCON1 bits.
pub const SIZE_4: u16 = 0;
pub const SIZE_8: u16 = 1;
pub const SIZE_16: u16 = 2;
pub const FLIP_X: u16 = 1 << 2;
pub const FLIP_Y: u16 = 1 << 3;
pub const KEY_A: u16 = 1 << 4;

const CHIP_END: u32 = 0x0040_0000;

/// Chip RAM the Lithograph reads and writes.
pub trait ChipRamMut {
    fn get(&self, addr: u32) -> u8;
    fn set(&mut self, addr: u32, v: u8);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lithograph {
    con0: u16,
    con1: u16,
    a: (u16, u16),
    b: (u16, u16),
    d: (u16, u16),
    pitch: [u16; 3],
    w: u16,
    h: u16,
    key: u16,
    color: u16,
    t: (u16, u16),
    line: u16,
    busy: bool,
    done: bool,
    /// The running job: rows (line pixels) done, slots saved up for the
    /// next row, and the line's Bresenham state (x, y, error).
    row: u16,
    credit: u32,
    lx: i32,
    ly: i32,
    lerr: i32,
}

fn lin(p: (u16, u16)) -> u32 {
    u32::from(p.0) << 16 | u32::from(p.1)
}

impl Lithograph {
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            LTHCON0 => self.con0,
            LTHCON1 => self.con1,
            LTHASEG => self.a.0,
            LTHAOFS => self.a.1,
            LTHBSEG => self.b.0,
            LTHBOFS => self.b.1,
            LTHDSEG => self.d.0,
            LTHDOFS => self.d.1,
            LTHAPITCH => self.pitch[0],
            LTHBPITCH => self.pitch[1],
            LTHDPITCH => self.pitch[2],
            LTHW => self.w,
            LTHH => self.h,
            LTHKEY => self.key,
            LTHCOLOR => self.color,
            LTHTSEG => self.t.0,
            LTHTOFS => self.t.1,
            LTHLINE => self.line,
            LTHSTAT => u16::from(self.busy) | u16::from(self.done) << 1,
            _ => 0,
        }
    }

    /// Register read: LTHSTAT's done bit clears.
    pub fn read(&mut self, off: u16) -> u16 {
        let v = self.peek(off);
        if off & !1 == LTHSTAT {
            self.done = false;
        }
        v
    }

    /// Register write; `true` for LTHSTART (the caller starts the job).
    pub fn write(&mut self, off: u16, v: u16) -> bool {
        match off & !1 {
            LTHCON0 => self.con0 = v & 0x037f,
            LTHCON1 => self.con1 = v & 0x00ff,
            LTHASEG => self.a.0 = v,
            LTHAOFS => self.a.1 = v,
            LTHBSEG => self.b.0 = v,
            LTHBOFS => self.b.1 = v,
            LTHDSEG => self.d.0 = v,
            LTHDOFS => self.d.1 = v,
            LTHAPITCH => self.pitch[0] = v,
            LTHBPITCH => self.pitch[1] = v,
            LTHDPITCH => self.pitch[2] = v,
            LTHW => self.w = v,
            LTHH => self.h = v,
            LTHKEY => self.key = v,
            LTHCOLOR => self.color = v,
            LTHTSEG => self.t.0 = v,
            LTHTOFS => self.t.1 = v,
            LTHLINE => self.line = v & 7,
            LTHSTART => return true,
            _ => {}
        }
        false
    }

    fn op(&self) -> u16 {
        self.con0 & 0x0300
    }

    fn bits(&self) -> u32 {
        match self.con1 & 3 {
            0 => 4,
            1 => 8,
            _ => 16,
        }
    }

    /// Start the job; `false` (and nothing runs) when a pointer it uses is
    /// outside chip RAM.
    pub fn start(&mut self) -> bool {
        let uses = [
            (self.con0 & USE_A != 0, self.a),
            (self.con0 & USE_B != 0 || self.op() == OP_SHADE, self.b),
            (true, self.d),
            (self.op() == OP_SHADE, self.t),
        ];
        if uses.iter().any(|(used, p)| *used && lin(*p) >= CHIP_END) {
            self.busy = false;
            return false;
        }
        self.busy = self.w > 0 && self.h > 0 || self.op() == OP_LINE;
        self.done = !self.busy;
        self.row = 0;
        self.credit = 0;
        self.lx = 0;
        self.ly = 0;
        self.lerr = 0;
        true
    }

    /// The 32-bit words a row of `channel` touches from byte offset `at`.
    fn words(&self, base: u32) -> u32 {
        let bytes = (u32::from(self.w) * self.bits()).div_ceil(8);
        ((base & 3) + bytes).div_ceil(4)
    }

    fn row_cost(&self) -> u32 {
        match self.op() {
            OP_LINE => 2,
            OP_FILL => self.words(lin(self.d)),
            OP_SHADE => {
                self.words(lin(self.b))
                    + self.words(lin(self.d))
                    + if self.con0 & USE_A != 0 {
                        self.words(lin(self.a))
                    } else {
                        0
                    }
            }
            _ => [(USE_A, self.a), (USE_B, self.b), (USE_D, self.d)]
                .iter()
                .filter(|(u, _)| self.con0 & u != 0)
                .map(|(_, p)| self.words(lin(*p)))
                .sum::<u32>()
                .max(1),
        }
    }

    /// Calls of [`Lithograph::slot`] up to and including the next one that
    /// does work (a row or a line pixel); the ones before only count.
    pub fn slots_to_work(&self) -> u32 {
        self.row_cost().saturating_sub(self.credit).max(1)
    }

    /// `n` slots (fewer than [`Lithograph::slots_to_work`]): the same as
    /// `n` calls of [`Lithograph::slot`], which only count then.
    pub fn skip_slots(&mut self, n: u32) {
        debug_assert!(!self.busy || n < self.slots_to_work());
        if self.busy {
            self.credit += n;
        }
    }

    /// One free slot for a busy Lithograph; `true` when the job finished in it.
    pub fn slot(&mut self, mem: &mut dyn ChipRamMut) -> bool {
        if !self.busy {
            return false;
        }
        self.credit += 1;
        let cost = self.row_cost();
        if self.credit < cost {
            return false;
        }
        self.credit -= cost;
        let finished = if self.op() == OP_LINE {
            self.line_pixel(mem)
        } else {
            self.do_row(mem);
            self.row += 1;
            self.row >= self.h
        };
        if finished {
            self.busy = false;
            self.done = true;
        }
        finished
    }

    /// Address and nibble of pixel `x` in the row at `row_base`, for a
    /// channel with 4-bit start phase `phase`.
    fn at(&self, row_base: u32, x: i32, phase: bool) -> (u32, Option<bool>) {
        match self.bits() {
            4 => {
                let n = x + i32::from(phase);
                let byte = row_base.wrapping_add_signed(n.div_euclid(2));
                (byte, Some(n.rem_euclid(2) == 0))
            }
            8 => (row_base.wrapping_add_signed(x), None),
            _ => (row_base.wrapping_add_signed(2 * x), None),
        }
    }

    fn read_px(&self, mem: &dyn ChipRamMut, row_base: u32, x: i32, phase: bool) -> u16 {
        let (a, nib) = self.at(row_base, x, phase);
        if a >= CHIP_END {
            return 0;
        }
        match (self.bits(), nib) {
            (4, Some(high)) => u16::from(if high {
                mem.get(a) >> 4
            } else {
                mem.get(a) & 15
            }),
            (8, _) => u16::from(mem.get(a)),
            _ => u16::from_le_bytes([mem.get(a), mem.get(a.wrapping_add(1))]),
        }
    }

    fn write_px(&self, mem: &mut dyn ChipRamMut, row_base: u32, x: i32, phase: bool, v: u16) {
        let (a, nib) = self.at(row_base, x, phase);
        if a >= CHIP_END {
            return;
        }
        match (self.bits(), nib) {
            (4, Some(high)) => {
                let old = mem.get(a);
                let v = (v & 15) as u8;
                mem.set(
                    a,
                    if high {
                        (old & 0x0f) | v << 4
                    } else {
                        (old & 0xf0) | v
                    },
                );
            }
            (8, _) => mem.set(a, v as u8),
            _ => {
                let [lo, hi] = v.to_le_bytes();
                mem.set(a, lo);
                mem.set(a.wrapping_add(1), hi);
            }
        }
    }

    fn row_base(&self, p: (u16, u16), pitch: u16, row: u16) -> u32 {
        lin(p).wrapping_add_signed(i32::from(pitch as i16) * i32::from(row))
    }

    fn do_row(&mut self, mem: &mut dyn ChipRamMut) {
        let mask = ((1u32 << self.bits()) - 1) as u16;
        let r = self.row;
        let src_row = if self.con1 & FLIP_Y != 0 {
            self.h - 1 - r
        } else {
            r
        };
        let a_row = self.row_base(self.a, self.pitch[0], src_row);
        let b_row = self.row_base(self.b, self.pitch[1], r);
        let d_row = self.row_base(self.d, self.pitch[2], r);
        let (pa, pb, pd) = (
            self.con1 & 0x20 != 0,
            self.con1 & 0x40 != 0,
            self.con1 & 0x80 != 0,
        );
        let w = i32::from(self.w);
        for x in 0..w {
            let ax = if self.con1 & FLIP_X != 0 {
                w - 1 - x
            } else {
                x
            };
            let use_a = self.con0 & USE_A != 0;
            let a = if use_a {
                self.read_px(mem, a_row, ax, pa)
            } else {
                0
            };
            if use_a && self.con1 & KEY_A != 0 && a == self.key & mask {
                continue;
            }
            let v = match self.op() {
                OP_FILL => self.color,
                OP_SHADE => {
                    let b = self.read_px(mem, b_row, x, pb);
                    let t = lin(self.t).wrapping_add(u32::from(b & 0xff));
                    (b & !0xff) | u16::from(mem.get(t))
                }
                _ => {
                    let b = if self.con0 & USE_B != 0 {
                        self.read_px(mem, b_row, x, pb)
                    } else {
                        0
                    };
                    let rop = self.con0 & 15;
                    let pick = |bit: u16, m: u16| if rop & bit != 0 { m } else { 0 };
                    pick(8, a & b) | pick(4, a & !b) | pick(2, !a & b) | pick(1, !a & !b)
                }
            };
            if self.op() == OP_FILL || self.con0 & USE_D != 0 {
                self.write_px(mem, d_row, x, pd, v & mask);
            }
        }
    }

    /// One pixel of a line; `true` after the last.
    fn line_pixel(&mut self, mem: &mut dyn ChipRamMut) -> bool {
        let (dx, dy) = (i32::from(self.w), i32::from(self.h));
        let (sx, sy) = (
            if self.line & 1 != 0 { -1 } else { 1 },
            if self.line & 2 != 0 { -1 } else { 1 },
        );
        let y_major = self.line & 4 != 0;
        let (major, minor) = if y_major { (dy, dx) } else { (dx, dy) };
        let row = lin(self.d).wrapping_add_signed(self.ly * i32::from(self.pitch[2] as i16));
        let mask = ((1u32 << self.bits()) - 1) as u16;
        self.write_px(mem, row, self.lx, self.con1 & 0x80 != 0, self.color & mask);
        let n = i32::from(self.row);
        if n >= major {
            return true;
        }
        self.row += 1;
        self.lerr += 2 * minor;
        if y_major {
            self.ly += sy;
            if self.lerr > major {
                self.lx += sx;
                self.lerr -= 2 * major;
            }
        } else {
            self.lx += sx;
            if self.lerr > major {
                self.ly += sy;
                self.lerr -= 2 * major;
            }
        }
        false
    }

    pub fn save(&self, w: &mut StateWriter) {
        for v in [
            self.con0,
            self.con1,
            self.a.0,
            self.a.1,
            self.b.0,
            self.b.1,
            self.d.0,
            self.d.1,
            self.pitch[0],
            self.pitch[1],
            self.pitch[2],
            self.w,
            self.h,
            self.key,
            self.color,
            self.t.0,
            self.t.1,
            self.line,
            self.row,
        ] {
            w.u16(v);
        }
        w.bool(self.busy);
        w.bool(self.done);
        w.u32(self.credit);
        w.u32(self.lx as u32);
        w.u32(self.ly as u32);
        w.u32(self.lerr as u32);
    }

    pub fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut b = Lithograph::default();
        let mut f = || c.u16();
        b.con0 = f()? & 0x037f;
        b.con1 = f()? & 0xff;
        b.a = (f()?, f()?);
        b.b = (f()?, f()?);
        b.d = (f()?, f()?);
        b.pitch = [f()?, f()?, f()?];
        b.w = f()?;
        b.h = f()?;
        b.key = f()?;
        b.color = f()?;
        b.t = (f()?, f()?);
        b.line = f()? & 7;
        b.row = f()?;
        b.busy = c.bool()?;
        b.done = c.bool()?;
        b.credit = c.u32()?.min(1 << 16);
        b.lx = c.u32()? as i32;
        b.ly = c.u32()? as i32;
        b.lerr = c.u32()? as i32;
        *self = b;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRamMut for Ram {
        fn get(&self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn set(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
    }

    fn ram() -> Ram {
        Ram(vec![0; 0x4_0000])
    }

    /// Set registers, start, run to the end; the slots it took.
    fn lith(ram: &mut Ram, regs: &[(u16, u16)]) -> u32 {
        let mut b = Lithograph::default();
        for &(r, v) in regs {
            b.write(r, v);
        }
        assert!(b.start());
        let mut slots = 0;
        while b.busy() {
            slots += 1;
            b.slot(ram);
            assert!(slots < 1_000_000);
        }
        assert_eq!(b.read(LTHSTAT), 2, "done");
        assert_eq!(b.read(LTHSTAT), 0, "done clears on read");
        slots
    }

    #[test]
    fn copy_with_rop_key_and_flips() {
        let mut m = ram();
        for i in 0..16u8 {
            m.0[0x1000 + usize::from(i)] = i; // A: 4 x 4, 8 bit
            m.0[0x2000 + usize::from(i)] = 0xf0; // B
        }
        let base = [
            (LTHCON1, SIZE_8),
            (LTHASEG, 0),
            (LTHAOFS, 0x1000),
            (LTHBSEG, 0),
            (LTHBOFS, 0x2000),
            (LTHDSEG, 0),
            (LTHDOFS, 0x3000),
            (LTHAPITCH, 4),
            (LTHBPITCH, 4),
            (LTHDPITCH, 8),
            (LTHW, 4),
            (LTHH, 4),
        ];
        // Plain copy A -> D: 1 A word + 1 D word a row.
        let mut regs = base.to_vec();
        regs.push((LTHCON0, 0xc | USE_A | USE_D));
        assert_eq!(lith(&mut m, &regs), 4 * 2);
        assert_eq!(&m.0[0x3000..0x3004], &[0, 1, 2, 3]);
        assert_eq!(&m.0[0x3008..0x300c], &[4, 5, 6, 7], "D pitch 8");
        // A OR B, colour key 0 on A, flipped in X and Y.
        let mut regs = base.to_vec();
        regs.extend([
            (LTHCON0, 0xe | USE_A | USE_B | USE_D),
            (LTHCON1, SIZE_8 | FLIP_X | FLIP_Y | KEY_A),
            (LTHKEY, 0),
            (LTHDOFS, 0x4000),
        ]);
        lith(&mut m, &regs);
        assert_eq!(
            &m.0[0x4000..0x4004],
            &[0xff, 0xfe, 0xfd, 0xfc],
            "row 3 mirrored, OR $F0"
        );
        assert_eq!(
            &m.0[0x4018..0x401c],
            &[0xf3, 0xf2, 0xf1, 0],
            "A = 0 keyed: D kept"
        );
    }

    #[test]
    fn four_and_sixteen_bit_pixels() {
        let mut m = ram();
        m.0[0x1000] = 0x12;
        m.0[0x1001] = 0x34;
        m.0[0x2000] = 0xaa;
        m.0[0x2001] = 0xaa;
        // 3 nibbles of A from phase 1 (2, 3, 4) to D from phase 1.
        lith(
            &mut m,
            &[
                (LTHCON0, 0xc | USE_A | USE_D),
                (LTHCON1, SIZE_4 | 0x20 | 0x80),
                (LTHAOFS, 0x1000),
                (LTHDOFS, 0x2000),
                (LTHW, 3),
                (LTHH, 1),
            ],
        );
        assert_eq!(
            &m.0[0x2000..0x2002],
            &[0xa2, 0x34],
            "nibble masking keeps the rest"
        );
        // 16-bit fill, negative pitch (rows upwards).
        lith(
            &mut m,
            &[
                (LTHCON0, OP_FILL),
                (LTHCON1, SIZE_16),
                (LTHDOFS, 0x3010),
                (LTHDPITCH, (-16i16) as u16),
                (LTHW, 2),
                (LTHH, 2),
                (LTHCOLOR, 0xf81f),
            ],
        );
        assert_eq!(&m.0[0x3010..0x3014], &[0x1f, 0xf8, 0x1f, 0xf8]);
        assert_eq!(
            &m.0[0x3000..0x3004],
            &[0x1f, 0xf8, 0x1f, 0xf8],
            "the row above"
        );
    }

    #[test]
    fn lines_follow_bresenham() {
        let mut m = ram();
        // From (0, 0) 6 right, 2 down: 7 pixels, 2 slots each.
        let slots = lith(
            &mut m,
            &[
                (LTHCON0, OP_LINE),
                (LTHCON1, SIZE_8),
                (LTHDOFS, 0x1000),
                (LTHDPITCH, 16),
                (LTHW, 6),
                (LTHH, 2),
                (LTHCOLOR, 9),
            ],
        );
        assert_eq!(slots, 14);
        let lit: Vec<(u32, u32)> = (0..64u32)
            .filter(|i| m.0[0x1000 + *i as usize] == 9)
            .map(|i| (i % 16, i / 16))
            .collect();
        assert_eq!(
            lit,
            [(0, 0), (1, 0), (2, 1), (3, 1), (4, 1), (5, 2), (6, 2)]
        );
        // Steep, up and left: y major, 3 up, 1 left.
        lith(
            &mut m,
            &[
                (LTHCON0, OP_LINE),
                (LTHCON1, SIZE_8),
                (LTHDOFS, 0x2034),
                (LTHDPITCH, 16),
                (LTHW, 1),
                (LTHH, 3),
                (LTHLINE, 1 | 2 | 4),
                (LTHCOLOR, 7),
            ],
        );
        let lit: Vec<u32> = (0..64u32)
            .filter(|i| m.0[0x2000 + *i as usize] == 7)
            .collect();
        assert_eq!(lit, [0x03, 0x13, 0x24, 0x34]);
    }

    #[test]
    fn shade_darkens_through_the_table_under_the_mask() {
        let mut m = ram();
        for i in 0..256 {
            m.0[0x5000 + i] = (i / 2) as u8; // table: half
        }
        m.0[0x1000..0x1004].copy_from_slice(&[1, 0, 1, 1]); // A: mask
        m.0[0x2000..0x2004].copy_from_slice(&[100, 100, 100, 100]); // B
        lith(
            &mut m,
            &[
                (LTHCON0, OP_SHADE | USE_A | USE_B | USE_D),
                (LTHCON1, SIZE_8 | KEY_A),
                (LTHAOFS, 0x1000),
                (LTHBOFS, 0x2000),
                (LTHDOFS, 0x2000),
                (LTHTOFS, 0x5000),
                (LTHW, 4),
                (LTHH, 1),
            ],
        );
        assert_eq!(&m.0[0x2000..0x2004], &[50, 100, 50, 50]);
    }

    #[test]
    fn a_pointer_outside_chip_ram_refuses_to_start() {
        let mut b = Lithograph::default();
        b.write(LTHCON0, 0xc | USE_A | USE_D);
        b.write(LTHASEG, 0x40);
        b.write(LTHW, 1);
        b.write(LTHH, 1);
        assert!(!b.start());
        assert!(!b.busy());
        b.write(LTHASEG, 0);
        assert!(b.start());
        assert!(b.busy());
    }

    #[test]
    fn savestate_mid_job() {
        let mut m = ram();
        let mut b = Lithograph::default();
        for (r, v) in [
            (LTHCON0, OP_FILL),
            (LTHCON1, SIZE_8),
            (LTHW, 64),
            (LTHH, 8),
            (LTHDPITCH, 64),
        ] {
            b.write(r, v);
        }
        b.start();
        for _ in 0..30 {
            b.slot(&mut m);
        }
        let mut w = StateWriter::new();
        b.save(&mut w);
        let bytes = w.into_bytes();
        let mut c = Lithograph::default();
        c.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, b);
    }
}
