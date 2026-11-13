//! The A600/A1200's PCMCIA slot with an SRAM memory card, behind Gayle.
//!
//! **Address spaces** (Commodore, NDK 3.1 `Examples1/PCMCIA/PCMCIA.General`;
//! Linux `amigayle.h`): common memory $600000–$9FFFFF (at most 4 MB, no
//! paging), attribute memory $A00000–$A1FFFF (the card information
//! structure, CIS: one tuple byte at each even address, odd addresses
//! $FF), I/O $A20000–$A3FFFF (an SRAM card has none: $FF), the card reset
//! at $A40000 (a write starts it, a read ends it; nothing to reset on an
//! SRAM card). Accesses take no wait states here (the real slot runs at
//! the speed set in Gayle's config register).
//!
//! **The card** is an image of its common memory, kept in memory while it
//! is in the slot (it is the machine's RAM when Kickstart uses it so, and
//! part of savestates) and written back to the image's backend by
//! [`MediaSlot::flush`] and on ejection. Its CIS:
//! - an image that starts with a CIS itself (CISTPL_DEVICE, $01, at byte 0,
//!   tuples on even bytes: a card prepared by PrepCard whose attribute
//!   accesses reach common memory, as on many SRAM cards) is read through
//!   to common memory;
//! - otherwise the card shows a generated CIS in attribute memory:
//!   CISTPL_DEVICE (SRAM, 250 ns, the card's size), and, when the image
//!   starts with an AmigaDOS file system ("DOS"), CISTPL_FORMAT (a disk
//!   of 512-byte blocks, no error detection, the image's length) and
//!   CISTPL_GEOMETRY, then CISTPL_VERS_1 and CISTPL_END.
//!
//! What Kickstart does with it (same NDK notes): card.resource takes an
//! SRAM card without a disk format found at power-up or reset as
//! low-priority fast RAM; carddisk.device mounts a card with CISTPL_FORMAT
//! as CC0:, which boots like a floppy.
//!
//! Tuple layouts after the PC Card Standard 2.0 (CISTPL_DEVICE device
//! info and size bytes, CISTPL_FORMAT, CISTPL_GEOMETRY, CISTPL_VERS_1),
//! from memory of it.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};

pub const COMMON: u32 = 0x60_0000;
pub const COMMON_END: u32 = 0x9f_ffff;
pub const ATTRIBUTE: u32 = 0xa0_0000;
/// The common memory's size.
pub const MAX_SIZE: usize = 0x40_0000;
const BLOCK: usize = 512;

/// The size units of CISTPL_DEVICE's size byte (codes 0–6).
const UNITS: [usize; 7] = [
    512,
    2 << 10,
    8 << 10,
    32 << 10,
    128 << 10,
    512 << 10,
    2 << 20,
];

/// The size byte of CISTPL_DEVICE for at least `len` bytes, and the size
/// it stands for: (count − 1) << 3 | unit code, the smallest such size
/// (in the largest unit that gives it).
pub fn device_size(len: usize) -> (u8, usize) {
    let mut best = (0x7e, 32 * UNITS[6]);
    for (code, unit) in UNITS.iter().enumerate() {
        let count = len.div_ceil(*unit).max(1);
        if count <= 32 && count * unit <= best.1 {
            best = (((count - 1) << 3 | code) as u8, count * unit);
        }
    }
    best
}

/// Sectors a track, tracks a cylinder, cylinders for `blocks` blocks.
fn geometry(blocks: usize) -> (u8, u8, u16) {
    for (s, t) in [
        (11, 2),
        (22, 2),
        (32, 1),
        (63, 1),
        (16, 1),
        (8, 1),
        (4, 1),
        (2, 1),
    ] {
        let per = s * t;
        if blocks.is_multiple_of(per) && blocks / per <= 0xffff {
            return (s as u8, t as u8, (blocks / per) as u16);
        }
    }
    (1, 1, blocks.min(0xffff) as u16)
}

/// The generated CIS for a card of `len` image bytes holding `size`
/// bytes of common memory.
pub fn cis(len: usize, disk: bool) -> Vec<u8> {
    let (size_byte, _) = device_size(len);
    // Device: SRAM (type 6), write protect switch respected, 250 ns.
    let mut t = vec![0x01, 3, 0x61, size_byte, 0xff];
    if disk {
        let n = len as u32;
        let blocks = (len / BLOCK) as u32;
        let mut f = vec![0x41, 20, 0x00, 0x00];
        f.extend(0u32.to_le_bytes()); // partition offset
        f.extend(n.to_le_bytes()); // partition bytes
        f.extend((BLOCK as u16).to_le_bytes());
        f.extend(blocks.to_le_bytes());
        f.extend(0u32.to_le_bytes()); // no error detection code
        t.extend(f);
        let (s, tr, c) = geometry(len / BLOCK);
        t.extend([0x1e, 4, s, tr]);
        t.extend(c.to_le_bytes());
    }
    let mut v = vec![0x15, 0, 4, 1];
    v.extend(b"MIMIC\0SRAM CARD\0");
    v.push(0xff);
    v[1] = (v.len() - 2) as u8;
    t.extend(v);
    t.push(0xff);
    t
}

/// The slot and its card.
#[derive(Debug, Default)]
pub struct Pcmcia {
    media: Option<Media>,
    /// The common memory (empty: no card).
    pub mem: Vec<u8>,
    /// The generated CIS (empty: the image brings its own).
    cis: Vec<u8>,
    protected: bool,
    dirty: bool,
    /// Accesses since the last frame (the slot's activity).
    activity: u32,
    shown: bool,
    /// The card went in or out since Gayle last looked (its CCDET change).
    pub changed: bool,
}

impl Clone for Pcmcia {
    /// A clone keeps the card's memory but not its image.
    fn clone(&self) -> Self {
        Self {
            media: None,
            mem: self.mem.clone(),
            cis: self.cis.clone(),
            protected: self.protected,
            dirty: self.dirty,
            activity: self.activity,
            shown: self.shown,
            changed: self.changed,
        }
    }
}

impl PartialEq for Pcmcia {
    fn eq(&self, o: &Self) -> bool {
        self.mem == o.mem && self.cis == o.cis && self.protected == o.protected
    }
}

impl Eq for Pcmcia {}

/// An image that starts with its own CIS on even bytes: CISTPL_DEVICE
/// ($01) at byte 0, its link (2–16) at byte 2, and a tuple code other than
/// 0 where the next tuple starts.
fn own_cis(image: &[u8]) -> bool {
    if image.len() < 64 || image[0] != 0x01 || !(2..=16).contains(&image[2]) {
        return false;
    }
    let next = 2 * (2 + usize::from(image[2]));
    image.get(next).is_some_and(|&c| c != 0)
}

impl Pcmcia {
    pub fn present(&self) -> bool {
        !self.mem.is_empty()
    }

    pub fn writable(&self) -> bool {
        self.present() && !self.protected
    }

    /// Move the image from `old` (a savestate was loaded into a clone).
    pub fn take_media(&mut self, old: &mut Pcmcia) {
        self.media = old.media.take();
    }

    /// A common memory byte ($FF without a card or past its end).
    pub fn common(&self, offset: u32) -> u8 {
        self.mem.get(offset as usize).copied().unwrap_or(0xff)
    }

    pub fn common_word(&self, offset: u32) -> u16 {
        u16::from(self.common(offset)) << 8 | u16::from(self.common(offset | 1))
    }

    pub fn set_common(&mut self, offset: u32, v: u8) {
        if self.protected {
            return;
        }
        if let Some(b) = self.mem.get_mut(offset as usize) {
            *b = v;
            self.dirty = true;
            self.activity += 1;
        }
    }

    /// An attribute memory byte: the CIS on even addresses.
    pub fn attribute(&self, offset: u32) -> u8 {
        if !self.present() {
            return 0xff;
        }
        if self.cis.is_empty() {
            return self.common(offset);
        }
        if offset & 1 != 0 {
            return 0xff;
        }
        self.cis.get(offset as usize / 2).copied().unwrap_or(0xff)
    }

    /// Once a frame: the activity shown.
    pub fn end_frame(&mut self) {
        self.shown = self.activity > 0;
        self.activity = 0;
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        if let (Some(m), true) = (&mut self.media, self.dirty) {
            let len = (m.backend.len() as usize).min(self.mem.len());
            m.backend.write(0, &self.mem[..len])?;
            self.dirty = false;
        }
        Ok(())
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u32(self.mem.len() as u32);
        w.raw(&self.mem);
        w.bool(self.changed);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let len = r.u32()? as usize;
        if len != self.mem.len() {
            return Err(StateError::Invalid("PCMCIA card differs"));
        }
        r.raw_into(&mut self.mem)?;
        self.changed = r.bool()?;
        Ok(())
    }
}

impl MediaSlot for Pcmcia {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if !matches!(media.kind, MediaKind::Cartridge | MediaKind::Disk) {
            return Err(MediaError::Unsupported(media.kind));
        }
        let len = media.backend.len() as usize;
        if len == 0 || len > MAX_SIZE {
            return Err(MediaError::Format(format!(
                "a PCMCIA memory card of 1 byte to 4 MB ({len} bytes)"
            )));
        }
        let mut image = vec![0; len];
        media.backend.read(0, &mut image)?;
        let _ = self.eject();
        self.cis = if own_cis(&image) {
            Vec::new()
        } else {
            cis(len, image.starts_with(b"DOS"))
        };
        // The common memory the CIS announces; past the image it reads 0.
        let (_, size) = device_size(len);
        image.resize(size.clamp(len, MAX_SIZE), 0);
        self.mem = image;
        self.media = Some(media);
        self.dirty = false;
        self.changed = true;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.mem.clear();
        self.cis.clear();
        if self.media.is_some() {
            self.changed = true;
        }
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        self.protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()?;
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.dirty || self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.shown,
            error: false,
            track: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_sizes_take_the_smallest_fitting_code() {
        assert_eq!(device_size(1 << 20), (1 << 3 | 5, 1 << 20)); // 2 × 512K
        assert_eq!(device_size(4 << 20), (1 << 3 | 6, 4 << 20)); // 2 × 2M
        assert_eq!(device_size(512), (0, 512));
        // 880K: 7 × 128K (no smaller multiple of a unit with ≤ 32 of them).
        assert_eq!(device_size(901_120), (6 << 3 | 4, 7 * (128 << 10)));
    }

    #[test]
    fn a_dos_image_gets_format_and_geometry_tuples() {
        let t = cis(901_120, true);
        assert_eq!(&t[..5], &[0x01, 3, 0x61, 6 << 3 | 4, 0xff]);
        assert_eq!((t[5], t[6], t[7]), (0x41, 20, 0)); // FORMAT, disk
        let nbytes = u32::from_le_bytes([t[13], t[14], t[15], t[16]]);
        assert_eq!(nbytes, 901_120);
        assert_eq!(u32::from_le_bytes([t[19], t[20], t[21], t[22]]), 1760);
        assert_eq!(&t[27..33], &[0x1e, 4, 11, 2, 80, 0]); // GEOMETRY
        assert_eq!(*t.last().unwrap(), 0xff);
        let plain = cis(1 << 20, false);
        assert_eq!(plain[5], 0x15, "no FORMAT: a RAM card");
    }
}
