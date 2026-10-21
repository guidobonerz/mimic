//! Amiga 3.5" double-density floppy drive: select, motor, head stepping,
//! the status lines, the drive identification and the disk as rotating
//! MFM tracks, on the CIA lines the Amiga wires them to (HRM appendix F;
//! chapter 8 "Disk"):
//!
//! - CIA-B port B (outputs): 7 /MTR, 6–3 /SEL3–/SEL0, 2 /SIDE, 1 DIR
//!   (1 = outward, towards cylinder 0), 0 /STEP.
//! - CIA-A port A (inputs, low active): 5 /RDY, 4 /TK0, 3 /WPRO, 2 /CHNG.
//! - /INDEX (once per revolution) goes to CIA-B /FLAG; read and write
//!   data go to Paula, one bit cell (2 µs) at a time ([`AmigaDrive::cell`]).
//!
//! A drive latches /MTR when it is selected. /STEP pulses move the head on
//! the falling edge. /CHNG goes low when the disk is removed and stays low
//! until a step pulse with a disk inside. With the motor off, /RDY carries
//! the drive's 32-bit identification, one bit per select: turning the
//! motor on and off restarts it (3.5" double-density drives send all ones).
//!
//! **Disks.** An ADF is encoded into 160 MFM tracks on insert
//! (`media::adf`); written tracks are decoded back into the image and
//! written through the backend on flush and eject. The disk is media, not
//! state: savestates keep the mechanics only.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::adf;

/// Identification of a 3.5" double-density drive.
pub const ID_35_DD: u32 = 0xffff_ffff;
/// Highest cylinder the head reaches.
pub const LAST_CYLINDER: u8 = 81;
/// Bit cells on a track.
pub const TRACK_BITS: u32 = (adf::TRACK_BYTES * 8) as u32;

struct Disk {
    media: Media,
    image: Vec<u8>,
    tracks: Vec<Vec<u8>>,
    dirty: Vec<bool>,
    protected: bool,
}

impl Disk {
    fn write_back(&mut self) -> Result<(), MediaError> {
        if !self.dirty.iter().any(|&d| d) {
            return Ok(());
        }
        for t in 0..adf::TRACKS {
            if std::mem::take(&mut self.dirty[t]) {
                let sectors = adf::decode_track(&self.tracks[t]);
                adf::store_track(&mut self.image, t, &sectors);
            }
        }
        self.media.backend.write(0, &self.image)
    }
}

pub struct AmigaDrive {
    pub id: u32,
    pub motor: bool,
    pub selected: bool,
    pub cylinder: u8,
    /// /SIDE low: the upper head.
    pub upper: bool,
    /// /CHNG asserted (low): the disk was removed.
    pub changed: bool,
    /// Bit cell under the head.
    pub pos: u32,
    id_shift: u32,
    id_bit: bool,
    step_level: bool,
    disk: Option<Disk>,
}

impl std::fmt::Debug for AmigaDrive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmigaDrive")
            .field("motor", &self.motor)
            .field("cylinder", &self.cylinder)
            .field("upper", &self.upper)
            .field("disk", &self.disk.as_ref().map(|d| &d.media.name))
            .finish()
    }
}

/// Clones the mechanics; the disk stays with the original (see
/// [`AmigaDrive::take_disk`]).
impl Clone for AmigaDrive {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            motor: self.motor,
            selected: self.selected,
            cylinder: self.cylinder,
            upper: self.upper,
            changed: self.changed,
            pos: self.pos,
            id_shift: self.id_shift,
            id_bit: self.id_bit,
            step_level: self.step_level,
            disk: None,
        }
    }
}

impl Default for AmigaDrive {
    fn default() -> Self {
        Self::new()
    }
}

impl AmigaDrive {
    pub fn new() -> Self {
        Self {
            id: ID_35_DD,
            motor: false,
            selected: false,
            cylinder: 0,
            upper: false,
            changed: true,
            pos: 0,
            id_shift: ID_35_DD,
            id_bit: false,
            step_level: true,
            disk: None,
        }
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    /// Move the disk of `other` (with its media) into this drive.
    pub fn take_disk(&mut self, other: &mut AmigaDrive) {
        self.disk = other.disk.take();
    }

    /// CIA-B port B changed; `sel` is this drive's /SELx bit (3–6).
    pub fn write_prb(&mut self, prb: u8, sel: u8) {
        let selected = prb & (1 << sel) == 0;
        if selected && !self.selected {
            let motor = prb & 0x80 == 0;
            if self.motor && !motor {
                // Motor turned off: the identification starts again.
                self.id_shift = self.id;
            }
            if !motor && !self.motor {
                self.id_bit = self.id_shift & 0x8000_0000 != 0;
                self.id_shift <<= 1;
            }
            self.motor = motor;
        }
        self.selected = selected;
        if selected {
            self.upper = prb & 0x04 == 0;
            let step = prb & 0x01 != 0;
            if self.step_level && !step {
                if prb & 0x02 != 0 {
                    self.cylinder = self.cylinder.saturating_sub(1);
                } else if self.cylinder < LAST_CYLINDER {
                    self.cylinder += 1;
                }
                if self.has_disk() {
                    self.changed = false;
                }
            }
            self.step_level = step;
        }
    }

    /// CIA-A port A bits 5–2 this drive pulls low while selected (1 = low).
    pub fn pulled_low(&self) -> u8 {
        if !self.selected {
            return 0;
        }
        let rdy = if self.motor { true } else { self.id_bit };
        let protected = self.disk.as_ref().is_some_and(|d| d.protected);
        u8::from(rdy) << 5
            | u8::from(self.cylinder == 0) << 4
            | u8::from(protected) << 3
            | u8::from(self.changed) << 2
    }

    fn track(&self) -> usize {
        usize::from(self.cylinder) * 2 + usize::from(self.upper)
    }

    /// One 2 µs bit cell while the motor turns: the bit under the head
    /// (written first if `write` is given and the disk is not protected)
    /// and whether the index hole passed.
    pub fn cell(&mut self, write: Option<bool>) -> (bool, bool) {
        if !self.motor {
            return (false, false);
        }
        let t = self.track();
        let pos = self.pos as usize;
        let mut bit = false;
        if let Some(d) = &mut self.disk {
            if let Some(track) = d.tracks.get_mut(t) {
                let mask = 0x80 >> (pos % 8);
                if let (Some(b), false) = (write, d.protected) {
                    if b {
                        track[pos / 8] |= mask;
                    } else {
                        track[pos / 8] &= !mask;
                    }
                    d.dirty[t] = true;
                }
                bit = track[pos / 8] & mask != 0;
            }
        }
        self.pos += 1;
        let index = self.pos >= TRACK_BITS;
        if index {
            self.pos = 0;
        }
        (bit, index && self.disk.is_some())
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"AFDD", 2, |w| {
            w.u32(self.id);
            w.u32(self.id_shift);
            w.u8(self.cylinder);
            w.u32(self.pos);
            for b in [
                self.motor,
                self.selected,
                self.upper,
                self.changed,
                self.id_bit,
                self.step_level,
            ] {
                w.bool(b);
            }
        });
    }

    /// Atomic: on error the drive is unchanged. The disk stays.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"AFDD", 2)?;
        let mut n = Self::new();
        n.id = c.u32()?;
        n.id_shift = c.u32()?;
        n.cylinder = c.u8()?;
        n.pos = c.u32()?;
        if n.cylinder > LAST_CYLINDER || n.pos >= TRACK_BITS {
            return Err(StateError::Invalid("floppy head position"));
        }
        for b in [
            &mut n.motor,
            &mut n.selected,
            &mut n.upper,
            &mut n.changed,
            &mut n.id_bit,
            &mut n.step_level,
        ] {
            *b = c.bool()?;
        }
        n.disk = self.disk.take();
        *self = n;
        Ok(())
    }
}

impl MediaSlot for AmigaDrive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut image = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut image)?;
        adf::check(&image)?;
        let tracks = (0..adf::TRACKS)
            .map(|t| adf::encode_track(&image, t))
            .collect();
        let _ = self.eject();
        self.disk = Some(Disk {
            media,
            image,
            tracks,
            dirty: vec![false; adf::TRACKS],
            protected: false,
        });
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let mut d = self.disk.take()?;
        let _ = d.write_back();
        self.changed = true;
        Some(d.media)
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disk {
            d.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.disk {
            Some(d) => d.write_back(),
            None => Ok(()),
        }
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.motor,
            error: false,
            track: Some(self.cylinder),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A backend over shared bytes, to see what is written back.
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl emu_core::media::MediaBackend for Shared {
        fn len(&self) -> u64 {
            self.0.lock().unwrap().len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let d = self.0.lock().unwrap();
            buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
            let mut d = self.0.lock().unwrap();
            d[offset as usize..offset as usize + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn disk(bytes: Arc<Mutex<Vec<u8>>>) -> Media {
        Media {
            name: "test.adf".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Shared(bytes)),
        }
    }

    /// /SEL0 low with the other lines as given.
    fn sel(d: &mut AmigaDrive, motor: bool, step: bool, outward: bool) {
        let prb = 0x78 & !0x08
            | if motor { 0 } else { 0x80 }
            | if step { 1 } else { 0 }
            | if outward { 2 } else { 0 }
            | 0x04;
        d.write_prb(prb, 3);
    }

    fn deselect(d: &mut AmigaDrive) {
        d.write_prb(0xff, 3);
    }

    #[test]
    fn steps_and_reports_track_zero() {
        let mut d = AmigaDrive::new();
        sel(&mut d, true, true, false);
        assert!(d.motor);
        assert_eq!(d.pulled_low() & 0x30, 0x30, "ready, track 0");
        for _ in 0..3 {
            sel(&mut d, true, false, false);
            sel(&mut d, true, true, false);
        }
        assert_eq!(d.cylinder, 3);
        assert_eq!(d.pulled_low() & 0x10, 0);
        assert_eq!(d.pulled_low() & 0x04, 0x04, "no disk: /CHNG stays low");
        deselect(&mut d);
        assert_eq!(d.pulled_low(), 0, "deselected");
    }

    #[test]
    fn identification_sends_all_ones() {
        let mut d = AmigaDrive::new();
        sel(&mut d, true, true, false);
        deselect(&mut d);
        sel(&mut d, false, true, false);
        deselect(&mut d);
        for _ in 0..32 {
            sel(&mut d, false, true, false);
            assert_eq!(d.pulled_low() & 0x20, 0x20);
            deselect(&mut d);
        }
    }

    #[test]
    fn reads_the_track_and_clears_change_on_step() {
        let bytes = Arc::new(Mutex::new(vec![0x5a; adf::ADF_SIZE]));
        let mut d = AmigaDrive::new();
        d.insert(disk(bytes)).unwrap();
        sel(&mut d, true, true, false);
        assert_eq!(d.pulled_low() & 0x04, 0x04, "/CHNG until a step");
        sel(&mut d, true, false, true); // step outward at track 0
        assert_eq!(d.pulled_low() & 0x04, 0);
        let mut bits = Vec::new();
        let mut index = 0;
        for _ in 0..TRACK_BITS {
            let (b, i) = d.cell(None);
            bits.push(b);
            index += usize::from(i);
        }
        assert_eq!(index, 1, "one index pulse per revolution");
        let bytes: Vec<u8> = bits
            .chunks(8)
            .map(|c| c.iter().fold(0, |v, &b| v << 1 | u8::from(b)))
            .collect();
        let sectors = adf::decode_track(&bytes);
        assert_eq!(sectors.len(), adf::SECTORS);
        assert!(sectors.iter().all(|s| s.data.iter().all(|&b| b == 0x5a)));
    }

    #[test]
    fn written_tracks_reach_the_backend() {
        let bytes = Arc::new(Mutex::new(vec![0; adf::ADF_SIZE]));
        let mut d = AmigaDrive::new();
        d.insert(disk(bytes.clone())).unwrap();
        sel(&mut d, true, true, false);
        let mut img = vec![0u8; adf::ADF_SIZE];
        img[..512].fill(0xa5);
        let track = adf::encode_track(&img, 0);
        // Write the track from bit 3 on.
        for _ in 0..3 {
            d.cell(None);
        }
        for i in 0..TRACK_BITS as usize {
            d.cell(Some(track[i / 8] >> (7 - i % 8) & 1 != 0));
        }
        d.flush().unwrap();
        let b = bytes.lock().unwrap();
        assert!(b[..512].iter().all(|&v| v == 0xa5));
        assert!(b[512..1024].iter().all(|&v| v == 0));
        drop(b);
        d.write_protect(true);
        assert_eq!(d.pulled_low() & 0x08, 0x08, "/WPRO");
        assert!(d.eject().is_some());
        assert_eq!(d.pulled_low() & 0x04, 0x04, "/CHNG after eject");
    }

    #[test]
    fn savestate_round_trip_keeps_the_disk() {
        let mut d = AmigaDrive::new();
        d.insert(disk(Arc::new(Mutex::new(vec![0; adf::ADF_SIZE]))))
            .unwrap();
        sel(&mut d, true, false, false);
        let mut w = StateWriter::new();
        d.save(&mut w);
        let data = w.into_bytes();
        d.load(&mut StateReader::new(&data)).unwrap();
        assert!(d.has_disk() && d.motor);
        assert!(d.load(&mut StateReader::new(&data[..4])).is_err());
        assert!(d.has_disk(), "a failed load keeps the disk");
    }
}
