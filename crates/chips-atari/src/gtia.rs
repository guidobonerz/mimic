//! GTIA (CO14805): colours, players/missiles, priority, collisions and the
//! video output of the Atari 8-bit computers. It turns ANTIC's playfield
//! value per colour clock into a colour register value per hires pixel.
//!
//! Registers (`$D000`, mirrored every 32 bytes up to `$D0FF`):
//!
//! | Reg     | Write                | Read                         |
//! | ------- | -------------------- | ---------------------------- |
//! | 00–03   | HPOSP0–3             | M0PF–M3PF (missile/playfield)|
//! | 04–07   | HPOSM0–3             | P0PF–P3PF                    |
//! | 08–0B   | SIZEP0–3             | M0PL–M3PL                    |
//! | 0C      | SIZEM                | P0PL (0C–0F: P0PL–P3PL)      |
//! | 0D–10   | GRAFP0–3             | 10–13: TRIG0–3               |
//! | 11      | GRAFM                | –                            |
//! | 12–15   | COLPM0–3             | 14: PAL                      |
//! | 16–19   | COLPF0–3             | –                            |
//! | 1A      | COLBK                | –                            |
//! | 1B      | PRIOR                | –                            |
//! | 1C      | VDELAY               | –                            |
//! | 1D      | GRACTL               | –                            |
//! | 1E      | HITCLR               | –                            |
//! | 1F      | CONSOL (bit 3 speaker) | CONSOL (0–2 START/SELECT/OPTION, 0 = down) |
//!
//! Colours: bits 7–4 hue, 3–1 luminance (bit 0 only counts in GTIA mode 9).
//! PRIOR: bits 0–3 priority select, 4 fifth player (missiles in PF3's
//! colour), 5 multicolour players, 6–7 GTIA modes 9/10/11.
//!
//! Priority logic after the GTIA data sheet's equations (as reproduced in
//! the Altirra Hardware Reference Manual): each object is enabled or
//! suppressed by the priority bits, and the colours of all enabled objects
//! are ORed — conflicting PRIOR settings give the mixed colours real
//! machines show.

use emu_core::frame::VideoStandard;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

use crate::antic::{Playfield, FIRST_LINE, VBLANK_LINE};

/// First colour clock of the output (the left edge of a wide playfield).
pub const FIRST_CC: u16 = 32;
/// Output size in hires pixels: colour clocks 32–223, lines 8–247.
pub const WIDTH: u32 = 384;
pub const HEIGHT: u32 = (VBLANK_LINE - FIRST_LINE) as u32;

const LAST_CC: u16 = FIRST_CC + (WIDTH as u16) / 2;
/// Colour clocks until the player logic sees a SIZEP write: from the third
/// colour clock after the write's cycle (Acid800 `gtia_pmresize`).
const SIZE_DELAY: u8 = 4;
/// The same for HPOSP, whose comparator acts on the fifth (Acid800
/// `gtia_pmoverlap`, `gtia_pmretrigger`).
const HPOS_DELAY: u8 = 6;
/// Colour clocks outside horizontal blank, $22–$DD (Acid800
/// `gtia_collision`: objects collide there and only there).
const VISIBLE_CC: (u16, u16) = (0x22, 0xdd);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gtia {
    standard: VideoStandard,
    pub hposp: [u8; 4],
    pub hposm: [u8; 4],
    pub sizep: [u8; 4],
    pub sizem: u8,
    pub grafp: [u8; 4],
    pub grafm: u8,
    pub colpm: [u8; 4],
    pub colpf: [u8; 4],
    pub colbk: u8,
    pub prior: u8,
    pub vdelay: u8,
    pub gractl: u8,
    pub consol_out: u8,
    /// Collision latches (4 bits each).
    m_pf: [u8; 4],
    p_pf: [u8; 4],
    m_pl: [u8; 4],
    p_pl: [u8; 4],
    /// Joystick triggers held down (TRIG0–3), and latched presses.
    trig_down: [bool; 4],
    trig_latched: [bool; 4],
    /// XL/XE: TRIG3 reads the cartridge's RD5 line (1 = cartridge present).
    cartridge: bool,
    /// False on the 400/800, where TRIG3 is joystick port 4's button.
    trig3_is_cartridge: bool,
    /// The hires bits of the previous cycle (GTIA mode 10's delay).
    prev_nibble: u8,
    /// Player shift registers, width counters, the HPOSP and SIZEP values
    /// the player logic uses and colour clocks until written ones apply,
    /// and the players showing at the current colour clock.
    p_shift: [u8; 4],
    p_ctr: [u8; 4],
    hpos_eff: [u8; 4],
    hpos_wait: [u8; 4],
    size_eff: [u8; 4],
    size_wait: [u8; 4],
    p_out: u8,
    /// Console keys held down: bit 0 START, 1 SELECT, 2 OPTION.
    pub console_keys: u8,
    pixels: Vec<u8>,
}

/// One colour clock of a player's width logic: whether the shift register
/// shifts, and the width counter's next value. Fitted to Acid800
/// `gtia_pmresize`, which changes SIZEP while a player is shown: 1x shifts
/// every clock; 2x on odd counts; 4x at count 3; size 10 ("1x alt") shifts
/// at counts 0 and 3 and holds the counter, so a player switched to it at
/// count 1 or 2 stops shifting.
fn width_step(size: u8, ctr: u8) -> (bool, u8) {
    match size & 3 {
        0 => (true, 0),
        1 => (ctr & 1 != 0, (ctr & 1) ^ 1),
        2 => (ctr == 0 || ctr == 3, ctr),
        _ => (ctr == 3, (ctr + 1) & 3),
    }
}

/// Width multiplier of a 2-bit size field (00 normal, 01 double,
/// 10 normal, 11 quadruple).
fn size_mult(size: u8) -> u16 {
    match size & 3 {
        1 => 2,
        3 => 4,
        _ => 1,
    }
}

impl Gtia {
    pub fn new(standard: VideoStandard) -> Self {
        Self {
            standard,
            hposp: [0; 4],
            hposm: [0; 4],
            sizep: [0; 4],
            sizem: 0,
            grafp: [0; 4],
            grafm: 0,
            colpm: [0; 4],
            colpf: [0; 4],
            colbk: 0,
            prior: 0,
            vdelay: 0,
            gractl: 0,
            consol_out: 0,
            m_pf: [0; 4],
            p_pf: [0; 4],
            m_pl: [0; 4],
            p_pl: [0; 4],
            trig_down: [false; 4],
            trig_latched: [false; 4],
            cartridge: false,
            trig3_is_cartridge: true,
            prev_nibble: 0,
            p_shift: [0; 4],
            p_ctr: [0; 4],
            hpos_eff: [0; 4],
            hpos_wait: [0; 4],
            size_eff: [0; 4],
            size_wait: [0; 4],
            p_out: 0,
            console_keys: 0,
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
        }
    }

    pub fn standard(&self) -> VideoStandard {
        self.standard
    }

    /// Colour register values per hires pixel, `WIDTH × HEIGHT`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The console speaker (CONSOL bit 3; the XL keyboard click).
    pub fn speaker(&self) -> bool {
        self.consol_out & 0x08 != 0
    }

    /// The XL/XE cartridge's RD5 line, read as TRIG3.
    pub fn set_cartridge(&mut self, rd5: bool) {
        self.cartridge = rd5;
    }

    /// 400/800: TRIG3 is a joystick button, not the cartridge sense.
    pub fn set_trig3_is_cartridge(&mut self, on: bool) {
        self.trig3_is_cartridge = on;
    }

    pub fn set_trigger(&mut self, n: usize, down: bool) {
        if let Some(t) = self.trig_down.get_mut(n) {
            *t = down;
            if down && self.gractl & 0x04 != 0 {
                self.trig_latched[n] = true;
            }
        }
    }

    pub fn peek(&self, reg: u8) -> u8 {
        let r = usize::from(reg & 0x1f);
        match r {
            0x00..=0x03 => self.m_pf[r],
            0x04..=0x07 => self.p_pf[r - 4],
            0x08..=0x0b => self.m_pl[r - 8],
            0x0c..=0x0f => self.p_pl[r - 0x0c],
            0x13 if self.trig3_is_cartridge => u8::from(self.cartridge),
            0x10..=0x13 => {
                let n = r - 0x10;
                u8::from(!(self.trig_down[n] || self.trig_latched[n]))
            }
            // PAL: bits 1–3 are 0 on PAL, 1 on NTSC machines.
            0x14 => match self.standard {
                VideoStandard::Pal => 0x01,
                VideoStandard::Ntsc => 0x0f,
            },
            // A 1 written to bits 0–2 pulls that key line low like a
            // pressed key (Acid800 `gtia_consol`).
            0x1f => (!(self.console_keys | self.consol_out) & 0x07) | 0x08,
            _ => 0x0f,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        let r = usize::from(reg & 0x1f);
        match r {
            0x00..=0x03 => {
                self.hposp[r] = v;
                self.hpos_wait[r] = HPOS_DELAY;
            }
            0x04..=0x07 => self.hposm[r - 4] = v,
            0x08..=0x0b => {
                self.sizep[r - 8] = v & 3;
                self.size_wait[r - 8] = SIZE_DELAY;
            }
            0x0c => self.sizem = v,
            0x0d..=0x10 => self.grafp[r - 0x0d] = v,
            0x11 => self.grafm = v,
            0x12..=0x15 => self.colpm[r - 0x12] = v & 0xfe,
            0x16..=0x19 => self.colpf[r - 0x16] = v & 0xfe,
            0x1a => self.colbk = v & 0xfe,
            0x1b => self.prior = v,
            0x1c => self.vdelay = v,
            0x1d => {
                self.gractl = v & 0x07;
                if v & 0x04 == 0 {
                    self.trig_latched = [false; 4];
                }
            }
            0x1e => {
                self.m_pf = [0; 4];
                self.p_pf = [0; 4];
                self.m_pl = [0; 4];
                self.p_pl = [0; 4];
            }
            0x1f => self.consol_out = v & 0x0f,
            _ => {}
        }
    }

    /// Player/missile DMA data from ANTIC (`which` 0–3 players, 4 missiles),
    /// taken if GRACTL enables it. VDELAY: a delayed object only loads on
    /// odd lines in double-line resolution (Acid800 `gtia_vdelay`).
    pub fn dma(&mut self, which: u8, v: u8, line: u16, single_line: bool) {
        if which == 4 {
            if self.gractl & 0x01 != 0 {
                let mut m = self.grafm;
                for n in 0..4 {
                    let delayed = self.vdelay & (1 << n) != 0 && !single_line && line & 1 == 0;
                    if !delayed {
                        let mask = 3 << (2 * n);
                        m = (m & !mask) | (v & mask);
                    }
                }
                self.grafm = m;
            }
        } else if self.gractl & 0x02 != 0 {
            let n = usize::from(which);
            let delayed = self.vdelay & (0x10 << n) != 0 && !single_line && line & 1 == 0;
            if !delayed {
                self.grafp[n] = v;
            }
        }
    }

    /// Advance the players by one colour clock. A player starts when the
    /// colour clock reaches its HPOSP: GRAFP is ORed into the shift
    /// register and the width counter restarts. An image still being
    /// shifted out goes on, overlapping the new one; caught in the middle
    /// of a wide pixel (counter not 0) it first shifts once (fitted to
    /// Acid800 `gtia_pmoverlap`).
    fn step_players(&mut self, cc: u16) {
        let mut out = 0;
        for n in 0..4 {
            if self.size_wait[n] > 0 {
                self.size_wait[n] -= 1;
                if self.size_wait[n] == 0 {
                    self.size_eff[n] = self.sizep[n];
                }
            }
            if self.hpos_wait[n] > 0 {
                self.hpos_wait[n] -= 1;
                if self.hpos_wait[n] == 0 {
                    self.hpos_eff[n] = self.hposp[n];
                }
            }
            if cc == u16::from(self.hpos_eff[n]) {
                if self.p_ctr[n] != 0 {
                    self.p_shift[n] <<= 1;
                }
                self.p_shift[n] |= self.grafp[n];
                self.p_ctr[n] = 0;
            }
            if self.p_shift[n] & 0x80 != 0 {
                out |= 1 << n;
            }
            let (shift, next) = width_step(self.size_eff[n], self.p_ctr[n]);
            if shift {
                self.p_shift[n] <<= 1;
            }
            self.p_ctr[n] = next;
        }
        self.p_out = out;
    }

    fn missile(&self, n: usize, cc: u16) -> bool {
        let mult = size_mult(self.sizem >> (2 * n));
        let off = cc.wrapping_sub(u16::from(self.hposm[n]));
        let bits = (self.grafm >> (2 * n)) & 3;
        off < 2 * mult && bits & (2 >> (off / mult)) != 0
    }

    /// Render the two colour clocks of one cycle on `line`.
    pub fn render_cycle(&mut self, line: u16, cycle: u16, pf: [Playfield; 2]) {
        if !(FIRST_LINE..VBLANK_LINE).contains(&line) {
            return;
        }
        let nibble = match pf {
            [Playfield::Hires(a), Playfield::Hires(b)] => (a << 2) | b,
            _ => 0,
        };
        // Mode 10 pixels lag one colour clock: the first colour clock of a
        // cycle still shows the previous cycle's pixel (Acid800
        // `gtia_collision2`).
        let prev_nibble = std::mem::replace(&mut self.prev_nibble, nibble);
        let cc0 = cycle * 2;
        let gtia_mode = self.prior >> 6;
        let row = usize::from(line - FIRST_LINE) * WIDTH as usize;
        for (k, &p) in pf.iter().enumerate() {
            let cc = cc0 + k as u16;
            // Players run through horizontal blank too.
            self.step_players(cc);
            if !(FIRST_CC..LAST_CC).contains(&cc) {
                continue;
            }
            let x = row + usize::from(cc - FIRST_CC) * 2;
            // Horizontal blank: black, and no collisions.
            if !(VISIBLE_CC.0..=VISIBLE_CC.1).contains(&cc) {
                self.pixels[x] = 0;
                self.pixels[x + 1] = 0;
                continue;
            }
            let [a, b] = if gtia_mode != 0 {
                let n = if gtia_mode == 2 && k == 0 {
                    prev_nibble
                } else {
                    nibble
                };
                let c = self.gtia_mode_colour(cc, gtia_mode, n);
                [c, c]
            } else {
                self.colour_clock(cc, p)
            };
            self.pixels[x] = a;
            self.pixels[x + 1] = b;
        }
    }

    /// One cycle of vertical blank after a hires last display line (ANTIC's
    /// hires bug, `Antic::hires_in_vblank`): nothing is drawn (the lines
    /// are outside the picture), but players and missiles go on and
    /// collide with each other; the playfield is hires background, which
    /// collides with nothing (Acid800 `antic_hiresbug`: P0/P1 overlap is
    /// seen on lines 248–249 only after a mode F last line).
    pub fn render_cycle_vblank(&mut self, cycle: u16) {
        for k in 0..2 {
            let cc = cycle * 2 + k;
            self.step_players(cc);
            if !(VISIBLE_CC.0..=VISIBLE_CC.1).contains(&cc) {
                continue;
            }
            let (p, m) = self.objects(cc);
            if p | m != 0 {
                self.collide(p, m, 0);
            }
        }
    }

    /// Objects at a colour clock: players and missiles as bit masks.
    fn objects(&self, cc: u16) -> (u8, u8) {
        let mut p = 0;
        let mut m = 0;
        for n in 0..4 {
            if self.p_out & (1 << n) != 0 {
                p |= 1 << n;
            }
            if self.missile(n, cc) {
                m |= 1 << n;
            }
        }
        (p, m)
    }

    fn collide(&mut self, p: u8, m: u8, pf_mask: u8) {
        for n in 0..4 {
            if p & (1 << n) != 0 {
                self.p_pf[n] |= pf_mask;
                self.p_pl[n] |= p & !(1 << n);
            }
            if m & (1 << n) != 0 {
                self.m_pf[n] |= pf_mask;
                self.m_pl[n] |= p;
            }
        }
    }

    /// Colour of the two half-clock pixels of one colour clock.
    fn colour_clock(&mut self, cc: u16, pf: Playfield) -> [u8; 2] {
        let (p, m) = self.objects(cc);
        let (pf_mask, hires) = match pf {
            Playfield::Background => (0u8, None),
            Playfield::Pf(n) => (1 << n, None),
            Playfield::Hires(bits) => (0x04, Some(bits)),
        };
        // Hires: only set pixels collide (as PF2).
        let collide_mask = match hires {
            Some(0) => 0,
            _ => pf_mask,
        };
        if p | m != 0 {
            self.collide(p, m, collide_mask);
        }
        let fifth = self.prior & 0x10 != 0;
        let multi = self.prior & 0x20 != 0;
        // Missiles join their players, or form the fifth player (PF3).
        let pl = if fifth { p } else { p | m };
        let pf_bits = pf_mask | if fifth && m != 0 { 0x08 } else { 0 };
        let pri = |b: u8| self.prior & (1 << b) != 0;
        let (pri0, pri1, pri2, pri3) = (pri(0), pri(1), pri(2), pri(3));
        let (p0, p1, p2, p3) = (pl & 1 != 0, pl & 2 != 0, pl & 4 != 0, pl & 8 != 0);
        let (f0, f1, f2, f3) = (
            pf_bits & 1 != 0,
            pf_bits & 2 != 0,
            pf_bits & 4 != 0,
            pf_bits & 8 != 0,
        );
        let p01 = p0 || p1;
        let p23 = p2 || p3;
        let pf01 = f0 || f1;
        let pf23 = f2 || f3;
        let pri01 = pri0 || pri1;
        let pri12 = pri1 || pri2;
        let pri23 = pri2 || pri3;
        let pri03 = pri0 || pri3;
        let sp0 = p0 && !(pf01 && pri23) && !(pri2 && pf23);
        let sp1 = p1 && !(pf01 && pri23) && !(pri2 && pf23) && (!p0 || multi);
        let sp2 = p2 && !p01 && !(pf23 && pri12) && !(pf01 && !pri0);
        let sp3 = p3 && !p01 && !(pf23 && pri12) && !(pf01 && !pri0) && (!p2 || multi);
        let sf3 = f3 && !(p23 && pri03) && !(p01 && !pri2);
        let sf0 = f0 && !(p23 && pri0) && !(p01 && pri01) && !sf3;
        let sf1 = f1 && !(p23 && pri0) && !(p01 && pri01) && !sf3;
        let sf2 = f2 && !(p23 && pri03) && !(p01 && !pri2) && !sf3;
        let sb = !p01 && !p23 && !pf01 && !pf23;
        let mut c = 0u8;
        for (on, col) in [
            (sp0, self.colpm[0]),
            (sp1, self.colpm[1]),
            (sp2, self.colpm[2]),
            (sp3, self.colpm[3]),
            (sf0, self.colpf[0]),
            (sf1, self.colpf[1]),
            (sf2, self.colpf[2]),
            (sf3, self.colpf[3]),
            (sb, self.colbk),
        ] {
            if on {
                c |= col;
            }
        }
        match hires {
            // Set hires pixels take PF1's luminance.
            Some(bits) => {
                let lum = (c & 0xf0) | (self.colpf[1] & 0x0f);
                [
                    if bits & 2 != 0 { lum } else { c },
                    if bits & 1 != 0 { lum } else { c },
                ]
            }
            None => [c, c],
        }
    }

    /// GTIA modes 9 (16 luminances), 10 (9 colour registers), 11 (16
    /// hues); players stay on top.
    fn gtia_mode_colour(&mut self, cc: u16, mode: u8, nibble: u8) -> u8 {
        let (p, m) = self.objects(cc);
        if p | m != 0 {
            // Only mode 10 has playfield collisions: colour indexes 4–7 and
            // 12–15 are PF0–PF3 (Acid800 `gtia_collision2`).
            let pf_mask = match (mode, nibble) {
                (2, 4..=7) => 1 << (nibble - 4),
                (2, 12..=15) => 1 << (nibble - 12),
                _ => 0,
            };
            self.collide(p, m, pf_mask);
            let pl = p | m;
            if let Some(n) = (0..4).find(|n| pl & (1 << n) != 0) {
                return self.colpm[n];
            }
        }
        match mode {
            1 => (self.colbk & 0xf0) | nibble,
            2 => match nibble {
                0..=3 => self.colpm[usize::from(nibble)],
                4..=7 => self.colpf[usize::from(nibble - 4)],
                12..=15 => self.colpf[usize::from(nibble - 12)],
                _ => self.colbk,
            },
            _ => (nibble << 4) | (self.colbk & 0x0f),
        }
    }
}

const TAG: [u8; 4] = *b"GTIA";
/// Version 2: mode 10 pixel delay, player shift registers.
const VERSION: u16 = 2;

impl Savestate for Gtia {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for a in [
                &self.hposp,
                &self.hposm,
                &self.sizep,
                &self.grafp,
                &self.colpm,
                &self.colpf,
                &self.m_pf,
                &self.p_pf,
                &self.m_pl,
                &self.p_pl,
            ] {
                w.raw(a);
            }
            for v in [
                self.sizem,
                self.grafm,
                self.colbk,
                self.prior,
                self.vdelay,
                self.gractl,
                self.consol_out,
                self.console_keys,
            ] {
                w.u8(v);
            }
            for v in self.trig_down.iter().chain(&self.trig_latched) {
                w.bool(*v);
            }
            w.raw(&self.pixels);
            w.u8(self.prev_nibble);
            for a in [
                &self.p_shift,
                &self.p_ctr,
                &self.hpos_eff,
                &self.hpos_wait,
                &self.size_eff,
                &self.size_wait,
            ] {
                w.raw(a);
            }
            w.u8(self.p_out);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        for a in [
            &mut n.hposp,
            &mut n.hposm,
            &mut n.sizep,
            &mut n.grafp,
            &mut n.colpm,
            &mut n.colpf,
            &mut n.m_pf,
            &mut n.p_pf,
            &mut n.m_pl,
            &mut n.p_pl,
        ] {
            c.raw_into(a)?;
        }
        n.sizem = c.u8()?;
        n.grafm = c.u8()?;
        n.colbk = c.u8()?;
        n.prior = c.u8()?;
        n.vdelay = c.u8()?;
        n.gractl = c.u8()?;
        n.consol_out = c.u8()?;
        n.console_keys = c.u8()?;
        for v in n.trig_down.iter_mut() {
            *v = c.bool()?;
        }
        for v in n.trig_latched.iter_mut() {
            *v = c.bool()?;
        }
        c.raw_into(&mut n.pixels)?;
        if version >= 2 {
            n.prev_nibble = c.u8()?;
            for a in [
                &mut n.p_shift,
                &mut n.p_ctr,
                &mut n.hpos_eff,
                &mut n.hpos_wait,
                &mut n.size_eff,
                &mut n.size_wait,
            ] {
                c.raw_into(a)?;
            }
            n.p_out = c.u8()?;
        } else {
            n.prev_nibble = 0;
            n.hpos_eff = n.hposp;
            n.size_eff = n.sizep;
        }
        if n.p_ctr.iter().chain(&n.size_eff).any(|&v| v > 3)
            || n.size_wait.iter().any(|&v| v > SIZE_DELAY)
            || n.hpos_wait.iter().any(|&v| v > HPOS_DELAY)
        {
            return Err(StateError::Invalid("GTIA players"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
