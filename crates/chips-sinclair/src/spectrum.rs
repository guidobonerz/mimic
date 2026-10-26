//! The ZX Spectrum's ULA (Ferranti 5C102E/5C112E in the 48K, 7K010E5 in
//! the 128K and +2): the display from video RAM, the CPU's contended
//! memory, port `$FE` (border, MIC, EAR, keyboard, tape input) and the
//! frame interrupt.
//!
//! - **Frame.** A line is [`Timing::t_per_line`] T-states (224 on the 48K,
//!   228 on the 128K), a frame [`Timing::lines`] lines (312, 311). /INT is
//!   held for the first [`Timing::int_len`] T-states of a frame (32, 36);
//!   the CPU's T-states count from that point.
//! - **Display.** 256 × 192 pixels from the bitmap at `$4000` of the screen
//!   bank and 32 × 24 attributes at `$5800` (ink bits 0–2, paper 3–5,
//!   bright 6, flash 7: ink and paper swap every 16 frames). Pixel row `y`
//!   of the bitmap is at `(y & $C0) << 5 | (y & 7) << 8 | (y & $38) << 2`.
//!   Display line 0 starts on T-state `display_line · t_per_line` (14336
//!   on the 48K, 14364 on the 128K); a line shows 128 T-states of pixels,
//!   two per T-state. Around it the border, in steps of 4 T-states (8
//!   pixels): a border change shows from the next step.
//! - **Contention.** While the ULA fetches a line's display bytes it holds
//!   the CPU's clock when the CPU addresses `$4000–$7FFF` (and the odd RAM
//!   banks on the 128K): from [`Timing::contend_start`] on each of the 192
//!   lines, for 128 T-states, a cycle starting on T-state `t` waits
//!   `6, 5, 4, 3, 2, 1, 0, 0` T-states by `(t − contend_start) mod 8`
//!   ([`SpectrumUla::delay`]). I/O cycles are delayed by the same table at
//!   their T-states depending on the address's high byte and bit 0
//!   ([`SpectrumUla::io_delay`]).
//! - **Floating bus.** A read from a port nothing answers sees the byte
//!   the ULA is fetching: per 8 T-states of a display line a bitmap byte,
//!   its attribute, the next bitmap byte, its attribute, then four idle
//!   T-states with `$FF` ([`SpectrumUla::floating`]).
//! - **Port $FE** (any even port). Write: bits 0–2 border, bit 3 MIC,
//!   bit 4 EAR (the speaker). Read: bits 0–4 the keyboard half-rows whose
//!   address line A8–A15 is low (0 = pressed), bit 6 the EAR input (the
//!   tape, or on an issue 3 board the EAR output bit), bits 5 and 7 high.
//!
//! The picture is [`WIDTH`] × [`HEIGHT`]: the display with 48 pixels of
//! border on every side, as a TV shows most of it; colours are indices
//! 0–15 of [`PALETTE`] (8–15 bright).
//!
//! Sources: Chris Smith, *The ZX Spectrum ULA: How to Design a
//! Microcomputer* (2010); the comp.sys.sinclair FAQ and World of Spectrum
//! FAQ "Contended memory", "128K timings"; Sinclair ZX Spectrum Service
//! Manual. The implementation is this project's own.

use emu_core::frame::Rgb8;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Picture: 256 × 192 display plus 48 pixels of border on each side.
pub const WIDTH: u32 = 352;
pub const HEIGHT: u32 = 288;
/// Border pixels left of and lines above the display.
pub const BORDER: u32 = 48;
/// Border steps (8 pixels, 4 T-states) per picture line.
const STEPS: u32 = WIDTH / 8;
/// Two pixels per T-state.
pub const PIXELS_PER_T: u32 = 2;

/// Frame timing of a ULA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub t_per_line: u32,
    pub lines: u32,
    pub int_len: u32,
    /// First T-state with a delay of 6.
    pub contend_start: u32,
    /// Raster line of display line 0.
    pub display_line: u32,
    /// The odd RAM banks are contended (128K, +2).
    pub banked: bool,
}

impl Timing {
    pub const fn frame_len(&self) -> u32 {
        self.t_per_line * self.lines
    }

    /// First T-state of display line 0.
    pub const fn display_start(&self) -> u32 {
        self.display_line * self.t_per_line
    }
}

/// 48K: 3.5 MHz, 224 × 312.
pub const TIMING_48K: Timing = Timing {
    t_per_line: 224,
    lines: 312,
    int_len: 32,
    contend_start: 14335,
    display_line: 64,
    banked: false,
};

/// 128K and +2: 3.5469 MHz, 228 × 311.
pub const TIMING_128K: Timing = Timing {
    t_per_line: 228,
    lines: 311,
    int_len: 36,
    contend_start: 14361,
    display_line: 63,
    banked: true,
};

/// The delays of a contended cycle by its T-state within the 8-T-state
/// fetch group.
const DELAYS: [u8; 8] = [6, 5, 4, 3, 2, 1, 0, 0];

/// The 15 colours (black twice): normal at about 84 % of full scale,
/// bright at full scale.
pub const PALETTE: [Rgb8; 16] = {
    const N: u8 = 0xd7;
    const B: u8 = 0xff;
    const fn c(r: u8, g: u8, b: u8) -> Rgb8 {
        Rgb8 { r, g, b }
    }
    [
        c(0, 0, 0),
        c(0, 0, N),
        c(N, 0, 0),
        c(N, 0, N),
        c(0, N, 0),
        c(0, N, N),
        c(N, N, 0),
        c(N, N, N),
        c(0, 0, 0),
        c(0, 0, B),
        c(B, 0, 0),
        c(B, 0, B),
        c(0, B, 0),
        c(0, B, B),
        c(B, B, 0),
        c(B, B, B),
    ]
};

/// Offset of bitmap row `y` (0–191) in the screen.
pub const fn bitmap_row(y: u32) -> usize {
    (((y & 0xc0) << 5) | ((y & 7) << 8) | ((y & 0x38) << 2)) as usize
}

/// Offset of the attribute row of pixel row `y`.
pub const fn attr_row(y: u32) -> usize {
    0x1800 + (y as usize >> 3) * 32
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpectrumUla {
    pub timing: Timing,
    /// Border colour 0–7.
    pub border: u8,
    /// MIC and EAR output bits of the last port $FE write.
    pub mic: bool,
    pub ear: bool,
    /// Issue 3 board: the EAR output shows in bit 6 of port $FE (issue 2:
    /// MIC does too).
    pub issue3: bool,
    /// Frames since power-on (the flash phase is bit 4).
    pub frames: u64,
    /// Next border step to draw (index over the picture).
    next_step: u32,
    pixels: Vec<u8>,
}

impl SpectrumUla {
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            border: 7,
            mic: false,
            ear: false,
            issue3: true,
            frames: 0,
            next_step: 0,
            pixels: vec![7; (WIDTH * HEIGHT) as usize],
        }
    }

    pub fn reset(&mut self) {
        self.border = 7;
        self.mic = false;
        self.ear = false;
        self.next_step = 0;
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// /INT on frame T-state `t`.
    #[inline]
    pub fn int_active(&self, t: u32) -> bool {
        t < self.timing.int_len
    }

    /// Wait states of a memory cycle (or internal T-state) on contended
    /// address starting on frame T-state `t`.
    #[inline]
    pub fn delay(&self, t: u32) -> u32 {
        let Some(rel) = t.checked_sub(self.timing.contend_start) else {
            return 0;
        };
        let tl = self.timing.t_per_line;
        if rel / tl >= 192 {
            return 0;
        }
        let col = rel % tl;
        if col >= 128 {
            0
        } else {
            u32::from(DELAYS[(col & 7) as usize])
        }
    }

    /// Wait states of an I/O cycle starting on frame T-state `t` to port
    /// `port` whose high byte is a contended address (`high_contended`).
    /// Patterns (C = contended T-state, N = not): high byte not contended
    /// and an even port N:1, C:3; odd N:4; contended high byte and even
    /// C:1, C:3; odd C:1, C:1, C:1, C:1.
    pub fn io_delay(&self, t: u32, port: u16, high_contended: bool) -> u32 {
        let ula = port & 1 == 0;
        let mut now = t;
        let mut wait = 0;
        let contend = |now: &mut u32, wait: &mut u32, len: u32| {
            let d = self.delay(*now);
            *wait += d;
            *now += d + len;
        };
        match (high_contended, ula) {
            (false, false) => {}
            (false, true) => {
                now += 1;
                contend(&mut now, &mut wait, 3);
            }
            (true, true) => {
                contend(&mut now, &mut wait, 1);
                contend(&mut now, &mut wait, 3);
            }
            (true, false) => {
                for _ in 0..4 {
                    contend(&mut now, &mut wait, 1);
                }
            }
        }
        wait
    }

    /// What an unattached port reads on frame T-state `t`: the byte the
    /// ULA fetches from `screen` then, or `$FF` while it fetches nothing.
    pub fn floating(&self, t: u32, screen: &[u8]) -> u8 {
        // The first bitmap byte is on the bus three T-states after the
        // first contended one (48K 14338, 128K 14364; Fuse Test
        // "Floating bus" on both).
        let Some(rel) = t.checked_sub(self.timing.contend_start + 3) else {
            return 0xff;
        };
        let tl = self.timing.t_per_line;
        let line = rel / tl;
        let col = rel % tl;
        if line >= 192 || col >= 128 {
            return 0xff;
        }
        let cell = (col / 8 * 2) as usize;
        let at = match col % 8 {
            0 => bitmap_row(line) + cell,
            1 => attr_row(line) + cell,
            2 => bitmap_row(line) + cell + 1,
            3 => attr_row(line) + cell + 1,
            _ => return 0xff,
        };
        screen.get(at).copied().unwrap_or(0xff)
    }

    /// Port $FE read: `keys` the pressed keys of the selected half-rows
    /// (bits 0–4, 1 = pressed), `tape` the EAR input level.
    pub fn read_fe(&self, keys: u8, tape: bool) -> u8 {
        let echo = if self.issue3 {
            self.ear
        } else {
            self.ear || self.mic
        };
        let ear_in = tape || echo;
        (!keys & 0x1f) | 0xa0 | u8::from(ear_in) << 6
    }

    /// Port $FE write (draw up to the write's T-state first).
    pub fn write_fe(&mut self, v: u8) {
        self.border = v & 7;
        self.mic = v & 0x08 != 0;
        self.ear = v & 0x10 != 0;
    }

    /// Frame T-state of border step `i`.
    #[inline]
    fn step_t(&self, i: u32) -> u32 {
        let line = self.timing.display_line - BORDER + i / STEPS;
        // The display starts BORDER pixels (BORDER / 2 T-states) into the
        // picture line.
        (line * self.timing.t_per_line + (i % STEPS) * 4) - BORDER / 2
    }

    /// Draw the picture up to (not including) frame T-state `t`, the
    /// display from `screen` (the 6912 bytes from `$4000` of the shown
    /// bank).
    pub fn render_to(&mut self, t: u32, screen: &[u8]) {
        const TOTAL: u32 = STEPS * HEIGHT;
        let flash = self.frames & 16 != 0;
        while self.next_step < TOTAL && self.step_t(self.next_step) < t {
            let i = self.next_step;
            self.next_step += 1;
            let (y, k) = (i / STEPS, i % STEPS);
            let out = &mut self.pixels[(i * 8) as usize..(i * 8 + 8) as usize];
            let dy = y.wrapping_sub(BORDER);
            let dk = k.wrapping_sub(BORDER / 8);
            if dy >= 192 || dk >= 32 {
                out.fill(self.border);
                continue;
            }
            let bits = screen
                .get(bitmap_row(dy) + dk as usize)
                .copied()
                .unwrap_or(0);
            let attr = screen.get(attr_row(dy) + dk as usize).copied().unwrap_or(0);
            let bright = (attr >> 3) & 8;
            let (mut ink, mut paper) = ((attr & 7) | bright, ((attr >> 3) & 7) | bright);
            if attr & 0x80 != 0 && flash {
                std::mem::swap(&mut ink, &mut paper);
            }
            for (b, px) in out.iter_mut().enumerate() {
                *px = if bits & (0x80 >> b) != 0 { ink } else { paper };
            }
        }
    }

    /// The frame is over: draw its rest and start the next.
    pub fn end_frame(&mut self, screen: &[u8]) {
        self.render_to(u32::MAX, screen);
        self.next_step = 0;
        self.frames += 1;
    }

    /// Raster line and T-state in the line of frame T-state `t`.
    pub fn beam(&self, t: u32) -> (u32, u32) {
        (t / self.timing.t_per_line, t % self.timing.t_per_line)
    }

    /// The picture row of raster line `line`, if it is in the picture.
    pub fn picture_row(&self, line: u32) -> Option<u32> {
        let row = line.checked_sub(self.timing.display_line - BORDER)?;
        (row < HEIGHT).then_some(row)
    }

    /// Picture column of T-state `c` of a line (`None` outside).
    pub fn picture_column(&self, c: u32) -> Option<u32> {
        let x = (c * PIXELS_PER_T + BORDER) % (self.timing.t_per_line * PIXELS_PER_T);
        (x < WIDTH).then_some(x)
    }
}

const TAG: [u8; 4] = *b"SULA";

impl Savestate for SpectrumUla {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u8(self.border);
            w.bool(self.mic);
            w.bool(self.ear);
            w.bool(self.issue3);
            w.u64(self.frames);
            w.u32(self.next_step);
            w.raw(&self.pixels);
        });
    }

    /// The timing is configuration.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let mut n = Self::new(self.timing);
        n.border = c.u8()? & 7;
        n.mic = c.bool()?;
        n.ear = c.bool()?;
        n.issue3 = c.bool()?;
        n.frames = c.u64()?;
        n.next_step = c.u32()?;
        if n.next_step > STEPS * HEIGHT {
            return Err(StateError::Invalid("ULA picture position"));
        }
        c.raw_into(&mut n.pixels)?;
        if n.pixels.iter().any(|&p| p > 15) {
            return Err(StateError::Invalid("ULA picture"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_table() {
        let u = SpectrumUla::new(TIMING_48K);
        assert_eq!(u.delay(14334), 0);
        let got: Vec<u32> = (14335..14343).map(|t| u.delay(t)).collect();
        assert_eq!(got, [6, 5, 4, 3, 2, 1, 0, 0]);
        assert_eq!(u.delay(14335 + 128), 0, "right border");
        assert_eq!(u.delay(14335 + 224), 6, "next line");
        assert_eq!(u.delay(14335 + 192 * 224), 0, "below the display");
        let u = SpectrumUla::new(TIMING_128K);
        assert_eq!(u.delay(14361), 6);
        assert_eq!(u.delay(14361 + 228 + 3), 3);
    }

    #[test]
    fn io_contention_patterns() {
        let u = SpectrumUla::new(TIMING_48K);
        // Odd port, uncontended high byte: N:4.
        assert_eq!(u.io_delay(14335, 0x00ff, false), 0);
        // Even port, contended high byte, from the group's start: C:1
        // (6), then C:3 at 14342 (0).
        assert_eq!(u.io_delay(14335, 0x40fe, true), 6);
        // Even port, uncontended: N:1 then C:3 at 14336 (5).
        assert_eq!(u.io_delay(14335, 0x00fe, false), 5);
        // Odd port, contended: four C:1 from 14335: 6, then 14342 (0),
        // 14343 (6, a new group), 14350 (0).
        assert_eq!(u.io_delay(14335, 0x40ff, true), 12);
    }

    #[test]
    fn floating_bus_order() {
        let u = SpectrumUla::new(TIMING_48K);
        let mut screen = vec![0u8; 6912];
        screen[0] = 0x11;
        screen[0x1800] = 0x22;
        screen[1] = 0x33;
        screen[0x1801] = 0x44;
        let t = 14338;
        let got: Vec<u8> = (t..t + 8).map(|t| u.floating(t, &screen)).collect();
        assert_eq!(got, [0x11, 0x22, 0x33, 0x44, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(u.floating(100, &screen), 0xff);
        // The 128K: three T-states after its first contended one too.
        let u = SpectrumUla::new(TIMING_128K);
        assert_eq!(u.floating(14364, &screen), 0x11);
        assert_eq!(u.floating(14365, &screen), 0x22);
    }

    #[test]
    fn picture_and_border() {
        let mut u = SpectrumUla::new(TIMING_48K);
        let mut screen = vec![0u8; 6912];
        screen[0] = 0x80;
        screen[0x1800] = 0x47; // bright white ink on black paper
        u.write_fe(2);
        u.end_frame(&screen);
        let px = u.pixels();
        let at = |x: u32, y: u32| px[(y * WIDTH + x) as usize];
        assert_eq!(at(0, 0), 2, "border");
        assert_eq!(at(48, 48), 15, "ink");
        assert_eq!(at(49, 48), 8, "bright paper");
        assert_eq!(at(304, 48), 2, "right border");
        // A border change at a line's display start shows from there.
        let mut u = SpectrumUla::new(TIMING_48K);
        u.write_fe(1);
        u.render_to(TIMING_48K.display_start(), &screen);
        u.write_fe(4);
        u.end_frame(&screen);
        let px = u.pixels();
        assert_eq!(px[(47 * WIDTH + 30) as usize], 1);
        assert_eq!(
            px[(48 * WIDTH + 10) as usize],
            1,
            "left border drawn before"
        );
        assert_eq!(px[(48 * WIDTH + 320) as usize], 4);
    }

    #[test]
    fn savestate_round_trip() {
        let mut u = SpectrumUla::new(TIMING_128K);
        u.write_fe(0x15);
        u.frames = 40;
        u.render_to(20000, &[0; 6912]);
        let mut w = StateWriter::new();
        u.save(&mut w);
        let bytes = w.into_bytes();
        let mut v = SpectrumUla::new(TIMING_128K);
        v.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(u, v);
    }
}
