//! The Model I Expansion Interface (option `ei=on`): the real-time clock,
//! the floppy controller with four drives and the printer port, mapped at
//! `$37E0–$37EF` (TRS-80 Model I Technical Reference Manual, Expansion
//! Interface):
//!
//! | Address | Read | Write |
//! | --- | --- | --- |
//! | `$37E0–$37E3` | interrupt latch: bit 7 the clock (cleared by the read), bit 6 the controller's INTRQ | drive select: bits 0–3 drives 0–3 (`$37E1` in the manual) |
//! | `$37E4–$37E7` | `$FF` (cassette select) | – |
//! | `$37E8–$37EB` | printer status: bit 7 busy, 6 paper out, 5 selected, 4 fault | printer data (no printer: ignored) |
//! | `$37EC–$37EF` | FD1771 status, track, sector, data | command, track, sector, data |
//!
//! The clock interrupts 40 times a second; the latch's bits drive the
//! Z80's INT line. Selecting a drive starts all motors, which run on for
//! about two seconds after the last select; a drive is ready while its
//! motor runs and a disk is in.
//!
//! Disks (slots "drive 0" to "drive 3"): DMK, JV1 or JV3 images
//! ([`media::dmk`]), written back to their files on flush and eject.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::dmk::Image;

use crate::fdc::{Chip, Drive, Fdc};

pub const DRIVES: usize = 4;
/// Printer status without a printer: not busy, paper, selected, no fault
/// (Level II's LPRINT then sends into the void).
const PRINTER_STATUS: u8 = 0x30;

pub struct ExpansionInterface {
    pub fdc: Fdc,
    pub drives: Vec<Drive>,
    /// The clock's interrupt is pending.
    pub rtc: bool,
    next_rtc: u64,
    rtc_period: u64,
    /// Clock until which the motors run.
    motor_until: u64,
    motor_clocks: u64,
    /// The last drive select byte.
    pub select: u8,
}

impl ExpansionInterface {
    pub fn new(clock_hz: u32) -> Self {
        let clock = u64::from(clock_hz);
        Self {
            fdc: Fdc::new(Chip::Fd1771, clock_hz),
            drives: (0..DRIVES).map(|_| Drive::default()).collect(),
            rtc: false,
            next_rtc: clock / 40,
            rtc_period: clock / 40,
            motor_until: 0,
            motor_clocks: clock * 2,
            select: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fdc.reset();
        self.rtc = false;
        self.motor_until = 0;
        self.select = 0;
    }

    /// The INT line.
    pub fn int(&self) -> bool {
        self.rtc || self.fdc.intrq
    }

    /// Run the clock and the controller up to `now`.
    pub fn tick(&mut self, now: u64) {
        while now >= self.next_rtc {
            self.rtc = true;
            self.next_rtc += self.rtc_period;
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

    pub fn read(&mut self, addr: u16, now: u64) -> u8 {
        self.tick(now);
        match addr & 0x0f {
            0x0..=0x3 => {
                let v = u8::from(self.rtc) << 7 | u8::from(self.fdc.intrq) << 6 | 0x3f;
                self.rtc = false;
                v
            }
            0x4..=0x7 => 0xff,
            0x8..=0xb => PRINTER_STATUS,
            0xc => self.fdc.read_status(now, &self.drives),
            0xd => self.fdc.track,
            0xe => self.fdc.sector,
            _ => self.fdc.read_data(),
        }
    }

    pub fn peek(&self, addr: u16, now: u64) -> u8 {
        match addr & 0x0f {
            0x0..=0x3 => u8::from(self.rtc) << 7 | u8::from(self.fdc.intrq) << 6 | 0x3f,
            0x4..=0x7 => 0xff,
            0x8..=0xb => PRINTER_STATUS,
            0xc => self.fdc.peek_status(now, &self.drives),
            0xd => self.fdc.track,
            0xe => self.fdc.sector,
            _ => self.fdc.data,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8, now: u64) {
        self.tick(now);
        match addr & 0x0f {
            0x0..=0x3 => {
                self.select = v;
                self.fdc.drive = (0..DRIVES).find(|d| v & (1 << d) != 0);
                if v & 0x0f != 0 {
                    self.motor_until = now + self.motor_clocks;
                }
                self.update_ready(now);
            }
            0xc => self.fdc.command(v, now, &mut self.drives),
            0xd => self.fdc.track = v,
            0xe => self.fdc.sector = v,
            0xf => self.fdc.write_data(v),
            _ => {}
        }
    }

    pub fn motor_on(&self, now: u64) -> bool {
        now < self.motor_until
    }

    /// Take the registers, clock and head positions from a loaded copy;
    /// the disks stay.
    pub fn restore(&mut self, from: ExpansionInterface) {
        self.fdc = from.fdc;
        self.rtc = from.rtc;
        self.next_rtc = from.next_rtc;
        self.motor_until = from.motor_until;
        self.select = from.select;
        for (d, f) in self.drives.iter_mut().zip(from.drives) {
            d.cylinder = f.cylinder;
        }
    }

    pub fn inserted(&self, drive: usize) -> bool {
        self.drives[drive].image.is_some()
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.fdc.save(w);
        w.bool(self.rtc);
        w.u64(self.next_rtc);
        w.u64(self.motor_until);
        w.u8(self.select);
        for d in &self.drives {
            w.u8(d.cylinder);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.fdc.load(r)?;
        self.rtc = r.bool()?;
        self.next_rtc = r.u64()?;
        self.motor_until = r.u64()?;
        self.select = r.u8()?;
        for d in &mut self.drives {
            d.cylinder = r.u8()?.min(80);
        }
        Ok(())
    }
}

fn write_back(d: &mut Drive) -> Result<(), MediaError> {
    if !std::mem::take(&mut d.dirty) {
        return Ok(());
    }
    let (Some(img), Some(media)) = (&d.image, &mut d.media) else {
        return Ok(());
    };
    let bytes = img.to_bytes(&d.original);
    media.backend.set_len(bytes.len() as u64)?;
    media.backend.write(0, &bytes)?;
    d.original = bytes;
    Ok(())
}

/// A drive as a media slot: DMK, JV1 or JV3 images.
impl MediaSlot for Drive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let image = Image::parse(&bytes)?;
        let _ = self.eject();
        self.image = Some(image);
        self.dirty = false;
        self.media = Some(media);
        self.original = bytes;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = write_back(self);
        self.image = None;
        self.original.clear();
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        self.write_protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        write_back(self)
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: false,
            error: false,
            track: Some(self.cylinder.saturating_add(1)),
        }
    }
}
