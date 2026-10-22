//! The ZX81 ULA.
//!
//! The ZX81 has no frame buffer: the Z80 "executes" the display file and
//! the ULA turns the opcode fetches into pixels.
//!
//! - **Display fetch.** On an M1 cycle with A15 set and data bit 6 clear
//!   (and the CPU not halted) the ULA gives the CPU a NOP instead and takes
//!   the byte as a character code: during the refresh cycle it reads the
//!   pattern byte at `I·256 + (code & $3F)·8 + LCNTR` (bit 7 of the code
//!   inverts it) into its shift register, which shifts out one pixel per
//!   6.5 MHz pixel clock — two per T-state, eight per 4-T-state NOP, so a
//!   row of 32 characters is 128 T-states of continuous pixels.
//! - **Lines.** A horizontal counter of [`T_PER_LINE`] T-states runs
//!   freely; each wrap is an HSYNC, which advances the 3-bit line counter
//!   LCNTR (the pattern row) and, while the NMI generator is on, pulses
//!   /NMI. The ROM's display routine lets the halted CPU wait for these.
//! - **/INT** comes from address line A6 during refresh: while the R
//!   register's bit 6 is 0 the CPU sees /INT asserted. The ROM loads R so
//!   that the interrupt arrives after exactly one row of characters.
//! - **/WAIT.** While the CPU is halted, the ULA holds /WAIT during the
//!   NMI pulse, so the NMI is acknowledged at a fixed point of the line
//!   whatever the HALT's phase ([`Ula::nmi_release_t`]).
//! - **Port $FE.** `IN` (A0 = 0): keyboard row by A8–A15 (bits 0–4, 0 =
//!   pressed), bit 6 = 0 for a 60 Hz machine (1 here, PAL), bit 7 the
//!   tape input; the read also starts VSYNC and resets LCNTR unless the NMI
//!   generator is on. Any `OUT` ends VSYNC; `OUT ($FD)` (A1 = 0) turns the
//!   NMI generator off, `OUT ($FE)` (A0 = 0, A1 = 1) on. The end of VSYNC is
//!   where the next field starts.
//!
//! The picture is kept as one bit per pixel in a fixed window of the
//! [`T_PER_LINE`] × 310-line raster: [`WIDTH`] × [`HEIGHT`] pixels starting
//! [`H_OFFSET`] T-states after HSYNC and [`V_OFFSET`] lines after VSYNC,
//! placed so the ROM's 32 × 24 display sits centred with equal borders.
//! Colour 0 is the white background, 1 black.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// T-states per line (3.25 MHz: 64 µs minus a bit — the ULA counts 207).
pub const T_PER_LINE: u32 = 207;
/// Lines of a PAL field as the ROM makes them (56 + 192 + 56 + sync).
pub const LINES_PAL: u32 = 310;
/// Picture window.
pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;
/// First T-state of the window after HSYNC (the ROM's rows start at
/// T-state 72 ± the display routine's phase; 256 pixels = 128 T-states).
pub const H_OFFSET: u32 = 35;
/// First line of the window after VSYNC (56 blank lines, 24 of border).
pub const V_OFFSET: u32 = 32;
/// Pixel clock: two pixels per T-state.
pub const PIXELS_PER_T: u32 = 2;

/// What one T-state produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickOut {
    /// An HSYNC began on this T-state.
    pub hsync: bool,
    /// The NMI generator pulsed.
    pub nmi: bool,
    /// A field ended (VSYNC finished): the picture is complete.
    pub field_done: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ula {
    /// T-state within the line (0 = HSYNC start).
    pub hcount: u32,
    /// Lines since VSYNC ended.
    pub line: u32,
    /// The pattern row counter (0–7).
    pub lcntr: u8,
    pub nmi_on: bool,
    pub vsync: bool,
    /// Fields since power-on.
    pub fields: u64,
    /// Pixels to shift out (MSB first) and how many are left.
    shift: u8,
    shift_left: u8,
    /// Tape input (EAR) level.
    pub ear: bool,
    pixels: Vec<u8>,
}

impl Default for Ula {
    fn default() -> Self {
        Self::new()
    }
}

impl Ula {
    pub fn new() -> Self {
        Self {
            hcount: 0,
            line: 0,
            lcntr: 0,
            nmi_on: false,
            vsync: false,
            fields: 0,
            shift: 0,
            shift_left: 0,
            ear: false,
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
        }
    }

    /// Power-on: the counters restart; the picture stays.
    pub fn reset(&mut self) {
        let pixels = std::mem::take(&mut self.pixels);
        *self = Self::new();
        self.pixels = pixels;
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// One T-state of the video signal: draws two pixels and advances the
    /// horizontal counter.
    pub fn tick(&mut self) -> TickOut {
        let mut out = TickOut::default();
        // The two pixels of this T-state.
        if !self.vsync {
            let y = self.line.wrapping_sub(V_OFFSET);
            let x0 = self
                .hcount
                .wrapping_sub(H_OFFSET)
                .wrapping_mul(PIXELS_PER_T);
            for i in 0..PIXELS_PER_T {
                let px = if self.shift_left > 0 {
                    let bit = self.shift & 0x80 != 0;
                    self.shift <<= 1;
                    self.shift_left -= 1;
                    u8::from(bit)
                } else {
                    0
                };
                let x = x0.wrapping_add(i);
                if y < HEIGHT && x < WIDTH {
                    self.pixels[(y * WIDTH + x) as usize] = px;
                }
            }
        } else {
            self.shift_left = 0;
        }
        self.hcount += 1;
        if self.hcount >= T_PER_LINE {
            self.hcount = 0;
            self.line += 1;
            out.hsync = true;
            if !self.vsync {
                self.lcntr = (self.lcntr + 1) & 7;
            }
            if self.nmi_on {
                out.nmi = true;
            }
        }
        out
    }

    /// An M1 fetch by the CPU: `addr` and the byte memory delivered.
    /// Returns the byte the CPU gets (a NOP for a display character) and
    /// loads the shift register from `pattern` (the byte at the pattern
    /// address, fetched by the caller from `pattern_addr`).
    pub fn m1(&mut self, addr: u16, byte: u8, halted: bool) -> (u8, Option<u16>) {
        if addr & 0x8000 != 0 && byte & 0x40 == 0 && !halted {
            // Not an address: the caller completes it with the I register.
            let low = u16::from(byte & 0x3f) << 3 | u16::from(self.lcntr);
            (0x00, Some(low | u16::from(byte & 0x80) << 8))
        } else {
            (byte, None)
        }
    }

    /// The pattern byte for a display character arrived: shift it out.
    pub fn load_pattern(&mut self, pattern: u8, invert: bool) {
        self.shift = if invert { !pattern } else { pattern };
        self.shift_left = 8;
    }

    /// `IN A,($FE)`: starts VSYNC (unless the NMI generator runs) and
    /// resets the line counter. `keys`: the selected rows' pressed keys
    /// (bits 0–4, 1 = pressed).
    pub fn in_fe(&mut self, keys: u8) -> u8 {
        if !self.nmi_on {
            self.vsync = true;
            self.lcntr = 0;
        }
        (!keys & 0x1f) | 0x20 | 0x40 | u8::from(self.ear) << 7
    }

    /// Any `OUT`: ends VSYNC (a field boundary); A1 = 0 stops, A0 = 0 with
    /// A1 = 1 starts the NMI generator. Returns true at a field boundary.
    pub fn out(&mut self, port: u16) -> bool {
        let mut field = false;
        if self.vsync {
            self.vsync = false;
            self.line = 0;
            self.fields += 1;
            field = true;
        }
        if port & 0x02 == 0 {
            self.nmi_on = false;
        } else if port & 0x01 == 0 {
            self.nmi_on = true;
        }
        field
    }

    /// No VSYNC for a whole field (FAST mode, tape routines): a TV loses
    /// sync and shows nothing; the picture is blanked and a field counted so
    /// the machine keeps delivering frames.
    pub fn force_field(&mut self) {
        self.pixels.fill(0);
        self.line = 0;
        self.fields += 1;
    }

    /// T-states from horizontal position `hcount` until the point where the
    /// ULA releases /WAIT after an NMI: the end of the HSYNC pulse,
    /// [`NMI_RELEASE`] T-states into the line.
    pub fn nmi_release_t(hcount: u32) -> u32 {
        (NMI_RELEASE + T_PER_LINE - hcount) % T_PER_LINE
    }
}

/// Where in the line a halted CPU is let go after the NMI (the HSYNC pulse
/// is about 16 T-states; the NMI acknowledge then takes its 11).
pub const NMI_RELEASE: u32 = 16;

const TAG: [u8; 4] = *b"ZULA";

impl Savestate for Ula {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u32(self.hcount);
            w.u32(self.line);
            w.u8(self.lcntr);
            w.bool(self.nmi_on);
            w.bool(self.vsync);
            w.u64(self.fields);
            w.u8(self.shift);
            w.u8(self.shift_left);
            w.bool(self.ear);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let mut n = Self::new();
        n.hcount = c.u32()?;
        n.line = c.u32()?;
        n.lcntr = c.u8()? & 7;
        n.nmi_on = c.bool()?;
        n.vsync = c.bool()?;
        n.fields = c.u64()?;
        n.shift = c.u8()?;
        n.shift_left = c.u8()?;
        n.ear = c.bool()?;
        if n.hcount >= T_PER_LINE || n.shift_left > 8 {
            return Err(StateError::Invalid("ULA counters"));
        }
        n.pixels = std::mem::take(&mut self.pixels);
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_fetches_become_nops_and_pixels() {
        let mut u = Ula::new();
        u.lcntr = 3;
        // A fetch below $8000, or with bit 6 set, passes through.
        assert_eq!(u.m1(0x4000, 0x27, false), (0x27, None));
        assert_eq!(
            u.m1(0xc000, 0x76, false),
            (0x76, None),
            "HALT keeps its bit 6"
        );
        // A character: NOP, pattern address low part code·8 + row, invert flag.
        assert_eq!(u.m1(0xc000, 0x27, false), (0x00, Some(0x27 << 3 | 3)));
        assert_eq!(
            u.m1(0xc000, 0xa7, false),
            (0x00, Some(0x8000 | 0x27 << 3 | 3))
        );
        assert_eq!(
            u.m1(0xc000, 0x27, true),
            (0x27, None),
            "no video while halted"
        );
        // Shift out $A5 over four T-states inside the window.
        u.line = V_OFFSET;
        u.hcount = H_OFFSET;
        u.load_pattern(0xa5, false);
        for _ in 0..4 {
            u.tick();
        }
        let row = &u.pixels()[..8];
        assert_eq!(row, &[1, 0, 1, 0, 0, 1, 0, 1]);
        assert_eq!(&u.pixels()[8..10], &[0, 0], "white after the pattern");
        u.hcount = H_OFFSET;
        u.load_pattern(0xa5, true);
        for _ in 0..4 {
            u.tick();
        }
        assert_eq!(&u.pixels()[..8], &[0, 1, 0, 1, 1, 0, 1, 0], "inverse video");
    }

    #[test]
    fn lines_hsync_lcntr_and_the_nmi_generator() {
        let mut u = Ula::new();
        assert!(!u.out(0xfe) && u.nmi_on, "OUT ($FE): NMI on");
        let mut nmis = 0;
        let mut hsyncs = 0;
        for _ in 0..T_PER_LINE * 10 {
            let o = u.tick();
            nmis += u32::from(o.nmi);
            hsyncs += u32::from(o.hsync);
        }
        assert_eq!((hsyncs, nmis), (10, 10));
        assert_eq!(u.lcntr, 10 & 7);
        assert_eq!(u.line, 10);
        u.out(0xfd);
        assert!(!u.nmi_on, "OUT ($FD): NMI off");
        // IN ($FE) starts VSYNC and resets LCNTR; the next OUT ends it and a
        // field begins.
        assert_eq!(u.in_fe(0b00101) & 0x1f, 0b11010);
        assert!(u.vsync);
        assert_eq!(u.lcntr, 0);
        for _ in 0..T_PER_LINE * 3 {
            u.tick();
        }
        assert_eq!(u.lcntr, 0, "LCNTR holds during VSYNC");
        assert!(u.out(0xff), "field boundary");
        assert_eq!((u.line, u.fields), (0, 1));
        // With the NMI generator on, IN ($FE) does not start VSYNC.
        u.out(0xfe);
        u.in_fe(0);
        assert!(!u.vsync);
        assert_eq!(Ula::nmi_release_t(0), NMI_RELEASE);
        assert_eq!(Ula::nmi_release_t(NMI_RELEASE + 1), T_PER_LINE - 1);
    }

    #[test]
    fn savestate_round_trip() {
        let mut u = Ula::new();
        u.out(0xfe);
        for _ in 0..500 {
            u.tick();
        }
        u.ear = true;
        let mut w = StateWriter::new();
        u.save(&mut w);
        let s = w.into_bytes();
        let mut back = Ula::new();
        back.load(&mut StateReader::new(&s)).unwrap();
        back.pixels = u.pixels.clone();
        assert_eq!(back, u);
    }
}
