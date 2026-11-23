//! The MAX-80's device registers, memory-mapped at the top of the first
//! half of its 4 KB block (offsets into the block; each register answers
//! at four addresses):
//!
//! | Offset | Read | Write |
//! | --- | --- | --- |
//! | `$7D0` | (RAM) | baud rate of serial channel A (COM8116 code, bits 3–0) |
//! | `$7D4` | (RAM) | baud rate of serial channel B |
//! | `$7D8` | (RAM) | MLATCH: bits 3–0 drive select 1–4 (one bit), 4 side, 5 the 8-inch connector (and the controller's doubled clock), 6 double density |
//! | `$7DC` | (RAM) | VLATCH: bits 7–4 the 4 KB page the block sits at, 3 double-width characters, 2–0 what the block's first 1 KB shows (see `lib.rs`) |
//! | `$7E0` | SYSFLG: bit 7 the tick, 6 the CRTC's display enable, 5 the 5.25-inch motors run, 4 the selected 8-inch disk is single-sided; a read at an even address clears the tick | CRTC HD46505: even the register number, odd the register |
//! | `$7E4` | Z80 SIO: data A, control A, data B, control B | the same |
//! | `$7E8` | printer status: bit 7 busy, 6 out of paper, 5 selected, 4 no fault | printer data (strobed) |
//! | `$7EC` | floppy controller MB8876: status, track, sector, data, all inverted | command, track, sector, data, inverted |
//! | `$7F0` | SASI data | SASI data |
//! | `$7F4` | USTAT: bit 7 the controller's DRQ, 6 its INTRQ, both low when active; bits 4–0 the SASI lines | SASI control |
//! | `$7F8` | the DIP switches (bits 2–0 the boot device: 1 a 5.25-inch floppy, 2 an 8-inch one, 0 none) | any write beeps |
//! | `$7FC` | Z80 PIO: data A, control A, data B, control B | the same |
//!
//! The tick comes 8 MHz / 2^17 = 61.035 times a second and pulls /INT
//! until SYSFLG is read. The PIO's port A carries the RAM banking in bits
//! 7–4 (see `lib.rs`) and the clock chip's data in bits 3–0, port B the
//! clock chip's address (bits 3–0) and its WRITE (4), READ (5) and HOLD
//! (6) lines. The clock chip is an MSM5832: thirteen digits, seconds to
//! years. Here it starts at 1984-01-01 00:00:00 and counts emulated
//! seconds (no host time: the machine stays deterministic); setting it
//! needs the TIME SET button on the back held, which is not built, so
//! writes change nothing.
//!
//! The 5.25-inch drives' motors run for about 3.1 seconds after a write to
//! MLATCH that selects their connector; 8-inch drives turn all the time.
//! A drive is ready when its disk is of the selected connector's kind.
//!
//! The beep is a free-running 1953 Hz tone (500 kHz / 256) switched on by
//! a write to `$7F8` until the fourth tick after it.
//!
//! Not built: the serial ports' lines (the SIO takes its registers, says
//! its transmitter is empty and receives nothing), the printer (always
//! ready, the bytes go nowhere), the SASI port (no drive answers), the
//! expansion bus.
//!
//! Sources: the lobo_max80 emulator's and diagnostic disk's documentation
//! of the machine (MIT; they cite Lobo's Technical Reference Manual, its
//! schematics and boot ROM source, and measurements on a machine); see
//! `docs/sources.md`. The implementation is this project's own.

use drives_other::wd17xx::{Chip, Drive, Fdc};
use mimic_core::{StateError, StateReader, StateWriter};

pub const DRIVES: usize = 4;
/// 15.2 MHz: the dot clock, three times the processor's.
pub const DOT_HZ: u64 = 15_200_000;
/// The processor's clock, rounded (the rate is `DOT_HZ / 3`).
pub const CPU_HZ: u32 = 5_066_667;
const PRINTER_READY: u8 = 0x30;
/// SIO RR0 with nothing happening: transmitter empty, CTS and DCD on.
const SIO_RR0: u8 = 0x2c;
/// How long the 5.25-inch motors run after a select, in milliseconds.
const MOTOR_MS: u64 = 3146;

/// The processor clock at which tick `n` comes.
pub fn tick_cycle(n: u64) -> u64 {
    (u128::from(n) * u128::from(DOT_HZ) * 131_072 / 24_000_000) as u64
}

/// One channel of the SIO: its register pointer and write registers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SioChannel {
    pub pointer: u8,
    pub wr: [u8; 8],
}

pub struct Io {
    pub fdc: Fdc,
    pub drives: Vec<Drive>,
    pub vlatch: u8,
    pub mlatch: u8,
    pub crtc: [u8; 18],
    pub crtc_index: u8,
    pub pio_a: u8,
    pub pio_b: u8,
    /// The PIO ports' input masks (mode 3: a 1 is an input pin).
    pub pio_mask: [u8; 2],
    /// The next control write to the port is its mask.
    pio_mask_next: [bool; 2],
    pub baud: [u8; 2],
    pub sio: [SioChannel; 2],
    /// The tick flag (SYSFLG bit 7, /INT).
    pub tick: bool,
    /// Ticks that have come.
    ticks: u64,
    /// The 5.25-inch motors run until this clock.
    motor_until: u64,
    /// The beeps not yet turned into sound, as spans of clocks.
    pub beeps: Vec<(u64, u64)>,
    /// The DIP switches; `None`: by the disk in the first drive.
    pub dip: Option<u8>,
    /// The clock at which the picture's current field began.
    pub field_start: u64,
    /// The printer's last byte and how many were sent.
    pub printed: u64,
}

impl Io {
    pub fn new(dip: Option<u8>) -> Self {
        Self {
            fdc: Fdc::new(Chip::Fd1793, CPU_HZ),
            drives: (0..DRIVES).map(|_| Drive::default()).collect(),
            vlatch: 0,
            mlatch: 0,
            crtc: [0; 18],
            crtc_index: 0,
            pio_a: 0,
            pio_b: 0,
            pio_mask: [0; 2],
            pio_mask_next: [false; 2],
            baud: [14; 2],
            sio: [SioChannel::default(); 2],
            tick: false,
            ticks: 0,
            motor_until: 0,
            beeps: Vec::new(),
            dip,
            field_start: 0,
            printed: 0,
        }
    }

    /// The reset line: the latches clear (the boot ROM shows at 0), the
    /// chips reset. The tick and the clock chip run on.
    pub fn reset(&mut self) {
        self.fdc.reset();
        self.vlatch = 0;
        self.mlatch = 0;
        self.crtc = [0; 18];
        self.crtc_index = 0;
        self.pio_a = 0;
        self.pio_b = 0;
        self.pio_mask = [0; 2];
        self.pio_mask_next = [false; 2];
        self.baud = [14; 2];
        self.sio = [SioChannel::default(); 2];
        self.tick = false;
        self.motor_until = 0;
        self.apply_mlatch();
    }

    /// The /INT line.
    pub fn int(&self) -> bool {
        self.tick
    }

    /// Double-width characters (VLATCH bit 3).
    pub fn wide(&self) -> bool {
        self.vlatch & 0x08 != 0
    }

    pub fn eight_inch(&self) -> bool {
        self.mlatch & 0x20 != 0
    }

    pub fn motor_on(&self, now: u64) -> bool {
        now < self.motor_until
    }

    /// The DIP switches as read.
    pub fn dip_switches(&self) -> u8 {
        self.dip.unwrap_or_else(|| match &self.drives[0].image {
            Some(img) if img.eight_inch => 2,
            _ => 1,
        })
    }

    fn apply_mlatch(&mut self) {
        let v = self.mlatch;
        self.fdc.drive = match v & 0x0f {
            1 => Some(0),
            2 => Some(1),
            4 => Some(2),
            8 => Some(3),
            _ => None,
        };
        self.fdc.side = (v >> 4) & 1;
        self.fdc.fast = v & 0x20 != 0;
        self.fdc.mfm = v & 0x40 != 0;
    }

    fn update_ready(&mut self, now: u64) {
        let eight = self.eight_inch();
        let disk = self
            .fdc
            .drive
            .and_then(|d| self.drives[d].image.as_ref())
            .is_some_and(|img| img.eight_inch == eight);
        self.fdc.ready = disk && (eight || self.motor_on(now));
        let motor = self.motor_on(now);
        for d in &mut self.drives {
            d.motor = d.image.as_ref().is_some_and(|i| i.eight_inch || motor);
        }
    }

    pub fn tick(&mut self, now: u64) {
        while now >= tick_cycle(self.ticks + 1) {
            self.ticks += 1;
            self.tick = true;
        }
        self.update_ready(now);
        self.fdc.tick(now, &mut self.drives);
    }

    /// The CRTC's picture: characters a line, lines a field, displayed
    /// characters and lines, lines a character row. `None` until the
    /// registers describe a picture.
    pub fn geometry(&self) -> Option<Geometry> {
        let r = &self.crtc;
        let g = Geometry {
            htotal: u32::from(r[0]) + 1,
            cols: u32::from(r[1]),
            rows: u32::from(r[6] & 0x7f),
            row_lines: u32::from(r[9] & 0x1f) + 1,
            lines: (u32::from(r[4] & 0x7f) + 1) * (u32::from(r[9] & 0x1f) + 1)
                + u32::from(r[5] & 0x1f),
            start: (u16::from(r[12]) << 8 | u16::from(r[13])) & 0x3fff,
        };
        // A field between 1/100 and 1/30 of a second.
        let field = g.field_clocks();
        let hz = u64::from(CPU_HZ);
        (g.htotal >= 2 && g.cols > 0 && g.rows > 0 && (hz / 100..=hz / 30).contains(&field))
            .then_some(g)
    }

    /// The CRTC's display enable at `now`.
    fn display_enable(&self, now: u64) -> bool {
        let Some(g) = self.geometry() else {
            return false;
        };
        let chars = now.saturating_sub(self.field_start) * 3 / 8;
        let (col, line) = (chars % u64::from(g.htotal), chars / u64::from(g.htotal));
        col < u64::from(g.cols) && line < u64::from(g.rows * g.row_lines)
    }

    /// The clock chip's data lines.
    fn rtc(&self, now: u64) -> u8 {
        if self.pio_b & 0x20 == 0 {
            // READ low: the open-drain lines float high.
            return 0x0f;
        }
        rtc_digit(self.pio_b & 0x0f, now / u64::from(CPU_HZ))
    }

    fn sysflg(&self, now: u64) -> u8 {
        let single = self.eight_inch()
            && self
                .fdc
                .drive
                .and_then(|d| self.drives[d].image.as_ref())
                .is_some_and(|img| img.eight_inch && img.sides == 1);
        u8::from(self.tick) << 7
            | u8::from(self.display_enable(now)) << 6
            | u8::from(self.motor_on(now)) << 5
            | u8::from(single) << 4
    }

    fn ustat(&self) -> u8 {
        u8::from(!self.fdc.drq()) << 7 | u8::from(!self.fdc.intrq) << 6
    }

    fn sio_peek(&self, reg: usize) -> u8 {
        if reg & 1 == 0 {
            return 0;
        }
        match self.sio[reg >> 1].pointer {
            0 => SIO_RR0,
            1 => 0x01,
            _ => 0,
        }
    }

    fn pio_peek(&self, reg: usize, now: u64) -> u8 {
        match reg {
            0 => (self.pio_a & !self.pio_mask[0]) | (self.rtc(now) & self.pio_mask[0]),
            2 => self.pio_b,
            _ => 0xff,
        }
    }

    /// A register read without its side effects (`off` from `$7E0`).
    pub fn peek(&self, off: u16, now: u64) -> u8 {
        let reg = usize::from(off & 3);
        match off & !3 {
            0x7e0 => self.sysflg(now),
            0x7e4 => self.sio_peek(reg),
            0x7e8 => PRINTER_READY,
            0x7ec => !match reg {
                0 => self.fdc.peek_status(now, &self.drives),
                1 => self.fdc.track,
                2 => self.fdc.sector,
                _ => self.fdc.data,
            },
            0x7f0 => 0,
            0x7f4 => self.ustat(),
            0x7f8 => self.dip_switches(),
            _ => self.pio_peek(reg, now),
        }
    }

    /// A read of the register at block offset `off` (`$7E0–$7FF`).
    pub fn read(&mut self, off: u16, now: u64) -> u8 {
        self.tick(now);
        let reg = usize::from(off & 3);
        match off & !3 {
            0x7e0 => {
                let v = self.sysflg(now);
                if off & 1 == 0 {
                    self.tick = false;
                }
                v
            }
            0x7e4 => {
                let v = self.sio_peek(reg);
                if reg & 1 != 0 {
                    self.sio[reg >> 1].pointer = 0;
                }
                v
            }
            0x7ec => !match reg {
                0 => self.fdc.read_status(now, &self.drives),
                1 => self.fdc.track,
                2 => self.fdc.sector,
                _ => self.fdc.read_data(),
            },
            _ => self.peek(off, now),
        }
    }

    /// A write to the register at block offset `off` (`$7D0–$7FF`).
    pub fn write(&mut self, off: u16, v: u8, now: u64) {
        self.tick(now);
        let reg = usize::from(off & 3);
        match off & !3 {
            0x7d0 => self.baud[0] = v & 0x0f,
            0x7d4 => self.baud[1] = v & 0x0f,
            0x7d8 => {
                self.mlatch = v;
                self.apply_mlatch();
                if !self.eight_inch() {
                    self.motor_until = now + u64::from(CPU_HZ) * MOTOR_MS / 1000;
                }
                self.update_ready(now);
            }
            0x7dc => self.vlatch = v,
            0x7e0 => {
                if off & 1 == 0 {
                    self.crtc_index = v & 0x1f;
                } else if let Some(r) = self.crtc.get_mut(usize::from(self.crtc_index)) {
                    *r = v;
                }
            }
            0x7e4 => {
                if reg & 1 != 0 {
                    let ch = &mut self.sio[reg >> 1];
                    if ch.pointer == 0 {
                        ch.wr[0] = v;
                        ch.pointer = v & 7;
                    } else {
                        ch.wr[usize::from(ch.pointer)] = v;
                        ch.pointer = 0;
                    }
                }
            }
            0x7e8 => self.printed += 1,
            0x7ec => match reg {
                0 => self.fdc.command(!v, now, &mut self.drives),
                1 => self.fdc.track = !v,
                2 => self.fdc.sector = !v,
                _ => self.fdc.write_data(!v),
            },
            0x7f8 => {
                // Until the fourth tick from now.
                let end = tick_cycle(self.ticks + 4);
                match self.beeps.last_mut() {
                    Some(last) if last.1 >= now => last.1 = end,
                    _ => self.beeps.push((now, end)),
                }
                // Nobody takes the sound: the oldest go.
                if self.beeps.len() > 32 {
                    self.beeps.remove(0);
                }
            }
            0x7fc => {
                let port = reg >> 1;
                if reg & 1 != 0 {
                    if std::mem::take(&mut self.pio_mask_next[port]) {
                        self.pio_mask[port] = v;
                    } else if v & 0xcf == 0xcf {
                        // Mode 3: the mask follows.
                        self.pio_mask_next[port] = true;
                    }
                } else if port == 0 {
                    self.pio_a = v;
                } else {
                    self.pio_b = v;
                }
            }
            _ => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.fdc.save(w);
        w.u8(self.vlatch);
        w.u8(self.mlatch);
        w.raw(&self.crtc);
        w.u8(self.crtc_index);
        w.u8(self.pio_a);
        w.u8(self.pio_b);
        w.raw(&self.pio_mask);
        w.bool(self.pio_mask_next[0]);
        w.bool(self.pio_mask_next[1]);
        w.raw(&self.baud);
        for ch in &self.sio {
            w.u8(ch.pointer);
            w.raw(&ch.wr);
        }
        w.bool(self.tick);
        w.u64(self.ticks);
        w.u64(self.motor_until);
        w.u32(self.beeps.len() as u32);
        for &(a, b) in &self.beeps {
            w.u64(a);
            w.u64(b);
        }
        w.u64(self.field_start);
        w.u64(self.printed);
        for d in &self.drives {
            w.u8(d.cylinder);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.fdc.load(r)?;
        self.vlatch = r.u8()?;
        self.mlatch = r.u8()?;
        r.raw_into(&mut self.crtc)?;
        self.crtc_index = r.u8()? & 0x1f;
        self.pio_a = r.u8()?;
        self.pio_b = r.u8()?;
        r.raw_into(&mut self.pio_mask)?;
        self.pio_mask_next = [r.bool()?, r.bool()?];
        r.raw_into(&mut self.baud)?;
        for ch in &mut self.sio {
            ch.pointer = r.u8()? & 7;
            r.raw_into(&mut ch.wr)?;
        }
        self.tick = r.bool()?;
        self.ticks = r.u64()?;
        self.motor_until = r.u64()?;
        let n = r.u32()?;
        if n > 64 {
            return Err(StateError::Invalid("MAX-80 beeps"));
        }
        self.beeps.clear();
        for _ in 0..n {
            self.beeps.push((r.u64()?, r.u64()?));
        }
        self.field_start = r.u64()?;
        self.printed = r.u64()?;
        for d in &mut self.drives {
            d.cylinder = r.u8()?.min(84);
        }
        self.apply_mlatch();
        Ok(())
    }

    /// Take everything but the disks from a loaded copy.
    pub fn restore(&mut self, from: Io) {
        let drives = std::mem::take(&mut self.drives);
        let dip = self.dip;
        let cylinders: Vec<u8> = from.drives.iter().map(|d| d.cylinder).collect();
        *self = from;
        self.drives = drives;
        self.dip = dip;
        for (d, c) in self.drives.iter_mut().zip(cylinders) {
            d.cylinder = c;
        }
    }
}

/// What the CRTC's registers make of the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// Characters a line, the displayed ones, displayed rows, scan lines a
    /// row and a field.
    pub htotal: u32,
    pub cols: u32,
    pub rows: u32,
    pub row_lines: u32,
    pub lines: u32,
    /// The video RAM address of the first character.
    pub start: u16,
}

impl Geometry {
    /// Processor clocks a line (a character is 8 dots, a clock 3) and a
    /// field.
    pub fn field_clocks(&self) -> u64 {
        u64::from(self.htotal) * u64::from(self.lines) * 8 / 3
    }
}

/// Digit `reg` of the MSM5832 `seconds` after 1984-01-01 00:00:00 (a
/// Sunday): 0–1 seconds, 2–3 minutes, 4–5 hours (bit 3 of the tens: 24
/// hour mode), 6 the day of the week, 7–8 the day (bit 2 of the tens: a
/// leap year), 9–10 the month, 11–12 the year.
pub fn rtc_digit(reg: u8, seconds: u64) -> u8 {
    let (s, m, h) = (seconds % 60, seconds / 60 % 60, seconds / 3600 % 24);
    let mut days = seconds / 86_400;
    let weekday = days % 7;
    // Four years are 1461 days, the first of them the leap year.
    let mut year = 84 + days / 1461 * 4;
    days %= 1461;
    let mut leap = true;
    loop {
        let len = if leap { 366 } else { 365 };
        if days < len {
            break;
        }
        days -= len;
        year += 1;
        leap = false;
    }
    let lens = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while days >= lens[month] {
        days -= lens[month];
        month += 1;
    }
    let (day, month, year) = (days + 1, month as u64 + 1, year % 100);
    let v = match reg {
        0 => s % 10,
        1 => s / 10,
        2 => m % 10,
        3 => m / 10,
        4 => h % 10,
        5 => (h / 10) | 8,
        6 => weekday,
        7 => day % 10,
        8 => (day / 10) | (u64::from(leap) << 2),
        9 => month % 10,
        10 => month / 10,
        11 => year % 10,
        12 => year / 10,
        _ => 0x0f,
    };
    v as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tick_comes_61_times_a_second_and_is_cleared_by_a_read() {
        let mut io = Io::new(None);
        io.tick(u64::from(CPU_HZ));
        assert_eq!(io.ticks, 61);
        assert!(io.int());
        // An odd address reads the flags and leaves the tick.
        assert_eq!(io.read(0x7e1, u64::from(CPU_HZ)) & 0x80, 0x80);
        assert!(io.int());
        assert_eq!(io.read(0x7e0, u64::from(CPU_HZ)) & 0x80, 0x80);
        assert!(!io.int());
        assert_eq!(io.read(0x7e0, u64::from(CPU_HZ)) & 0x80, 0);
    }

    #[test]
    fn the_drive_latch_and_the_inverted_controller() {
        let mut io = Io::new(None);
        io.write(0x7d8, 0x62, 0); // drive 2, 8-inch, double density
        assert_eq!(io.fdc.drive, Some(1));
        assert!(io.fdc.fast && io.fdc.mfm);
        assert!(!io.motor_on(1));
        io.write(0x7d8, 0x01, 0); // drive 1, 5.25-inch: the motors start
        assert!(io.motor_on(u64::from(CPU_HZ) * 3));
        assert!(!io.motor_on(u64::from(CPU_HZ) * 4));
        assert_eq!(io.read(0x7e0, 10) & 0x20, 0x20);
        // The registers hold what was written, inverted on the bus.
        io.write(0x7ee, !0x12, 20);
        assert_eq!(io.fdc.sector, 0x12);
        assert_eq!(io.read(0x7ee, 30), !0x12);
        // No command runs: DRQ and INTRQ read high.
        assert_eq!(io.read(0x7f4, 40) & 0xc0, 0xc0);
        // No disk: the boot device is the 5.25-inch floppy.
        assert_eq!(io.read(0x7f8, 50), 1);
    }

    #[test]
    fn the_pio_reads_the_clock_chip_on_its_input_pins() {
        let mut io = Io::new(None);
        io.write(0x7fd, 0xcf, 0); // port A: mode 3
        io.write(0x7fd, 0x0f, 0); // bits 3-0 inputs
        io.write(0x7fc, 0x40, 0);
        io.write(0x7fe, 0x20 | 12, 0); // READ, the years' tens
        assert_eq!(io.read(0x7fc, 0), 0x48);
        io.write(0x7fe, 0x20 | 5, 0); // the hours' tens: 24 hour mode
        assert_eq!(io.read(0x7fc, 0), 0x48);
        io.write(0x7fe, 5, 0); // READ low: the lines float
        assert_eq!(io.read(0x7fc, 0), 0x4f);
    }

    #[test]
    fn the_clock_chip_counts_the_calendar() {
        let digits = |seconds: u64| -> String {
            [12, 11, 10, 9, 8, 7, 5, 4, 3, 2, 1, 0]
                .iter()
                .map(|&r| {
                    let mask = if r == 5 || r == 8 { 3 } else { 15 };
                    char::from(b'0' + (rtc_digit(r, seconds) & mask))
                })
                .collect()
        };
        assert_eq!(digits(0), "840101000000");
        assert_eq!(digits(59 * 86_400 + 86_399), "840229235959");
        assert_eq!(digits(366 * 86_400), "850101000000");
        assert_eq!(digits((366 + 59) * 86_400), "850301000000");
        assert_eq!(rtc_digit(8, 0) & 4, 4, "1984 is a leap year");
        assert_eq!(rtc_digit(8, 366 * 86_400) & 4, 0);
        assert_eq!(rtc_digit(6, 86_400), 1, "a Monday");
    }

    #[test]
    fn a_beep_lasts_to_the_fourth_tick() {
        let mut io = Io::new(None);
        io.write(0x7f8, 0, 100);
        assert_eq!(io.beeps, vec![(100, tick_cycle(4))]);
        // A write while it sounds makes it longer, not a second one.
        let later = tick_cycle(2) + 5;
        io.write(0x7f8, 0, later);
        assert_eq!(io.beeps, vec![(100, tick_cycle(6))]);
    }

    #[test]
    fn the_crtc_table_gives_the_display_enable() {
        let mut io = Io::new(None);
        // 120 characters a line, 80 shown; 24 rows of 9 lines, 264 a field.
        for (r, v) in [(0, 119), (1, 80), (4, 28), (5, 3), (6, 24), (9, 8)] {
            io.write(0x7e0, r, 0);
            io.write(0x7e1, v, 0);
        }
        let g = io.geometry().unwrap();
        assert_eq!((g.lines, g.cols, g.rows, g.row_lines), (264, 80, 24, 9));
        // 59.97 fields a second.
        assert_eq!(g.field_clocks(), 84_480);
        assert_eq!(io.read(0x7e1, 0) & 0x40, 0x40);
        // Character 80 of a line is past the picture: 80 * 8 / 3 clocks.
        assert_eq!(io.read(0x7e1, 214) & 0x40, 0);
        // Line 216 and on are the vertical blank.
        assert_eq!(io.read(0x7e1, 215 * 320) & 0x40, 0x40);
        assert_eq!(io.read(0x7e1, 216 * 320) & 0x40, 0);
    }
}
