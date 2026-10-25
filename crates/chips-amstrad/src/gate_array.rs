//! Gate Array (40007/40008/40010): video, palette, screen mode, ROM
//! enables, interrupts — and the arbitration of the bus between the Z80
//! and video (cpcwiki, "Gate Array").
//!
//! **Port &7Fxx** (A15 = 0, A14 = 1), function in bits 7–6:
//!
//! | Bits 7–6 | Function | Bits                                              |
//! | -------- | -------- | ------------------------------------------------- |
//! | 00       | PENR     | bit 4 = 1 border, else bits 3–0 pen               |
//! | 01       | INKR     | bits 4–0 hardware colour of the selected pen      |
//! | 10       | RMR      | bits 1–0 mode, 2 lower ROM off, 3 upper ROM off, 4 reset the interrupt counter; bit 5 is ignored (only the Plus ASIC decodes it; Arnold acid test cpctest "MRER … bit 5=1") |
//! | 11       | MMR      | RAM configuration — the 6128's PAL, not the Gate Array ([`GateArray::write`] returns it) |
//!
//! **Video.** Each microsecond the Gate Array fetches two bytes from the
//! first 64 KB at `MA13–12 · RA2–0 · MA9–0 · 0/1` (bits 15–14, 13–11,
//! 10–1, 0) and turns them into 16 pixels at 16 MHz: mode 0 two pixels of
//! 16 pens, mode 1 four of 4, mode 2 eight of 2, mode 3 two of 4. A new
//! mode takes effect at the next HSYNC. Outside DISPTMG the border pen
//! shows.
//!
//! **Interrupts.** A 6-bit counter counts HSYNC falling edges; at 52 it
//! requests an interrupt and restarts. Two HSYNCs after VSYNC starts the
//! counter restarts too, requesting an interrupt if it was 32 or more (so
//! the six interrupts of a frame stay in step with it). The request stays
//! until the Z80 acknowledges it, which also clears bit 5 of the counter
//! (no interrupt within 32 lines of the last), or RMR bit 4 clears both.
//!
//! **Monitor.** The picture is the part of the field a monitor shows: 48 µs
//! of each line from 14 µs after HSYNC starts (4 µs of left border with
//! the standard CRTC settings), 272 lines from 36 lines after VSYNC starts;
//! without sync the monitor free-runs at 64 µs × 312 lines.
//!
//! **CPU wait states.** Video and the Z80 share the RAM; the Gate Array
//! holds the Z80's /WAIT low on three of every four 4 MHz T-states, so
//! every bus access is stretched until the T-state the Z80 samples /WAIT
//! on (T2 of opcode fetches and memory cycles, the automatic TW of I/O
//! cycles) is the free one. Internal T-states are not stretched. With the
//! Z80's M-cycle placement this gives the well-known instruction lengths in
//! whole microseconds ("NOPs"), e.g. `OUT (C),r` 4 µs, `EX (SP),HL` 6 µs
//! (cpcwiki, "Z80" — CPC instruction timings; checked by
//! `machine-cpc/tests/z80_timing.rs`).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

use crate::crtc::CrtcOut;

/// Picture size in 16 MHz pixels: 48 µs × 272 lines.
pub const WIDTH: u32 = 768;
pub const HEIGHT: u32 = 272;
/// Pixels per microsecond.
const PIXELS_PER_US: usize = 16;
/// Monitor line length and field length when free-running.
const LINE_US: u8 = 64;
const FIELD_LINES: u16 = 312;
/// Monitor position after the microsecond HSYNC starts: the next line
/// begins 13 µs later.
const HSYNC_PHASE: u8 = LINE_US - 13;
/// Monitor line after the line VSYNC starts: the field begins 36 lines
/// later.
const VSYNC_PHASE: u16 = FIELD_LINES - 36;
/// Hardware colour 20, black: sync.
const BLACK: u8 = 20;

/// Wait states for a Z80 access sampling /WAIT on master T-state `t`
/// (4 MHz T-states since power-on): /WAIT is released on T-states with
/// `t % 4 == 0`.
pub fn cpu_wait_states(t: u64) -> u32 {
    ((4 - t % 4) % 4) as u32
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateArray {
    /// Selected pen (16 = border).
    pub pen: u8,
    /// Hardware colours of pens 0–15 and the border (16).
    pub inks: [u8; 17],
    /// Last RMR value (bits 4–0).
    pub rmr: u8,
    /// The mode in effect (latched at HSYNC).
    pub mode: u8,
    /// Interrupt line counter and the pending request.
    pub int_counter: u8,
    pub int_pending: bool,
    /// HSYNCs to go after VSYNC before the counter restarts.
    vsync_wait: u8,
    prev_hsync: bool,
    prev_vsync: bool,
    /// Monitor beam: microsecond in the line, line in the field.
    mon_x: u8,
    mon_y: u16,
    fields: u64,
    field_done: bool,
    pixels: Vec<u8>,
}

impl Default for GateArray {
    fn default() -> Self {
        Self::new()
    }
}

impl GateArray {
    pub fn new() -> Self {
        Self {
            pen: 0,
            inks: [BLACK; 17],
            rmr: 0,
            mode: 0,
            int_counter: 0,
            int_pending: false,
            vsync_wait: 0,
            prev_hsync: false,
            prev_vsync: false,
            mon_x: 0,
            mon_y: 0,
            fields: 0,
            field_done: false,
            pixels: vec![BLACK; (WIDTH * HEIGHT) as usize],
        }
    }

    /// Reset (power-up): mode 0, both ROMs on, no interrupt.
    pub fn reset(&mut self) {
        self.rmr = 0;
        self.mode = 0;
        self.int_counter = 0;
        self.int_pending = false;
        self.vsync_wait = 0;
    }

    /// Pixels as hardware colour numbers (0–31), `WIDTH × HEIGHT`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn lower_rom(&self) -> bool {
        self.rmr & 0x04 == 0
    }

    pub fn upper_rom(&self) -> bool {
        self.rmr & 0x08 == 0
    }

    /// Monitor beam (line, microsecond) and completed fields.
    pub fn beam(&self) -> (u16, u8, u64) {
        (self.mon_y, self.mon_x, self.fields)
    }

    /// A field was completed since the last call.
    pub fn take_field_done(&mut self) -> bool {
        std::mem::take(&mut self.field_done)
    }

    /// Port &7Fxx. Returns the value for the RAM configuration (MMR),
    /// which the machine's PAL decodes.
    pub fn write(&mut self, v: u8) -> Option<u8> {
        match v >> 6 {
            0 => {
                self.pen = if v & 0x10 != 0 { 16 } else { v & 0x0f };
            }
            1 => self.inks[usize::from(self.pen)] = v & 0x1f,
            2 => {
                self.rmr = v & 0x1f;
                if v & 0x10 != 0 {
                    self.int_counter = 0;
                    self.int_pending = false;
                }
            }
            _ => return Some(v),
        }
        None
    }

    /// The Z80 acknowledged the interrupt.
    pub fn acknowledge(&mut self) {
        self.int_pending = false;
        self.int_counter &= 0x1f;
    }

    /// One microsecond: draw what the CRTC shows from `ram` (the first
    /// 64 KB), then run the interrupt counter and the monitor's sync.
    pub fn tick(&mut self, crtc: CrtcOut, ram: &[u8]) {
        self.draw(crtc, ram);
        let hsync_start = crtc.hsync && !self.prev_hsync;
        let hsync_end = !crtc.hsync && self.prev_hsync;
        let vsync_start = crtc.vsync && !self.prev_vsync;
        self.prev_hsync = crtc.hsync;
        self.prev_vsync = crtc.vsync;
        if hsync_start {
            self.mode = self.rmr & 3;
        }
        if vsync_start {
            self.vsync_wait = 2;
        }
        if hsync_end {
            self.int_counter += 1;
            if self.int_counter == 52 {
                self.int_counter = 0;
                self.int_pending = true;
            }
            if self.vsync_wait > 0 {
                self.vsync_wait -= 1;
                if self.vsync_wait == 0 {
                    if self.int_counter >= 32 {
                        self.int_pending = true;
                    }
                    self.int_counter = 0;
                }
            }
        }
        // Monitor beam.
        let mut x = if hsync_start {
            HSYNC_PHASE
        } else {
            self.mon_x + 1
        };
        if x >= LINE_US {
            x = 0;
            self.mon_y += 1;
            if self.mon_y >= FIELD_LINES {
                self.mon_y = 0;
                self.fields += 1;
                self.field_done = true;
            }
        }
        self.mon_x = x;
        if vsync_start {
            self.mon_y = VSYNC_PHASE;
        }
    }

    fn draw(&mut self, crtc: CrtcOut, ram: &[u8]) {
        let (x, y) = (usize::from(self.mon_x), usize::from(self.mon_y));
        if x >= WIDTH as usize / PIXELS_PER_US || y >= HEIGHT as usize {
            return;
        }
        let at = y * WIDTH as usize + x * PIXELS_PER_US;
        let out = &mut self.pixels[at..at + PIXELS_PER_US];
        if crtc.hsync || crtc.vsync {
            out.fill(BLACK);
            return;
        }
        if !crtc.disp {
            out.fill(self.inks[16]);
            return;
        }
        let ma = usize::from(crtc.ma);
        let addr = ((ma & 0x3000) << 2) | (usize::from(crtc.ra & 7) << 11) | ((ma & 0x3ff) << 1);
        let mut pens = [0u8; 16];
        for (k, &b) in [ram[addr & 0xffff], ram[(addr + 1) & 0xffff]]
            .iter()
            .enumerate()
        {
            let bit = |n: u8| (b >> n) & 1;
            let half = &mut pens[k * 8..k * 8 + 8];
            match self.mode {
                0 => {
                    let a = bit(7) | bit(3) << 1 | bit(5) << 2 | bit(1) << 3;
                    let c = bit(6) | bit(2) << 1 | bit(4) << 2 | bit(0) << 3;
                    half[..4].fill(a);
                    half[4..].fill(c);
                }
                1 => {
                    for i in 0..4 {
                        let p = bit(7 - i) | bit(3 - i) << 1;
                        half[usize::from(i) * 2..usize::from(i) * 2 + 2].fill(p);
                    }
                }
                2 => {
                    for i in 0..8 {
                        half[usize::from(i)] = bit(7 - i);
                    }
                }
                _ => {
                    half[..4].fill(bit(7) | bit(3) << 1);
                    half[4..].fill(bit(6) | bit(2) << 1);
                }
            }
        }
        for (o, p) in out.iter_mut().zip(pens) {
            *o = self.inks[usize::from(p)];
        }
    }
}

/// The address of the first of the two bytes the Gate Array fetches for
/// CRTC address `ma` and row address `ra`: `MA13–12 · RA2–0 · MA9–0 · 0`
/// (bits 15–14, 13–11, 10–1, 0); MA11–10 and RA4–3 are not connected.
pub fn video_address(ma: u16, ra: u8) -> u16 {
    ((ma & 0x3000) << 2) | (u16::from(ra & 7) << 11) | ((ma & 0x3ff) << 1)
}

/// The pens of the pixels a screen byte holds in `mode`, left to right,
/// and how many there are: mode 0 two of 16 pens (bits 7 3 5 1 and 6 2 4
/// 0, least significant first), mode 1 four of 4 (bits 7/3 … 4/0), mode 2
/// eight of 2, mode 3 two of 4 (bits 7/3 and 6/2). For the graphics
/// views; [`GateArray`]'s drawing keeps its own decoding, specialised per
/// mode (sharing this function cost the CPC about 20 % of its speed,
/// measured 2026-10-25); `byte_pens_match_the_drawn_pixels` checks that
/// both agree.
pub fn byte_pens(mode: u8, b: u8) -> ([u8; 8], usize) {
    let bit = |n: u8| (b >> n) & 1;
    let mut px = [0u8; 8];
    let n = match mode & 3 {
        0 => {
            px[0] = bit(7) | bit(3) << 1 | bit(5) << 2 | bit(1) << 3;
            px[1] = bit(6) | bit(2) << 1 | bit(4) << 2 | bit(0) << 3;
            2
        }
        1 => {
            for i in 0..4u8 {
                px[usize::from(i)] = bit(7 - i) | bit(3 - i) << 1;
            }
            4
        }
        2 => {
            for i in 0..8u8 {
                px[usize::from(i)] = bit(7 - i);
            }
            8
        }
        _ => {
            px[0] = bit(7) | bit(3) << 1;
            px[1] = bit(6) | bit(2) << 1;
            2
        }
    };
    (px, n)
}

const TAG: [u8; 4] = *b"GATE";
const VERSION: u16 = 1;

impl Savestate for GateArray {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.inks);
            for v in [
                self.pen,
                self.rmr,
                self.mode,
                self.int_counter,
                self.vsync_wait,
                self.mon_x,
            ] {
                w.u8(v);
            }
            for v in [
                self.int_pending,
                self.prev_hsync,
                self.prev_vsync,
                self.field_done,
            ] {
                w.bool(v);
            }
            w.u16(self.mon_y);
            w.u64(self.fields);
            w.raw(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        c.raw_into(&mut n.inks)?;
        for v in [
            &mut n.pen,
            &mut n.rmr,
            &mut n.mode,
            &mut n.int_counter,
            &mut n.vsync_wait,
            &mut n.mon_x,
        ] {
            *v = c.u8()?;
        }
        for v in [
            &mut n.int_pending,
            &mut n.prev_hsync,
            &mut n.prev_vsync,
            &mut n.field_done,
        ] {
            *v = c.bool()?;
        }
        n.mon_y = c.u16()?;
        n.fields = c.u64()?;
        c.raw_into(&mut n.pixels)?;
        if n.pen > 16
            || n.inks.iter().any(|&i| i > 31)
            || n.mode > 3
            || n.int_counter > 63
            || n.mon_x >= LINE_US
            || n.mon_y >= FIELD_LINES
            || n.pixels.iter().any(|&p| p > 31)
        {
            return Err(StateError::Invalid("Gate Array"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crtc::{Crtc, CrtcType};

    #[test]
    fn waits_until_the_free_t_state() {
        assert_eq!(cpu_wait_states(8), 0);
        assert_eq!(cpu_wait_states(9), 3);
        assert_eq!(cpu_wait_states(10), 2);
        assert_eq!(cpu_wait_states(11), 1);
    }

    #[test]
    fn rmr_ignores_bit_5() {
        let mut ga = GateArray::new();
        ga.write(0b1000_1100);
        ga.write(0b1010_0001);
        assert_eq!(ga.rmr, 0b0_0001, "ROMs back on, mode 1");
    }

    fn cpc_crtc() -> Crtc {
        let mut c = Crtc::new(CrtcType::T1);
        for (r, v) in [63, 40, 46, 0x8e, 38, 0, 25, 30, 0, 7, 0, 0, 0x30, 0]
            .into_iter()
            .enumerate()
        {
            c.select(r as u8);
            c.write(v);
        }
        c
    }

    #[test]
    fn six_interrupts_per_field_in_step_with_vsync() {
        let mut crtc = cpc_crtc();
        let mut ga = GateArray::new();
        let ram = vec![0u8; 0x10000];
        // Settle, then count interrupts and their lines over one field.
        let mut ints = Vec::new();
        for us in 0..(3 * 312 * 64) {
            ga.tick(crtc.tick(), &ram);
            if ga.int_pending {
                ga.acknowledge();
                if us >= 2 * 312 * 64 {
                    ints.push(us / 64 % 312);
                }
            }
        }
        assert_eq!(ints.len(), 6, "{ints:?}");
        // 52 lines apart.
        for w in ints.windows(2) {
            assert_eq!((w[1] + 312 - w[0]) % 312, 52, "{ints:?}");
        }
    }

    #[test]
    fn modes_decode_pixels_and_border() {
        let mut crtc = cpc_crtc();
        let mut ga = GateArray::new();
        let mut ram = vec![0u8; 0x10000];
        // Screen at &C000: first byte of the first line.
        ram[0xc000] = 0b1000_1000; // mode 1: pixel 0 pen 3, mode 2: pixels 0 and 4
        for (pen, colour) in [(0, 20), (1, 1), (2, 2), (3, 3)] {
            ga.write(pen);
            ga.write(0x40 | colour);
        }
        ga.write(0x10);
        ga.write(0x40 | 4); // border blue
        ga.write(0x81); // mode 1
        for _ in 0..(2 * 312 * 64) {
            ga.tick(crtc.tick(), &ram);
        }
        let row = (36 * WIDTH) as usize;
        let px = ga.pixels();
        // Left border 4 µs, then the screen.
        assert_eq!(px[row], 4, "border");
        let s = row + 4 * 16;
        assert_eq!(&px[s..s + 4], &[3, 3, 20, 20]);
    }

    #[test]
    fn byte_pens_match_the_drawn_pixels() {
        let mut ram = vec![0u8; 0x10000];
        let out = CrtcOut {
            disp: true,
            ..CrtcOut::default()
        };
        for mode in 0..4 {
            for b in [0x00, 0x81, 0x5a, 0xc3, 0x0f, 0xff] {
                ram[0] = b;
                ram[1] = !b;
                let mut ga = GateArray::new();
                for pen in 0..16 {
                    ga.inks[pen] = pen as u8;
                }
                ga.mode = mode;
                ga.draw(out, &ram);
                let mut want = Vec::new();
                for v in [b, !b] {
                    let (px, n) = byte_pens(mode, v);
                    for &p in &px[..n] {
                        want.extend(std::iter::repeat_n(p, 8 / n));
                    }
                }
                assert_eq!(&ga.pixels()[..16], &want[..], "mode {mode} byte {b:02x}");
            }
        }
    }

    #[test]
    fn savestate_round_trip() {
        let mut ga = GateArray::new();
        ga.write(0x41);
        ga.write(0x92);
        let mut w = StateWriter::new();
        ga.save(&mut w);
        let bytes = w.into_bytes();
        let mut g = GateArray::new();
        g.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(ga, g);
    }
}
