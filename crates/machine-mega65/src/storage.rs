//! The 45IO27's storage side: the F011-compatible floppy controller with two
//! drives holding D81 images, and the SD card controller with a raw card
//! image.
//!
//! **F011** (`$D080`–`$D08F`, C65 and MEGA65 personalities). `$D080`
//! control (IRQ, LED, MOTOR, SWAP, SIDE, drive select 0–7), `$D081`
//! command, `$D082` status A (BUSY DRQ EQ RNF CRC LOST PROT TK0), `$D083`
//! status B (RDREQ WTREQ RUN WGATE DSKIN INDEX IRQ DSKCHG), `$D084` track,
//! `$D085` sector, `$D086` side (the header to find), `$D087` data (the
//! 512-byte sector buffer through the CPU pointer), `$D088` clock, `$D089`
//! step time. Commands: `$40` read sector, `$80` write sector, `$10` step
//! out, `$18` step in, `$14` time a step, `$20` spin up, `$00` cancel,
//! `$01` clear the buffer pointers (added to a command: unbuffered;
//! `+4`/`+6` are read recovery variants, the same here). A D81 image (80
//! tracks, 2 sides, 10 sectors of 512 bytes, track after track with side 0
//! first) answers the sector the track, sector (1–10) and side registers
//! name, wherever the head is (the MEGA65's auto-tune). Commands finish at
//! once (BUSY is not seen); the buffer then holds the sector, the pointers
//! are equal (EQ) and DRQ shows the disk wrote last; reading `$D087`
//! moves the CPU pointer. `$D689` bit 5 swaps drives 0 and 1.
//!
//! **SD card** (`$D680`–`$D68F`, MEGA65 personality). `$D680` command
//! (`$02` read, `$03`–`$06` write, after the write gate `$57`, or `$4D`
//! for sector 0; `$81`/`$82` map the sector buffer at `$DE00`–`$DFFF` or
//! not, `$83`/`$84` fill mode with `$D686`, `$00`/`$10` reset, `$01`/`$11`
//! release, `$41` SDHC, `$C0`/`$C1` card slot) and status (bit 2 reset, 3
//! buffer mapped, 4 SDHC, 7 slot 1); `$D681`–`$D684` the sector number;
//! the card's 512-byte buffer. A card image is a plain array of sectors
//! (only slot 0 holds one).
//!
//! **The sector buffers.** `$D689` bit 7 (BUFSEL) shows the SD card's
//! buffer (1) or the F011's (0) at `$DE00` (when mapped) and, repeated, at
//! `$FFD6000`–`$FFD6FFF`.
//!
//! Not yet: real floppy timing (seek, rotation, BUSY), raw flux and track
//! DMA, write pre-compensation, MEGA disks (64 MB images), the hypervisor's
//! disk image registers (`$D68B`–`$D693`, the image placed by Hyppo), the
//! hypervisor's SD card file services (with the real hypervisor, milestone
//! 8). The SD card's sectors live in its medium, not in savestates.
//!
//! Sources: the MEGA65 user guide, `appendix-45io27-registers.tex` (the
//! F011, buffered sector operations, the 1581 layout, the SD controller
//! and its command table); the C65 system specification, section 2.5
//! (the F011 registers and commands); the `@IO` lines of `sdcardio.vhdl`
//! ($D080–$D09F, $D680–$D69F).

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};

/// A D81 image's size.
pub const D81_SIZE: usize = 819_200;
const SECTOR: usize = 512;

/// A floppy in a drive.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Floppy {
    bytes: Vec<u8>,
    protected: bool,
    dirty: bool,
}

/// One drive's media slot. A clone has no medium (see
/// [`Storage::take_media`]).
#[derive(Default)]
pub struct Drive {
    disk: Option<Floppy>,
    media: Option<Media>,
    /// The disk was changed since the drive was last selected.
    changed: bool,
    /// The controller is working on this drive now.
    active: bool,
    head: u8,
}

impl Clone for Drive {
    fn clone(&self) -> Self {
        Self {
            disk: self.disk.clone(),
            media: None,
            changed: self.changed,
            active: self.active,
            head: self.head,
        }
    }
}

impl Drive {
    fn store(&mut self) -> Result<(), MediaError> {
        if let (Some(d), Some(m)) = (&mut self.disk, &mut self.media) {
            if d.dirty {
                let len = (m.backend.len() as usize).min(d.bytes.len());
                m.backend.write(0, &d.bytes[..len])?;
                d.dirty = false;
            }
        }
        Ok(())
    }
}

impl MediaSlot for Drive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let len = media.backend.len() as usize;
        if len != D81_SIZE && len != D81_SIZE + 800 {
            return Err(MediaError::Format(format!(
                "not a D81 image ({len} bytes, expected {D81_SIZE})"
            )));
        }
        let mut bytes = vec![0; D81_SIZE];
        media.backend.read(0, &mut bytes)?;
        let _ = self.eject();
        self.disk = Some(Floppy {
            bytes,
            protected: false,
            dirty: false,
        });
        self.media = Some(media);
        self.changed = true;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.store();
        self.disk = None;
        self.changed = true;
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disk {
            d.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.store()?;
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.disk.as_ref().is_some_and(|d| d.dirty)
            || self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.active,
            error: false,
            track: self.disk.as_ref().map(|_| self.head + 1),
        }
    }
}

/// The SD card slot: a raw image read and written by sector. A clone has
/// no medium.
#[derive(Default)]
pub struct SdCard {
    media: Option<Media>,
    /// A sector access failed (beyond the image, a backend error).
    error: bool,
    active: bool,
}

impl Clone for SdCard {
    fn clone(&self) -> Self {
        Self {
            media: None,
            error: self.error,
            active: self.active,
        }
    }
}

impl SdCard {
    fn sectors(&self) -> u64 {
        self.media
            .as_ref()
            .map_or(0, |m| m.backend.len() / SECTOR as u64)
    }

    fn read(&mut self, sector: u32, buf: &mut [u8; SECTOR]) -> bool {
        let n = self.sectors();
        match &mut self.media {
            Some(m) if u64::from(sector) < n => m
                .backend
                .read(u64::from(sector) * SECTOR as u64, buf)
                .is_ok(),
            _ => false,
        }
    }

    fn write(&mut self, sector: u32, buf: &[u8; SECTOR]) -> bool {
        let n = self.sectors();
        match &mut self.media {
            Some(m) if u64::from(sector) < n => m
                .backend
                .write(u64::from(sector) * SECTOR as u64, buf)
                .is_ok(),
            _ => false,
        }
    }
}

impl MediaSlot for SdCard {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        if media.backend.len() < SECTOR as u64 || !media.backend.len().is_multiple_of(SECTOR as u64)
        {
            return Err(MediaError::Format(
                "an SD card image is a whole number of 512-byte sectors".into(),
            ));
        }
        self.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.active,
            error: self.error,
            track: None,
        }
    }
}

/// The media of the storage slots, taken out while a state loads.
pub struct Slots(Option<Media>, Option<Media>, Option<Media>);

/// The floppy controller's and SD controller's registers and buffers.
#[derive(Clone)]
pub struct Storage {
    pub drives: [Drive; 2],
    pub sd: SdCard,
    /// `$D080`–`$D08F` as written.
    fdc: [u8; 16],
    fdc_buf: [u8; SECTOR],
    cpu_ptr: u16,
    disk_ptr: u16,
    /// The disk was last to touch the buffer.
    drq: bool,
    rnf: bool,
    rdreq: bool,
    wtreq: bool,
    sd_buf: [u8; SECTOR],
    sd_addr: u32,
    sd_reset: bool,
    sdhc: bool,
    slot1: bool,
    /// The write gate: 1 for sectors > 0, 2 for sector 0.
    gate: u8,
    fill: bool,
    fill_value: u8,
    /// The sector buffer at `$DE00`.
    pub mapped: bool,
    /// `$D689`: bit 5 swap drives, bit 7 BUFSEL.
    pub d689: u8,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("fdc", &self.fdc)
            .field("sd_addr", &self.sd_addr)
            .field("mapped", &self.mapped)
            .finish_non_exhaustive()
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            drives: [Drive::default(), Drive::default()],
            sd: SdCard::default(),
            fdc: [0; 16],
            fdc_buf: [0; SECTOR],
            cpu_ptr: 0,
            disk_ptr: 0,
            drq: false,
            rnf: false,
            rdreq: false,
            wtreq: false,
            sd_buf: [0; SECTOR],
            sd_addr: 0,
            sd_reset: false,
            sdhc: true,
            slot1: false,
            gate: 0,
            fill: false,
            fill_value: 0,
            mapped: false,
            d689: 0,
        }
    }
}

impl Storage {
    /// Take the media out (a clone keeps none).
    pub fn take_media(&mut self) -> Slots {
        Slots(
            self.drives[0].media.take(),
            self.drives[1].media.take(),
            self.sd.media.take(),
        )
    }

    pub fn put_media(&mut self, s: Slots) {
        self.drives[0].media = s.0;
        self.drives[1].media = s.1;
        self.sd.media = s.2;
    }

    /// The drive the control register selects (`None`: 2–7, not fitted).
    fn selected(&self) -> Option<usize> {
        let ds = usize::from(self.fdc[0] & 7);
        (ds < 2).then(|| ds ^ usize::from(self.d689 >> 5 & 1))
    }

    fn eq(&self) -> bool {
        self.cpu_ptr == self.disk_ptr
    }

    /// F011 register `reg` (0–15) without side effects.
    pub fn fdc_peek(&self, reg: u16) -> u8 {
        let drive = self.selected().map(|i| &self.drives[i]);
        let disk = drive.and_then(|d| d.disk.as_ref());
        match reg & 15 {
            2 => {
                u8::from(self.drq) << 6
                    | u8::from(self.eq()) << 5
                    | u8::from(self.rnf) << 4
                    | u8::from(disk.is_some_and(|d| d.protected)) << 1
                    | u8::from(drive.is_some_and(|d| d.head == 0))
            }
            3 => {
                u8::from(self.rdreq) << 7
                    | u8::from(self.wtreq) << 6
                    | u8::from(self.rdreq) << 5
                    | u8::from(disk.is_some()) << 3
                    | u8::from(drive.is_some_and(|d| d.changed))
            }
            7 => self.fdc_buf[self.buf_index(self.cpu_ptr)],
            r => self.fdc[usize::from(r)],
        }
    }

    /// The buffer index of a pointer (SWAP inverts bit 8 for the CPU).
    fn buf_index(&self, ptr: u16) -> usize {
        let swap = if self.fdc[0] & 0x10 != 0 { 0x100 } else { 0 };
        usize::from((ptr ^ swap) & 0x1ff)
    }

    pub fn fdc_read(&mut self, reg: u16) -> u8 {
        let v = self.fdc_peek(reg);
        if reg & 15 == 7 {
            self.cpu_ptr = (self.cpu_ptr + 1) & 0x1ff;
            self.drq = false;
        }
        v
    }

    pub fn fdc_write(&mut self, reg: u16, v: u8) {
        let r = usize::from(reg & 15);
        match r {
            0 => {
                let was = self.selected();
                self.fdc[0] = v;
                if let Some(i) = was {
                    if self.selected() != was {
                        // Deselecting clears the disk change.
                        self.drives[i].changed = false;
                    }
                }
            }
            1 => self.command(v),
            7 => {
                let i = self.buf_index(self.cpu_ptr);
                self.fdc_buf[i] = v;
                self.cpu_ptr = (self.cpu_ptr + 1) & 0x1ff;
                self.drq = false;
            }
            2 | 3 => {}
            _ => self.fdc[r] = v,
        }
    }

    /// An F011 command (finished at once).
    fn command(&mut self, c: u8) {
        self.fdc[1] = c;
        self.rnf = false;
        if c & 0x01 != 0 || c == 0x01 {
            self.cpu_ptr = 0;
            self.disk_ptr = 0;
        }
        let Some(i) = self.selected() else {
            if c & 0xc0 != 0 {
                self.rnf = true;
            }
            return;
        };
        match c & 0xfe {
            0x00 => {}
            0x10 => self.drives[i].head = self.drives[i].head.saturating_sub(1),
            0x18 => self.drives[i].head = (self.drives[i].head + 1).min(84),
            0x14 | 0x20 => {}
            0x40 | 0x44 | 0x46 | 0x60 => self.read_sector(i),
            0x80 | 0x84 | 0xa0 => self.write_sector(i),
            _ => {}
        }
    }

    /// The image offset of the sector the registers name.
    fn offset(&self) -> Option<usize> {
        let (track, sector, side) = (
            usize::from(self.fdc[4]),
            usize::from(self.fdc[5]),
            usize::from(self.fdc[6] & 1),
        );
        (track < 80 && (1..=10).contains(&sector))
            .then(|| ((track * 2 + side) * 10 + sector - 1) * SECTOR)
    }

    fn read_sector(&mut self, i: usize) {
        let at = self.offset();
        let Some((disk, at)) = self.drives[i].disk.as_ref().zip(at) else {
            self.rnf = true;
            self.rdreq = false;
            return;
        };
        let data: [u8; SECTOR] = disk.bytes[at..at + SECTOR].try_into().expect("a sector");
        for (k, b) in data.into_iter().enumerate() {
            let p = (self.disk_ptr + k as u16) & 0x1ff;
            self.fdc_buf[usize::from(p)] = b;
        }
        self.drives[i].head = self.fdc[4];
        self.rdreq = true;
        self.wtreq = false;
        self.drq = true;
    }

    fn write_sector(&mut self, i: usize) {
        let at = self.offset();
        let buf = self.fdc_buf;
        let disk_ptr = self.disk_ptr;
        let Some((disk, at)) = self.drives[i].disk.as_mut().zip(at) else {
            self.rnf = true;
            self.wtreq = false;
            return;
        };
        if disk.protected {
            return;
        }
        for k in 0..SECTOR {
            disk.bytes[at + k] = buf[usize::from((disk_ptr + k as u16) & 0x1ff)];
        }
        disk.dirty = true;
        self.drives[i].head = self.fdc[4];
        self.wtreq = true;
        self.rdreq = false;
        self.drq = true;
    }

    /// SD controller register `reg` (0–`$F` of `$D680`) without side
    /// effects.
    pub fn sd_peek(&self, reg: u16) -> u8 {
        match reg & 0x1f {
            0x00 => {
                u8::from(self.sd_reset) << 2
                    | u8::from(self.mapped) << 3
                    | u8::from(self.sdhc) << 4
                    | u8::from(self.sd.error) << 6
                    | u8::from(self.slot1) << 7
            }
            r @ 0x01..=0x04 => (self.sd_addr >> (8 * (r - 1))) as u8,
            0x06 => self.fill_value,
            0x08 => self.disk_ptr as u8,
            0x09 => {
                (self.d689 & 0xa0)
                    | (self.disk_ptr >> 8) as u8 & 1
                    | u8::from(self.drq && self.eq()) << 1
            }
            _ => 0,
        }
    }

    pub fn sd_write(&mut self, reg: u16, v: u8) {
        match reg & 0x1f {
            0x00 => self.sd_command(v),
            r @ 0x01..=0x04 => {
                let shift = 8 * u32::from(r - 1);
                self.sd_addr = (self.sd_addr & !(0xff << shift)) | u32::from(v) << shift;
            }
            0x06 => self.fill_value = v,
            0x09 => self.d689 = v & 0xa0,
            _ => {}
        }
    }

    fn sd_command(&mut self, c: u8) {
        let gate = std::mem::take(&mut self.gate);
        match c {
            0x00 | 0x10 => self.sd_reset = true,
            0x01 | 0x11 => self.sd_reset = false,
            0x02 if !self.slot1 && !self.sd_reset => {
                let mut buf = [0; SECTOR];
                self.sd.error = !self.sd.read(self.sd_addr, &mut buf);
                if !self.sd.error {
                    self.sd_buf = buf;
                }
            }
            0x03..=0x06 if !self.slot1 && !self.sd_reset => {
                let allowed = if self.sd_addr == 0 {
                    gate == 2
                } else {
                    gate == 1
                };
                if allowed {
                    let buf = if self.fill {
                        [self.fill_value; SECTOR]
                    } else {
                        self.sd_buf
                    };
                    self.sd.error = !self.sd.write(self.sd_addr, &buf);
                } else {
                    self.sd.error = true;
                }
            }
            0x40 => self.sdhc = false,
            0x41 => self.sdhc = true,
            0x44 | 0x45 => self.sd.error = false,
            0x4d => self.gate = 2,
            0x57 => self.gate = 1,
            0x81 => self.mapped = true,
            0x82 => self.mapped = false,
            0x83 => self.fill = true,
            0x84 => self.fill = false,
            0xc0 => self.slot1 = false,
            0xc1 => self.slot1 = true,
            _ => {}
        }
    }

    pub fn sd_inserted(&self) -> bool {
        self.sd.media.is_some()
    }

    /// The sector buffer BUFSEL shows (at `$DE00` and `$FFD6xxx`).
    pub fn buffer_peek(&self, offset: u16) -> u8 {
        let i = usize::from(offset & 0x1ff);
        if self.d689 & 0x80 != 0 {
            self.sd_buf[i]
        } else {
            self.fdc_buf[i]
        }
    }

    pub fn buffer_write(&mut self, offset: u16, v: u8) {
        let i = usize::from(offset & 0x1ff);
        if self.d689 & 0x80 != 0 {
            self.sd_buf[i] = v;
        } else {
            self.fdc_buf[i] = v;
        }
    }

    /// Mark the drive the controller works on (for status displays).
    pub fn update_activity(&mut self) {
        let sel = self.selected();
        let motor = self.fdc[0] & 0x20 != 0;
        for (i, d) in self.drives.iter_mut().enumerate() {
            d.active = motor && sel == Some(i);
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.fdc);
        w.raw(&self.fdc_buf);
        w.u16(self.cpu_ptr);
        w.u16(self.disk_ptr);
        for v in [self.drq, self.rnf, self.rdreq, self.wtreq] {
            w.bool(v);
        }
        w.raw(&self.sd_buf);
        w.u32(self.sd_addr);
        for v in [self.sd_reset, self.sdhc, self.slot1, self.fill, self.mapped] {
            w.bool(v);
        }
        w.u8(self.gate);
        w.u8(self.fill_value);
        w.u8(self.d689);
        for d in &self.drives {
            w.u8(d.head);
            w.bool(d.changed);
            let bytes: &[u8] = d.disk.as_ref().map_or(&[], |f| &f.bytes);
            w.bytes(bytes);
            w.bool(d.disk.as_ref().is_some_and(|f| f.dirty));
            w.bool(d.disk.as_ref().is_some_and(|f| f.protected));
        }
    }

    /// The drives' disks come back from the state (their media stay).
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        r.raw_into(&mut self.fdc)?;
        r.raw_into(&mut self.fdc_buf)?;
        self.cpu_ptr = r.u16()? & 0x1ff;
        self.disk_ptr = r.u16()? & 0x1ff;
        self.drq = r.bool()?;
        self.rnf = r.bool()?;
        self.rdreq = r.bool()?;
        self.wtreq = r.bool()?;
        r.raw_into(&mut self.sd_buf)?;
        self.sd_addr = r.u32()?;
        self.sd_reset = r.bool()?;
        self.sdhc = r.bool()?;
        self.slot1 = r.bool()?;
        self.fill = r.bool()?;
        self.mapped = r.bool()?;
        self.gate = r.u8()?;
        self.fill_value = r.u8()?;
        self.d689 = r.u8()? & 0xa0;
        for d in &mut self.drives {
            d.head = r.u8()?;
            d.changed = r.bool()?;
            let bytes = r.bytes()?;
            let dirty = r.bool()?;
            let protected = r.bool()?;
            d.disk = match bytes.len() {
                0 => None,
                D81_SIZE => Some(Floppy {
                    bytes: bytes.to_vec(),
                    protected,
                    dirty,
                }),
                _ => return Err(StateError::Invalid("D81 image size")),
            };
        }
        Ok(())
    }
}
