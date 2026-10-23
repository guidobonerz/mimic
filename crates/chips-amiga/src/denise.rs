//! Denise (8362): bitplane shifters, playfield modes and colour output
//! (HRM chapter 3, "Playfield hardware"; BPLCON0–2 in appendix A).
//!
//! - **BPLCON0**: bit 15 HIRES, 14–12 BPU (planes), 11 HOMOD (HAM), 10 DBLPF
//!   (dual playfield), 9 COLOR, 2 LACE.
//! - **BPLCON1**: PF2H (7–4) and PF1H (3–0), the scroll delay of the even
//!   and odd planes in low-resolution pixels.
//! - **BPLCON2**: PF2PRI (6), playfield 2 in front of playfield 1.
//! - **COLOR00–31**: 12-bit RGB ($0RGB).
//!
//! Agnus writes BPL1DAT–BPL6DAT; writing BPL1DAT moves all planes into the
//! shift registers after the scroll delay, and arms the display for the
//! line. Pixels appear inside the horizontal display window (DIWSTRT /
//! DIWSTOP, low-resolution pixel positions) on lines where planes were
//! fetched; elsewhere COLOR00 shows.
//!
//! Six planes without HAM or dual playfield are extra-half-brite: plane 6
//! halves the colour of planes 1–5.
//!
//! **Sprites** (HRM chapter 4, "Sprite hardware"): SPRxPOS (VSTART 7–0,
//! HSTART 8–1), SPRxCTL (VSTOP 7–0, ATTACH 7, VSTART 8 in bit 2, VSTOP 8
//! in bit 1, HSTART 0 in bit 0), SPRxDATA/SPRxDATB (low/high colour bit).
//! Writing CTL disarms a sprite, writing DATA arms it; an armed sprite
//! starts its 16 low-resolution pixels whenever the beam reaches HSTART
//! (same pixel positions as DIWSTRT) and shows only inside the horizontal
//! display window. Pairs 0/1 … 6/7 use COLOR17–19 … COLOR29–31; an odd
//! sprite with ATTACH combines with its even partner into 15 colours
//! (COLOR17–31). Lower-numbered sprites are in front; BPLCON2 PF1P (2–0)
//! and PF2P (5–3) put playfield 1/2 behind the sprite pairs below the
//! value (single playfield: PF2P).
//!
//! **Collisions** (HRM chapter 7, "Collision detection"): CLXCON bits 15–12
//! ENSP7/5/3/1 include odd sprites in their pair, 11–6 ENBP6–1 select the
//! planes compared with the match values MVBP6–1 (5–0); a disabled plane
//! always matches. CLXDAT (cleared by reading; bit 15 reads 1): 14–9
//! sprite pairs 4–6, 2–6, 2–4, 0–6, 0–4, 0–2; 8–5 even planes with pairs
//! 6, 4, 2, 0; 4–1 odd planes with pairs 6, 4, 2, 0; 0 "even with odd
//! planes". Collisions count inside the horizontal display window on
//! lines with bitplane data (a BPL1DAT write).
//!
//! Photographs of A500s running the vAmiga test suite's `sprcoll` programs
//! (see `docs/sources.md`) show the playfield bits differently from the
//! HRM: bit 0 is set wherever the *odd* planes match (whether or not the
//! even ones do), and in single-playfield mode the odd-plane sprite bits
//! 4–1 need the even planes to match as well (in dual-playfield mode they
//! follow the odd planes alone, as documented).
//!
//! **Coordinates.** Positions are low-resolution pixels as DIWSTRT counts
//! them: Denise colour clock `h` covers pixels `2h` and `2h + 1`; data
//! fetched in slot `h` loads at pixel `2h + 3` (so DDFSTRT $38 lines up
//! with DIWSTRT $81); a sprite starts one pixel right of its HSTART.
//! Denise's clock runs two colour clocks behind Agnus' horizontal position
//! (`custom.rs`, `DENISE_LAG`), so a register write shows from pixel
//! `2h − 4` of the Agnus clock `h` it happens in. Denise's line runs from
//! colour clock [`LINE_START`] to the same clock of the next Agnus line, so
//! the right border can pass the end of the Agnus line.

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

/// First colour clock of a Denise line; earlier clocks finish the previous
/// line's right border.
pub const LINE_START: u16 = 9;
/// Pixels from the bitplane fetch slot to the load (low resolution).
const LOAD_DELAY: u32 = 3;

/// Loads that can wait per plane group: a word arrives every 16
/// high-resolution pixels in high resolution (32 in low resolution) and a
/// scroll of 15 delays its load by 36 pixels, so up to three wait at once.
const QUEUE: usize = 4;

/// A pending parallel load into the odd or even plane shifters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Load {
    /// High-resolution pixel position (Denise line coordinates).
    at: u32,
    data: [u16; 3],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denise {
    pub colors: [u16; 32],
    pub bplcon0: u16,
    pub bplcon1: u16,
    pub bplcon2: u16,
    pub diwstrt: u16,
    pub diwstop: u16,
    pub bpldat: [u16; 6],
    /// Shift registers, planes 1–6.
    shift: [u16; 6],
    /// Pending loads for odd (1, 3, 5) and even (2, 4, 6) planes, the
    /// earliest first. Several can wait: with a large scroll value the next
    /// words are fetched before the previous load is due (low resolution:
    /// scroll 13–15, Desert Dream's moon; high resolution: from scroll 5,
    /// its end scroller uses 15). See [`QUEUE`].
    pending: [[Option<Load>; QUEUE]; 2],
    /// High-resolution pixels since each group's load (low-resolution
    /// shifting moves on every second one).
    phase: [u8; 2],
    /// BPL1DAT was written on this line.
    armed: bool,
    /// HAM: the colour of the previous pixel.
    ham: u16,
    pub sprites: [Sprite; 8],
    pub clxcon: u16,
    /// Collisions seen since CLXDAT was last read (bits 14–0).
    pub clxdat: u16,
}

/// One sprite's registers and shifter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sprite {
    pub pos: u16,
    pub ctl: u16,
    pub data: u16,
    pub datb: u16,
    pub armed: bool,
    a: u16,
    b: u16,
    /// High-resolution pixels left in the shifter.
    count: u8,
}

impl Sprite {
    /// Horizontal start in low-resolution pixels.
    pub fn hstart(&self) -> u32 {
        u32::from(self.pos & 0xff) << 1 | u32::from(self.ctl & 1)
    }

    pub fn vstart(&self) -> u16 {
        self.pos >> 8 | (self.ctl & 4) << 6
    }

    pub fn vstop(&self) -> u16 {
        self.ctl >> 8 | (self.ctl & 2) << 7
    }
}

impl Default for Denise {
    fn default() -> Self {
        Self::new()
    }
}

/// A 12-bit colour as RGB.
pub fn rgb12(c: u16) -> Rgb8 {
    let n = |v: u16| ((v & 15) * 17) as u8;
    Rgb8 {
        r: n(c >> 8),
        g: n(c >> 4),
        b: n(c),
    }
}

impl Denise {
    pub fn new() -> Self {
        Self {
            colors: [0; 32],
            bplcon0: 0,
            bplcon1: 0,
            bplcon2: 0,
            diwstrt: 0,
            diwstop: 0,
            bpldat: [0; 6],
            shift: [0; 6],
            pending: [[None; QUEUE]; 2],
            phase: [0; 2],
            armed: false,
            ham: 0,
            sprites: [Sprite::default(); 8],
            clxcon: 0,
            clxdat: 0,
        }
    }

    /// Collision bits for one pixel: playfield `index`, sprite values (see
    /// the module doc for how the playfield bits follow the hardware).
    fn collisions(&self, index: u8, vals: &[u8; 8]) -> u16 {
        let enable = (self.clxcon >> 6) as u8 & 0x3f;
        let matchv = self.clxcon as u8 & 0x3f;
        let matches = |planes: u8| (index ^ matchv) & enable & planes == 0;
        let odd = matches(0b01_0101);
        let even = matches(0b10_1010);
        let dual = self.bplcon0 & 0x0400 != 0;
        let sprite_odd = odd && (dual || even);
        let mut group = [false; 4];
        for (g, on) in group.iter_mut().enumerate() {
            let odd_on = self.clxcon & (0x1000 << g) != 0;
            *on = vals[2 * g] != 0 || (odd_on && vals[2 * g + 1] != 0);
        }
        let mut v = u16::from(odd);
        for (g, &on) in group.iter().enumerate() {
            if on {
                v |= u16::from(sprite_odd) << (1 + g) | u16::from(even) << (5 + g);
            }
        }
        const PAIRS: [(usize, usize); 6] = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
        for (i, &(a, b)) in PAIRS.iter().enumerate() {
            if group[a] && group[b] {
                v |= 1 << (9 + i);
            }
        }
        v
    }

    /// SPRxPOS (0), SPRxCTL (1), SPRxDATA (2), SPRxDATB (3).
    pub fn write_sprite(&mut self, n: usize, reg: usize, v: u16) {
        let s = &mut self.sprites[n];
        match reg {
            0 => s.pos = v,
            1 => {
                s.ctl = v;
                s.armed = false;
            }
            2 => {
                s.data = v;
                s.armed = true;
            }
            _ => s.datb = v,
        }
    }

    /// The frontmost sprite colour at this pixel (pair, colour register)
    /// and every sprite's value.
    fn sprite_pixel(&mut self, x: u32) -> (Option<(u8, u8)>, [u8; 8]) {
        let lx = x / 2;
        let mut vals = [0u8; 8];
        for (n, s) in self.sprites.iter_mut().enumerate() {
            // A sprite's first pixel is one low-resolution pixel right of
            // HSTART (WinUAE screenshot of Interference's tunnel frame,
            // `docs/sources.md`: HSTART $84 shows one pixel inside a window
            // starting at 148, HSTART $1AE one pixel inside one ending at 432).
            if x.is_multiple_of(2) && s.armed && lx == s.hstart() + 1 {
                s.a = s.data;
                s.b = s.datb;
                s.count = 32;
            }
            if s.count > 0 {
                vals[n] = (s.a >> 15) as u8 | ((s.b >> 15) as u8) << 1;
                s.count -= 1;
                if s.count % 2 == 0 {
                    s.a <<= 1;
                    s.b <<= 1;
                }
            }
        }
        let front = (0..4u8).find_map(|pair| {
            let (even, odd) = (vals[2 * pair as usize], vals[2 * pair as usize + 1]);
            if self.sprites[2 * pair as usize + 1].ctl & 0x80 != 0 {
                let v = odd << 2 | even;
                (v != 0).then_some((pair, 16 + v))
            } else if even != 0 {
                Some((pair, 16 + 4 * pair + even))
            } else if odd != 0 {
                Some((pair, 16 + 4 * pair + odd))
            } else {
                None
            }
        });
        (front, vals)
    }

    pub fn hires(&self) -> bool {
        self.bplcon0 & 0x8000 != 0
    }

    /// Planes enabled in BPLCON0 (7 counts as none on the OCS).
    pub fn planes(&self) -> usize {
        match (self.bplcon0 >> 12) & 7 {
            7 => 0,
            n => usize::from(n),
        }
    }

    /// Horizontal display window in low-resolution pixels: DIWSTRT bit 8
    /// is 0, DIWSTOP bit 8 is 1.
    fn window(&self) -> (u32, u32) {
        (
            u32::from(self.diwstrt & 0xff),
            0x100 | u32::from(self.diwstop & 0xff),
        )
    }

    /// Agnus fetched a plane word in slot `h` (Denise line colour clock).
    pub fn write_bpldat(&mut self, plane: usize, v: u16, h: u32) {
        self.bpldat[plane] = v;
        if plane == 0 {
            let base = 2 * (2 * h + LOAD_DELAY);
            let odd = u32::from(self.bplcon1 & 15);
            let even = u32::from((self.bplcon1 >> 4) & 15);
            let d = self.bpldat;
            self.queue(
                0,
                Load {
                    at: base + 2 * odd,
                    data: [d[0], d[2], d[4]],
                },
            );
            self.queue(
                1,
                Load {
                    at: base + 2 * even,
                    data: [d[1], d[3], d[5]],
                },
            );
            self.armed = true;
        }
    }

    /// Queue a load for plane group `g` behind those still waiting; loads
    /// at or after the new one's time are dropped (a new line or a smaller
    /// scroll value). A full queue drops its oldest load.
    fn queue(&mut self, g: usize, load: Load) {
        let q = &mut self.pending[g];
        let mut n = q
            .iter()
            .take_while(|l| l.is_some_and(|l| l.at < load.at))
            .count();
        if n == QUEUE {
            q.rotate_left(1);
            n -= 1;
        }
        q[n] = Some(load);
        for l in &mut q[n + 1..] {
            *l = None;
        }
    }

    /// Start of a Denise line.
    pub fn start_line(&mut self) {
        self.armed = false;
        self.ham = self.colors[0];
    }

    /// The four high-resolution pixels of colour clock `h` (Denise line
    /// coordinates) as 12-bit colours.
    pub fn output(&mut self, h: u32, out: &mut [u16; 4]) {
        let (start, stop) = self.window();
        let hires = self.hires();
        let planes = self.planes();
        let sprites_live = self.sprites.iter().any(|s| s.armed || s.count > 0);
        for (i, px) in out.iter_mut().enumerate() {
            let x = 4 * h + i as u32;
            for g in 0..2 {
                if let Some(l) = self.pending[g][0] {
                    if x >= l.at {
                        for (k, &w) in l.data.iter().enumerate() {
                            self.shift[g + 2 * k] = w;
                        }
                        self.phase[g] = 0;
                        self.pending[g].rotate_left(1);
                        self.pending[g][QUEUE - 1] = None;
                    }
                }
            }
            let mut index = 0u8;
            for p in 0..planes {
                if self.shift[p] & 0x8000 != 0 {
                    index |= 1 << p;
                }
            }
            for g in 0..2 {
                self.phase[g] = self.phase[g].wrapping_add(1);
                if hires || self.phase[g].is_multiple_of(2) {
                    for p in (g..6).step_by(2) {
                        self.shift[p] <<= 1;
                    }
                }
            }
            let lx = x / 2;
            if lx >= stop && self.armed {
                self.armed = false;
            }
            let inside = lx >= start && lx < stop;
            let (pf, front) = if self.armed && inside {
                self.color(index, planes)
            } else {
                (self.colors[0], None)
            };
            let (sprite, vals) = if sprites_live {
                self.sprite_pixel(x)
            } else {
                (None, [0; 8])
            };
            // Sprites show inside the window once the line's bitplane data
            // has started (BPL1DAT); a line without any plane data shows none,
            // whether BPLCON0 has no planes or bitplane DMA is off (vAmigaTS
            // diwclip/clip1h and sprbpu1 photographs, OCS and ECS alike: the
            // sprites stop in the BPU = 0 gaps between sprbpu1's boxes; WinUAE
            // with `tools/amiga_sprtest.py`). Interference hides its sprite
            // boxes behind its raster bars by switching to BPU = 0 four lines
            // before it stops sprite DMA.
            let sprite = sprite.filter(|_| inside && self.armed);
            if inside && self.armed {
                if vals != [0; 8] {
                    self.clxdat |= self.collisions(index, &vals);
                } else if self.clxdat & 1 == 0 {
                    // Without sprites only the playfield bit can come.
                    self.clxdat |= self.collisions(index, &vals) & 1;
                }
            }
            *px = match (sprite, front) {
                (Some((pair, c)), Some(pf_no)) => {
                    let prio = if pf_no == 1 {
                        self.bplcon2 & 7
                    } else {
                        (self.bplcon2 >> 3) & 7
                    };
                    if u16::from(pair) >= prio {
                        pf
                    } else {
                        self.colors[usize::from(c)]
                    }
                }
                (Some((_, c)), None) => self.colors[usize::from(c)],
                (None, _) => pf,
            };
        }
    }

    /// The playfield colour of a pixel and the playfield (1 or 2) it comes
    /// from, `None` where both are transparent.
    fn color(&mut self, index: u8, planes: usize) -> (u16, Option<u8>) {
        let ham = self.bplcon0 & 0x0800 != 0;
        let dual = self.bplcon0 & 0x0400 != 0;
        let single = (index != 0).then_some(2);
        if ham && planes >= 5 {
            let data = u16::from(index & 15);
            let c = match index >> 4 {
                0 => self.colors[usize::from(data)],
                1 => (self.ham & 0xff0) | data,
                2 => (self.ham & 0x0ff) | data << 8,
                _ => (self.ham & 0xf0f) | data << 4,
            };
            self.ham = c;
            return (c, single);
        }
        if dual {
            let pf1 = (index & 1) | (index >> 1 & 2) | (index >> 2 & 4);
            let pf2 = (index >> 1 & 1) | (index >> 2 & 2) | (index >> 3 & 4);
            let pf2_front = self.bplcon2 & 0x40 != 0;
            let (c, front) = match (pf1 != 0, pf2 != 0) {
                (false, false) => (0, None),
                (true, false) => (pf1, Some(1)),
                (false, true) => (8 + pf2, Some(2)),
                (true, true) if pf2_front => (8 + pf2, Some(2)),
                (true, true) => (pf1, Some(1)),
            };
            return (self.colors[usize::from(c)], front);
        }
        let c = self.colors[usize::from(index & 31)];
        if planes == 6 && index & 32 != 0 {
            // Extra half-brite.
            ((c >> 1) & 0x777, single)
        } else {
            (c, single)
        }
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        for c in self.colors {
            w.u16(c);
        }
        for v in [
            self.bplcon0,
            self.bplcon1,
            self.bplcon2,
            self.diwstrt,
            self.diwstop,
            self.ham,
        ] {
            w.u16(v);
        }
        for v in self.bpldat.iter().chain(&self.shift) {
            w.u16(*v);
        }
        for p in self.pending.iter().flatten() {
            w.bool(p.is_some());
            let l = p.unwrap_or_default();
            w.u32(l.at);
            for v in l.data {
                w.u16(v);
            }
        }
        w.raw(&self.phase);
        w.bool(self.armed);
        w.u16(self.clxcon);
        w.u16(self.clxdat);
        for s in &self.sprites {
            for v in [s.pos, s.ctl, s.data, s.datb, s.a, s.b] {
                w.u16(v);
            }
            w.bool(s.armed);
            w.u8(s.count);
        }
    }

    /// `version` is the OCS chunk's: version 2 saved one pending load per
    /// group, version 3 two, from 4 on [`QUEUE`].
    pub(crate) fn load(r: &mut StateReader<'_>, version: u16) -> Result<Self, StateError> {
        let mut d = Self::new();
        for c in &mut d.colors {
            *c = r.u16()? & 0x0fff;
        }
        for v in [
            &mut d.bplcon0,
            &mut d.bplcon1,
            &mut d.bplcon2,
            &mut d.diwstrt,
            &mut d.diwstop,
            &mut d.ham,
        ] {
            *v = r.u16()?;
        }
        for v in d.bpldat.iter_mut().chain(d.shift.iter_mut()) {
            *v = r.u16()?;
        }
        let depth = match version {
            0..=2 => 1,
            3 => 2,
            _ => QUEUE,
        };
        for q in &mut d.pending {
            for p in q.iter_mut().take(depth) {
                let some = r.bool()?;
                let mut l = Load {
                    at: r.u32()?,
                    data: [0; 3],
                };
                for v in &mut l.data {
                    *v = r.u16()?;
                }
                *p = some.then_some(l);
            }
            // Keep the queue packed: nothing after the first gap.
            if let Some(gap) = q.iter().position(Option::is_none) {
                for l in &mut q[gap..] {
                    *l = None;
                }
            }
        }
        r.raw_into(&mut d.phase)?;
        d.armed = r.bool()?;
        d.clxcon = r.u16()?;
        d.clxdat = r.u16()? & 0x7fff;
        for s in &mut d.sprites {
            for v in [
                &mut s.pos,
                &mut s.ctl,
                &mut s.data,
                &mut s.datb,
                &mut s.a,
                &mut s.b,
            ] {
                *v = r.u16()?;
            }
            s.armed = r.bool()?;
            s.count = r.u8()?.min(32);
        }
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(d: &mut Denise, from: u32, to: u32) -> Vec<u16> {
        let mut px = Vec::new();
        for h in from..to {
            let mut out = [0; 4];
            d.output(h, &mut out);
            px.extend(out);
        }
        px
    }

    #[test]
    fn lowres_plane_appears_at_the_window_start() {
        let mut d = Denise::new();
        d.colors[0] = 0x000;
        d.colors[1] = 0xfff;
        d.bplcon0 = 0x1200;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        d.write_bpldat(0, 0xc000, 0x3f);
        let px = line(&mut d, 0x3f, 0x44);
        // Hires pixel 2·$81 = $102 is index $102 − 4·$3F = 6.
        assert_eq!(&px[..6], &[0; 6]);
        assert_eq!(&px[6..10], &[0xfff; 4], "two low-resolution pixels");
        assert_eq!(px[10], 0);
    }

    #[test]
    fn scroll_delays_odd_planes() {
        let mut d = Denise::new();
        d.colors[1] = 0xf00;
        d.bplcon0 = 0x1200;
        d.bplcon1 = 0x0001;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        d.write_bpldat(0, 0x8000, 0x3f);
        let px = line(&mut d, 0x3f, 0x44);
        assert_eq!(&px[6..10], &[0, 0, 0xf00, 0xf00]);
    }

    #[test]
    fn a_large_scroll_keeps_every_word() {
        // BPL1DAT every 8 colour clocks; with PF1H = 15 each load waits 36
        // high-resolution pixels, past the next fetch (Desert Dream's moon
        // sliding in from the right).
        let mut d = Denise::new();
        d.colors[1] = 0xf00;
        d.bplcon0 = 0x1200;
        d.bplcon1 = 0x000f;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        let mut px = Vec::new();
        for n in 0..3u32 {
            d.write_bpldat(0, 0x8000 >> n, 0x3f + 8 * n);
            px.extend(line(&mut d, 0x3f + 8 * n, 0x47 + 8 * n));
        }
        px.extend(line(&mut d, 0x57, 0x5f));
        // Word n shows at hires pixel 4·$3F + 6 + 30 + 32n (+ 2n for its bit).
        for n in 0..3usize {
            let at = 36 + 34 * n;
            assert_eq!(&px[at..at + 2], &[0xf00; 2], "word {n}");
            assert_eq!(px[at - 1], 0, "word {n} start");
        }
    }

    #[test]
    fn a_large_hires_scroll_keeps_every_word() {
        // High resolution: BPL1DAT every 4 colour clocks (16 pixels); with
        // PF1H = 15 each load waits 36 pixels, so three are pending at once
        // (Desert Dream's end scroller).
        let mut d = Denise::new();
        d.colors[1] = 0xf00;
        d.bplcon0 = 0x9200;
        d.bplcon1 = 0x000f;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        let mut px = Vec::new();
        for n in 0..4u32 {
            d.write_bpldat(0, 0x8000 >> n, 0x3f + 4 * n);
            px.extend(line(&mut d, 0x3f + 4 * n, 0x43 + 4 * n));
        }
        px.extend(line(&mut d, 0x4f, 0x5f));
        // Word n shows from pixel 4·$3F + 6 + 30 + 16n; its bit n one pixel
        // per bit later.
        for n in 0..4usize {
            let at = 36 + 17 * n;
            assert_eq!(px[at], 0xf00, "word {n}");
            assert_eq!(px[at - 1], 0, "word {n} start");
            assert_eq!(px[at + 1], 0, "word {n} is one hires pixel");
        }
    }

    #[test]
    fn half_brite_and_ham() {
        let mut d = Denise::new();
        d.colors[3] = 0xeca;
        d.bplcon0 = 0x6000;
        assert_eq!(d.color(32 | 3, 6).0, 0x765);
        d.bplcon0 = 0x6800;
        d.ham = 0x123;
        assert_eq!(d.color(0x20 | 0xf, 6).0, 0xf23, "modify red");
        assert_eq!(d.color(0x30 | 0x7, 6).0, 0xf73, "modify green");
        assert_eq!(d.color(0x3, 6).0, 0xeca, "palette");
    }

    #[test]
    fn dual_playfield_priority() {
        let mut d = Denise::new();
        d.colors[1] = 0x100;
        d.colors[9] = 0x900;
        d.bplcon0 = 0x2400;
        assert_eq!(d.color(0b11, 2), (0x100, Some(1)), "PF1 in front");
        d.bplcon2 = 0x40;
        assert_eq!(d.color(0b11, 2), (0x900, Some(2)), "PF2PRI");
        assert_eq!(d.color(0b10, 2).0, 0x900);
        assert_eq!(d.color(0, 2), (0, None));
    }

    #[test]
    fn collision_bits() {
        let mut d = Denise::new();
        let mut vals = [0u8; 8];
        vals[0] = 1;
        vals[3] = 2;
        // Nothing enabled: every plane matches, so sprite 0 collides with
        // both playfields, and the playfield bit is set.
        assert_eq!(d.collisions(0, &vals), 0b10_0011);
        // ENSP3 puts sprite 3 into pair 1: pair 0 with pair 1.
        d.clxcon = 0x2000;
        assert_eq!(d.collisions(0, &vals) & 0x7e00, 1 << 9);
        // Plane 1 enabled, match value 1: odd planes match only where set.
        d.clxcon = 0x0041;
        assert_eq!(d.collisions(0, &vals) & 0x1f, 0b0_0000, "even ok, odd not");
        assert_eq!(d.collisions(1, &vals) & 0x1f, 0b0_0011);
        // sprcoll8: planes 1 and 2 enabled with match 0, index 2: the odd
        // planes match, the even ones not. Single playfield: no sprite
        // bits, but bit 0; dual playfield: the odd sprite bit too.
        d.clxcon = 0x00c0;
        assert_eq!(d.collisions(2, &vals) & 0x1ff, 0b0_0000_0001);
        d.bplcon0 = 0x0400;
        assert_eq!(d.collisions(2, &vals) & 0x1ff, 0b0_0000_0011);
        // sprcollbrd1: all planes enabled with match 0, index 1: only the
        // even planes match, so just the even sprite bit.
        d.bplcon0 = 0;
        d.clxcon = 0x0fc0;
        assert_eq!(d.collisions(1, &vals) & 0x1ff, 0b0_0010_0000);
    }

    #[test]
    fn playfield_collision_with_every_plane_enabled() {
        let mut d = Denise::new();
        d.clxcon = 0xffff;
        assert_eq!(
            d.collisions(1, &[0; 8]) & 1,
            0,
            "planes 2-6 read 0, match wants 1"
        );
        assert_eq!(d.collisions(0x3f, &[0; 8]) & 1, 1);
        d.clxcon = 0x0fc0;
        assert_eq!(d.collisions(1, &[0; 8]) & 1, 0);
        assert_eq!(d.collisions(0, &[0; 8]) & 1, 1);
    }

    #[test]
    fn sprites_show_at_hstart_with_priorities() {
        let mut d = Denise::new();
        d.colors[0] = 0x000;
        d.colors[1] = 0x00f;
        d.colors[17] = 0xf00;
        d.colors[21] = 0x0f0;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        // No planes in BPLCON0: no sprite shows.
        d.bplcon0 = 0x0200;
        // Sprite 0 at low-resolution pixel $A0, sprite 2 one pixel later.
        d.write_sprite(0, 0, 0x2c50);
        d.write_sprite(0, 1, 0x2d00);
        d.write_sprite(0, 3, 0x0000);
        d.write_sprite(0, 2, 0x8000);
        d.write_sprite(2, 0, 0x2c50);
        d.write_sprite(2, 1, 0x2d01);
        d.write_sprite(2, 3, 0x0000);
        d.write_sprite(2, 2, 0xc000);
        d.start_line();
        let px = line(&mut d, 0x4f, 0x53);
        assert_eq!(px, vec![0; px.len()]);
        // One plane with empty data: sprites show after BPL1DAT.
        d.bplcon0 = 0x1200;
        d.write_sprite(0, 2, 0x8000);
        d.write_sprite(2, 2, 0xc000);
        d.start_line();
        d.write_bpldat(0, 0, 0x40);
        let px = line(&mut d, 0x4f, 0x53);
        // Hires pixel 2·$A1 = $142 (one pixel right of HSTART) is index
        // $142 − 4·$4F = 6.
        assert_eq!(
            &px[5..13],
            &[0, 0xf00, 0xf00, 0x0f0, 0x0f0, 0x0f0, 0x0f0, 0]
        );
        // A playfield in front of pair 1 (PF2P = 1) hides sprite 2 only.
        d.bplcon0 = 0x1200;
        d.bplcon2 = 1 << 3;
        d.start_line();
        d.write_bpldat(0, 0xffff, 0x4c);
        let px = line(&mut d, 0x4f, 0x53);
        assert_eq!(&px[6..11], &[0xf00, 0xf00, 0x00f, 0x00f, 0x00f]);
    }
}
