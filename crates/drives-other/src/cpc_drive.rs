//! A CPC disc drive (the 664/6128's internal 3" drive, or an external one)
//! holding a DSK image ([`media::dsk`]) for the µPD765A
//! ([`chips_amstrad::fdc`]).
//!
//! The drive is ready while a disc is in and the motor (shared by both
//! drives, port &FA7E) runs. The head moves over 42 cylinders. Writes and
//! formats change the image in memory; it goes back to the media when the
//! motor stops, on flush and on eject.

use chips_amstrad::fdc::{FloppyDrive, SectorInfo};
use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};
use media::dsk::{Dsk, FormatId};

/// Cylinders the head can reach.
const CYLINDERS: u8 = 42;

struct Disc {
    media: Media,
    dsk: Dsk,
    dirty: bool,
    protected: bool,
}

#[derive(Default)]
pub struct CpcDrive {
    disc: Option<Disc>,
    cylinder: u8,
    motor: bool,
}

impl CpcDrive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_disc(&self) -> bool {
        self.disc.is_some()
    }

    pub fn set_motor(&mut self, on: bool) {
        if self.motor && !on {
            let _ = self.write_back();
        }
        self.motor = on;
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        let Some(d) = &mut self.disc else {
            return Ok(());
        };
        if !std::mem::take(&mut d.dirty) {
            return Ok(());
        }
        d.media.backend.write(0, &d.dsk.bytes)
    }

    fn sector(&self, side: u8, index: usize) -> Option<media::dsk::Sector> {
        let d = self.disc.as_ref()?;
        d.dsk
            .track(self.cylinder, side)?
            .sectors
            .get(index)
            .copied()
    }
}

impl FloppyDrive for CpcDrive {
    fn ready(&self) -> bool {
        self.motor && self.disc.is_some()
    }

    fn write_protected(&self) -> bool {
        self.disc.as_ref().is_none_or(|d| d.protected)
    }

    fn two_sided(&self) -> bool {
        self.disc.as_ref().is_some_and(|d| d.dsk.sides == 2)
    }

    fn cylinder(&self) -> u8 {
        self.cylinder
    }

    fn step(&mut self, outward: bool) {
        self.cylinder = if outward {
            self.cylinder.saturating_sub(1)
        } else {
            (self.cylinder + 1).min(CYLINDERS - 1)
        };
    }

    fn sectors(&self, side: u8) -> Vec<SectorInfo> {
        let Some(d) = &self.disc else {
            return Vec::new();
        };
        d.dsk
            .track(self.cylinder, side)
            .map(|t| {
                t.sectors
                    .iter()
                    .map(|s| SectorInfo {
                        id: s.id,
                        st1: s.st1,
                        st2: s.st2,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn read(&self, side: u8, index: usize) -> Vec<u8> {
        match (self.sector(side, index), &self.disc) {
            (Some(s), Some(d)) => d.dsk.data(&s).to_vec(),
            _ => Vec::new(),
        }
    }

    fn write(&mut self, side: u8, index: usize, data: &[u8]) -> bool {
        let Some(s) = self.sector(side, index) else {
            return false;
        };
        let Some(d) = &mut self.disc else {
            return false;
        };
        if d.protected {
            return false;
        }
        d.dsk.write(&s, data);
        d.dirty = true;
        true
    }

    fn format(&mut self, side: u8, n: u8, gap3: u8, filler: u8, ids: &[[u8; 4]]) -> bool {
        let cylinder = self.cylinder;
        let Some(d) = &mut self.disc else {
            return false;
        };
        if d.protected {
            return false;
        }
        let ids: Vec<FormatId> = ids.iter().map(|&id| FormatId { id }).collect();
        let ok = d.dsk.format(cylinder, side, n, gap3, filler, &ids).is_ok();
        d.dirty |= ok;
        ok
    }
}

impl MediaSlot for CpcDrive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let dsk = Dsk::parse(&bytes)?;
        let _ = self.write_back();
        self.disc = Some(Disc {
            media,
            dsk,
            dirty: false,
            protected: false,
        });
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.disc.take().map(|d| d.media)
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disc {
            d.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }
}

const TAG: [u8; 4] = *b"CDRV";
const VERSION: u16 = 1;

impl Savestate for CpcDrive {
    /// The disc is media, not state: a savestate keeps the head and the
    /// motor.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.cylinder);
            w.bool(self.motor);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let cylinder = c.u8()?;
        let motor = c.bool()?;
        if cylinder >= CYLINDERS {
            return Err(StateError::Invalid("drive cylinder"));
        }
        self.cylinder = cylinder;
        self.motor = motor;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;

    struct Mem(Vec<u8>);

    impl MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            self.0[o..o + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn drive() -> CpcDrive {
        let mut d = CpcDrive::new();
        d.insert(Media {
            name: "blank".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(media::dsk::blank_data(40))),
        })
        .unwrap();
        d
    }

    #[test]
    fn ready_with_disc_and_motor() {
        let mut d = drive();
        assert!(!d.ready());
        d.set_motor(true);
        assert!(d.ready());
        assert_eq!(d.sectors(0).len(), 9);
        assert!(d.sectors(1).is_empty(), "one side");
    }

    #[test]
    fn writes_reach_the_media_when_the_motor_stops() {
        let mut d = drive();
        d.set_motor(true);
        for _ in 0..3 {
            d.step(false);
        }
        assert!(d.write(0, 0, &[0x42; 512]));
        d.set_motor(false);
        let media = d.eject().unwrap();
        let mut b = media.backend;
        let mut bytes = vec![0; b.len() as usize];
        b.read(0, &mut bytes).unwrap();
        let dsk = Dsk::parse(&bytes).unwrap();
        let s = dsk.track(3, 0).unwrap().sectors[0];
        assert_eq!(dsk.data(&s)[0], 0x42);
    }

    #[test]
    fn write_protect() {
        let mut d = drive();
        d.write_protect(true);
        assert!(d.write_protected());
        assert!(!d.write(0, 0, &[1; 512]));
    }
}
