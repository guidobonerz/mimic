//! Denise (8362, ECS 8373) and the AGA Lisa: bitplane shifters, playfield
//! modes and colour output (HRM chapter 3, "Playfield hardware"; BPLCON0–2
//! in appendix A, the ECS additions in appendix C, the AGA registers as
//! documented in the AGA chip set notes of the Amiga Developer CD).
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
//! **ECS Denise 8373** ([`Denise::ecs`]): DENISEID ($7C) reads $FC in its
//! low byte (the OCS chip leaves the bus floating, $FFFF). BPLCON0 bit 0
//! ECSENA enables the BPLCON3 ($106) bits 5 BRDRBLNK (the border shows
//! black instead of COLOR00) and 1 BRDSPRT (sprites show in the border);
//! bits 4 BRDNTRAN, 2 ZDCLKEN and 0 EXTBLKEN drive the genlock pins only.
//! DIWHIGH ($1E4) bits 5 and 13 are bit 8 of the window's horizontal start
//! and stop (a DIWSTRT/DIWSTOP write restores start 0, stop 1). BPLCON0
//! bit 6 SHRES is super-hires: 35 ns pixels, at most two planes, two
//! significant bits per colour gun (HRM appendix C; which two is this
//! project's reading: the upper ones). The picture keeps its 70 ns
//! columns, so every second super-hires pixel is dropped.
//!
//! **AGA Lisa** ([`Denise::aga`], implies the ECS features): DENISEID $F8;
//! eight bitplanes (BPLCON0 bit 4 BPU3; the index of up to eight bits,
//! XORed with BPLCON4 bits 15–8 BPLAM, selects one of 256 colour
//! registers of 24 bits). COLORxx writes go to the bank BPLCON3 bits
//! 15–13 select (32 registers each); with BPLCON3 bit 9 LOCT set they
//! load the low four bits of each gun, otherwise the high four bits
//! (copied to the low ones too, so 12-bit code shows 12-bit colours).
//! HAM with eight planes (HAM8): planes 1–2 are the control bits, planes
//! 3–8 the six bits that replace a gun's upper six bits. Dual playfield:
//! playfield 2's colours start at the offset BPLCON3 bits 12–10 PF2OF
//! select (0, 2, 4, 8, 16, 32, 64, 128; Kickstart sets 8, the OCS value,
//! and so does power-on here). BPLCON2 bit 9 KILLEHB turns extra-half-brite
//! off. BPLCON1 bits 11–10 / 9–8 (odd) and 15–14 / 13–12 (even planes)
//! extend the scroll to 35 ns steps up to 256 of them. Sprites: BPLCON4
//! bits 7–4 ESPRM and 3–0 OSPRM are the upper four bits of the colour
//! index of even and odd sprites (power-on $11: colours 16–31 as on the
//! OCS); SPRxCTL bits 4–3 add 35 ns position bits, bit 6 and 5 are bit 9
//! of VSTART and VSTOP; BPLCON3 bits 7–6 SPRES pick the sprite pixel
//! width (0 and 1: 140 ns, 2: 70 ns, 3: 35 ns, shown at 70 ns here); FMODE
//! ($1FC) bits 3–2 make sprites 32 or 64 pixels wide, bits 1–0 let Alice
//! deliver 32 or 64 bits per bitplane fetch ([`Denise::write_bpldat`]
//! takes the data left-aligned in 64 bits). CLXCON2 ($10E) bits 7–6 and
//! 1–0 enable and match planes 8 and 7 in collisions.
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
//!
//! Colours are 24-bit RGB (`$RRGGBB`) throughout; a 12-bit register value
//! becomes `$RRGGBB` with each nibble doubled ([`expand12`]).

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

/// First colour clock of a Denise line; earlier clocks finish the previous
/// line's right border.
pub const LINE_START: u16 = 9;
/// Pixels from the bitplane fetch slot to the load (low resolution).
const LOAD_DELAY: u32 = 3;

/// Loads that can wait per plane group: a word arrives every 16
/// high-resolution pixels in high resolution (32 in low resolution) and a
/// scroll of 15 delays its load by 36 pixels, so up to three wait at once;
/// the AGA scroll of up to 127 high-resolution pixels needs more.
const QUEUE: usize = 8;

/// A pending parallel load into the odd or even plane shifters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Load {
    /// High-resolution pixel position (Denise line coordinates).
    at: u32,
    /// Planes 1/3/5/7 or 2/4/6/8, left-aligned.
    data: [u64; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denise {
    /// 24-bit colours; the OCS/ECS chips use the first 32.
    pub colors: [u32; 256],
    pub bplcon0: u16,
    pub bplcon1: u16,
    pub bplcon2: u16,
    /// ECS Denise and AGA only (see the module doc).
    pub bplcon3: u16,
    /// AGA only.
    pub bplcon4: u16,
    pub diwstrt: u16,
    pub diwstop: u16,
    /// DIWHIGH's horizontal bits (5: start H8, 13: stop H8); the OCS
    /// default is stop H8 = 1.
    pub diwhigh: u16,
    /// FMODE's sprite width bits (AGA; Alice keeps the bitplane bits).
    pub fmode: u16,
    /// The ECS 8373 is fitted (configuration).
    pub ecs: bool,
    /// The AGA Lisa is fitted (configuration; implies `ecs`).
    pub aga: bool,
    /// The last data of each plane, left-aligned in 64 bits.
    pub bpldat: [u64; 8],
    /// Shift registers, planes 1–8, left-aligned.
    shift: [u64; 8],
    /// Pending loads for odd (1, 3, 5, 7) and even (2, 4, 6, 8) planes, the
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
    ham: u32,
    pub sprites: [Sprite; 8],
    pub clxcon: u16,
    /// AGA: planes 7 and 8 in collisions.
    pub clxcon2: u16,
    /// Collisions seen since CLXDAT was last read (bits 14–0).
    pub clxdat: u16,
}

/// One sprite's registers and shifter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sprite {
    pub pos: u16,
    pub ctl: u16,
    /// SPRxDATA/SPRxDATB, left-aligned (16, 32 or 64 bits wide).
    pub data: u64,
    pub datb: u64,
    pub armed: bool,
    a: u64,
    b: u64,
    /// Output (70 ns) pixels left in the shifter.
    count: u16,
    /// Output pixels per sprite pixel (2 low resolution, 1 high).
    step: u8,
}

impl Sprite {
    /// Horizontal start in low-resolution pixels.
    pub fn hstart(&self) -> u32 {
        u32::from(self.pos & 0xff) << 1 | u32::from(self.ctl & 1)
    }

    /// AGA: the horizontal start in 35 ns pixels (SPRxCTL bits 4–3 below
    /// the OCS bits).
    fn hstart_shres(&self) -> u32 {
        self.hstart() << 2 | u32::from(self.ctl >> 3 & 3)
    }

    pub fn vstart(&self) -> u16 {
        self.pos >> 8 | (self.ctl & 4) << 6
    }

    pub fn vstop(&self) -> u16 {
        self.ctl >> 8 | (self.ctl & 2) << 7
    }

    /// AGA: VSTART with bit 9 (SPRxCTL bit 6).
    pub fn vstart_aga(&self) -> u16 {
        self.vstart() | (self.ctl & 0x40) << 3
    }

    /// AGA: VSTOP with bit 9 (SPRxCTL bit 5).
    pub fn vstop_aga(&self) -> u16 {
        self.vstop() | (self.ctl & 0x20) << 4
    }
}

impl Default for Denise {
    fn default() -> Self {
        Self::new()
    }
}

/// A 12-bit colour ($0RGB) as 24 bits, each nibble doubled.
pub fn expand12(c: u16) -> u32 {
    let n = |v: u16| u32::from(v & 15) * 0x11;
    n(c >> 8) << 16 | n(c >> 4) << 8 | n(c)
}

/// A 12-bit colour as RGB.
pub fn rgb12(c: u16) -> Rgb8 {
    rgb24(expand12(c))
}

/// A 24-bit colour as RGB.
pub fn rgb24(c: u32) -> Rgb8 {
    Rgb8 {
        r: (c >> 16) as u8,
        g: (c >> 8) as u8,
        b: c as u8,
    }
}

/// Power-on BPLCON3 of the AGA Lisa: PF2OF = 8 (Kickstart's value too).
const BPLCON3_AGA: u16 = 0x0c00;
/// Power-on BPLCON4: sprite colours 16–31 as on the OCS.
const BPLCON4_DEFAULT: u16 = 0x0011;

impl Denise {
    pub fn new() -> Self {
        Self {
            colors: [0; 256],
            bplcon0: 0,
            bplcon1: 0,
            bplcon2: 0,
            bplcon3: 0,
            bplcon4: BPLCON4_DEFAULT,
            diwstrt: 0,
            diwstop: 0,
            diwhigh: 0x2000,
            fmode: 0,
            ecs: false,
            aga: false,
            bpldat: [0; 8],
            shift: [0; 8],
            pending: [[None; QUEUE]; 2],
            phase: [0; 2],
            armed: false,
            ham: 0,
            sprites: [Sprite::default(); 8],
            clxcon: 0,
            clxcon2: 0,
            clxdat: 0,
        }
    }

    /// Fit the AGA Lisa (power-on register values included).
    pub fn set_aga(&mut self, aga: bool) {
        self.aga = aga;
        if aga {
            self.ecs = true;
            self.bplcon3 = BPLCON3_AGA;
        }
    }

    /// COLORxx written with `v` (12 bits; on the AGA into the bank and
    /// nibble half BPLCON3 selects).
    pub fn write_color(&mut self, n: usize, v: u16) {
        if !self.aga {
            self.colors[n & 31] = expand12(v & 0xfff);
            return;
        }
        let i = usize::from(self.bplcon3 >> 13 & 7) * 32 + (n & 31);
        let low = expand12(v & 0xfff) & 0x0f0f0f;
        if self.bplcon3 & 0x200 != 0 {
            self.colors[i] = (self.colors[i] & 0xf0f0f0) | low;
        } else {
            self.colors[i] = low * 0x11;
        }
    }

    /// Colour register `n` as the 12-bit value its upper nibbles form.
    pub fn color12(&self, n: usize) -> u16 {
        let c = self.colors[n & 255];
        ((c >> 12) & 0xf00 | (c >> 8) & 0x0f0 | (c >> 4) & 0x00f) as u16
    }

    /// Collision bits for one pixel: playfield `index`, sprite values (see
    /// the module doc for how the playfield bits follow the hardware).
    fn collisions(&self, index: u8, vals: &[u8; 8]) -> u16 {
        let mut enable = (self.clxcon >> 6) as u8 & 0x3f;
        let mut matchv = self.clxcon as u8 & 0x3f;
        if self.aga {
            enable |= self.clxcon2 as u8 & 0xc0;
            matchv |= (self.clxcon2 as u8 & 3) << 6;
        }
        let matches = |planes: u8| (index ^ matchv) & enable & planes == 0;
        let odd = matches(0b0101_0101);
        let even = matches(0b1010_1010);
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

    /// SPRxPOS (0), SPRxCTL (1), SPRxDATA (2), SPRxDATB (3) written with a
    /// 16-bit value (CPU or Copper).
    pub fn write_sprite(&mut self, n: usize, reg: usize, v: u16) {
        self.write_sprite_wide(n, reg, u64::from(v) << 48);
    }

    /// A sprite register loaded by DMA: POS/CTL from the top 16 bits,
    /// DATA/DATB left-aligned (16, 32 or 64 bits as FMODE says).
    pub fn write_sprite_wide(&mut self, n: usize, reg: usize, v: u64) {
        let s = &mut self.sprites[n];
        match reg {
            0 => s.pos = (v >> 48) as u16,
            1 => {
                s.ctl = (v >> 48) as u16;
                s.armed = false;
            }
            2 => {
                s.data = v;
                s.armed = true;
            }
            _ => s.datb = v,
        }
    }

    /// Sprite width in sprite pixels (FMODE bits 3–2 on the AGA).
    fn sprite_width(&self) -> u16 {
        if !self.aga {
            return 16;
        }
        match (self.fmode >> 2) & 3 {
            0 => 16,
            3 => 64,
            _ => 32,
        }
    }

    /// Output pixels per sprite pixel (BPLCON3 SPRES on the AGA).
    fn sprite_step(&self) -> u8 {
        if self.aga && (self.bplcon3 >> 6) & 3 >= 2 {
            1
        } else {
            2
        }
    }

    /// The frontmost sprite colour at this pixel (pair, colour register)
    /// and every sprite's value.
    fn sprite_pixel(&mut self, x: u32) -> (Option<(u8, u8)>, [u8; 8]) {
        let width = self.sprite_width();
        let step = self.sprite_step();
        let aga = self.aga;
        let mut vals = [0u8; 8];
        for (n, s) in self.sprites.iter_mut().enumerate() {
            // A sprite's first pixel is one low-resolution pixel right of
            // HSTART (WinUAE screenshot of Interference's tunnel frame,
            // `docs/sources.md`: HSTART $84 shows one pixel inside a window
            // starting at 148, HSTART $1AE one pixel inside one ending at 432).
            let start = if aga {
                s.hstart_shres() / 2 + 2
            } else {
                2 * (s.hstart() + 1)
            };
            if s.armed && x == start {
                s.a = s.data;
                s.b = s.datb;
                s.count = width * u16::from(step);
                s.step = step;
            }
            if s.count > 0 {
                vals[n] = (s.a >> 63) as u8 | ((s.b >> 63) as u8) << 1;
                s.count -= 1;
                if s.count % u16::from(s.step.max(1)) == 0 {
                    s.a <<= 1;
                    s.b <<= 1;
                }
            }
        }
        let bank = |n: usize| -> u8 {
            if !aga {
                return 16;
            }
            let b = if n & 1 == 0 {
                (self.bplcon4 >> 4) & 15
            } else {
                self.bplcon4 & 15
            };
            (b << 4) as u8
        };
        let front = (0..4u8).find_map(|pair| {
            let (even, odd) = (vals[2 * pair as usize], vals[2 * pair as usize + 1]);
            if self.sprites[2 * pair as usize + 1].ctl & 0x80 != 0 {
                let v = odd << 2 | even;
                (v != 0).then_some((pair, bank(2 * pair as usize + 1) + v))
            } else if even != 0 {
                Some((pair, bank(2 * pair as usize) + 4 * pair + even))
            } else if odd != 0 {
                Some((pair, bank(2 * pair as usize + 1) + 4 * pair + odd))
            } else {
                None
            }
        });
        (front, vals)
    }

    pub fn hires(&self) -> bool {
        self.bplcon0 & 0x8000 != 0
    }

    /// Super-hires (ECS Denise, BPLCON0 SHRES).
    pub fn shres(&self) -> bool {
        self.ecs && self.bplcon0 & 0x0040 != 0
    }

    /// Planes enabled in BPLCON0 (7 counts as none on the OCS; super-hires
    /// has at most two on the ECS Denise; the AGA has BPU3 for eight).
    pub fn planes(&self) -> usize {
        let n = usize::from((self.bplcon0 >> 12) & 7);
        if self.aga {
            let n = n | usize::from(self.bplcon0 & 0x10) >> 1;
            return n.min(8);
        }
        let n = if n == 7 { 0 } else { n };
        if self.shres() {
            n.min(2)
        } else {
            n
        }
    }

    /// The BPLCON3 bits ECSENA enables (ECS Denise).
    fn con3(&self) -> u16 {
        if self.ecs && self.bplcon0 & 1 != 0 {
            self.bplcon3
        } else {
            0
        }
    }

    /// Horizontal display window in low-resolution pixels: DIWSTRT bit 8
    /// is 0, DIWSTOP bit 8 is 1 unless DIWHIGH says otherwise.
    fn window(&self) -> (u32, u32) {
        (
            u32::from(self.diwstrt & 0xff) | u32::from(self.diwhigh & 0x20) << 3,
            u32::from(self.diwstop & 0xff) | u32::from(self.diwhigh & 0x2000) >> 5,
        )
    }

    /// The scroll delays of the odd and even planes in high-resolution
    /// pixels (BPLCON1; the AGA's extra bits).
    fn scroll(&self) -> (u32, u32) {
        let c = u32::from(self.bplcon1);
        let odd = (c & 15) << 1;
        let even = (c >> 4 & 15) << 1;
        if !self.aga {
            return (odd, even);
        }
        (
            odd | (c >> 9 & 1) | (c >> 10 & 3) << 5,
            even | (c >> 13 & 1) | (c >> 14 & 3) << 5,
        )
    }

    /// Agnus fetched plane `plane` in slot `h` (Denise line colour clock):
    /// `v` left-aligned in 64 bits (16 bits from a CPU or Copper write).
    pub fn write_bpldat(&mut self, plane: usize, v: u64, h: u32) {
        self.bpldat[plane] = v;
        if plane == 0 {
            let base = 2 * (2 * h + LOAD_DELAY);
            let (odd, even) = self.scroll();
            let d = self.bpldat;
            self.queue(
                0,
                Load {
                    at: base + odd,
                    data: [d[0], d[2], d[4], d[6]],
                },
            );
            self.queue(
                1,
                Load {
                    at: base + even,
                    data: [d[1], d[3], d[5], d[7]],
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
    /// coordinates) as 24-bit colours.
    pub fn output(&mut self, h: u32, out: &mut [u32; 4]) {
        let (start, stop) = self.window();
        let hires = self.hires();
        let shres = self.shres();
        let planes = self.planes();
        let con3 = self.con3();
        let border_blank = con3 & 0x20 != 0;
        let border_sprites = con3 & 0x02 != 0;
        let sprites_live = self.sprites.iter().any(|s| s.armed || s.count > 0);
        let xor = if self.aga {
            (self.bplcon4 >> 8) as u8
        } else {
            0
        };
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
                if self.shift[p] & (1 << 63) != 0 {
                    index |= 1 << p;
                }
            }
            for g in 0..2 {
                self.phase[g] = self.phase[g].wrapping_add(1);
                if shres {
                    // Two 35 ns pixels per column; the second is dropped.
                    for p in (g..8).step_by(2) {
                        self.shift[p] <<= 2;
                    }
                } else if hires || self.phase[g].is_multiple_of(2) {
                    for p in (g..8).step_by(2) {
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
                self.color(index ^ xor, planes)
            } else if border_blank {
                (0, None)
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
            let sprite = sprite.filter(|_| (inside && self.armed) || border_sprites);
            if inside && self.armed {
                if vals != [0; 8] {
                    self.clxdat |= self.collisions(index, &vals);
                } else if self.clxdat & 1 == 0 {
                    // Without sprites only the playfield bit can come.
                    self.clxdat |= self.collisions(index, &vals) & 1;
                }
            }
            let c = match (sprite, front) {
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
            *px = if shres && !self.aga {
                // Two bits per gun: the upper ones, spread over the byte.
                let hi = c & 0xc0c0c0;
                hi | hi >> 2 | hi >> 4 | hi >> 6
            } else {
                c
            };
        }
    }

    /// Playfield 2's colour offset in dual playfield mode (AGA: BPLCON3
    /// PF2OF; otherwise 8).
    fn pf2_offset(&self) -> u8 {
        if !self.aga {
            return 8;
        }
        match (self.bplcon3 >> 10) & 7 {
            0 => 0,
            n => 1 << n,
        }
    }

    /// The playfield colour of a pixel and the playfield (1 or 2) it comes
    /// from, `None` where both are transparent.
    fn color(&mut self, index: u8, planes: usize) -> (u32, Option<u8>) {
        let ham = self.bplcon0 & 0x0800 != 0;
        let dual = self.bplcon0 & 0x0400 != 0;
        let single = (index != 0).then_some(2);
        if ham && planes >= 5 {
            let ham8 = self.aga && planes >= 7;
            let (control, data) = if ham8 {
                // HAM8: planes 1–2 control, planes 3–8 the upper six bits.
                (index & 3, u32::from(index >> 2))
            } else {
                (index >> 4 & 3, u32::from(index & 15))
            };
            let gun = |shift: u32, old: u32| -> u32 {
                let v = if ham8 {
                    data << 2 | (old >> shift) & 3
                } else {
                    data * 0x11
                };
                (old & !(0xff << shift)) | v << shift
            };
            let c = match control {
                0 => self.colors[data as usize],
                1 => gun(0, self.ham),
                2 => gun(16, self.ham),
                _ => gun(8, self.ham),
            };
            self.ham = c;
            return (c, single);
        }
        if dual {
            let pf1 = (index & 1) | (index >> 1 & 2) | (index >> 2 & 4) | (index >> 3 & 8);
            let pf2 = (index >> 1 & 1) | (index >> 2 & 2) | (index >> 3 & 4) | (index >> 4 & 8);
            let pf2_front = self.bplcon2 & 0x40 != 0;
            let off = self.pf2_offset();
            let (c, front) = match (pf1 != 0, pf2 != 0) {
                (false, false) => (0, None),
                (true, false) => (pf1, Some(1)),
                (false, true) => (off.wrapping_add(pf2), Some(2)),
                (true, true) if pf2_front => (off.wrapping_add(pf2), Some(2)),
                (true, true) => (pf1, Some(1)),
            };
            return (self.colors[usize::from(c)], front);
        }
        let kill_ehb = self.aga && self.bplcon2 & 0x200 != 0;
        if planes == 6 && !kill_ehb {
            let c = self.colors[usize::from(index & 31)];
            if index & 32 != 0 {
                // Extra half-brite.
                return ((c >> 1) & 0x7f7f7f, single);
            }
            return (c, single);
        }
        let index = if self.aga { index } else { index & 31 };
        (self.colors[usize::from(index)], single)
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        for c in self.colors {
            w.u32(c);
        }
        for v in [
            self.bplcon0,
            self.bplcon1,
            self.bplcon2,
            self.diwstrt,
            self.diwstop,
            self.bplcon3,
            self.diwhigh,
            self.bplcon4,
            self.fmode,
            self.clxcon2,
        ] {
            w.u16(v);
        }
        w.u32(self.ham);
        for v in self.bpldat.iter().chain(&self.shift) {
            w.u64(*v);
        }
        for p in self.pending.iter().flatten() {
            w.bool(p.is_some());
            let l = p.unwrap_or_default();
            w.u32(l.at);
            for v in l.data {
                w.u64(v);
            }
        }
        w.raw(&self.phase);
        w.bool(self.armed);
        w.u16(self.clxcon);
        w.u16(self.clxdat);
        for s in &self.sprites {
            w.u16(s.pos);
            w.u16(s.ctl);
            for v in [s.data, s.datb, s.a, s.b] {
                w.u64(v);
            }
            w.bool(s.armed);
            w.u16(s.count);
            w.u8(s.step);
        }
    }

    /// `version` is the OCS chunk's: version 2 saved one pending load per
    /// group, version 3 two, 4–7 four 16-bit loads for six planes with
    /// 32 colours of 12 bits; 7 added BPLCON3 and DIWHIGH; 8 is the AGA
    /// layout (256 colours of 24 bits, eight planes of 64 bits).
    pub(crate) fn load(r: &mut StateReader<'_>, version: u16) -> Result<Self, StateError> {
        if version >= 8 {
            return Self::load_v8(r);
        }
        let mut d = Self::new();
        for c in &mut d.colors[..32] {
            *c = expand12(r.u16()? & 0x0fff);
        }
        let mut ham12 = 0;
        for v in [
            &mut d.bplcon0,
            &mut d.bplcon1,
            &mut d.bplcon2,
            &mut d.diwstrt,
            &mut d.diwstop,
            &mut ham12,
        ] {
            *v = r.u16()?;
        }
        d.ham = expand12(ham12 & 0xfff);
        if version >= 7 {
            d.bplcon3 = r.u16()?;
            d.diwhigh = r.u16()?;
        }
        for v in d.bpldat.iter_mut().take(6) {
            *v = u64::from(r.u16()?) << 48;
        }
        for v in d.shift.iter_mut().take(6) {
            *v = u64::from(r.u16()?) << 48;
        }
        let depth = match version {
            0..=2 => 1,
            3 => 2,
            _ => 4,
        };
        for q in &mut d.pending {
            for p in q.iter_mut().take(depth) {
                let some = r.bool()?;
                let mut l = Load {
                    at: r.u32()?,
                    data: [0; 4],
                };
                for v in l.data.iter_mut().take(3) {
                    *v = u64::from(r.u16()?) << 48;
                }
                *p = some.then_some(l);
            }
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
            s.pos = r.u16()?;
            s.ctl = r.u16()?;
            s.data = u64::from(r.u16()?) << 48;
            s.datb = u64::from(r.u16()?) << 48;
            s.a = u64::from(r.u16()?) << 48;
            s.b = u64::from(r.u16()?) << 48;
            s.armed = r.bool()?;
            s.count = u16::from(r.u8()?.min(32));
            s.step = 2;
        }
        Ok(d)
    }

    fn load_v8(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut d = Self::new();
        for c in &mut d.colors {
            *c = r.u32()? & 0xff_ffff;
        }
        for v in [
            &mut d.bplcon0,
            &mut d.bplcon1,
            &mut d.bplcon2,
            &mut d.diwstrt,
            &mut d.diwstop,
            &mut d.bplcon3,
            &mut d.diwhigh,
            &mut d.bplcon4,
            &mut d.fmode,
            &mut d.clxcon2,
        ] {
            *v = r.u16()?;
        }
        d.ham = r.u32()? & 0xff_ffff;
        for v in d.bpldat.iter_mut().chain(d.shift.iter_mut()) {
            *v = r.u64()?;
        }
        for q in &mut d.pending {
            for p in q.iter_mut() {
                let some = r.bool()?;
                let mut l = Load {
                    at: r.u32()?,
                    data: [0; 4],
                };
                for v in &mut l.data {
                    *v = r.u64()?;
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
            s.pos = r.u16()?;
            s.ctl = r.u16()?;
            for v in [&mut s.data, &mut s.datb, &mut s.a, &mut s.b] {
                *v = r.u64()?;
            }
            s.armed = r.bool()?;
            s.count = r.u16()?.min(128);
            s.step = r.u8()?.min(2);
        }
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(d: &mut Denise, from: u32, to: u32) -> Vec<u32> {
        let mut px = Vec::new();
        for h in from..to {
            let mut out = [0; 4];
            d.output(h, &mut out);
            px.extend(out);
        }
        px
    }

    /// A 16-bit plane word as the fetch delivers it.
    fn w(v: u16) -> u64 {
        u64::from(v) << 48
    }

    const WHITE: u32 = 0xff_ffff;
    const RED: u32 = 0xff_0000;

    #[test]
    fn lowres_plane_appears_at_the_window_start() {
        let mut d = Denise::new();
        d.write_color(0, 0x000);
        d.write_color(1, 0xfff);
        d.bplcon0 = 0x1200;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        d.write_bpldat(0, w(0xc000), 0x3f);
        let px = line(&mut d, 0x3f, 0x44);
        // Hires pixel 2·$81 = $102 is index $102 − 4·$3F = 6.
        assert_eq!(&px[..6], &[0; 6]);
        assert_eq!(&px[6..10], &[WHITE; 4], "two low-resolution pixels");
        assert_eq!(px[10], 0);
    }

    #[test]
    fn scroll_delays_odd_planes() {
        let mut d = Denise::new();
        d.write_color(1, 0xf00);
        d.bplcon0 = 0x1200;
        d.bplcon1 = 0x0001;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        d.write_bpldat(0, w(0x8000), 0x3f);
        let px = line(&mut d, 0x3f, 0x44);
        assert_eq!(&px[6..10], &[0, 0, RED, RED]);
    }

    #[test]
    fn a_large_scroll_keeps_every_word() {
        // BPL1DAT every 8 colour clocks; with PF1H = 15 each load waits 36
        // high-resolution pixels, past the next fetch (Desert Dream's moon
        // sliding in from the right).
        let mut d = Denise::new();
        d.write_color(1, 0xf00);
        d.bplcon0 = 0x1200;
        d.bplcon1 = 0x000f;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        let mut px = Vec::new();
        for n in 0..3u32 {
            d.write_bpldat(0, w(0x8000 >> n), 0x3f + 8 * n);
            px.extend(line(&mut d, 0x3f + 8 * n, 0x47 + 8 * n));
        }
        px.extend(line(&mut d, 0x57, 0x5f));
        // Word n shows at hires pixel 4·$3F + 6 + 30 + 32n (+ 2n for its bit).
        for n in 0..3usize {
            let at = 36 + 34 * n;
            assert_eq!(&px[at..at + 2], &[RED; 2], "word {n}");
            assert_eq!(px[at - 1], 0, "word {n} start");
        }
    }

    #[test]
    fn a_large_hires_scroll_keeps_every_word() {
        // High resolution: BPL1DAT every 4 colour clocks (16 pixels); with
        // PF1H = 15 each load waits 36 pixels, so three are pending at once
        // (Desert Dream's end scroller).
        let mut d = Denise::new();
        d.write_color(1, 0xf00);
        d.bplcon0 = 0x9200;
        d.bplcon1 = 0x000f;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.start_line();
        let mut px = Vec::new();
        for n in 0..4u32 {
            d.write_bpldat(0, w(0x8000 >> n), 0x3f + 4 * n);
            px.extend(line(&mut d, 0x3f + 4 * n, 0x43 + 4 * n));
        }
        px.extend(line(&mut d, 0x4f, 0x5f));
        // Word n shows from pixel 4·$3F + 6 + 30 + 16n; its bit n one pixel
        // per bit later.
        for n in 0..4usize {
            let at = 36 + 17 * n;
            assert_eq!(px[at], RED, "word {n}");
            assert_eq!(px[at - 1], 0, "word {n} start");
            assert_eq!(px[at + 1], 0, "word {n} is one hires pixel");
        }
    }

    #[test]
    fn half_brite_and_ham() {
        let mut d = Denise::new();
        d.write_color(3, 0xeca);
        d.bplcon0 = 0x6000;
        assert_eq!(d.color(32 | 3, 6).0, 0x776655);
        d.bplcon0 = 0x6800;
        d.ham = 0x112233;
        assert_eq!(d.color(0x20 | 0xf, 6).0, 0xff2233, "modify red");
        assert_eq!(d.color(0x30 | 0x7, 6).0, 0xff7733, "modify green");
        assert_eq!(d.color(0x3, 6).0, 0xeeccaa, "palette");
    }

    #[test]
    fn dual_playfield_priority() {
        let mut d = Denise::new();
        d.write_color(1, 0x100);
        d.write_color(9, 0x900);
        d.bplcon0 = 0x2400;
        assert_eq!(d.color(0b11, 2), (0x110000, Some(1)), "PF1 in front");
        d.bplcon2 = 0x40;
        assert_eq!(d.color(0b11, 2), (0x990000, Some(2)), "PF2PRI");
        assert_eq!(d.color(0b10, 2).0, 0x990000);
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
        d.write_color(0, 0x000);
        d.write_color(1, 0x00f);
        d.write_color(17, 0xf00);
        d.write_color(21, 0x0f0);
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
            &[0, RED, RED, 0x00ff00, 0x00ff00, 0x00ff00, 0x00ff00, 0]
        );
        // A playfield in front of pair 1 (PF2P = 1) hides sprite 2 only.
        d.bplcon0 = 0x1200;
        d.bplcon2 = 1 << 3;
        d.start_line();
        d.write_bpldat(0, w(0xffff), 0x4c);
        let px = line(&mut d, 0x4f, 0x53);
        assert_eq!(&px[6..11], &[RED, RED, 0x0000ff, 0x0000ff, 0x0000ff]);
    }

    #[test]
    fn ecs_border_blank_and_border_sprites_need_ecsena() {
        let mut d = Denise::new();
        d.ecs = true;
        d.write_color(0, 0x123);
        d.write_color(17, 0xf00);
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.bplcon0 = 0x0200;
        d.bplcon3 = 0x22;
        d.write_sprite(0, 0, 0x2c50);
        d.write_sprite(0, 1, 0x2d00);
        d.write_sprite(0, 3, 0);
        d.write_sprite(0, 2, 0x8000);
        d.start_line();
        let px = line(&mut d, 0x4f, 0x53);
        assert_eq!(px, vec![0x112233; 16], "ECSENA off: COLOR00, no sprite");
        d.bplcon0 = 0x0201;
        d.write_sprite(0, 2, 0x8000);
        d.start_line();
        let px = line(&mut d, 0x4f, 0x53);
        assert_eq!(&px[..6], &[0; 6], "border blank");
        assert_eq!(&px[6..8], &[RED; 2], "sprite in the border");
        assert_eq!(px[8], 0);
    }

    #[test]
    fn diwhigh_moves_the_window_edges_past_256() {
        let mut d = Denise::new();
        d.ecs = true;
        assert_eq!(d.window(), (0, 0x100));
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2c01;
        d.diwhigh = 0x2020;
        assert_eq!(d.window(), (0x181, 0x101));
        d.diwhigh = 0x0000;
        assert_eq!(d.window(), (0x81, 0x01));
    }

    #[test]
    fn super_hires_shows_two_planes_with_two_bits_per_gun() {
        let mut d = Denise::new();
        d.ecs = true;
        d.write_color(1, 0xf00);
        d.write_color(3, 0x7b5);
        d.bplcon0 = 0x2040;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        assert_eq!(d.planes(), 2);
        d.bplcon0 = 0x4040;
        assert_eq!(d.planes(), 2, "capped at two");
        d.bplcon0 = 0x2040;
        d.start_line();
        d.write_bpldat(1, w(0x3000), 0x3f);
        d.write_bpldat(0, w(0xf000), 0x3f);
        let px = line(&mut d, 0x3f, 0x44);
        // Hires pixel 2·$81 = $102 is index 6; every second 35 ns pixel is
        // dropped, so the four bits show as two columns.
        assert_eq!(&px[6..9], &[RED, 0x55aa55, 0]);
    }

    #[test]
    fn aga_palette_banks_and_low_nibbles() {
        let mut d = Denise::new();
        d.set_aga(true);
        assert_eq!(d.bplcon3, 0x0c00);
        d.write_color(1, 0x123);
        assert_eq!(d.colors[1], 0x112233);
        d.bplcon3 = 0x0c00 | 0x200;
        d.write_color(1, 0x456);
        assert_eq!(d.colors[1], 0x142536, "low nibbles");
        d.bplcon3 = 0x0c00 | 0x6000;
        d.write_color(2, 0xabc);
        assert_eq!(d.colors[3 * 32 + 2], 0xaabbcc, "bank 3");
        assert_eq!(d.color12(3 * 32 + 2), 0xabc);
        assert_eq!(d.color12(1), 0x123);
    }

    #[test]
    fn aga_eight_planes_ham8_and_dual_playfield_offsets() {
        let mut d = Denise::new();
        d.set_aga(true);
        d.bplcon0 = 0x0210;
        assert_eq!(d.planes(), 8);
        d.bplcon0 = 0x7200;
        assert_eq!(d.planes(), 7);
        d.colors[0xa5] = 0x102030;
        assert_eq!(d.color(0xa5, 8).0, 0x102030, "256 colours");
        // HAM8: planes 1-2 control, 3-8 data into the upper six bits.
        d.bplcon0 = 0x0a10;
        d.ham = 0x112233;
        assert_eq!(
            d.color(0b1111_1101, 8).0,
            0x1122ff,
            "blue := 111111 + old 11"
        );
        assert_eq!(
            d.color(0b0000_0010, 8).0,
            0x0122ff,
            "red := 000000 + old 01"
        );
        assert_eq!(
            d.color(0b1010_1011, 8).0,
            0x01aaff,
            "green := 101010 + old 10"
        );
        // Dual playfield with PF2OF = 64.
        d.bplcon0 = 0x0410;
        d.bplcon3 = 6 << 10;
        d.colors[64 + 3] = 0x445566;
        assert_eq!(d.color(0b1010, 8), (0x445566, Some(2)));
        // EHB off with KILLEHB.
        d.bplcon0 = 0x6000;
        d.bplcon3 = 0x0c00;
        d.colors[3] = 0xeeccaa;
        d.colors[35] = 0x010203;
        assert_eq!(d.color(35, 6).0, 0x776655, "EHB");
        d.bplcon2 = 0x200;
        assert_eq!(d.color(35, 6).0, 0x010203, "KILLEHB: colour 35");
        // BPLAM XORs the index.
        d.bplcon2 = 0;
        d.bplcon4 = 0x0100 | 0x11;
        d.bplcon0 = 0x1200;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.colors[1] = 0xff0000;
        d.colors[0] = 0;
        d.start_line();
        d.write_bpldat(0, 0, 0x3f);
        let px = line(&mut d, 0x3f, 0x44);
        assert_eq!(px[6], 0xff0000, "empty plane data shows colour 1");
    }

    #[test]
    fn aga_wide_sprites_and_colour_banks() {
        let mut d = Denise::new();
        d.set_aga(true);
        d.colors[0x21] = 0x00ff00;
        d.colors[0x11] = 0xff0000;
        d.diwstrt = 0x2c81;
        d.diwstop = 0x2cc1;
        d.bplcon0 = 0x1200;
        // Even sprites from bank 2: colour $21 for value 1.
        d.bplcon4 = 0x0021;
        d.fmode = 0x000c;
        d.write_sprite(0, 0, 0x2c50);
        d.write_sprite(0, 1, 0x2d00);
        d.write_sprite_wide(0, 3, 0);
        d.write_sprite_wide(0, 2, 0x8000_0000_0000_0001);
        d.start_line();
        d.write_bpldat(0, 0, 0x40);
        let px = line(&mut d, 0x4f, 0x53 + 32);
        assert_eq!(&px[5..9], &[0, 0x00ff00, 0x00ff00, 0]);
        // 64 pixels wide: bit 0 shows 126 hires pixels after bit 63.
        assert_eq!(&px[6 + 126..6 + 129], &[0x00ff00, 0x00ff00, 0]);
        // Hires sprites (SPRES = 2): one output pixel per sprite pixel.
        d.bplcon3 = 0x0c00 | 0x80;
        d.fmode = 0;
        d.write_sprite(0, 2, 0xc000);
        d.start_line();
        d.write_bpldat(0, 0, 0x40);
        let px = line(&mut d, 0x4f, 0x53);
        assert_eq!(&px[5..9], &[0, 0x00ff00, 0x00ff00, 0]);
        assert_eq!(d.color12(0x21), 0x0f0);
    }
}
