//! The Blitter: Agnus' block transfer and line engine (HRM chapter 6,
//! "Blitter hardware").
//!
//! **Area mode.** Up to three sources (A, B, C) and a destination D, each
//! with a pointer and a modulo; `BLTSIZE` (height bits 15–6, 0 = 1024;
//! width in words bits 5–0, 0 = 64) starts the blit. A is masked by
//! BLTAFWM in the first and BLTALWM in the last word of a row; A and B are
//! shifted right by ASH/BSH (BLTCON0/BLTCON1 bits 15–12), the bits shifted
//! out entering the next word (left shifts in descending mode, BLTCON1
//! DESC bit 1, where pointers count down). The minterm (BLTCON0 bits 7–0)
//! picks for each of the eight A/B/C combinations whether D is 1. Fill
//! (descending only: IFE bit 3 inclusive, EFE bit 4 exclusive, FCI bit 2
//! the carry at each row start) fills between set bits from right to left.
//! A disabled source uses its data register; D is written when USED is
//! set. BZERO reports whether every D word was zero.
//!
//! **Line mode** (BLTCON1 bit 0): draws `height` pixels with an error term
//! in BLTAPT (BLTBMOD = 4·dminor, BLTAMOD = 4·(dminor − dmajor)); octant
//! bits SUD (4: the occasional step is vertical), SUL (3: occasional step
//! up/left), AUL (2: the regular step up/left); SING (1) draws one pixel
//! per row; BLTCON1 bits 15–12 start the BLTBDAT texture; BLTADAT (normally
//! $8000) shifted by ASH is the pixel; C/D point at the word holding it,
//! BLTCMOD is the row width in bytes. The first pixel goes to BLTDPT, the
//! others to the C address.
//!
//! **Timing.** Each word takes as many DMA slots as it uses channels (at
//! least two); a line pixel four. Words are computed when their last slot
//! is granted, so CPU reads during a blit see the progress word by word.

use emu_core::{StateError, StateReader, StateWriter};

use crate::chipram::ChipRam;

const A: usize = 0;
const B: usize = 1;
const C: usize = 2;
const D: usize = 3;

/// BLTCON1 bits.
mod con1 {
    pub const LINE: u16 = 0x0001;
    pub const DESC: u16 = 0x0002;
    pub const SING: u16 = 0x0002;
    pub const FCI: u16 = 0x0004;
    pub const AUL: u16 = 0x0004;
    pub const IFE: u16 = 0x0008;
    pub const SUL: u16 = 0x0008;
    pub const EFE: u16 = 0x0010;
    pub const SUD: u16 = 0x0010;
    pub const SIGN: u16 = 0x0040;
}

/// Slots from the BLTSIZE write to the first blitter cycle.
const START_DELAY: u8 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Job {
    width: u16,
    height: u16,
    row: u16,
    col: u16,
    /// Slots used for the current word or pixel.
    slots: u8,
    delay: u8,
    a_old: u16,
    b_old: u16,
    carry: bool,
    /// Line mode: a pixel was drawn on the current row (SING).
    row_drawn: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blitter {
    pub con0: u16,
    pub con1: u16,
    pub afwm: u16,
    pub alwm: u16,
    /// BLTAPT, BLTBPT, BLTCPT, BLTDPT.
    pub pt: [u32; 4],
    /// BLTAMOD … BLTDMOD (signed, even).
    pub modulo: [u16; 4],
    pub adat: u16,
    pub bdat: u16,
    pub cdat: u16,
    pub busy: bool,
    pub zero: bool,
    job: Option<Job>,
}

/// D for the eight combinations of A, B, C selected by `lf`.
pub fn minterm(lf: u8, a: u16, b: u16, c: u16) -> u16 {
    let mut d = 0;
    for i in 0..8 {
        if lf & (1 << i) != 0 {
            let term = (if i & 4 != 0 { a } else { !a })
                & (if i & 2 != 0 { b } else { !b })
                & (if i & 1 != 0 { c } else { !c });
            d |= term;
        }
    }
    d
}

/// Fill one word from bit 0 up; returns the word and the carry out.
pub fn fill(d: u16, mut carry: bool, exclusive: bool) -> (u16, bool) {
    let mut out = 0;
    for bit in 0..16 {
        let set = d & (1 << bit) != 0;
        let v = if exclusive {
            carry ^= set;
            carry
        } else {
            let v = carry | set;
            carry ^= set;
            v
        };
        if v {
            out |= 1 << bit;
        }
    }
    (out, carry)
}

impl Blitter {
    pub fn new() -> Self {
        Self::default()
    }

    fn uses(&self, ch: usize) -> bool {
        self.con0 & (0x0800 >> ch) != 0
    }

    fn line_mode(&self) -> bool {
        self.con1 & con1::LINE != 0
    }

    /// BLTSIZE: start a blit.
    pub fn start(&mut self, size: u16) {
        let height = match size >> 6 {
            0 => 1024,
            h => h,
        };
        let width = match size & 0x3f {
            0 => 64,
            w => w,
        };
        self.busy = true;
        self.zero = true;
        self.job = Some(Job {
            width,
            height,
            delay: START_DELAY,
            carry: self.con1 & con1::FCI != 0,
            ..Job::default()
        });
    }

    /// Every colour clock: the start delay runs down.
    pub fn tick(&mut self) {
        if let Some(job) = &mut self.job {
            job.delay = job.delay.saturating_sub(1);
        }
    }

    /// Does the Blitter want a DMA slot now?
    pub fn wants_slot(&self) -> bool {
        self.job.is_some_and(|j| j.delay == 0)
    }

    fn slots_per_unit(&self) -> u8 {
        if self.line_mode() {
            return 4;
        }
        let n = (0..4).filter(|&ch| self.uses(ch)).count() as u8;
        n.max(2)
    }

    /// One granted DMA slot. Returns true when the blit finished in it.
    pub fn cycle(&mut self, chip: &mut ChipRam) -> bool {
        let Some(mut job) = self.job else {
            return false;
        };
        job.slots += 1;
        if job.slots < self.slots_per_unit() {
            self.job = Some(job);
            return false;
        }
        job.slots = 0;
        let done = if self.line_mode() {
            self.line_pixel(chip, &mut job)
        } else {
            self.area_word(chip, &mut job)
        };
        if done {
            self.job = None;
            self.busy = false;
        } else {
            self.job = Some(job);
        }
        done
    }

    fn step(&mut self, ch: usize, desc: bool) {
        self.pt[ch] = if desc {
            self.pt[ch].wrapping_sub(2)
        } else {
            self.pt[ch].wrapping_add(2)
        };
    }

    fn area_word(&mut self, chip: &mut ChipRam, job: &mut Job) -> bool {
        let desc = self.con1 & con1::DESC != 0;
        let ash = u32::from(self.con0 >> 12);
        let bsh = u32::from(self.con1 >> 12);
        if self.uses(A) {
            self.adat = chip.word(self.pt[A]);
            self.step(A, desc);
        }
        if self.uses(B) {
            self.bdat = chip.word(self.pt[B]);
            self.step(B, desc);
        }
        if self.uses(C) {
            self.cdat = chip.word(self.pt[C]);
            self.step(C, desc);
        }
        let mut a = self.adat;
        if job.col == 0 {
            a &= self.afwm;
        }
        if job.col == job.width - 1 {
            a &= self.alwm;
        }
        let shift = |new: u16, old: u16, sh: u32| -> u16 {
            if desc {
                (((u32::from(new) << 16 | u32::from(old)) << sh) >> 16) as u16
            } else {
                ((u32::from(old) << 16 | u32::from(new)) >> sh) as u16
            }
        };
        let a_shifted = shift(a, job.a_old, ash);
        let b_shifted = shift(self.bdat, job.b_old, bsh);
        job.a_old = a;
        job.b_old = self.bdat;
        let mut d = minterm(self.con0 as u8, a_shifted, b_shifted, self.cdat);
        if self.con1 & (con1::IFE | con1::EFE) != 0 {
            let (f, carry) = fill(d, job.carry, self.con1 & con1::EFE != 0);
            d = f;
            job.carry = carry;
        }
        if d != 0 {
            self.zero = false;
        }
        if self.uses(D) {
            chip.set_word(self.pt[D], d);
            self.step(D, desc);
        }
        job.col += 1;
        if job.col < job.width {
            return false;
        }
        job.col = 0;
        job.carry = self.con1 & con1::FCI != 0;
        for ch in 0..4 {
            if self.uses(ch) {
                let m = self.modulo[ch] as i16 as i32 as u32;
                self.pt[ch] = if desc {
                    self.pt[ch].wrapping_sub(m)
                } else {
                    self.pt[ch].wrapping_add(m)
                };
            }
        }
        job.row += 1;
        job.row >= job.height
    }

    fn line_pixel(&mut self, chip: &mut ChipRam, job: &mut Job) -> bool {
        let ash = self.con0 >> 12;
        let bsh = self.con1 >> 12;
        let n = job.row;
        // Texture: bit 15 first when BSH = ASH.
        let tex_bit = (15 + bsh).wrapping_sub(ash).wrapping_sub(n) & 15;
        let b = if self.bdat & (1 << tex_bit) != 0 {
            0xffff
        } else {
            0
        };
        let mut a = self.adat >> ash;
        if self.con1 & con1::SING != 0 && job.row_drawn {
            a = 0;
        }
        let c = chip.word(self.pt[C]);
        self.cdat = c;
        let d = minterm(self.con0 as u8, a, b, c);
        if d != 0 {
            self.zero = false;
        }
        if self.uses(D) {
            chip.set_word(self.pt[D], d);
        }
        if a != 0 {
            job.row_drawn = true;
        }
        // Step: the error term decides the occasional step.
        let error = self.pt[A] as u16 as i16 as i32;
        let sign = if n == 0 {
            self.con1 & con1::SIGN != 0
        } else {
            error < 0
        };
        let (step_minor, error) = if sign {
            (false, error + i32::from(self.modulo[B] as i16))
        } else {
            (true, error + i32::from(self.modulo[A] as i16))
        };
        self.pt[A] = (self.pt[A] & 0xffff_0000) | (error as u16 as u32);
        let horizontal_major = self.con1 & con1::SUD != 0;
        let mut ash = ash;
        let mut x_step = |cpt: &mut u32, neg: bool| {
            if neg {
                if ash == 0 {
                    ash = 15;
                    *cpt = cpt.wrapping_sub(2);
                } else {
                    ash -= 1;
                }
            } else if ash == 15 {
                ash = 0;
                *cpt = cpt.wrapping_add(2);
            } else {
                ash += 1;
            }
        };
        let cmod = self.modulo[C] as i16 as i32 as u32;
        let mut cpt = self.pt[C];
        let y_step = |cpt: &mut u32, neg: bool, row_drawn: &mut bool| {
            *cpt = if neg {
                cpt.wrapping_sub(cmod)
            } else {
                cpt.wrapping_add(cmod)
            };
            *row_drawn = false;
        };
        let minor_neg = self.con1 & con1::SUL != 0;
        let major_neg = self.con1 & con1::AUL != 0;
        if horizontal_major {
            if step_minor {
                y_step(&mut cpt, minor_neg, &mut job.row_drawn);
            }
            x_step(&mut cpt, major_neg);
        } else {
            if step_minor {
                x_step(&mut cpt, minor_neg);
            }
            y_step(&mut cpt, major_neg, &mut job.row_drawn);
        }
        self.pt[C] = cpt;
        self.pt[D] = cpt;
        self.con0 = (self.con0 & 0x0fff) | ash << 12;
        self.con1 = (self.con1 & !con1::SIGN) | if error < 0 { con1::SIGN } else { 0 };
        job.row += 1;
        job.row >= job.height
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        for v in [self.con0, self.con1, self.afwm, self.alwm] {
            w.u16(v);
        }
        for p in self.pt {
            w.u32(p);
        }
        for m in self.modulo {
            w.u16(m);
        }
        for v in [self.adat, self.bdat, self.cdat] {
            w.u16(v);
        }
        w.bool(self.busy);
        w.bool(self.zero);
        w.bool(self.job.is_some());
        let j = self.job.unwrap_or_default();
        for v in [j.width, j.height, j.row, j.col, j.a_old, j.b_old] {
            w.u16(v);
        }
        w.u8(j.slots);
        w.u8(j.delay);
        w.bool(j.carry);
        w.bool(j.row_drawn);
    }

    pub(crate) fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut b = Self::new();
        for v in [&mut b.con0, &mut b.con1, &mut b.afwm, &mut b.alwm] {
            *v = r.u16()?;
        }
        for p in &mut b.pt {
            *p = r.u32()?;
        }
        for m in &mut b.modulo {
            *m = r.u16()?;
        }
        for v in [&mut b.adat, &mut b.bdat, &mut b.cdat] {
            *v = r.u16()?;
        }
        b.busy = r.bool()?;
        b.zero = r.bool()?;
        let has_job = r.bool()?;
        let mut j = Job::default();
        for v in [
            &mut j.width,
            &mut j.height,
            &mut j.row,
            &mut j.col,
            &mut j.a_old,
            &mut j.b_old,
        ] {
            *v = r.u16()?;
        }
        j.slots = r.u8()?;
        j.delay = r.u8()?;
        j.carry = r.bool()?;
        j.row_drawn = r.bool()?;
        if has_job {
            if j.width == 0 || j.width > 64 || j.height == 0 || j.height > 1024 || j.col >= j.width
            {
                return Err(StateError::Invalid("blitter job"));
            }
            b.job = Some(j);
        }
        Ok(b)
    }
}

#[cfg(test)]
mod tests;
