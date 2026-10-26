//! ZX Microdrive: an endless tape loop of sectors, each a header block and
//! a record block ([`media::mdr`]), read and written through the
//! Interface 1.
//!
//! The drive is modelled at byte level, not in real time: the head is
//! always somewhere in a block's gap, preamble or data, and moves on as
//! the Interface 1 polls its status and transfers bytes (the IF1 holds
//! the CPU with /WAIT until each byte arrives, so the program's timing is
//! not involved):
//!
//! - **Gap:** the status shows GAP for [`GAP_POLLS`] polls (the IF1 ROM
//!   waits for eight in a row), then the preamble.
//! - **Preamble:** no GAP, no SYNC for [`PREAMBLE_POLLS`] polls (the ROM
//!   waits for six), then SYNC.
//! - **Data:** each read takes the block's next byte; the first status
//!   poll after a transfer moves the head to the next block's gap, and so
//!   do [`PASS_POLLS`] polls without a transfer (the block went by).
//!
//! Writing (R/W low) goes into the block under the head — the record that
//! follows a header just read, or the next block: the first 12 bytes the
//! IF1 sends are the preamble (ten zeros and two $FF) and are not stored;
//! bytes past the block's end are dropped. The cartridge is written back
//! on flush and eject.
//!
//! Sources: Sinclair ZX Interface 1 and Interface 2 Service Manual (the
//! drive and its status lines); the Interface 1 ROM's own microdrive
//! routines (how it polls GAP and SYNC and moves bytes, read from the
//! user's ROM); Ian Logan, *Spectrum Microdrive Book*.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};
use media::mdr::{Cartridge, HEADER_LEN, IMAGE_LEN, RECORD_LEN, SECTORS};

/// Status polls showing GAP before the preamble.
pub const GAP_POLLS: u8 = 12;
/// Status polls in the preamble before SYNC.
pub const PREAMBLE_POLLS: u8 = 4;
/// Status polls in a block's data without a byte read before the head
/// has passed it (the IF1 ROM reads at once after SYNC).
pub const PASS_POLLS: u8 = 64;
/// The byte FORMAT fills records with (and checks).
pub const FORMAT_FILL: u8 = 0xfc;
/// Bytes of preamble the Interface 1 sends before a block's data.
const PREAMBLE_BYTES: usize = 12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Gap,
    Preamble,
    Data,
}

/// Status lines of the drive under the head.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub gap: bool,
    pub sync: bool,
    pub write_protected: bool,
}

#[derive(Default)]
pub struct Microdrive {
    cartridge: Option<(Media, Cartridge)>,
    dirty: bool,
    motor: bool,
    /// Sector under the head and whether it is in the record block.
    sector: usize,
    record: bool,
    phase: Phase,
    polls: u8,
    /// Bytes moved in the current block (data phase; writes count the
    /// preamble too).
    index: usize,
    writing: bool,
}

impl Microdrive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_cartridge(&self) -> bool {
        self.cartridge.is_some()
    }

    pub fn set_motor(&mut self, on: bool) {
        if self.motor && !on {
            let _ = self.write_back();
        }
        self.motor = on;
    }

    pub fn motor(&self) -> bool {
        self.motor
    }

    fn block_len(&self) -> usize {
        if self.record {
            RECORD_LEN
        } else {
            HEADER_LEN
        }
    }

    fn next_block(&mut self) {
        if self.record {
            self.sector = (self.sector + 1) % SECTORS;
        }
        self.record = !self.record;
        self.phase = Phase::Gap;
        self.polls = 0;
        self.index = 0;
    }

    /// The status lines, as the Interface 1 polls them; polling moves the
    /// head on (see the module doc).
    pub fn poll(&mut self) -> Status {
        let Some((_, cart)) = &self.cartridge else {
            return Status::default();
        };
        let write_protected = cart.protected;
        if !self.motor {
            return Status {
                write_protected,
                ..Status::default()
            };
        }
        if self.phase == Phase::Data && self.index > 0 && !self.writing {
            self.next_block();
        }
        let (gap, sync) = match self.phase {
            Phase::Gap => {
                self.polls += 1;
                if self.polls >= GAP_POLLS {
                    self.phase = Phase::Preamble;
                    self.polls = 0;
                }
                (true, false)
            }
            Phase::Preamble => {
                self.polls += 1;
                if self.polls >= PREAMBLE_POLLS {
                    self.phase = Phase::Data;
                    self.polls = 0;
                    self.index = 0;
                    (false, true)
                } else {
                    (false, false)
                }
            }
            Phase::Data => {
                // Nobody reads: the block passes under the head.
                self.polls += 1;
                if self.polls >= PASS_POLLS {
                    self.next_block();
                }
                (false, true)
            }
        };
        Status {
            gap,
            sync,
            write_protected,
        }
    }

    /// The next byte of the block under the head ($FF without a
    /// cartridge). Past a record's end comes the format's filler
    /// [`FORMAT_FILL`] (FORMAT writes 629 bytes of record and its check
    /// reads 627 of them back; later saves leave that tail on the tape),
    /// past a header's end $FF.
    pub fn read(&mut self) -> u8 {
        if !self.motor || self.writing {
            return 0xff;
        }
        let Some((_, cart)) = &self.cartridge else {
            return 0xff;
        };
        if self.phase != Phase::Data {
            // Reading before SYNC: the head is in the data anyway.
            self.phase = Phase::Data;
            self.index = 0;
        }
        let i = self.index;
        self.index += 1;
        let base = if self.record { HEADER_LEN } else { 0 };
        if i < self.block_len() {
            cart.sectors[self.sector][base + i]
        } else if self.record {
            FORMAT_FILL
        } else {
            0xff
        }
    }

    /// R/W: `true` starts writing into the block under the head (the next
    /// one if this one was just read), `false` ends it.
    pub fn set_write(&mut self, on: bool) {
        if on == self.writing || !self.motor || self.cartridge.is_none() {
            return;
        }
        if on {
            if self.phase == Phase::Data && self.index > 0 {
                self.next_block();
            }
            self.phase = Phase::Data;
            self.index = 0;
        } else if self.index == 0 {
            // Nothing written: stay before the block.
            self.phase = Phase::Gap;
        }
        self.writing = on;
    }

    /// A byte from the Interface 1 while writing.
    pub fn write(&mut self, v: u8) {
        if !self.writing {
            return;
        }
        let i = self.index;
        self.index += 1;
        let base = if self.record { HEADER_LEN } else { 0 };
        let len = self.block_len();
        let sector = self.sector;
        if let Some((_, cart)) = &mut self.cartridge {
            if cart.protected {
                return;
            }
            if let Some(d) = i.checked_sub(PREAMBLE_BYTES).filter(|&d| d < len) {
                cart.sectors[sector][base + d] = v;
                self.dirty = true;
            }
        }
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        match &mut self.cartridge {
            Some((media, cart)) => {
                let bytes = cart.to_bytes();
                let len = media.backend.len() as usize;
                media.backend.write(0, &bytes[..len.min(bytes.len())])
            }
            None => Ok(()),
        }
    }
}

impl MediaSlot for Microdrive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let len = media.backend.len() as usize;
        if len > IMAGE_LEN {
            return Err(MediaError::Format(format!("MDR: {len} bytes")));
        }
        let mut bytes = vec![0; len];
        media.backend.read(0, &mut bytes)?;
        let cart = Cartridge::parse(&bytes)?;
        let _ = self.eject();
        self.cartridge = Some((media, cart));
        self.sector = 0;
        self.record = false;
        self.phase = Phase::Gap;
        self.polls = 0;
        self.index = 0;
        self.writing = false;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.writing = false;
        self.cartridge.take().map(|(m, _)| m)
    }

    /// The cartridge's tab (as long as it is in the drive).
    fn write_protect(&mut self, on: bool) {
        if let Some((_, c)) = &mut self.cartridge {
            c.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }

    /// Active while the motor runs; the "track" is the sector under the
    /// head (1-based, in tape order).
    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.motor && self.cartridge.is_some(),
            error: false,
            track: self.cartridge.as_ref().map(|_| (self.sector + 1) as u8),
        }
    }
}

const TAG: [u8; 4] = *b"MDRV";

impl Savestate for Microdrive {
    /// The cartridge is media: a savestate keeps the motor and the head's
    /// place.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.bool(self.motor);
            w.u16(self.sector as u16);
            w.bool(self.record);
            w.u8(self.phase as u8);
            w.u8(self.polls);
            w.u16(self.index as u16);
            w.bool(self.writing);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let motor = c.bool()?;
        let sector = usize::from(c.u16()?);
        let record = c.bool()?;
        let phase = match c.u8()? {
            0 => Phase::Gap,
            1 => Phase::Preamble,
            2 => Phase::Data,
            _ => return Err(StateError::Invalid("microdrive phase")),
        };
        let polls = c.u8()?;
        let index = usize::from(c.u16()?);
        let writing = c.bool()?;
        if sector >= SECTORS {
            return Err(StateError::Invalid("microdrive sector"));
        }
        (self.motor, self.sector, self.record, self.phase) = (motor, sector, record, phase);
        (self.polls, self.index, self.writing) = (polls, index, writing);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;
    use std::sync::{Arc, Mutex};

    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl MediaBackend for Shared {
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

    fn drive(cart: Cartridge) -> (Microdrive, Arc<Mutex<Vec<u8>>>) {
        let bytes = Arc::new(Mutex::new(cart.to_bytes()));
        let mut d = Microdrive::new();
        d.insert(Media {
            name: "c.mdr".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Shared(bytes.clone())),
        })
        .unwrap();
        d.set_motor(true);
        (d, bytes)
    }

    /// Poll as the IF1 ROM does: gap, then no gap, then SYNC.
    fn find_block(d: &mut Microdrive) {
        let mut gaps = 0;
        while gaps < 8 {
            gaps = if d.poll().gap { gaps + 1 } else { 0 };
        }
        while d.poll().gap {}
        while !d.poll().sync {}
    }

    #[test]
    fn headers_and_records_alternate() {
        let mut c = Cartridge::blank();
        c.sectors[0][0] = 0x01; // HDFLAG
        c.sectors[0][1] = 254; // HDNUMB
        c.sectors[0][HEADER_LEN] = 0x06; // RECFLG
        c.sectors[1][1] = 253;
        let (mut d, _) = drive(c);
        find_block(&mut d);
        let h: Vec<u8> = (0..HEADER_LEN).map(|_| d.read()).collect();
        assert_eq!((h[0], h[1]), (1, 254));
        find_block(&mut d);
        assert_eq!(d.read(), 0x06, "the record follows");
        find_block(&mut d);
        d.read();
        assert_eq!(d.read(), 253, "sector 1's header");
    }

    #[test]
    fn writes_the_record_after_a_header_and_flushes() {
        let (mut d, bytes) = drive(Cartridge::blank());
        find_block(&mut d);
        for _ in 0..HEADER_LEN {
            d.read();
        }
        d.set_write(true);
        for i in 0..(PREAMBLE_BYTES + RECORD_LEN + 3) {
            d.write(i as u8);
        }
        d.set_write(false);
        d.set_motor(false);
        let b = bytes.lock().unwrap();
        assert_eq!(b[HEADER_LEN], PREAMBLE_BYTES as u8, "preamble skipped");
        assert_eq!(
            b[HEADER_LEN + RECORD_LEN - 1],
            (PREAMBLE_BYTES + RECORD_LEN - 1) as u8
        );
        assert_eq!(b[HEADER_LEN + RECORD_LEN], 0, "next sector untouched");
    }

    #[test]
    fn protection_and_no_cartridge() {
        let mut c = Cartridge::blank();
        c.protected = true;
        let (mut d, bytes) = drive(c);
        assert!(d.poll().write_protected);
        find_block(&mut d);
        d.set_write(true);
        for _ in 0..40 {
            d.write(0x55);
        }
        d.set_write(false);
        d.flush().unwrap();
        assert!(bytes.lock().unwrap()[..HEADER_LEN].iter().all(|&b| b == 0));
        let mut e = Microdrive::new();
        e.set_motor(true);
        assert_eq!(e.poll(), Status::default(), "no cartridge: no gap ever");
        assert_eq!(e.read(), 0xff);
    }

    #[test]
    fn savestate_round_trip() {
        let (mut d, _) = drive(Cartridge::blank());
        find_block(&mut d);
        d.read();
        let mut w = StateWriter::new();
        d.save(&mut w);
        let s = w.into_bytes();
        let (mut e, _) = drive(Cartridge::blank());
        e.load(&mut StateReader::new(&s)).unwrap();
        assert_eq!(
            (e.sector, e.record, e.phase, e.index),
            (d.sector, d.record, d.phase, d.index)
        );
    }
}
