//! TIA (Television Interface Adaptor), the Atari 2600's video and sound
//! chip. It has no frame buffer: the CPU races the beam and sets the
//! registers line by line.
//!
//! Timing: 228 colour clocks per line, the first 68 are horizontal
//! blanking, 160 are picture. The CPU runs at a third of the colour clock
//! (76 cycles per line); a register write takes effect from the first
//! colour clock of the CPU cycle after it. A write to WSYNC holds the CPU
//! (RDY low) until the next line starts. VSYNC (bit 1) is the vertical
//! sync pulse, VBLANK (bit 1) blanks the picture.
//!
//! Fields: this model starts a field when VSYNC ends: line 0 is the first
//! line after the sync pulse, as a TV locks to it. Without a sync pulse
//! the field ends after [`free_run_lines`], like a TV that loses sync.
//!
//! Playfield: 40 bits of 4 pixels per line, the left half from PF0 (D4–D7),
//! PF1 (D7–D0) and PF2 (D0–D7), the right half repeated or mirrored
//! (CTRLPF D0). Each bit is taken from the registers 4 colour clocks
//! before it is shown (from colour clock 64 on), so a write changes the
//! picture from the next bit boundary.
//!
//! Movable objects (two players, two missiles, the ball) each have a
//! position counter that counts the 160 "motion clocks" of the visible
//! line and pauses during horizontal blanking, so an object keeps its
//! position from line to line. A counter wrap starts the main copy;
//! players and missiles also start the close, medium and far copies of
//! NUSIZ at 16, 32 and 64 clocks. Graphics appear 5 clocks after the start
//! for players (6 when stretched) and 4 for missiles and the ball. A reset
//! strobe (RESP0 …) restarts the counter, so the object shows up 5 (4)
//! pixels right of where the beam was, on the following lines; a reset
//! during blanking puts players at pixel 3 and missiles and the ball at 2.
//! RESBL also starts the ball at once.
//!
//! HMOVE extends the blanking by 8 pixels on its line (moving everything
//! 8 pixels right, the "comb" on the left edge) and sends each object up
//! to 15 extra motion clocks, one every 4 colour clocks, while a 4-bit
//! counter runs down and has not yet met the object's HMxx value (D7
//! inverted). An object whose value changes so that the counter never
//! meets it keeps receiving extra clocks until the next HMOVE (the "Cosmic
//! Ark" effect). Extra clocks also advance graphics being drawn.
//!
//! Sound: two channels, each clocked twice per line (colour clock ÷ 114,
//! ≈ 31.4 kHz) and divided by AUDF + 1. On every divider pulse the
//! channel's 5-bit polynomial counter steps; AUDC D1–D0 decide whether the
//! pulse goes on (always, on a ÷31 pattern, or when the 5-bit polynomial
//! puts out a 1), and AUDC D3–D2 what it does: step the 4-bit polynomial
//! and output it, toggle the output (÷2), output the 9-bit polynomial
//! (AUDC 8) or the 5-bit one (AUDC 9), or toggle every third pulse (÷6,
//! ÷93). AUDC 0 and 11 hold the output high. This reproduces the AUDC
//! table of the Stella Programmer's Guide (4/5 ÷2, 6/10 ÷31, 12/13 ÷6,
//! 14 ÷93, 8 white noise, 1 the 4-bit polynomial). AUDV scales a channel
//! (4 bits); the two are mixed linearly ([`Tia::audio_level`]).
//!
//! Inputs: I4/I5 are the fire buttons (latched low while VBLANK D6 is
//! set). I0–I3 are the paddle pots: VBLANK D7 grounds their capacitors;
//! once released each charges through its pot and the input reads high
//! when the charge time has passed, proportional to the resistance, up to
//! [`PADDLE_LINES`] lines for a fully turned paddle ([`Tia::set_pot`]).
//! Keypads and other digital devices pull I0–I5 low directly
//! ([`Tia::set_input`]).
//!
//! Output: [`WIDTH`] × [`height`] pixels of colour register values ÷ 2
//! (the TIA ignores bit 0), index into [`crate::palette::tia`].
//!
//! Registers (write `$00–$2C`, read `$00–$0D`; the machine passes A0–A5):
//! see [`W`] and [`R`]. Sources: Steve Wright, "Stella Programmer's Guide"
//! (1979); Andrew Towers, "Atari 2600 TIA Hardware Notes" (2003,
//! <https://www.atarihq.com/danb/files/TIA_HW_Notes.txt>): counter decodes,
//! start delays, HMOVE clock stuffing, graphics registers and VDEL; the
//! blanking reset positions are the commonly documented values.

use emu_core::frame::VideoStandard;
use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

/// Colour clocks per line and of horizontal blanking.
pub const CLOCKS_PER_LINE: u8 = 228;
pub const HBLANK: u8 = 68;
/// End of blanking on a line with HMOVE.
const HBLANK_HMOVE: u8 = 76;
/// Picture width in colour clocks (one pixel each).
pub const WIDTH: u32 = 160;

/// Write register names, by address.
pub const W: [&str; 0x2d] = [
    "VSYNC", "VBLANK", "WSYNC", "RSYNC", "NUSIZ0", "NUSIZ1", "COLUP0", "COLUP1", "COLUPF",
    "COLUBK", "CTRLPF", "REFP0", "REFP1", "PF0", "PF1", "PF2", "RESP0", "RESP1", "RESM0", "RESM1",
    "RESBL", "AUDC0", "AUDC1", "AUDF0", "AUDF1", "AUDV0", "AUDV1", "GRP0", "GRP1", "ENAM0",
    "ENAM1", "ENABL", "HMP0", "HMP1", "HMM0", "HMM1", "HMBL", "VDELP0", "VDELP1", "VDELBL",
    "RESMP0", "RESMP1", "HMOVE", "HMCLR", "CXCLR",
];

/// Read register names, by address.
pub const R: [&str; 14] = [
    "CXM0P", "CXM1P", "CXP0FB", "CXP1FB", "CXM0FB", "CXM1FB", "CXBLPF", "CXPPMM", "INPT0", "INPT1",
    "INPT2", "INPT3", "INPT4", "INPT5",
];

pub const VSYNC: u8 = 0x00;
pub const VBLANK: u8 = 0x01;
pub const WSYNC: u8 = 0x02;
pub const RSYNC: u8 = 0x03;
pub const NUSIZ0: u8 = 0x04;
pub const NUSIZ1: u8 = 0x05;
pub const COLUP0: u8 = 0x06;
pub const COLUP1: u8 = 0x07;
pub const COLUPF: u8 = 0x08;
pub const COLUBK: u8 = 0x09;
pub const CTRLPF: u8 = 0x0a;
pub const REFP0: u8 = 0x0b;
pub const REFP1: u8 = 0x0c;
pub const PF0: u8 = 0x0d;
pub const PF1: u8 = 0x0e;
pub const PF2: u8 = 0x0f;
pub const RESP0: u8 = 0x10;
pub const RESP1: u8 = 0x11;
pub const RESM0: u8 = 0x12;
pub const RESM1: u8 = 0x13;
pub const RESBL: u8 = 0x14;
pub const GRP0: u8 = 0x1b;
pub const GRP1: u8 = 0x1c;
pub const ENAM0: u8 = 0x1d;
pub const ENAM1: u8 = 0x1e;
pub const ENABL: u8 = 0x1f;
pub const HMP0: u8 = 0x20;
pub const HMBL: u8 = 0x24;
pub const VDELP0: u8 = 0x25;
pub const VDELP1: u8 = 0x26;
pub const VDELBL: u8 = 0x27;
pub const RESMP0: u8 = 0x28;
pub const RESMP1: u8 = 0x29;
pub const HMOVE: u8 = 0x2a;
pub const HMCLR: u8 = 0x2b;
pub const CXCLR: u8 = 0x2c;
pub const AUDC0: u8 = 0x15;
pub const AUDF0: u8 = 0x17;
pub const AUDV0: u8 = 0x19;

/// Colour clocks of the line on which the sound channels are clocked.
const AUDIO_CLOCKS: [u8; 2] = [0, 114];

/// One sound channel's counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Channel {
    /// Audio clocks since the last divider pulse.
    divider: u8,
    /// Polynomial counters (bit 0 is the output) and the ÷31 position.
    poly4: u8,
    poly5: u8,
    poly9: u16,
    div31: u8,
    /// Pulses since the last ÷3 toggle.
    div3: u8,
    out: bool,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            divider: 0,
            poly4: 0x0f,
            poly5: 0x1f,
            poly9: 0x1ff,
            div31: 0,
            div3: 0,
            out: true,
        }
    }
}

impl Channel {
    /// One audio clock with the channel's AUDC and AUDF.
    fn clock(&mut self, audc: u8, audf: u8) {
        if self.divider < audf & 0x1f {
            self.divider += 1;
            return;
        }
        self.divider = 0;
        // 5-bit polynomial x^5 + x^3 + 1 (period 31), stepped on every pulse.
        let p5 = self.poly5;
        self.poly5 = (p5 >> 1) | ((p5 ^ (p5 >> 2)) & 1) << 4;
        self.div31 = (self.div31 + 1) % 31;
        let audc = audc & 0x0f;
        if audc == 0x00 || audc == 0x0b {
            self.out = true;
            return;
        }
        let pass = match audc & 3 {
            2 => self.div31 == 0 || self.div31 == 18,
            3 => self.poly5 & 1 != 0,
            _ => true,
        };
        if !pass {
            return;
        }
        match audc >> 2 {
            0 => {
                // 4-bit polynomial x^4 + x^3 + 1 (period 15).
                let p4 = self.poly4;
                self.poly4 = (p4 >> 1) | ((p4 ^ (p4 >> 1)) & 1) << 3;
                self.out = self.poly4 & 1 != 0;
            }
            1 => self.out = !self.out,
            2 => match audc {
                0x08 => {
                    // 9-bit polynomial x^9 + x^5 + 1 (period 511).
                    let p9 = self.poly9;
                    self.poly9 = (p9 >> 1) | ((p9 ^ (p9 >> 4)) & 1) << 8;
                    self.out = self.poly9 & 1 != 0;
                }
                0x09 => self.out = self.poly5 & 1 != 0,
                _ => self.out = !self.out,
            },
            _ => {
                self.div3 += 1;
                if self.div3 == 3 {
                    self.div3 = 0;
                    self.out = !self.out;
                }
            }
        }
    }

    fn save(&self, w: &mut StateWriter) {
        for v in [self.divider, self.poly4, self.poly5, self.div31, self.div3] {
            w.u8(v);
        }
        w.u16(self.poly9);
        w.bool(self.out);
    }

    fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        self.divider = c.u8()?;
        self.poly4 = c.u8()?;
        self.poly5 = c.u8()?;
        self.div31 = c.u8()?;
        self.div3 = c.u8()?;
        self.poly9 = c.u16()?;
        self.out = c.bool()?;
        if self.divider > 31
            || self.poly4 & !0x0f != 0
            || self.poly5 & !0x1f != 0
            || self.poly9 & !0x1ff != 0
            || self.div31 >= 31
            || self.div3 >= 3
        {
            return Err(StateError::Invalid("TIA sound"));
        }
        Ok(())
    }
}

/// Lines a paddle at full resistance needs to charge its input (the
/// usual calibration: about 380 lines, 190 at the centre).
pub const PADDLE_LINES: u32 = 380;

/// Lines of a field as the TV standard defines them (the height of the
/// output picture).
pub fn height(standard: VideoStandard) -> u32 {
    match standard {
        VideoStandard::Ntsc => 262,
        VideoStandard::Pal => 312,
    }
}

/// Lines after which a field without VSYNC ends.
pub fn free_run_lines(standard: VideoStandard) -> u16 {
    height(standard) as u16 + 40
}

/// Movable objects, in the order of the HMxx registers.
const P0: usize = 0;
const P1: usize = 1;
const M0: usize = 2;
const M1: usize = 3;
const BL: usize = 4;

/// Position counter and graphics output of one movable object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mover {
    /// Motion clocks since the counter last wrapped or was reset (0–159).
    counter: u8,
    /// The next motion clock does not count (reset during the picture).
    skip: bool,
    /// Motion clocks until graphics start; 0: no start pending.
    delay: u8,
    /// The pending start is the main copy's.
    delay_main: bool,
    /// Motion clocks since graphics started; `None`: not drawing.
    draw: Option<u8>,
    /// What is being drawn is the main copy.
    main: bool,
    /// HMOVE: still wants extra motion clocks.
    moving: bool,
}

impl Mover {
    /// A reset strobe: during the picture the counter restarts from the
    /// current pixel, during blanking it is set so that the object lands
    /// at the documented blanking position.
    fn reset(&mut self, visible: bool) {
        self.counter = u8::from(!visible);
        self.skip = visible;
    }

    /// One motion clock. `len`: graphics length in clocks, `copies`: copy
    /// starts (bit 0 close, 1 medium, 2 far), `delay`: start delay.
    fn clock(&mut self, len: u8, copies: u8, delay: u8) {
        if let Some(i) = self.draw {
            let n = i + 1;
            self.draw = (n < len).then_some(n);
        }
        if self.delay > 0 {
            self.delay -= 1;
            if self.delay == 0 {
                self.draw = Some(0);
                self.main = self.delay_main;
            }
        }
        if std::mem::take(&mut self.skip) {
            return;
        }
        self.counter = if self.counter == 159 {
            0
        } else {
            self.counter + 1
        };
        let start = match self.counter {
            0 => Some(true),
            16 if copies & 1 != 0 => Some(false),
            32 if copies & 2 != 0 => Some(false),
            64 if copies & 4 != 0 => Some(false),
            _ => None,
        };
        if let Some(main) = start {
            self.delay = delay;
            self.delay_main = main;
        }
    }

    fn save(&self, w: &mut StateWriter) {
        w.u8(self.counter);
        w.bool(self.skip);
        w.u8(self.delay);
        w.bool(self.delay_main);
        w.u8(self.draw.unwrap_or(0xff));
        w.bool(self.main);
        w.bool(self.moving);
    }

    fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        self.counter = c.u8()?;
        self.skip = c.bool()?;
        self.delay = c.u8()?;
        self.delay_main = c.bool()?;
        self.draw = match c.u8()? {
            0xff => None,
            d => Some(d),
        };
        self.main = c.bool()?;
        self.moving = c.bool()?;
        if self.counter >= 160 || self.delay > 6 || self.draw.is_some_and(|d| d >= 32) {
            return Err(StateError::Invalid("TIA object"));
        }
        Ok(())
    }
}

/// Copy starts of NUSIZ D0–D2: bit 0 close (16), 1 medium (32), 2 far (64).
fn copies(nusiz: u8) -> u8 {
    [0, 1, 2, 3, 4, 0, 6, 0][usize::from(nusiz & 7)]
}

/// Player width in motion clocks per graphics bit.
fn player_width(nusiz: u8) -> u8 {
    match nusiz & 7 {
        5 => 2,
        7 => 4,
        _ => 1,
    }
}

/// Collision latch bits: register `r` keeps D7 in bit `2r`, D6 in `2r + 1`.
fn collisions(p0: bool, p1: bool, m0: bool, m1: bool, bl: bool, pf: bool) -> u16 {
    let pairs = [
        m0 && p1,
        m0 && p0,
        m1 && p0,
        m1 && p1,
        p0 && pf,
        p0 && bl,
        p1 && pf,
        p1 && bl,
        m0 && pf,
        m0 && bl,
        m1 && pf,
        m1 && bl,
        bl && pf,
        false,
        p0 && p1,
        m0 && m1,
    ];
    pairs
        .iter()
        .enumerate()
        .fold(0, |acc, (i, &on)| acc | (u16::from(on) << i))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tia {
    standard: VideoStandard,
    /// Last value written to each register.
    pub regs: [u8; 0x2d],
    /// Colour clock within the line (0–227) and line within the field.
    cc: u8,
    line: u16,
    frame: u64,
    /// The CPU waits for the next line.
    wsync: bool,
    /// A field ended since the last [`Tia::tick`] reported it.
    field_ended: bool,
    /// Levels of the input pins I0–I5 (bit n = INPTn high).
    inputs: u8,
    /// VBLANK D6: I4/I5 latch low levels; bit n low = latched low.
    latched: u8,
    movers: [Mover; 5],
    /// Player graphics and ball enable: "new" (written) and "old" (copied
    /// on the other player's GRP write) values for vertical delay.
    grp_new: [u8; 2],
    grp_old: [u8; 2],
    enabl_new: bool,
    enabl_old: bool,
    /// The playfield bit being shown, and the next one (taken 4 colour
    /// clocks ahead).
    pf_out: bool,
    pf_next: bool,
    /// HMOVE: colour clocks since the strobe while its counter runs.
    hmove: Option<u8>,
    /// HMOVE blanking extension latch (cleared when the line wraps).
    hmove_blank: bool,
    /// Collision latches, see [`collisions`].
    cx: u16,
    channels: [Channel; 2],
    /// Colour clocks since power-up (time base of the paddle charge).
    clocks: u64,
    /// Charge time of I0–I3 in colour clocks; `None`: nothing connected.
    pots: [Option<u32>; 4],
    /// When VBLANK D7 last released the paddle capacitors.
    charge_from: u64,
    pixels: Vec<u8>,
}

impl Tia {
    pub fn new(standard: VideoStandard) -> Self {
        Self {
            standard,
            regs: [0; 0x2d],
            cc: 0,
            line: 0,
            frame: 0,
            wsync: false,
            field_ended: false,
            inputs: 0x3f,
            latched: 0x3f,
            movers: [Mover::default(); 5],
            grp_new: [0; 2],
            grp_old: [0; 2],
            enabl_new: false,
            enabl_old: false,
            pf_out: false,
            pf_next: false,
            hmove: None,
            hmove_blank: false,
            cx: 0,
            channels: [Channel::default(); 2],
            clocks: 0,
            pots: [None; 4],
            charge_from: 0,
            pixels: vec![0; (WIDTH * height(standard)) as usize],
        }
    }

    pub fn standard(&self) -> VideoStandard {
        self.standard
    }

    pub fn clock(&self) -> u8 {
        self.cc
    }

    pub fn line(&self) -> u16 {
        self.line
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// RDY for the CPU: low while it waits for WSYNC.
    pub fn rdy(&self) -> bool {
        !self.wsync
    }

    /// The mixed sound output: AUDV × channel output of both channels,
    /// 0–30.
    pub fn audio_level(&self) -> u8 {
        (0..2)
            .map(|i| {
                if self.channels[i].out {
                    self.reg(AUDV0 + i as u8) & 0x0f
                } else {
                    0
                }
            })
            .sum()
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    fn reg(&self, r: u8) -> u8 {
        self.regs[usize::from(r)]
    }

    /// The movable objects' counters run (picture, outside the HMOVE
    /// blanking extension).
    fn motion(&self) -> bool {
        self.cc >= HBLANK && !(self.hmove_blank && self.cc < HBLANK_HMOVE)
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        let reg = reg & 0x3f;
        let Some(slot) = self.regs.get_mut(usize::from(reg)) else {
            return;
        };
        let before = *slot;
        *slot = v;
        let visible = self.motion();
        match reg {
            VSYNC if before & 2 != 0 && v & 2 == 0 => self.end_field(),
            VBLANK => {
                if before & 0x40 == 0 && v & 0x40 != 0 {
                    self.latched = 0x3f;
                }
                if before & 0x80 != 0 && v & 0x80 == 0 {
                    self.charge_from = self.clocks;
                }
            }
            WSYNC => self.wsync = true,
            // The line counter restarts: blanking begins 4 clocks later.
            RSYNC => self.cc = CLOCKS_PER_LINE - 4,
            RESP0 | RESP1 | RESM0 | RESM1 => {
                self.movers[usize::from(reg - RESP0)].reset(visible);
            }
            RESBL => {
                let ball = &mut self.movers[BL];
                ball.reset(visible);
                // The ball's reset also starts it (the delay counts from
                // this clock during the picture).
                ball.delay = if visible { 5 } else { 3 };
                ball.delay_main = true;
            }
            GRP0 => {
                self.grp_new[0] = v;
                self.grp_old[1] = self.grp_new[1];
            }
            GRP1 => {
                self.grp_new[1] = v;
                self.grp_old[0] = self.grp_new[0];
                self.enabl_old = self.enabl_new;
            }
            ENABL => self.enabl_new = v & 2 != 0,
            HMOVE => {
                for m in &mut self.movers {
                    m.moving = true;
                }
                self.hmove = Some(0);
                self.hmove_blank = true;
            }
            HMCLR => self.regs[usize::from(HMP0)..=usize::from(HMBL)].fill(0),
            CXCLR => self.cx = 0,
            _ => {}
        }
    }

    /// Read registers drive D7 and D6 only; the machine fills in the rest
    /// from the data bus. No side effects.
    pub fn peek(&self, reg: u8) -> u8 {
        match reg & 0x0f {
            r @ 0..=7 => {
                (((self.cx >> (2 * r)) & 1) << 7 | ((self.cx >> (2 * r + 1)) & 1) << 6) as u8
            }
            r @ 8..=11 => {
                let n = usize::from(r - 8);
                let charged = self.reg(VBLANK) & 0x80 == 0
                    && self.pots[n].is_some_and(|t| self.clocks - self.charge_from >= u64::from(t));
                (u8::from(charged) & (self.inputs >> n)) << 7
            }
            r @ 12..=13 => {
                let level = if self.reg(VBLANK) & 0x40 != 0 {
                    self.inputs & self.latched
                } else {
                    self.inputs
                };
                ((level >> (r - 8)) & 1) << 7
            }
            _ => 0,
        }
    }

    /// Level of input pin `n` (0–5): paddles on I0–I3, fire buttons on
    /// I4 (left port) and I5 (right port), low = pressed.
    pub fn set_input(&mut self, n: u8, high: bool) {
        let bit = 1 << n;
        if high {
            self.inputs |= bit;
        } else {
            self.inputs &= !bit;
            if n >= 4 && self.reg(VBLANK) & 0x40 != 0 {
                self.latched &= !bit;
            }
        }
    }

    /// The pot on input `n` (0–3): `analog` 0 (no resistance, charges at
    /// once) to 65534 (fully turned); 65535 is an open line that never
    /// charges.
    pub fn set_pot(&mut self, n: u8, analog: u16) {
        let full = u64::from(PADDLE_LINES) * u64::from(CLOCKS_PER_LINE);
        self.pots[usize::from(n & 3)] =
            (analog != u16::MAX).then(|| (u64::from(analog) * full / 0xfffe) as u32);
    }

    /// Playfield bit `k` (0–39) of the line from the registers.
    fn pf_bit(&self, k: u8) -> bool {
        let k = if k < 20 {
            k
        } else if self.reg(CTRLPF) & 1 != 0 {
            39 - k
        } else {
            k - 20
        };
        match k {
            0..=3 => self.reg(PF0) >> (4 + k) & 1 != 0,
            4..=11 => self.reg(PF1) >> (11 - k) & 1 != 0,
            _ => self.reg(PF2) >> (k - 12) & 1 != 0,
        }
    }

    /// One motion clock for every object.
    fn clock_movers(&mut self) {
        for i in 0..5 {
            self.clock_mover(i);
        }
    }

    fn clock_mover(&mut self, i: usize) {
        match i {
            P0 | P1 => {
                let nusiz = self.reg(NUSIZ0 + i as u8);
                let w = player_width(nusiz);
                self.movers[i].clock(8 * w, copies(nusiz), if w > 1 { 6 } else { 5 });
            }
            M0 | M1 => {
                let nusiz = self.reg(NUSIZ0 + (i - M0) as u8);
                let len = 1 << ((nusiz >> 4) & 3);
                self.movers[i].clock(len, copies(nusiz), 4);
                // Locked to its player: hidden, kept at the player's centre.
                if self.reg(RESMP0 + (i - M0) as u8) & 2 != 0 {
                    let p = self.movers[i - M0];
                    let offset = match player_width(self.reg(NUSIZ0 + (i - M0) as u8)) {
                        1 => 3,
                        2 => 6,
                        _ => 10,
                    };
                    let m = &mut self.movers[i];
                    m.counter = ((u16::from(p.counter) + 160 - offset - 1) % 160) as u8;
                    m.skip = false;
                }
            }
            _ => {
                let len = 1 << ((self.reg(CTRLPF) >> 4) & 3);
                self.movers[BL].clock(len, 0, 4);
            }
        }
    }

    /// HMOVE's counter: from 8 clocks after the strobe, one step every 4
    /// colour clocks; each object gets an extra motion clock per step until
    /// the step number meets its HMxx value.
    fn hmove_tick(&mut self) {
        match self.hmove {
            Some(t) => {
                let t = t + 1;
                self.hmove = Some(t);
                if t < 8 || !(t - 8).is_multiple_of(4) {
                    return;
                }
                let step = (t - 8) / 4;
                for i in 0..5 {
                    let want = ((self.reg(HMP0 + i as u8) >> 4) ^ 8) & 15;
                    if self.movers[i].moving && step == want {
                        self.movers[i].moving = false;
                    }
                    if self.movers[i].moving {
                        self.clock_mover(i);
                    }
                }
                if step == 15 {
                    self.hmove = None;
                }
            }
            // Objects whose comparison never matched keep moving.
            None if self.cc.is_multiple_of(4) => {
                for i in 0..5 {
                    if self.movers[i].moving {
                        self.clock_mover(i);
                    }
                }
            }
            None => {}
        }
    }

    /// What each object puts out at the current clock.
    fn outputs(&self) -> [bool; 5] {
        let player = |i: usize| {
            let m = &self.movers[i];
            m.draw.is_some_and(|d| {
                let bit = d / player_width(self.reg(NUSIZ0 + i as u8));
                let bit = if self.reg(REFP0 + i as u8) & 8 != 0 {
                    bit
                } else {
                    7 - bit
                };
                let vdel = self.reg(VDELP0 + i as u8) & 1 != 0;
                let g = if vdel {
                    self.grp_old[i]
                } else {
                    self.grp_new[i]
                };
                g >> bit & 1 != 0
            })
        };
        let missile = |i: usize| {
            self.movers[M0 + i].draw.is_some()
                && self.reg(ENAM0 + i as u8) & 2 != 0
                && self.reg(RESMP0 + i as u8) & 2 == 0
        };
        let ball_on = if self.reg(VDELBL) & 1 != 0 {
            self.enabl_old
        } else {
            self.enabl_new
        };
        [
            player(P0),
            player(P1),
            missile(0),
            missile(1),
            self.movers[BL].draw.is_some() && ball_on,
        ]
    }

    /// One colour clock. Returns true when a field ended (on VSYNC or
    /// free-running).
    pub fn tick(&mut self) -> bool {
        self.hmove_tick();
        let cc = self.cc;
        if (64..228).contains(&cc) && (cc - 64).is_multiple_of(4) {
            self.pf_out = self.pf_next;
            if cc < 224 {
                self.pf_next = self.pf_bit((cc - 64) / 4);
            }
        }
        if cc >= HBLANK {
            let motion = self.motion();
            if motion {
                self.clock_movers();
            }
            let x = u32::from(cc - HBLANK);
            let colour = if motion {
                let [p0, p1, m0, m1, bl] = self.outputs();
                let pf = self.pf_out;
                self.cx |= collisions(p0, p1, m0, m1, bl, pf);
                self.colour(x, p0 || m0, p1 || m1, bl, pf)
            } else {
                0
            };
            let y = u32::from(self.line);
            if y < height(self.standard) {
                let colour = if self.reg(VBLANK) & 2 != 0 { 0 } else { colour };
                self.pixels[(y * WIDTH + x) as usize] = colour;
            }
        }
        self.clocks += 1;
        if AUDIO_CLOCKS.contains(&cc) {
            for i in 0..2 {
                let (audc, audf) = (self.reg(AUDC0 + i as u8), self.reg(AUDF0 + i as u8));
                self.channels[i].clock(audc, audf);
            }
        }
        self.cc += 1;
        if self.cc == CLOCKS_PER_LINE {
            self.cc = 0;
            self.wsync = false;
            self.hmove_blank = false;
            self.line += 1;
            if self.line >= free_run_lines(self.standard) {
                self.end_field();
            }
        }
        std::mem::take(&mut self.field_ended)
    }

    /// Colour (register value ÷ 2) of pixel `x` from the objects present.
    fn colour(&self, x: u32, p0: bool, p1: bool, bl: bool, pf: bool) -> u8 {
        let ctrl = self.reg(CTRLPF);
        let priority = ctrl & 4 != 0;
        let pf_colour = if ctrl & 2 != 0 && !priority {
            if x < 80 {
                self.reg(COLUP0)
            } else {
                self.reg(COLUP1)
            }
        } else {
            self.reg(COLUPF)
        };
        let field = if pf {
            Some(pf_colour)
        } else if bl {
            Some(self.reg(COLUPF))
        } else {
            None
        };
        let players = if p0 {
            Some(self.reg(COLUP0))
        } else if p1 {
            Some(self.reg(COLUP1))
        } else {
            None
        };
        let c = if priority {
            field.or(players)
        } else {
            players.or(field)
        };
        c.unwrap_or(self.reg(COLUBK)) >> 1
    }

    fn end_field(&mut self) {
        // Lines the field did not reach show black.
        let from = (u32::from(self.line) + 1).min(height(self.standard)) * WIDTH;
        self.pixels[from as usize..].fill(0);
        self.line = 0;
        self.frame += 1;
        self.field_ended = true;
    }

    /// Power-up (the input pins and pots stay as the controllers drive
    /// them).
    pub fn reset(&mut self) {
        let (inputs, pots) = (self.inputs, self.pots);
        *self = Self::new(self.standard);
        self.inputs = inputs;
        self.pots = pots;
    }
}

const TAG: [u8; 4] = *b"TIA ";
const VERSION: u16 = 1;

impl Savestate for Tia {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.regs);
            w.u8(self.cc);
            w.u16(self.line);
            w.u64(self.frame);
            w.bool(self.wsync);
            w.bool(self.field_ended);
            w.u8(self.inputs);
            w.u8(self.latched);
            for m in &self.movers {
                m.save(w);
            }
            w.raw(&self.grp_new);
            w.raw(&self.grp_old);
            w.bool(self.enabl_new);
            w.bool(self.enabl_old);
            w.bool(self.pf_out);
            w.bool(self.pf_next);
            w.u8(self.hmove.unwrap_or(0xff));
            w.bool(self.hmove_blank);
            w.u16(self.cx);
            for ch in &self.channels {
                ch.save(w);
            }
            w.u64(self.clocks);
            for p in &self.pots {
                w.u32(p.unwrap_or(u32::MAX));
            }
            w.u64(self.charge_from);
            w.raw(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        c.raw_into(&mut n.regs)?;
        n.cc = c.u8()?;
        n.line = c.u16()?;
        n.frame = c.u64()?;
        n.wsync = c.bool()?;
        n.field_ended = c.bool()?;
        n.inputs = c.u8()?;
        n.latched = c.u8()?;
        for m in &mut n.movers {
            m.load(&mut c)?;
        }
        c.raw_into(&mut n.grp_new)?;
        c.raw_into(&mut n.grp_old)?;
        n.enabl_new = c.bool()?;
        n.enabl_old = c.bool()?;
        n.pf_out = c.bool()?;
        n.pf_next = c.bool()?;
        n.hmove = match c.u8()? {
            0xff => None,
            t if t <= 8 + 4 * 15 => Some(t),
            _ => return Err(StateError::Invalid("TIA HMOVE")),
        };
        n.hmove_blank = c.bool()?;
        n.cx = c.u16()?;
        for ch in &mut n.channels {
            ch.load(&mut c)?;
        }
        n.clocks = c.u64()?;
        for p in &mut n.pots {
            *p = match c.u32()? {
                u32::MAX => None,
                t => Some(t),
            };
        }
        n.charge_from = c.u64()?;
        if n.charge_from > n.clocks {
            return Err(StateError::Invalid("TIA paddles"));
        }
        c.raw_into(&mut n.pixels)?;
        if n.cc >= CLOCKS_PER_LINE || n.line >= free_run_lines(n.standard) {
            return Err(StateError::Invalid("TIA beam"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
