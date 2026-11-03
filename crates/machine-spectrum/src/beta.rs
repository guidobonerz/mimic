//! Beta Disk interface (Beta 128) with TR-DOS: option `beta=on` (ROM role
//! "trdos", 16 KB). `RANDOMIZE USR 15616` enters TR-DOS.
//!
//! The ROM pages in at an opcode fetch from `$3D00–$3DFF` while the 48K
//! BASIC ROM is selected and out at a fetch from `$4000` up
//! ([`crate::memory`]); the ports answer only while it is in: `$1F` command
//! / status, `$3F` track, `$5F` sector, `$7F` data of a WD1793, `$FF` the
//! system register. Writes to `$FF`: bits 0–1 drive, bit 2 clear resets the
//! controller, bit 4 clear selects the upper side. Reads: bit 7 INTRQ, bit
//! 6 DRQ.
//!
//! TRD images hold 256-byte sectors numbered 1–16, track after track, side
//! after side; byte `$8E3` (the disc information in sector 9 of track 0)
//! tells the geometry, and an image cut short after its last used track is
//! padded. Formatting writes nothing to the image.
//!
//! Sources: Beta Disk User's Club newsletters (ports), the Beta Disk
//! Interface article on Wikipedia, the TR-DOS 5.01 ROM's behaviour.

use chips_acorn::wd1772::{Floppy, Wd1772};
use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const DRIVES: usize = 4;
const SECTOR: usize = 256;
const SECTORS: u32 = 16;
/// The disc type byte in the disc information sector.
const DISC_TYPE: usize = 0x8e3;

/// A TRD image as a floppy; `None` if it is no such image.
fn floppy(mut bytes: Vec<u8>) -> Option<Floppy> {
    let (sides, tracks) = match bytes.get(DISC_TYPE)? {
        0x16 => (2, 80),
        0x17 => (2, 40),
        0x18 => (1, 80),
        0x19 => (1, 40),
        _ => match bytes.len() {
            n if n <= 163_840 => (1, 40),
            n if n <= 327_680 => (2, 40),
            _ => (2, 80),
        },
    };
    let size = (sides * tracks * SECTORS) as usize * SECTOR;
    if bytes.len() > size {
        return None;
    }
    bytes.resize(size, 0);
    Some(Floppy {
        bytes,
        sides,
        tracks,
        sectors: SECTORS,
        sector_size: SECTOR as u32,
        first_sector: 1,
        write_protected: false,
        dirty: false,
    })
}

pub struct BetaDisk {
    fdc: Wd1772,
    disks: [Option<Floppy>; DRIVES],
    media: [Option<Media>; DRIVES],
    system: u8,
    /// Sevenths of an 8 MHz clock left over.
    rest: u32,
    /// The drive [`MediaSlot`] calls act on.
    pub selected: usize,
}

impl Default for BetaDisk {
    fn default() -> Self {
        Self::new()
    }
}

impl BetaDisk {
    pub fn new() -> Self {
        let mut fdc = Wd1772::new_1793();
        fdc.drive = Some(0);
        Self {
            fdc,
            disks: Default::default(),
            media: Default::default(),
            system: 0,
            rest: 0,
            selected: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fdc.reset();
    }

    pub fn inserted(&self, drive: usize) -> bool {
        self.disks[drive].is_some()
    }

    /// A port the interface decodes.
    pub fn claims(port: u16) -> bool {
        matches!(port & 0xff, 0x1f | 0x3f | 0x5f | 0x7f | 0xff)
    }

    fn register(port: u16) -> u32 {
        u32::from(port as u8 >> 5)
    }

    fn update_ready(&mut self) {
        let d = self.fdc.drive.unwrap_or(0);
        self.fdc.ready = self.disks[d].is_some();
    }

    pub fn read(&mut self, port: u16) -> u8 {
        self.update_ready();
        if port & 0xff == 0xff {
            return 0x3f | u8::from(self.fdc.intrq) << 7 | u8::from(self.fdc.drq) << 6;
        }
        self.fdc.read(Self::register(port))
    }

    pub fn write(&mut self, port: u16, v: u8) {
        if port & 0xff == 0xff {
            self.system = v;
            self.fdc.drive = Some(usize::from(v & 3));
            self.fdc.side = u32::from(v & 0x10 == 0);
            if v & 4 == 0 {
                self.fdc.reset();
            }
            self.update_ready();
            return;
        }
        self.update_ready();
        self.fdc.write(Self::register(port), v, &self.disks);
    }

    /// `t` T-states at 3.5 MHz went by.
    #[inline]
    pub fn tick(&mut self, t: u32) {
        let sevenths = t * 16 + self.rest;
        self.rest = sevenths % 7;
        self.fdc.tick(sevenths / 7, &mut self.disks);
    }

    /// Write the drive's changes into its media backend.
    fn store(&mut self, drive: usize) -> Result<(), MediaError> {
        if let (Some(d), Some(m)) = (&mut self.disks[drive], &mut self.media[drive]) {
            if d.dirty {
                let len = (m.backend.len() as usize).min(d.bytes.len());
                m.backend.write(0, &d.bytes[..len])?;
                d.dirty = false;
            }
        }
        Ok(())
    }
}

impl MediaSlot for BetaDisk {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let disk = floppy(bytes).ok_or_else(|| MediaError::Format("not a TRD image".into()))?;
        let _ = self.eject();
        self.disks[self.selected] = Some(disk);
        self.media[self.selected] = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.store(self.selected);
        self.disks[self.selected] = None;
        self.media[self.selected].take()
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disks[self.selected] {
            d.write_protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.store(self.selected)?;
        match &mut self.media[self.selected] {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.disks[self.selected].as_ref().is_some_and(|d| d.dirty)
            || self.media[self.selected]
                .as_ref()
                .is_some_and(|m| m.backend.dirty())
    }

    /// Busy while the controller works on this drive; the head's track.
    fn status(&self) -> DriveStatus {
        let here = self.fdc.drive == Some(self.selected);
        DriveStatus {
            active: here && self.fdc.busy(),
            error: false,
            track: self.disks[self.selected]
                .as_ref()
                .map(|_| self.fdc.head(self.selected) + 1),
        }
    }
}

const TAG: [u8; 4] = *b"BETA";

impl Savestate for BetaDisk {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u8(self.system);
            w.u32(self.rest);
            self.fdc.save(w);
            for d in &self.disks {
                let bytes: &[u8] = d.as_ref().map_or(&[], |d| &d.bytes);
                w.bytes(bytes);
                w.bool(d.as_ref().is_some_and(|d| d.dirty));
            }
        });
    }

    /// Discs stay as they are unless the state holds the same size.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        self.system = c.u8()?;
        self.rest = c.u32()? % 7;
        self.fdc.load(&mut c)?;
        for d in &mut self.disks {
            let bytes = c.bytes()?;
            let dirty = c.bool()?;
            if let Some(d) = d {
                if d.bytes.len() == bytes.len() {
                    d.bytes.copy_from_slice(bytes);
                    d.dirty = dirty;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(kind: u8, len: usize) -> Vec<u8> {
        let mut b = vec![0; len];
        b[DISC_TYPE] = kind;
        b
    }

    #[test]
    fn the_disc_type_gives_the_geometry_and_short_images_are_padded() {
        let d = floppy(image(0x16, 0x4000)).unwrap();
        assert_eq!((d.sides, d.tracks, d.bytes.len()), (2, 80, 655_360));
        let d = floppy(image(0x19, 0x1000)).unwrap();
        assert_eq!((d.sides, d.tracks, d.bytes.len()), (1, 40, 163_840));
        assert!(floppy(vec![0; 100]).is_none());
    }

    #[test]
    fn sectors_count_from_one_and_the_system_port_picks_the_side() {
        let mut b = BetaDisk::new();
        let mut bytes = image(0x16, 655_360);
        bytes[0] = 0x11; // track 0, side 0, sector 1
        bytes[16 * 256 + 256] = 0x22; // track 0, side 1, sector 2
        b.disks[0] = floppy(bytes);
        let read = |b: &mut BetaDisk, side: u8, sector: u8| {
            b.write(0xff, 0x0c | side << 4);
            b.write(0x5f, sector);
            b.write(0x1f, 0x80);
            for _ in 0..4000 {
                b.tick(4);
                if b.read(0xff) & 0x40 != 0 {
                    return b.read(0x7f);
                }
            }
            panic!("no data");
        };
        assert_eq!(read(&mut b, 1, 1), 0x11, "bit 4 set: side 0");
        b.write(0x1f, 0xd0);
        assert_eq!(read(&mut b, 0, 2), 0x22, "bit 4 clear: side 1");
    }
}
