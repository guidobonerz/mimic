//! The V9938's commands (Technical Data Book, chapter 4 "Commands").
//!
//! Registers: R32/R33 SX, R34/R35 SY, R36/R37 DX, R38/R39 DY, R40/R41 NX,
//! R42/R43 NY, R44 CLR (the colour, and the data port of the transfers),
//! R45 ARG (`0 MXC MXD MXS DIY DIX EQ MAJ`: DIX and DIY set go left and
//! up), R46 the command (bits 7–4) and the logical operation (bits 3–0).
//! Writing R46 starts a command; S2's CE is set while it runs.
//!
//! | Code | Command | What it does                                          |
//! | ---- | ------- | ----------------------------------------------------- |
//! | 0    | STOP    | ends the running command                              |
//! | 4    | POINT   | S7 = the colour at (SX, SY)                           |
//! | 5    | PSET    | CLR to (DX, DY) through the logical operation         |
//! | 6    | SRCH    | from (SX, SY) along DIX for CLR (EQ: for another colour); BD and S8/S9 |
//! | 7    | LINE    | from (DX, DY): NX the long side, NY the short one, MAJ set: the long side is Y |
//! | 8    | LMMV    | fills NX × NY at (DX, DY) with CLR, logical           |
//! | 9    | LMMM    | copies NX × NY from (SX, SY) to (DX, DY), logical     |
//! | 10   | LMCM    | NX × NY from (SX, SY) to the CPU, a pixel per S7 read |
//! | 11   | LMMC    | the CPU's pixels (R44) to NX × NY at (DX, DY), logical |
//! | 12   | HMMV    | fills with the byte CLR                               |
//! | 13   | HMMM    | copies bytes                                          |
//! | 14   | YMMM    | copies bytes from (DX, SY) to (DX, DY), to the screen's edge |
//! | 15   | HMMC    | the CPU's bytes (R44) to NX × NY at (DX, DY)          |
//!
//! Logical operations: 0 IMP (the source), 1 AND, 2 OR, 3 EOR, 4 NOT; with
//! bit 3 set a source of 0 leaves the pixel as it is.
//!
//! The commands work in G4–G7 only, on the whole VRAM as a plane of 256 ×
//! 1024 (G4), 512 × 1024 (G5), 512 × 512 (G6) or 256 × 512 (G7) pixels.
//! NX = 0 counts as 1024 and NY = 0 as 1024; a row ends at the screen's
//! edge, and going up the block ends at line 0. The byte commands move
//! whole bytes (two or four pixels in G4–G6). While a block runs, SY, DY
//! and NY follow the rows done.
//!
//! Transfers: LMMC and HMMC take their first pixel or byte from R44 as it
//! is when the command starts (the program puts it there first), then set
//! TR and take one more per write to R44.
//! LMCM puts the first pixel into S7 and sets TR; each read of S7 fetches
//! the next.
//!
//! Speed: a command gets the chip's master clocks as they pass and spends
//! a fixed number per pixel or byte ([`cost`]). The numbers are rounded
//! estimates of this model, the same with the display and the sprites on
//! or off; the real chip is faster while the screen is blanked.

use emu_core::{StateError, StateReader, StateWriter};

use super::{mode, V9938};

const ARG_MAJ: u8 = 0x01;
const ARG_EQ: u8 = 0x02;
const ARG_DIX: u8 = 0x04;
const ARG_DIY: u8 = 0x08;

/// Master clocks one pixel or byte of command `op` takes.
fn cost(op: u8) -> i32 {
    match op {
        6 => 80,
        7 => 120,
        8 => 96,
        9 => 128,
        12 => 48,
        13 => 88,
        14 => 64,
        _ => 0,
    }
}

/// The plane a command works on.
#[derive(Clone, Copy)]
struct Geom {
    width: u16,
    y_mask: u16,
    /// log2 of the pixels per byte and of the bytes per line.
    shift: u8,
    row: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct State {
    /// The running command (1–15), 0 none: S2's CE.
    pub op: u8,
    /// S2's TR and BD, S7, S8/S9.
    pub tr: bool,
    pub bd: bool,
    pub color: u8,
    pub found_x: u16,
    /// The current source and destination column, and those a row starts at.
    sx: u16,
    dx: u16,
    sx0: u16,
    dx0: u16,
    /// LINE's current line.
    y: u16,
    /// Pixels or bytes left in the row (LINE: points left), a row's
    /// count, rows left.
    n: u16,
    per_row: u16,
    rows: u16,
    /// LINE's error term.
    acc: u32,
    /// Master clocks not spent yet.
    budget: i32,
}

impl State {
    pub(super) fn save(&self, w: &mut StateWriter) {
        w.u8(self.op);
        w.bool(self.tr);
        w.bool(self.bd);
        w.u8(self.color);
        for v in [
            self.found_x,
            self.sx,
            self.dx,
            self.sx0,
            self.dx0,
            self.y,
            self.n,
            self.per_row,
            self.rows,
        ] {
            w.u16(v);
        }
        w.u32(self.acc);
        w.u32(self.budget as u32);
    }

    pub(super) fn load(c: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut s = State {
            op: c.u8()? & 15,
            tr: c.bool()?,
            bd: c.bool()?,
            color: c.u8()?,
            ..State::default()
        };
        for v in [
            &mut s.found_x,
            &mut s.sx,
            &mut s.dx,
            &mut s.sx0,
            &mut s.dx0,
            &mut s.y,
            &mut s.n,
            &mut s.per_row,
            &mut s.rows,
        ] {
            *v = c.u16()? & 0x7ff;
        }
        s.acc = c.u32()? & 0xffff;
        s.budget = (c.u32()? as i32).clamp(0, 4096);
        Ok(s)
    }
}

impl V9938 {
    fn geom(&self) -> Option<Geom> {
        let (width, y_mask, shift, row) = match self.mode() {
            mode::G4 => (256, 1023, 1, 7),
            mode::G5 => (512, 1023, 2, 7),
            mode::G6 => (512, 511, 1, 8),
            mode::G7 => (256, 511, 0, 8),
            _ => return None,
        };
        Some(Geom {
            width,
            y_mask,
            shift,
            row,
        })
    }

    fn reg16(&self, n: usize, mask: u16) -> u16 {
        (u16::from(self.regs[n]) | u16::from(self.regs[n + 1]) << 8) & mask
    }

    fn set_reg16(&mut self, n: usize, v: u16) {
        self.regs[n] = v as u8;
        self.regs[n + 1] = (v >> 8) as u8;
    }

    fn byte_addr(g: Geom, x: u16, y: u16) -> usize {
        usize::from(y & g.y_mask) << g.row | usize::from(x & (g.width - 1)) >> g.shift
    }

    /// The pixel's place in its byte: shift and mask.
    fn pixel_bits(g: Geom, x: u16) -> (u8, u8) {
        let per_byte = 1u16 << g.shift;
        let bits = 8 >> g.shift;
        let sh = (per_byte - 1 - (x & (per_byte - 1))) as u8 * bits;
        (sh, (0xffu16 >> (8 - bits)) as u8)
    }

    fn point(&self, g: Geom, x: u16, y: u16) -> u8 {
        let (sh, mask) = Self::pixel_bits(g, x);
        self.vram[Self::byte_addr(g, x, y)] >> sh & mask
    }

    /// Colour `c` to a pixel through the logical operation.
    fn pset(&mut self, g: Geom, x: u16, y: u16, c: u8) {
        let lop = self.regs[46] & 15;
        let (sh, mask) = Self::pixel_bits(g, x);
        let a = Self::byte_addr(g, x, y);
        let src = c & mask;
        if lop & 8 != 0 && src == 0 {
            return;
        }
        let dst = self.vram[a] >> sh & mask;
        let v = match lop & 7 {
            0 => src,
            1 => src & dst,
            2 => src | dst,
            3 => src ^ dst,
            4 => !src & mask,
            _ => dst,
        };
        self.vram[a] = (self.vram[a] & !(mask << sh)) | v << sh;
    }

    /// A write to R46.
    pub(super) fn cmd_start(&mut self, v: u8) {
        let op = v >> 4;
        self.cmd.op = 0;
        self.cmd.tr = false;
        self.cmd.budget = 0;
        let Some(g) = self.geom() else {
            return;
        };
        let arg = self.regs[45];
        let left = arg & ARG_DIX != 0;
        let sx = self.reg16(32, g.width - 1);
        let sy = self.reg16(34, 0x3ff);
        let dx = self.reg16(36, g.width - 1);
        let dy = self.reg16(38, 0x3ff);
        let nx = match self.reg16(40, 0x3ff) {
            0 => 1024,
            n => n,
        };
        let ny = match self.reg16(42, 0x3ff) {
            0 => 1024,
            n => n,
        };
        match op {
            4 => self.cmd.color = self.point(g, sx, sy),
            5 => self.pset(g, dx, dy, self.regs[44]),
            6 => {
                self.cmd.sx = sx;
                self.cmd.op = op;
            }
            7 => {
                self.cmd.dx = dx;
                self.cmd.y = dy;
                self.cmd.per_row = self.reg16(40, 0x3ff);
                self.cmd.n = self.cmd.per_row + 1;
                self.cmd.acc = u32::from(self.cmd.per_row / 2);
                self.cmd.op = op;
            }
            8..=15 => {
                // Which of the two corners there are, and the unit: a
                // pixel, or a byte's worth of pixels.
                let (has_src, has_dst) = match op {
                    10 => (true, false),
                    9 | 13 => (true, true),
                    _ => (false, true),
                };
                let bytes = op >= 12;
                let unit = if bytes { 1u16 << g.shift } else { 1 };
                let (sx, dx) = if op == 14 { (dx, dx) } else { (sx, dx) };
                let (sx, dx) = (sx & !(unit - 1), dx & !(unit - 1));
                // The columns to the edge, from the corner nearest to it.
                let near = match (has_src || op == 14, has_dst) {
                    (true, true) if left => sx.min(dx),
                    (true, true) => sx.max(dx),
                    (true, false) => sx,
                    _ => dx,
                };
                let room = if left {
                    near / unit + 1
                } else {
                    (g.width - near) / unit
                };
                let count = if op == 14 {
                    room
                } else {
                    match nx / unit {
                        0 => g.width / unit,
                        n => n,
                    }
                    .min(room)
                };
                let top = match (has_src || op == 14, has_dst) {
                    (true, true) => sy.min(dy),
                    (true, false) => sy,
                    _ => dy,
                };
                let rows = if arg & ARG_DIY != 0 {
                    ny.min(top + 1)
                } else {
                    ny
                };
                self.cmd.sx0 = sx;
                self.cmd.dx0 = dx;
                self.cmd.sx = sx;
                self.cmd.dx = dx;
                self.cmd.per_row = count;
                self.cmd.n = count;
                self.cmd.rows = rows;
                self.cmd.op = op;
                match op {
                    10 => self.block_unit(g, 0),
                    11 | 15 => {
                        self.cmd.tr = true;
                        self.block_unit(g, self.regs[44]);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// `clocks` master clocks for the running command.
    pub(super) fn cmd_run(&mut self, clocks: i32) {
        let cost = cost(self.cmd.op);
        if cost == 0 {
            // A transfer: the CPU sets the pace.
            return;
        }
        let Some(g) = self.geom() else {
            self.cmd.op = 0;
            return;
        };
        self.cmd.budget += clocks;
        while self.cmd.budget >= cost && self.cmd.op != 0 {
            self.cmd.budget -= cost;
            match self.cmd.op {
                6 => self.search_unit(g),
                7 => self.line_unit(g),
                _ => self.block_unit(g, 0),
            }
        }
    }

    /// A write to R44: the next pixel or byte of LMMC and HMMC.
    pub(super) fn cmd_data(&mut self, v: u8) {
        if matches!(self.cmd.op, 11 | 15) {
            match self.geom() {
                Some(g) => self.block_unit(g, v),
                None => self.cmd.op = 0,
            }
        }
    }

    /// S7 was read: LMCM's next pixel.
    pub(super) fn cmd_read(&mut self) {
        self.cmd.tr = false;
        if self.cmd.op == 10 {
            match self.geom() {
                Some(g) => self.block_unit(g, 0),
                None => self.cmd.op = 0,
            }
        }
    }

    /// One pixel or byte of a block command; `data` is the CPU's.
    fn block_unit(&mut self, g: Geom, data: u8) {
        let op = self.cmd.op;
        let arg = self.regs[45];
        let (sx, dx) = (self.cmd.sx, self.cmd.dx);
        let sy = self.reg16(34, 0x3ff);
        let dy = self.reg16(38, 0x3ff);
        match op {
            8 => self.pset(g, dx, dy, self.regs[44]),
            9 => {
                let c = self.point(g, sx, sy);
                self.pset(g, dx, dy, c);
            }
            10 => {
                self.cmd.color = self.point(g, sx, sy);
                self.cmd.tr = true;
            }
            11 => self.pset(g, dx, dy, data),
            12 => self.vram[Self::byte_addr(g, dx, dy)] = self.regs[44],
            13 | 14 => {
                self.vram[Self::byte_addr(g, dx, dy)] = self.vram[Self::byte_addr(g, sx, sy)]
            }
            _ => self.vram[Self::byte_addr(g, dx, dy)] = data,
        }
        let unit = if op >= 12 { 1u16 << g.shift } else { 1 };
        let step = |x: u16| {
            if arg & ARG_DIX != 0 {
                x.wrapping_sub(unit)
            } else {
                x.wrapping_add(unit)
            }
        };
        self.cmd.sx = step(sx);
        self.cmd.dx = step(dx);
        self.cmd.n = self.cmd.n.saturating_sub(1);
        if self.cmd.n > 0 {
            return;
        }
        // The row is done.
        let next = |y: u16| {
            if arg & ARG_DIY != 0 {
                y.wrapping_sub(1) & 0x3ff
            } else {
                (y + 1) & 0x3ff
            }
        };
        if matches!(op, 9 | 10 | 13 | 14) {
            self.set_reg16(34, next(sy));
        }
        if op != 10 {
            self.set_reg16(38, next(dy));
        }
        let ny = self.reg16(42, 0x3ff).wrapping_sub(1) & 0x3ff;
        self.set_reg16(42, ny);
        self.cmd.rows = self.cmd.rows.saturating_sub(1);
        if self.cmd.rows == 0 {
            self.cmd.op = 0;
            if op != 10 {
                self.cmd.tr = false;
            }
            return;
        }
        self.cmd.sx = self.cmd.sx0;
        self.cmd.dx = self.cmd.dx0;
        self.cmd.n = self.cmd.per_row;
    }

    /// One point of LINE.
    fn line_unit(&mut self, g: Geom) {
        let arg = self.regs[45];
        let (x, y) = (self.cmd.dx, self.cmd.y);
        self.pset(g, x, y, self.regs[44]);
        self.cmd.n = self.cmd.n.saturating_sub(1);
        // The next point: a step along the long side, and one along the
        // short side whenever the error term runs out.
        let short = u32::from(self.reg16(42, 0x3ff));
        let side = self.cmd.acc < short;
        if side {
            self.cmd.acc += u32::from(self.cmd.per_row);
        }
        self.cmd.acc = self.cmd.acc.saturating_sub(short);
        let y_long = arg & ARG_MAJ != 0;
        let (step_x, step_y) = if y_long { (side, true) } else { (true, side) };
        let mut done = self.cmd.n == 0;
        if step_x {
            let nx = if arg & ARG_DIX != 0 {
                x.wrapping_sub(1)
            } else {
                x + 1
            };
            done |= nx >= g.width;
            self.cmd.dx = nx;
        }
        if step_y {
            self.cmd.y = if arg & ARG_DIY != 0 {
                y.wrapping_sub(1) & 0x3ff
            } else {
                (y + 1) & 0x3ff
            };
        }
        if done {
            self.set_reg16(38, self.cmd.y);
            self.cmd.op = 0;
        }
    }

    /// One pixel of SRCH.
    fn search_unit(&mut self, g: Geom) {
        let arg = self.regs[45];
        let x = self.cmd.sx;
        let sy = self.reg16(34, 0x3ff);
        let (_, mask) = Self::pixel_bits(g, x);
        let equal = self.point(g, x, sy) == self.regs[44] & mask;
        let found = equal != (arg & ARG_EQ != 0);
        let next = if arg & ARG_DIX != 0 {
            x.wrapping_sub(1)
        } else {
            x + 1
        };
        if found || next >= g.width {
            self.cmd.bd = found;
            self.cmd.found_x = x;
            self.cmd.op = 0;
        } else {
            self.cmd.sx = next;
        }
    }
}
