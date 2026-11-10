//! The TI disk controller card of the Peripheral Expansion Box: an
//! FD1771 ([`drives_other::wd17xx`]) for up to three single-density
//! drives DSK1–DSK3, its DSR ROM (8 KB, user-supplied) and the wait-state
//! logic that lets the DSR move bytes without polling.
//!
//! - CRU base `>1100` (bits `>880`–`>887`). Written: bit 0 maps the DSR
//!   ROM at `>4000`; 1 triggers the drive motors (a one-shot of about
//!   4.23 s); 2 enables the wait states; 3 head load; 4–6 select drive 1–3;
//!   7 the side. Read: 0 the FD1771's head loaded, 1–3 the selected
//!   drive, 4 the motors running, 5 always 0, 6 always 1, 7 the side.
//! - While the ROM is mapped, `>5FF0`–`>5FFF` are the FD1771's registers,
//!   on the high byte of the data bus and inverted: read `>5FF0` status,
//!   `>5FF2` track, `>5FF4` sector, `>5FF6` data; write `>5FF8` command,
//!   `>5FFA` track, `>5FFC` sector, `>5FFE` data.
//! - With the wait states enabled, an access to the registers holds the
//!   CPU until the FD1771 raises DRQ or INTRQ ([`DiskCard::hold`]).
//! - The FD1771 is ready while the motors run and the selected drive has
//!   a disk. Its INTRQ reaches the CPU only through the wait logic.
//!
//! Disks: TI sector dumps (`media::dmk::Image::parse_ti_dsk`).
//!
//! Sources: Texas Instruments, *Disk Memory System* manual; Thierry
//! Nouspikel, "The TI-99/4A Tech Pages", the TI disk controller card (CRU
//! bits, the inverted register addresses, the wait-state circuit and the
//! motor one-shot). The implementation is this project's own.

use drives_other::wd17xx::{Chip, Drive, Fdc};
use emu_core::{StateError, StateReader, StateWriter};

/// The card's first CRU bit (R12 = `>1100`).
pub const CRU_BASE: u16 = 0x880;
pub const DRIVES: usize = 3;
/// The motor one-shot, in seconds × 100.
const MOTOR_CENTISECONDS: u64 = 423;
/// The longest the wait logic holds the CPU (a guard: a real card would
/// hang without DRQ or INTRQ).
const MAX_HOLD_SECONDS: u64 = 2;

pub struct DiskCard {
    rom: Vec<u8>,
    pub fdc: Fdc,
    pub drives: Vec<Drive>,
    hz: u64,
    /// The CRU output bits 0–7.
    out: u8,
    motor_until: u64,
}

impl DiskCard {
    pub fn new(rom: Vec<u8>, hz: u32) -> Self {
        Self {
            rom,
            fdc: Fdc::new(Chip::Fd1771, hz),
            drives: (0..DRIVES)
                .map(|_| Drive {
                    ti_dsk: true,
                    ..Drive::default()
                })
                .collect(),
            hz: u64::from(hz),
            out: 0,
            motor_until: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fdc.reset();
        self.out = 0;
        self.motor_until = 0;
    }

    pub fn rom_on(&self) -> bool {
        self.out & 1 != 0
    }

    fn motors(&self, now: u64) -> bool {
        now < self.motor_until
    }

    fn update(&mut self, now: u64) {
        self.fdc.drive = (0..DRIVES).find(|d| self.out & (0x10 << d) != 0);
        self.fdc.side = (self.out >> 7) & 1;
        self.fdc.ready = self.motors(now)
            && self
                .fdc
                .drive
                .is_some_and(|d| self.drives[d].image.is_some());
    }

    /// Run the controller up to `now`.
    pub fn tick(&mut self, now: u64) {
        self.update(now);
        self.fdc.tick(now, &mut self.drives);
    }

    pub fn cru_write(&mut self, bit: u16, v: bool, now: u64) {
        let b = (bit - CRU_BASE) as u8;
        if v {
            self.out |= 1 << b;
        } else {
            self.out &= !(1 << b);
        }
        if b == 1 && v {
            self.motor_until = now + self.hz * MOTOR_CENTISECONDS / 100;
        }
        self.tick(now);
    }

    pub fn cru_read(&self, bit: u16, now: u64) -> bool {
        match bit - CRU_BASE {
            0 => self.fdc.peek_status(now, &self.drives) & 0x20 != 0,
            b @ 1..=3 => self.out & (0x10 << (b - 1)) != 0,
            4 => self.motors(now),
            5 => false,
            6 => true,
            _ => self.out & 0x80 != 0,
        }
    }

    /// With the wait states on, an access to the registers waits for DRQ
    /// or INTRQ: the cycles it waits from `now`.
    pub fn hold(&mut self, addr: u16, now: u64) -> u32 {
        if !self.rom_on() || self.out & 4 == 0 || addr < 0x5ff0 {
            return 0;
        }
        let step = 32;
        let mut t = now;
        let limit = now + self.hz * MAX_HOLD_SECONDS;
        self.tick(t);
        while !self.fdc.drq() && !self.fdc.intrq && t < limit {
            t += step;
            self.tick(t);
        }
        (t - now) as u32
    }

    /// A word read at `addr` in `>4000`–`>5FFF` (the ROM must be on).
    pub fn read(&mut self, addr: u16, now: u64) -> u16 {
        if addr < 0x5ff0 {
            return self.peek(addr, now);
        }
        self.tick(now);
        let v = match addr & 0x0e {
            0x0 => self.fdc.read_status(now, &self.drives),
            0x2 => self.fdc.track,
            0x4 => self.fdc.sector,
            0x6 => self.fdc.read_data(),
            _ => 0xff,
        };
        u16::from(!v) << 8
    }

    pub fn peek(&self, addr: u16, now: u64) -> u16 {
        let a = usize::from(addr & 0x1ffe);
        if addr >= 0x5ff0 {
            let v = match addr & 0x0e {
                0x0 => self.fdc.peek_status(now, &self.drives),
                0x2 => self.fdc.track,
                0x4 => self.fdc.sector,
                0x6 => self.fdc.data,
                _ => 0xff,
            };
            return u16::from(!v) << 8;
        }
        match self.rom.get(a..a + 2) {
            Some(w) => u16::from_be_bytes([w[0], w[1]]),
            None => 0,
        }
    }

    pub fn write(&mut self, addr: u16, v: u16, now: u64) {
        if addr < 0x5ff8 {
            return;
        }
        self.tick(now);
        let byte = !(v >> 8) as u8;
        match addr & 0x0e {
            0x8 => self.fdc.command(byte, now, &mut self.drives),
            0xa => self.fdc.track = byte,
            0xc => self.fdc.sector = byte,
            _ => self.fdc.write_data(byte),
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.fdc.save(w);
        w.u8(self.out);
        w.u64(self.motor_until);
        for d in &self.drives {
            w.u8(d.cylinder);
        }
    }

    /// The registers and head positions; the disks stay.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut fdc = self.fdc.clone();
        fdc.load(r)?;
        let out = r.u8()?;
        let motor_until = r.u64()?;
        let mut cyl = [0u8; DRIVES];
        for c in &mut cyl {
            *c = r.u8()?.min(80);
        }
        self.fdc = fdc;
        self.out = out;
        self.motor_until = motor_until;
        for (d, c) in self.drives.iter_mut().zip(cyl) {
            d.cylinder = c;
        }
        Ok(())
    }
}
