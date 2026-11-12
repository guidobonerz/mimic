//! The disk interface of the Spectravideo SVI-738: an FD1793-type
//! controller ([`drives_other::wd17xx`], double density) with two drives,
//! reached through eight addresses in its disk ROM's page.
//!
//! The 16 KB disk ROM (user-supplied, role "msx-disk-svi738") sits at
//! $4000–$7FFF of slot 3-1; while that page shows it, $7FB8–$7FBF are the
//! interface:
//!
//! | Address | Read                                   | Write                    |
//! | ------- | -------------------------------------- | ------------------------ |
//! | $7FB8   | controller status                      | command                  |
//! | $7FB9   | track register                         | track register           |
//! | $7FBA   | sector register                        | sector register          |
//! | $7FBB   | data register                          | data register            |
//! | $7FBC   | bit 7 INTRQ, bit 6 low while DRQ; the other bits high | bit 0 drive A, bit 1 drive B, bit 2 side 1, bit 3 motor |
//! | $7FBD–$7FBF | $FF                                | —                        |
//!
//! The ROM and the registers show again in page 2 ($8000–$BFFF, the
//! registers at $BFB8–$BFBF): MSX-DOS, which has RAM in page 1, puts the
//! disk ROM's slot into page 2 and reaches the controller there (its
//! code waits on $BFB8).
//!
//! A drive is ready while it is selected, the motor bit is set and a disk
//! is in. Disks are sector dumps ([`media::dmk::Image::parse_msx_dsk`]).
//!
//! Sources: the layout is read off the disk ROM's own code (its transfer
//! loops poll $7FBC: return on bit 7, move a byte while bit 6 is low; it
//! writes 1 or 2 with 8 for a drive with its motor and 4 for the second
//! side) and agrees with what is commonly said of the SVI-738's interface;
//! the meaning of the control register's upper bits (the ROM sets bits 7–6
//! from its drive table) is unknown and they are ignored. No board was
//! measured.

use drives_other::wd17xx::{Chip, Drive, Fdc};
use emu_core::{StateError, StateReader, StateWriter};

pub const DRIVES: usize = 2;
/// The first of the interface's addresses, and of their repeat in page 2.
pub const BASE: u16 = 0x7fb8;
pub const MIRROR: u16 = 0xbfb8;

pub struct DiskInterface {
    pub fdc: Fdc,
    pub drives: Vec<Drive>,
    /// The control register ($7FBC).
    pub control: u8,
}

impl DiskInterface {
    pub fn new(hz: u32) -> Self {
        let mut fdc = Fdc::new(Chip::Fd1793, hz);
        fdc.mfm = true;
        Self {
            fdc,
            drives: (0..DRIVES)
                .map(|_| Drive {
                    msx_dsk: true,
                    ..Drive::default()
                })
                .collect(),
            control: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fdc.reset();
        self.fdc.mfm = true;
        self.control = 0;
    }

    fn update(&mut self) {
        self.fdc.drive = (0..DRIVES).find(|d| self.control & (1 << d) != 0);
        self.fdc.side = self.control >> 2 & 1;
        self.fdc.ready = self.control & 0x08 != 0
            && self
                .fdc
                .drive
                .is_some_and(|d| self.drives[d].image.is_some());
        for d in 0..DRIVES {
            self.drives[d].motor = self.active(d);
        }
    }

    /// Run the controller up to `now` (CPU T-states).
    pub fn tick(&mut self, now: u64) {
        self.update();
        self.fdc.tick(now, &mut self.drives);
    }

    /// The motor runs with a drive selected.
    pub fn active(&self, drive: usize) -> bool {
        self.control & 0x08 != 0 && self.control & (1 << drive) != 0
    }

    fn lines(&self) -> u8 {
        0x3f | u8::from(self.fdc.intrq) << 7 | u8::from(!self.fdc.drq()) << 6
    }

    /// A read of `addr` ($7FB8–$7FBF).
    pub fn read(&mut self, addr: u16, now: u64) -> u8 {
        self.tick(now);
        match addr & 7 {
            0 => self.fdc.read_status(now, &self.drives),
            3 => self.fdc.read_data(),
            _ => self.peek(addr, now),
        }
    }

    /// Side-effect-free read.
    pub fn peek(&self, addr: u16, now: u64) -> u8 {
        match addr & 7 {
            0 => self.fdc.peek_status(now, &self.drives),
            1 => self.fdc.track,
            2 => self.fdc.sector,
            3 => self.fdc.data,
            4 => self.lines(),
            _ => 0xff,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8, now: u64) {
        self.tick(now);
        match addr & 7 {
            0 => self.fdc.command(v, now, &mut self.drives),
            1 => self.fdc.track = v,
            2 => self.fdc.sector = v,
            3 => self.fdc.write_data(v),
            4 => {
                self.control = v;
                self.update();
            }
            _ => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.fdc.save(w);
        w.u8(self.control);
        for d in &self.drives {
            w.u8(d.cylinder);
        }
    }

    /// The registers and head positions; the disks stay.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut fdc = self.fdc.clone();
        fdc.load(r)?;
        let control = r.u8()?;
        let mut cyl = [0u8; DRIVES];
        for c in &mut cyl {
            *c = r.u8()?.min(80);
        }
        self.fdc = fdc;
        self.fdc.mfm = true;
        self.control = control;
        for (d, c) in self.drives.iter_mut().zip(cyl) {
            d.cylinder = c;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, MediaSlot};

    struct Bytes(Vec<u8>);

    impl MediaBackend for Bytes {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
        fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    const HZ: u32 = 3_579_545;

    /// Poll $7FBC as the disk ROM does until INTRQ, taking the bytes.
    fn transfer(d: &mut DiskInterface, mut now: u64) -> (Vec<u8>, u64) {
        let mut bytes = Vec::new();
        for _ in 0..2_000_000 {
            now += 20;
            let lines = d.read(BASE + 4, now);
            if lines & 0x80 != 0 {
                break;
            }
            if lines & 0x40 == 0 {
                bytes.push(d.read(BASE + 3, now));
            }
        }
        (bytes, now)
    }

    #[test]
    fn a_sector_is_read_through_the_registers() {
        let mut image = vec![0u8; 737_280];
        // Cylinder 2, side 1, sector 5.
        let at = ((2 * 2 + 1) * 9 + 4) * 512;
        for (i, b) in image[at..at + 512].iter_mut().enumerate() {
            *b = i as u8 ^ 0x5a;
        }
        let mut d = DiskInterface::new(HZ);
        d.drives[0]
            .insert(Media {
                kind: MediaKind::Disk,
                name: "t.dsk".into(),
                backend: Box::new(Bytes(image.clone())),
            })
            .unwrap();
        assert_eq!(d.peek(BASE + 4, 0), 0x7f, "no INTRQ, no DRQ");
        // Without the motor the drive is not ready.
        d.write(BASE + 4, 0x01, 0);
        assert_ne!(d.read(BASE, 0) & 0x80, 0);
        d.write(BASE + 4, 0x0d, 0);
        assert!(d.active(0) && !d.active(1));
        assert_eq!(d.read(BASE, 0) & 0x80, 0, "ready");
        // SEEK to cylinder 2, then READ SECTOR 5.
        d.write(BASE + 3, 2, 10);
        d.write(BASE, 0x10, 20);
        let (none, now) = transfer(&mut d, 20);
        assert!(none.is_empty());
        assert_eq!((d.read(BASE + 1, now), d.drives[0].cylinder), (2, 2));
        d.write(BASE + 2, 5, now);
        assert_eq!(d.peek(BASE + 2, now), 5);
        d.write(BASE, 0x80, now);
        let (bytes, now) = transfer(&mut d, now);
        assert_eq!(bytes, image[at..at + 512]);
        assert_eq!(d.read(BASE, now) & 0x1d, 0, "no error bits");
        assert_eq!(
            d.peek(BASE + 4, now) & 0x80,
            0,
            "the status read took INTRQ"
        );
        // The state keeps registers and the head's place.
        let mut w = StateWriter::new();
        d.save(&mut w);
        let data = w.into_bytes();
        let mut e = DiskInterface::new(HZ);
        e.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!((e.control, e.fdc.track, e.drives[0].cylinder), (0x0d, 2, 2));
        assert!(e.load(&mut StateReader::new(&data[..5])).is_err());
        d.reset();
        assert_eq!(d.control, 0);
    }
}
