//! The disc interface at `&FE80`–`&FE9F` with two drives (media slots
//! "drive 0" and "drive 1"), each one- or two-sided; Acorn DFS calls side 1
//! of drive 0 "drive 2".
//!
//! **Model B**: the 8271 (bit 2 of the address selects the data register),
//! its INT on the 6502's NMI, the side through its OPO line.
//!
//! **B+**: the WD1770 at `&FE84`–`&FE87` (status/command, track, sector,
//! data) and the drive control latch at `&FE80`: bit 0 drive 0, bit 1
//! drive 1, bit 2 side 1, bit 3 single density, bit 5 the controller's
//! reset (here any change of the bit resets it: its polarity is not in the
//! sources at hand), bit 4 not used here; it reads as &FF. INTRQ and DRQ together drive the
//! NMI, so every byte interrupts. The 1770 runs at 8 MHz.
//!
//! Images: SSD (one side, track after track: 10 sectors of 256 bytes,
//! numbered 0–9) and DSD (both sides, track 0 side 0, track 0 side 1, …);
//! ADFS S/M/L (`.adf`, `.adl`: 16 sectors, see [`adfs_floppy`]); told
//! apart by the name's extension. An image cut short after its last
//! used track is padded to 40 or 80 tracks; an empty file is a blank
//! 80-track disc. Writes reach the medium on flush or eject.
//!
//! Sources: Acorn, *Disc Filing System User Guide* (SSD/DSD layout as the
//! DFS uses it: 10 × 256-byte sectors per track); the 8271 in
//! `chips_acorn::i8271`; the B+ drive control latch and 1770 addresses
//! from the *New Advanced User Guide* (Bray, Dickens, Holmes, 1987) as
//! summarised on the BBC Micro wiki pages "Western Digital WD1770 FDC" and
//! "NAUG Ch16 — Filing systems" (kieranhj.github.io/llm-beeb-wiki,
//! consulted 2026-11-11).

use chips_acorn::i8271::I8271;
use chips_acorn::wd1772::{Floppy, Wd1772};
use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};

pub const DRIVES: usize = 2;
const SECTORS: u32 = 10;
const SECTOR: u32 = 256;
const TRACK: usize = (SECTORS * SECTOR) as usize;

/// An SSD or DSD image as a floppy; `None` if it is too big.
pub fn floppy(mut bytes: Vec<u8>, double_sided: bool) -> Option<Floppy> {
    let sides: u32 = if double_sided { 2 } else { 1 };
    let per_track = TRACK * sides as usize;
    let tracks = if bytes.is_empty() || bytes.len() > 40 * per_track {
        80
    } else {
        40
    };
    let size = tracks * per_track;
    if bytes.len() > size {
        return None;
    }
    bytes.resize(size, 0);
    Some(Floppy {
        bytes,
        sides,
        tracks: tracks as u32,
        sectors: SECTORS,
        sector_size: SECTOR,
        first_sector: 0,
        write_protected: false,
        dirty: false,
    })
}

/// The controller of the model.
#[derive(Clone, Debug)]
pub enum Fdc {
    I8271(I8271),
    Wd1770 {
        fdc: Wd1772,
        control: u8,
    },
    /// The Master: control at `&FE24`, the 1770 at `&FE28`–`&FE2B`.
    Master {
        fdc: Wd1772,
        control: u8,
    },
}

impl Fdc {
    /// The NMI line.
    pub fn nmi(&self) -> bool {
        match self {
            Fdc::I8271(f) => f.int,
            Fdc::Wd1770 { fdc, .. } | Fdc::Master { fdc, .. } => fdc.intrq || fdc.drq,
        }
    }

    pub fn busy(&self) -> bool {
        match self {
            Fdc::I8271(f) => f.busy(),
            Fdc::Wd1770 { fdc, .. } | Fdc::Master { fdc, .. } => fdc.busy(),
        }
    }

    fn selected(&self) -> Option<usize> {
        match self {
            Fdc::I8271(f) => f.selected(),
            Fdc::Wd1770 { fdc, .. } | Fdc::Master { fdc, .. } => fdc.drive,
        }
    }

    fn head(&self, drive: usize) -> u8 {
        match self {
            Fdc::I8271(f) => f.head(drive),
            Fdc::Wd1770 { fdc, .. } | Fdc::Master { fdc, .. } => fdc.head(drive),
        }
    }

    /// Power-on state, the same kind of controller.
    pub fn power_on(&self) -> Self {
        match self {
            Fdc::I8271(_) => Fdc::I8271(I8271::new()),
            Fdc::Wd1770 { .. } => Fdc::Wd1770 {
                fdc: Wd1772::new(),
                control: 0,
            },
            Fdc::Master { .. } => Fdc::Master {
                fdc: Wd1772::new(),
                control: 0,
            },
        }
    }

    fn save(&self, w: &mut StateWriter) {
        match self {
            Fdc::I8271(f) => {
                w.u8(0);
                f.save(w);
            }
            Fdc::Wd1770 { fdc, control } => {
                w.u8(1);
                w.u8(*control);
                fdc.save(w);
            }
            Fdc::Master { fdc, control } => {
                w.u8(2);
                w.u8(*control);
                fdc.save(w);
            }
        }
    }

    fn load(&self, r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let kind = r.u8()?;
        match (self, kind) {
            (Fdc::I8271(f), 0) => {
                let mut f = f.clone();
                f.load(r)?;
                Ok(Fdc::I8271(f))
            }
            (Fdc::Wd1770 { fdc, .. }, 1) => {
                let control = r.u8()?;
                let mut fdc = fdc.clone();
                fdc.load(r)?;
                apply_control(&mut fdc, control);
                Ok(Fdc::Wd1770 { fdc, control })
            }
            (Fdc::Master { fdc, .. }, 2) => {
                let control = r.u8()?;
                let mut fdc = fdc.clone();
                fdc.load(r)?;
                apply_master_control(&mut fdc, control);
                Ok(Fdc::Master { fdc, control })
            }
            _ => Err(StateError::Invalid("disc controller kind")),
        }
    }
}

/// The B+ drive control latch cannot be read: the bus reads high there.
/// DFS 2.26 looks for its 1770 so: the track register holds &5A, and
/// `&FE80 AND 3` is not 0.
const LATCH_READ: u8 = 0xff;

/// The Master's drive control latch (`&FE24`): bit 0–2 drive 0–2, bit 3
/// the 1770's reset (low), bit 4 side 1, bit 5 single density.
fn apply_master_control(fdc: &mut Wd1772, v: u8) {
    fdc.drive = match v & 7 {
        1 | 3 | 5 | 7 => Some(0),
        2 | 6 => Some(1),
        _ => None,
    };
    fdc.side = u32::from(v & 0x10 != 0);
    fdc.single_density = v & 0x20 != 0;
}

/// The B+ drive control latch's lines to the 1770.
fn apply_control(fdc: &mut Wd1772, v: u8) {
    fdc.drive = match v & 3 {
        1 | 3 => Some(0),
        2 => Some(1),
        _ => None,
    };
    fdc.side = u32::from(v & 4 != 0);
    fdc.single_density = v & 8 != 0;
}

/// An ADFS image: S (160 KB, 40 tracks), M (320 KB, 80 tracks) one-sided,
/// L (640 KB, `.adl`) two-sided with the sides interleaved track by
/// track; 16 sectors of 256 bytes (numbered 0–15), double density. Short
/// images are padded to the next of these sizes.
pub fn adfs_floppy(mut bytes: Vec<u8>) -> Option<Floppy> {
    let size = [163_840, 327_680, 655_360]
        .into_iter()
        .find(|&n| bytes.len() <= n)?;
    bytes.resize(size, 0);
    Floppy::from_image(bytes)
}

pub struct Disc {
    pub fdc: Fdc,
    pub disks: [Option<Floppy>; DRIVES],
    media: [Option<Media>; DRIVES],
    /// The drive [`MediaSlot`] calls act on.
    pub selected: usize,
}

impl Default for Disc {
    fn default() -> Self {
        Self::new()
    }
}

impl Disc {
    /// The Model B's: the 8271.
    pub fn new() -> Self {
        Self::with(Fdc::I8271(I8271::new()))
    }

    /// The B+'s: the 1770 and its control latch.
    pub fn new_1770() -> Self {
        Self::with(Fdc::Wd1770 {
            fdc: Wd1772::new(),
            control: 0,
        })
    }

    /// The Master's: the 1770 at `&FE28` with its latch at `&FE24`.
    pub fn new_master() -> Self {
        Self::with(Fdc::Master {
            fdc: Wd1772::new(),
            control: 0,
        })
    }

    fn with(fdc: Fdc) -> Self {
        Self {
            fdc,
            disks: Default::default(),
            media: Default::default(),
            selected: 0,
        }
    }

    pub fn inserted(&self, drive: usize) -> bool {
        self.disks[drive].is_some()
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        match &mut self.fdc {
            Fdc::I8271(f) => f.read(addr),
            Fdc::Wd1770 { fdc, .. } => {
                if addr & 4 == 0 {
                    LATCH_READ
                } else {
                    fdc.read(u32::from(addr & 3))
                }
            }
            Fdc::Master { fdc, .. } => {
                if addr < 0xfe28 {
                    LATCH_READ
                } else {
                    fdc.read(u32::from(addr & 3))
                }
            }
        }
    }

    pub fn peek(&self, addr: u16) -> u8 {
        match &self.fdc {
            Fdc::I8271(f) => f.peek(addr),
            Fdc::Wd1770 { fdc, .. } => {
                if addr & 4 == 0 {
                    LATCH_READ
                } else {
                    fdc.peek(u32::from(addr & 3))
                }
            }
            Fdc::Master { fdc, .. } => {
                if addr < 0xfe28 {
                    LATCH_READ
                } else {
                    fdc.peek(u32::from(addr & 3))
                }
            }
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match &mut self.fdc {
            Fdc::I8271(f) => f.write(addr, v, &mut self.disks),
            Fdc::Wd1770 { fdc, control } => {
                if addr & 4 == 0 {
                    if (v ^ *control) & 0x20 != 0 {
                        fdc.reset();
                    }
                    *control = v;
                    apply_control(fdc, v);
                } else {
                    fdc.write(u32::from(addr & 3), v, &self.disks);
                }
            }
            Fdc::Master { fdc, control } => {
                if addr < 0xfe28 {
                    // Reset while bit 3 is low.
                    if v & 0x08 == 0 {
                        fdc.reset();
                    }
                    *control = v;
                    apply_master_control(fdc, v);
                } else {
                    fdc.write(u32::from(addr & 3), v, &self.disks);
                }
            }
        }
    }

    /// One 2 MHz cycle.
    #[inline]
    pub fn tick(&mut self) {
        match &mut self.fdc {
            Fdc::I8271(f) => f.tick(1, &mut self.disks),
            Fdc::Wd1770 { fdc, .. } | Fdc::Master { fdc, .. } => fdc.tick(4, &mut self.disks),
        }
    }

    /// Write the drive's changes into its media backend (growing it when
    /// the changes lie past a short image's end).
    fn store(&mut self, drive: usize) -> Result<(), MediaError> {
        if let (Some(d), Some(m)) = (&mut self.disks[drive], &mut self.media[drive]) {
            if d.dirty {
                let len = m.backend.len() as usize;
                if d.bytes[len.min(d.bytes.len())..].iter().any(|&b| b != 0) {
                    m.backend.set_len(d.bytes.len() as u64)?;
                }
                let len = (m.backend.len() as usize).min(d.bytes.len());
                m.backend.write(0, &d.bytes[..len])?;
                d.dirty = false;
            }
        }
        Ok(())
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.fdc.save(w);
        for d in &self.disks {
            let bytes: &[u8] = d.as_ref().map_or(&[], |d| &d.bytes);
            w.bytes(bytes);
            w.bool(d.as_ref().is_some_and(|d| d.dirty));
        }
    }

    /// Parse a state into a copy of the controller and the discs (discs
    /// keep their bytes unless the state holds the same size); `commit`
    /// takes it over.
    pub fn load(
        &self,
        r: &mut StateReader<'_>,
    ) -> Result<(Fdc, [Option<Floppy>; DRIVES]), StateError> {
        let fdc = self.fdc.load(r)?;
        let mut disks = self.disks.clone();
        for d in &mut disks {
            let bytes = r.bytes()?;
            let dirty = r.bool()?;
            if let Some(d) = d {
                if d.bytes.len() == bytes.len() {
                    d.bytes.copy_from_slice(bytes);
                    d.dirty = dirty;
                }
            }
        }
        Ok((fdc, disks))
    }

    pub fn commit(&mut self, (fdc, disks): (Fdc, [Option<Floppy>; DRIVES])) {
        self.fdc = fdc;
        self.disks = disks;
    }
}

impl MediaSlot for Disc {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let name = media.name.to_ascii_lowercase();
        let disk = if name.ends_with(".adf") || name.ends_with(".adl") {
            adfs_floppy(bytes).ok_or_else(|| {
                MediaError::Format("not an ADFS S/M/L image (160/320/640 KB)".into())
            })?
        } else {
            floppy(bytes, name.ends_with(".dsd"))
                .ok_or_else(|| MediaError::Format("not an SSD/DSD image (too big)".into()))?
        };
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
        let here = self.fdc.selected() == Some(self.selected);
        DriveStatus {
            active: here && self.fdc.busy(),
            error: false,
            track: self.disks[self.selected]
                .as_ref()
                .map(|_| self.fdc.head(self.selected) + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_padded_to_40_or_80_tracks() {
        let d = floppy(vec![1; 5000], false).unwrap();
        assert_eq!((d.sides, d.tracks, d.bytes.len()), (1, 40, 102_400));
        let d = floppy(vec![1; 150_000], false).unwrap();
        assert_eq!(d.tracks, 80);
        let d = floppy(Vec::new(), true).unwrap();
        assert_eq!((d.sides, d.tracks), (2, 80));
        assert!(floppy(vec![0; 204_801], false).is_none());
        // DSD: track 0 side 1 follows track 0 side 0.
        let mut b = vec![0; 2 * TRACK];
        b[TRACK] = 0x77;
        let d = floppy(b, true).unwrap();
        assert_eq!(d.offset(0, 1, 0), Some(TRACK));
    }
}
