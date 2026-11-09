//! The Model III's I/O ports (TRS-80 Model III Technical Reference Manual,
//! port map):
//!
//! | Port | Read | Write |
//! | --- | --- | --- |
//! | `$E0–$E3` | maskable interrupt status, active low: bit 2 the clock | interrupt mask: bit 2 enables the clock |
//! | `$E4–$E7` | NMI status, active low: bit 7 the controller's INTRQ, bit 6 the motor timeout, bit 5 the RESET button | NMI mask: bit 7 INTRQ, bit 6 motor timeout |
//! | `$EC–$EF` | clears the clock interrupt | bit 1 cassette motor, bit 2 32 columns, bit 3 the alternate character set, bit 5 video wait states |
//! | `$F0–$F3` | FD1793 status, track, sector, data | command, track, sector, data |
//! | `$F4–$F7` | – | drive select: bits 0–3 drives, bit 4 side, bit 6 WAIT, bit 7 double density |
//! | `$F8–$FB` | printer status | printer data |
//! | `$FF` | cassette | cassette |
//!
//! The clock interrupts 30 times a second. Selecting a drive starts the
//! motors for about three seconds. With WAIT set, an access to the data
//! register holds the CPU until the controller has a byte (DRQ) or ends
//! (INTRQ) — the ROM's boot reads its sector with `INI` that way and leaves
//! the loop through the NMI the controller's INTRQ raises.

use emu_core::{StateError, StateReader, StateWriter};

use crate::fdc::{Chip, Drive, Fdc};

pub const DRIVES: usize = 4;
const PRINTER_STATUS: u8 = 0x30;
/// How long WAIT may hold the CPU at most (a second).
const MAX_WAIT_SECONDS: u64 = 1;

pub struct Model3Io {
    pub fdc: Fdc,
    pub drives: Vec<Drive>,
    clock_hz: u64,
    /// Maskable interrupt mask (port `$E0`) and NMI mask (`$E4`).
    pub int_mask: u8,
    pub nmi_mask: u8,
    /// Port `$EC` latch and drive select latch (`$F4`).
    pub ec: u8,
    pub select: u8,
    pub rtc: bool,
    next_rtc: u64,
    rtc_period: u64,
    motor_until: u64,
    motor_clocks: u64,
    /// The motors stopped since the last select (NMI source).
    pub motor_timeout: bool,
    /// The RESET button is held.
    pub reset_button: bool,
}

impl Model3Io {
    pub fn new(clock_hz: u32) -> Self {
        let clock = u64::from(clock_hz);
        Self {
            fdc: Fdc::new(Chip::Fd1793, clock_hz),
            drives: (0..DRIVES).map(|_| Drive::default()).collect(),
            clock_hz: clock,
            int_mask: 0,
            nmi_mask: 0,
            ec: 0,
            select: 0,
            rtc: false,
            next_rtc: clock / 30,
            rtc_period: clock / 30,
            motor_until: 0,
            motor_clocks: clock * 3,
            motor_timeout: false,
            reset_button: false,
        }
    }

    pub fn reset(&mut self) {
        self.fdc.reset();
        self.int_mask = 0;
        self.nmi_mask = 0;
        self.ec = 0;
        self.select = 0;
        self.rtc = false;
        self.motor_until = 0;
        self.motor_timeout = false;
    }

    /// The INT line: the clock while enabled.
    pub fn int(&self) -> bool {
        self.rtc && self.int_mask & 0x04 != 0
    }

    /// The NMI line.
    pub fn nmi(&self) -> bool {
        (self.fdc.intrq && self.nmi_mask & 0x80 != 0)
            || (self.motor_timeout && self.nmi_mask & 0x40 != 0)
            || self.reset_button
    }

    pub fn wide(&self) -> bool {
        self.ec & 0x04 != 0
    }

    pub fn tick(&mut self, now: u64) {
        while now >= self.next_rtc {
            self.rtc = true;
            self.next_rtc += self.rtc_period;
        }
        if self.motor_until != 0 && now >= self.motor_until {
            self.motor_until = 0;
            self.motor_timeout = true;
        }
        self.update_ready(now);
        self.fdc.tick(now, &mut self.drives);
    }

    fn update_ready(&mut self, now: u64) {
        self.fdc.ready = now < self.motor_until
            && self
                .fdc
                .drive
                .is_some_and(|d| self.drives[d].image.is_some());
    }

    /// With WAIT set and the controller busy without a byte: the clocks
    /// the CPU is held from `now` until DRQ or the command's end.
    fn wait(&mut self, now: u64) -> u64 {
        if self.select & 0x40 == 0 {
            return 0;
        }
        let limit = now + self.clock_hz * MAX_WAIT_SECONDS;
        let mut t = now;
        while self.fdc.busy() && !self.fdc.drq() && t < limit {
            t += 8;
            self.tick(t);
        }
        t - now
    }

    /// An IN from `port` at `now`. Returns the byte and the WAIT clocks.
    pub fn read(&mut self, port: u8, now: u64) -> (u8, u64) {
        self.tick(now);
        match port {
            0xe0..=0xe3 => (!(u8::from(self.rtc) << 2), 0),
            0xe4..=0xe7 => {
                let v = !(u8::from(self.fdc.intrq) << 7
                    | u8::from(self.motor_timeout) << 6
                    | u8::from(self.reset_button) << 5);
                (v, 0)
            }
            0xec..=0xef => {
                self.rtc = false;
                (0xff, 0)
            }
            0xf0 => (self.fdc.read_status(now, &self.drives), 0),
            0xf1 => (self.fdc.track, 0),
            0xf2 => (self.fdc.sector, 0),
            0xf3 => {
                let w = self.wait(now);
                (self.fdc.read_data(), w)
            }
            0xf8..=0xfb => (PRINTER_STATUS, 0),
            _ => (0xff, 0),
        }
    }

    pub fn peek(&self, port: u8, now: u64) -> u8 {
        match port {
            0xe0..=0xe3 => !(u8::from(self.rtc) << 2),
            0xe4..=0xe7 => {
                !(u8::from(self.fdc.intrq) << 7
                    | u8::from(self.motor_timeout) << 6
                    | u8::from(self.reset_button) << 5)
            }
            0xf0 => self.fdc.peek_status(now, &self.drives),
            0xf1 => self.fdc.track,
            0xf2 => self.fdc.sector,
            0xf3 => self.fdc.data,
            0xf8..=0xfb => PRINTER_STATUS,
            _ => 0xff,
        }
    }

    /// An OUT to `port` at `now`. Returns the WAIT clocks.
    pub fn write(&mut self, port: u8, v: u8, now: u64) -> u64 {
        self.tick(now);
        match port {
            0xe0..=0xe3 => self.int_mask = v,
            0xe4..=0xe7 => self.nmi_mask = v,
            0xec..=0xef => self.ec = v,
            0xf0 => self.fdc.command(v, now, &mut self.drives),
            0xf1 => self.fdc.track = v,
            0xf2 => self.fdc.sector = v,
            0xf3 => {
                let w = self.wait(now);
                self.fdc.write_data(v);
                return w;
            }
            0xf4..=0xf7 => {
                self.select = v;
                self.fdc.drive = (0..DRIVES).find(|d| v & (1 << d) != 0);
                self.fdc.side = (v >> 4) & 1;
                self.fdc.mfm = v & 0x80 != 0;
                if v & 0x0f != 0 {
                    self.motor_until = now + self.motor_clocks;
                    self.motor_timeout = false;
                }
                self.update_ready(now);
            }
            _ => {}
        }
        0
    }

    pub fn inserted(&self, drive: usize) -> bool {
        self.drives[drive].image.is_some()
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.fdc.save(w);
        w.u8(self.int_mask);
        w.u8(self.nmi_mask);
        w.u8(self.ec);
        w.u8(self.select);
        w.bool(self.rtc);
        w.u64(self.next_rtc);
        w.u64(self.motor_until);
        w.bool(self.motor_timeout);
        w.bool(self.reset_button);
        for d in &self.drives {
            w.u8(d.cylinder);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.fdc.load(r)?;
        self.int_mask = r.u8()?;
        self.nmi_mask = r.u8()?;
        self.ec = r.u8()?;
        self.select = r.u8()?;
        self.rtc = r.bool()?;
        self.next_rtc = r.u64()?;
        self.motor_until = r.u64()?;
        self.motor_timeout = r.bool()?;
        self.reset_button = r.bool()?;
        for d in &mut self.drives {
            d.cylinder = r.u8()?.min(80);
        }
        Ok(())
    }

    /// Take everything but the disks from a loaded copy.
    pub fn restore(&mut self, from: Model3Io) {
        self.fdc = from.fdc;
        self.int_mask = from.int_mask;
        self.nmi_mask = from.nmi_mask;
        self.ec = from.ec;
        self.select = from.select;
        self.rtc = from.rtc;
        self.next_rtc = from.next_rtc;
        self.motor_until = from.motor_until;
        self.motor_timeout = from.motor_timeout;
        self.reset_button = from.reset_button;
        for (d, f) in self.drives.iter_mut().zip(from.drives) {
            d.cylinder = f.cylinder;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use media::dmk::{build_track, Image, SectorSpec};

    /// A one-track double-density disk with 18 sectors of 256 bytes.
    fn dd_drive() -> Drive {
        let specs: Vec<SectorSpec> = (1..=18)
            .map(|s| SectorSpec {
                track: 0,
                side: 0,
                sector: s,
                size: 256,
                dam: 0xfb,
                data: vec![s; 256],
                mfm: true,
            })
            .collect();
        let track = build_track(&specs, true);
        let mut bytes = vec![0u8; 16];
        bytes[1] = 1;
        let len = (track.data.len() + 128) as u16;
        bytes[2..4].copy_from_slice(&len.to_le_bytes());
        bytes[4] = 0x10; // single-sided
        let mut table = [0u8; 128];
        for (i, p) in track.idam.iter().enumerate() {
            let v = p + 128;
            table[i * 2..i * 2 + 2].copy_from_slice(&(v | 0x8000).to_le_bytes());
        }
        bytes.extend_from_slice(&table);
        bytes.extend_from_slice(&track.data);
        Drive {
            image: Some(Image::parse(&bytes).unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn wait_holds_the_cpu_until_each_byte() {
        let mut io = Model3Io::new(2_027_520);
        io.drives[0] = dd_drive();
        io.write(0xf4, 0xc1, 0); // drive 0, WAIT, double density
        io.write(0xf2, 3, 0);
        io.write(0xf0, 0x80, 0); // READ SECTOR
        let mut now = 0;
        let mut got = Vec::new();
        for _ in 0..256 {
            let (v, w) = io.read(0xf3, now);
            now += w + 20;
            got.push(v);
        }
        assert!(got.iter().all(|&b| b == 3), "{:?}", &got[..8]);
        // The command ends: INTRQ, and with the mask an NMI.
        io.write(0xe4, 0x80, now);
        for _ in 0..100 {
            now += 100;
            io.tick(now);
        }
        assert!(io.fdc.intrq && io.nmi());
        let (e4, _) = io.read(0xe4, now);
        assert_eq!(e4 & 0xe0, 0x60, "INTRQ low-active, RESET not held");
    }

    #[test]
    fn clock_interrupt_and_its_clear() {
        let mut io = Model3Io::new(2_027_520);
        io.write(0xe0, 0x04, 0);
        io.tick(2_027_520 / 30 + 1);
        assert!(io.int());
        assert_eq!(io.read(0xe0, 70_000).0 & 0x04, 0);
        io.read(0xec, 70_000);
        assert!(!io.int());
    }
}
