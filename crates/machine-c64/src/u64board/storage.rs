//! The board's storage media: the SD card slot and the hub's three USB
//! ports, each a media slot of the machine holding a block medium (an
//! image file, or a folder the frontend shows as a FAT volume). The
//! board's blocks reach them as [`chips_ultimate::Storage`].
//!
//! The media's contents are not part of the savestate (as with every
//! disk image in mimic); what is written goes to the medium's backend and
//! becomes permanent on [`MediaSlot::flush`], which the machine does by
//! itself [`QUIET_FRAMES`] frames after the firmware's last write (as a
//! stick is written when the board has written it).

use chips_ultimate::{unit, Storage, SECTOR};
use mimic_core::media::{Media, MediaError, MediaKind, MediaSlot};

/// Units: the SD card, then the USB ports.
pub const UNITS: usize = 1 + unit::USB_PORTS as usize;
/// Frames without a write before written media are flushed (2 s PAL).
pub const QUIET_FRAMES: u32 = 100;

#[derive(Default)]
pub struct Unit {
    media: Option<Media>,
    generation: u32,
    protected: bool,
    /// Frames since the last write (output bookkeeping, not state).
    quiet: u32,
}

impl Unit {
    pub fn inserted(&self) -> bool {
        self.media.is_some()
    }

    pub fn name(&self) -> Option<&str> {
        self.media.as_ref().map(|m| m.name.as_str())
    }
}

#[derive(Default)]
pub struct BoardStorage {
    pub units: [Unit; UNITS],
}

impl Storage for BoardStorage {
    fn sectors(&self, unit: u8) -> Option<u64> {
        let m = self.units.get(usize::from(unit))?.media.as_ref()?;
        Some(m.backend.len() / SECTOR as u64)
    }

    fn generation(&self, unit: u8) -> u32 {
        self.units
            .get(usize::from(unit))
            .map_or(0, |u| u.generation)
    }

    fn write_protected(&self, unit: u8) -> bool {
        self.units
            .get(usize::from(unit))
            .is_some_and(|u| u.protected)
    }

    fn read(&mut self, unit: u8, lba: u64, buf: &mut [u8]) -> bool {
        let Some(m) = self
            .units
            .get_mut(usize::from(unit))
            .and_then(|u| u.media.as_mut())
        else {
            return false;
        };
        let (Some(at), Some(buf)) = (lba.checked_mul(SECTOR as u64), buf.get_mut(..SECTOR)) else {
            return false;
        };
        at + SECTOR as u64 <= m.backend.len() && m.backend.read(at, buf).is_ok()
    }

    fn write(&mut self, unit: u8, lba: u64, data: &[u8]) -> bool {
        let Some(u) = self.units.get_mut(usize::from(unit)) else {
            return false;
        };
        if u.protected {
            return false;
        }
        let Some(m) = u.media.as_mut() else {
            return false;
        };
        let (Some(at), Some(data)) = (lba.checked_mul(SECTOR as u64), data.get(..SECTOR)) else {
            return false;
        };
        let done = at + SECTOR as u64 <= m.backend.len() && m.backend.write(at, data).is_ok();
        u.quiet = 0;
        done
    }
}

impl BoardStorage {
    /// Once a frame: media written and quiet for [`QUIET_FRAMES`] are
    /// flushed. The names of those that failed, with the error.
    pub fn frame(&mut self) -> Vec<String> {
        let mut failed = Vec::new();
        for u in &mut self.units {
            u.quiet = u.quiet.saturating_add(1);
            if u.quiet == QUIET_FRAMES && u.media.as_ref().is_some_and(|m| m.backend.dirty()) {
                if let Err(e) = MediaSlot::flush(u) {
                    failed.push(format!("{}: {e}", u.name().unwrap_or("?")));
                }
            }
        }
        failed
    }
}

/// A unit is a media slot of the machine.
impl MediaSlot for Unit {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let len = media.backend.len();
        if len < 64 * SECTOR as u64 || !len.is_multiple_of(SECTOR as u64) {
            return Err(MediaError::Format(format!(
                "{}: not a block medium (size {len}, a multiple of 512 bytes is needed)",
                media.name
            )));
        }
        self.media = Some(media);
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let m = self.media.take()?;
        self.generation = self.generation.wrapping_add(1);
        Some(m)
    }

    fn write_protect(&mut self, on: bool) {
        self.protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }
}
