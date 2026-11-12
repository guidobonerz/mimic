//! The MSX's cassette recorder playing and recording CAS images
//! ([`media::msx_cas`]).
//!
//! **Signal** (MSX Red Book, "Cassette": the BIOS's TAPOON/TAPOUT format
//! at 1200 baud). A block is a header tone of 2400 Hz and then its bytes;
//! a bit is 1/1200 s long: one cycle of 1200 Hz for a 0, two cycles of
//! 2400 Hz for a 1; a byte is a start bit (0), eight data bits from bit 0
//! up and two stop bits (1). The header before a file's first block is
//! 15 360 cycles long (6.4 s), the others 3 840 (1.6 s). The deck puts a
//! second of silence before a long header and half a second before a
//! short one (its own choice; the BIOS stops the motor between blocks
//! anyway).
//!
//! **Motor.** The computer's motor relay ([`MsxTape::set_motor`]) starts
//! and stops the tape while PLAY or RECORD is down. Inserting a tape
//! presses PLAY, so `CLOAD` alone loads.
//!
//! **Recording.** With RECORD down the deck follows the computer's output
//! ([`MsxTape::write`]) and decodes it back into bytes: half cycles are
//! measured from edge to edge, so the output's polarity does not matter;
//! 400 like half cycles in a row are a header tone and give the length of
//! a short one, a half cycle half as long again or more is a long one. Either baud rate of the BIOS (1200,
//! 2400) is understood; played back the tape is always 1200 baud, which
//! the BIOS recognises by itself. Recording replaces the tape from the
//! block under the head on. The image is written back when RECORD is
//! released (STOP, PLAY, rewinding), on flush and on eject; the backend
//! must be able to grow.
//!
//! The machine keeps the deck in step with [`MsxTape::sync`] (CPU
//! T-states) before it looks at the level or changes a line.
//!
//! Keys ([`MediaSlot::control`]): "play", "record", "stop", "rewind",
//! "forward".

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};
use media::msx_cas;

/// Tape time: ticks of 1/4800 s, half a cycle of 2400 Hz.
const TICK_HZ: u64 = 4800;
const LONG_HEADER: u64 = 15_360 * 2;
const SHORT_HEADER: u64 = 3_840 * 2;
const LONG_GAP: u64 = TICK_HZ;
const SHORT_GAP: u64 = TICK_HZ / 2;
/// Ticks of a byte: 11 bits of four ticks.
const BYTE: u64 = 44;
/// Like half cycles in a row that make a header tone.
const HEADER_RUN: u32 = 400;

/// The recording side: the computer's output decoded into bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Decoder {
    /// The output's level and the time of its last edge.
    level: bool,
    last_edge: Option<u64>,
    /// The last half cycle's length and how many like it came in a row.
    prev: u64,
    run: u32,
    /// The length of a short half cycle, from the last header tone.
    short: u64,
    /// A header tone came since the last byte: the next byte opens a block.
    new_block: bool,
    /// Inside a byte: bits taken (start bit included) and the data so far.
    bits: u8,
    data: u8,
    /// Inside a bit: quarters of it that have passed (a short half cycle is
    /// one, a long one two) and whether it is made of long ones (a 0).
    quarters: u8,
    zero: bool,
}

impl Decoder {
    fn drop_byte(&mut self) {
        self.bits = 0;
        self.quarters = 0;
    }

    /// A half cycle of `len` T-states ended. Returns a finished byte and
    /// whether it opens a block.
    fn half(&mut self, len: u64) -> Option<(u8, bool)> {
        let like = self.prev != 0 && len * 4 >= self.prev * 3 && len * 4 <= self.prev * 5;
        self.run = if like { self.run + 1 } else { 0 };
        self.prev = len;
        if self.run >= HEADER_RUN && self.bits == 0 {
            self.short = len;
            self.new_block = true;
        }
        if self.short == 0 {
            return None;
        }
        if len * 2 > self.short * 7 {
            // Far too long: silence. Whatever was begun is lost.
            self.drop_byte();
            return None;
        }
        let long = len * 2 >= self.short * 3;
        if self.bits == 0 && self.quarters == 0 && !long {
            // Waiting for a start bit.
            return None;
        }
        if self.quarters == 0 {
            self.zero = long;
        } else if self.zero != long {
            // Neither a 0 nor a 1.
            self.drop_byte();
            return None;
        }
        self.quarters += if long { 2 } else { 1 };
        if self.quarters < 4 {
            return None;
        }
        self.quarters = 0;
        match self.bits {
            0 => self.data = 0,
            1..=8 => self.data |= u8::from(!self.zero) << (self.bits - 1),
            _ => {}
        }
        if self.bits == 0 && !self.zero {
            return None;
        }
        self.bits += 1;
        if self.bits < 10 {
            return None;
        }
        // The first stop bit is in. The second is not waited for: after a
        // block's last byte its last half cycle has no edge to end it.
        self.bits = 0;
        self.run = 0;
        Some((self.data, std::mem::take(&mut self.new_block)))
    }
}

pub struct MsxTape {
    media: Option<Media>,
    blocks: Vec<Vec<u8>>,
    cpu_hz: u64,
    play: bool,
    record: bool,
    motor: bool,
    protected: bool,
    /// The head: the block and the tick into its signal.
    block: usize,
    tick: u64,
    /// T-states × 4800 not yet a whole tick.
    phase: u64,
    /// The time [`MsxTape::sync`] last saw.
    synced: u64,
    /// Recorded blocks not yet written to the image.
    dirty: bool,
    decoder: Decoder,
}

impl MsxTape {
    pub fn new(cpu_hz: u32) -> Self {
        Self {
            media: None,
            blocks: Vec::new(),
            cpu_hz: u64::from(cpu_hz.max(1)),
            play: false,
            record: false,
            motor: false,
            protected: false,
            block: 0,
            tick: 0,
            phase: 0,
            synced: 0,
            dirty: false,
            decoder: Decoder::default(),
        }
    }

    pub fn has_tape(&self) -> bool {
        self.media.is_some()
    }

    /// The tape's blocks as they are now.
    pub fn blocks(&self) -> &[Vec<u8>] {
        &self.blocks
    }

    /// The block under the head and the number of blocks.
    pub fn counter(&self) -> (usize, usize) {
        (self.block, self.blocks.len())
    }

    /// The tape moves.
    pub fn running(&self) -> bool {
        self.motor && (self.play || self.record) && self.media.is_some()
    }

    /// Gap, header and data ticks of block `i`.
    fn parts(&self, i: usize) -> (u64, u64, u64) {
        let b = &self.blocks[i];
        let data = b.len() as u64 * BYTE;
        if i == 0 || msx_cas::is_file_header(b) {
            (LONG_GAP, LONG_HEADER, data)
        } else {
            (SHORT_GAP, SHORT_HEADER, data)
        }
    }

    /// The level the computer's cassette input sees (true = high).
    pub fn level(&self) -> bool {
        if !self.running() || !self.play || self.block >= self.blocks.len() {
            return false;
        }
        let (gap, header, _) = self.parts(self.block);
        let Some(t) = self.tick.checked_sub(gap) else {
            return false;
        };
        let Some(t) = t.checked_sub(header) else {
            return t & 1 == 0;
        };
        let Some(&byte) = self.blocks[self.block].get((t / BYTE) as usize) else {
            return false;
        };
        let (bit, sub) = ((t % BYTE) / 4, t % 4);
        let one = match bit {
            0 => false,
            1..=8 => byte >> (bit - 1) & 1 != 0,
            _ => true,
        };
        if one {
            sub & 1 == 0
        } else {
            sub < 2
        }
    }

    /// Bring the deck up to the machine's time `now` (CPU T-states).
    pub fn sync(&mut self, now: u64) {
        let delta = now.saturating_sub(self.synced);
        self.synced = now;
        if !self.running() {
            return;
        }
        self.phase += delta * TICK_HZ;
        let ticks = self.phase / self.cpu_hz;
        self.phase %= self.cpu_hz;
        if !self.play {
            return;
        }
        self.tick += ticks;
        while self.block < self.blocks.len() {
            let (gap, header, data) = self.parts(self.block);
            let len = gap + header + data;
            if self.tick < len {
                return;
            }
            self.tick -= len;
            self.block += 1;
        }
        // The end of the tape: PLAY springs up.
        self.play = false;
        self.tick = 0;
    }

    /// The machine's time starts anew at `now` (reset, a loaded state).
    pub fn set_time(&mut self, now: u64) {
        self.synced = now;
    }

    /// The motor relay, at time `now`.
    pub fn set_motor(&mut self, on: bool, now: u64) {
        self.sync(now);
        if self.motor && !on {
            // What a recording left unfinished is lost with the stop.
            self.decoder = Decoder {
                level: self.decoder.level,
                ..Decoder::default()
            };
        }
        self.motor = on;
    }

    /// The computer's cassette output, at time `now`.
    pub fn write(&mut self, level: bool, now: u64) {
        if level == self.decoder.level {
            return;
        }
        self.decoder.level = level;
        if !self.record || !self.running() {
            return;
        }
        let Some(last) = self.decoder.last_edge.replace(now) else {
            return;
        };
        if let Some((byte, new_block)) = self.decoder.half(now - last) {
            if new_block || self.blocks.is_empty() {
                self.blocks.push(Vec::new());
            }
            if let Some(b) = self.blocks.last_mut() {
                b.push(byte);
            }
            self.block = self.blocks.len();
            self.dirty = true;
        }
    }

    fn wind(&mut self, to: usize) {
        self.release_record();
        self.play = false;
        self.block = to;
        self.tick = 0;
        self.phase = 0;
    }

    /// RECORD comes up: the image gets what was recorded.
    fn release_record(&mut self) {
        if self.record {
            self.record = false;
            self.decoder = Decoder::default();
            let _ = self.write_back();
        }
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        if !self.dirty {
            return Ok(());
        }
        let Some(media) = &mut self.media else {
            return Ok(());
        };
        let bytes = msx_cas::image(&self.blocks);
        if media.backend.len() != bytes.len() as u64 {
            media.backend.set_len(bytes.len() as u64)?;
        }
        media.backend.write(0, &bytes)?;
        self.dirty = false;
        Ok(())
    }
}

impl MediaSlot for MsxTape {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let blocks = msx_cas::blocks(&bytes)?;
        let _ = self.eject();
        self.blocks = blocks;
        self.media = Some(media);
        self.wind(0);
        self.play = true;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.wind(0);
        let _ = self.write_back();
        self.dirty = false;
        self.blocks.clear();
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        self.protected = on;
        if on {
            self.release_record();
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    /// Active while the tape moves.
    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.running(),
            error: false,
            track: None,
        }
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        if self.media.is_none() && command != "stop" {
            return Err(MediaError::NoMedia);
        }
        match command {
            "play" => {
                self.release_record();
                self.play = self.block < self.blocks.len();
            }
            "record" => {
                if self.protected {
                    return Err(MediaError::Control("the tape is write-protected".into()));
                }
                // From the block under the head on the tape is new.
                self.play = false;
                self.record = true;
                self.blocks.truncate(self.block);
                self.tick = 0;
                self.decoder = Decoder {
                    level: self.decoder.level,
                    ..Decoder::default()
                };
                self.dirty = true;
            }
            "stop" => {
                self.release_record();
                self.play = false;
            }
            "rewind" => self.wind(0),
            "forward" => self.wind(self.blocks.len()),
            other => {
                return Err(MediaError::Control(format!(
                    "'{other}' (play, record, stop, rewind, forward)"
                )))
            }
        }
        Ok(())
    }
}

const TAG: [u8; 4] = *b"MTAP";

impl Savestate for MsxTape {
    /// The tape is media, not state: a savestate keeps the keys, the head's
    /// place and the decoder on whatever tape is inserted.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.bool(self.play);
            w.bool(self.record);
            w.bool(self.motor);
            w.u32(self.block as u32);
            w.u64(self.tick);
            w.u64(self.phase);
            w.u64(self.synced);
            let d = &self.decoder;
            w.bool(d.level);
            w.bool(d.last_edge.is_some());
            w.u64(d.last_edge.unwrap_or(0));
            w.u64(d.prev);
            w.u32(d.run);
            w.u64(d.short);
            w.bool(d.new_block);
            w.u8(d.bits);
            w.u8(d.data);
            w.u8(d.quarters);
            w.bool(d.zero);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let play = c.bool()?;
        let record = c.bool()?;
        let motor = c.bool()?;
        let block = c.u32()? as usize;
        let tick = c.u64()?;
        let phase = c.u64()?;
        let synced = c.u64()?;
        let level = c.bool()?;
        let has_edge = c.bool()?;
        let last_edge = c.u64()?;
        let decoder = Decoder {
            level,
            last_edge: has_edge.then_some(last_edge),
            prev: c.u64()?,
            run: c.u32()?,
            short: c.u64()?,
            new_block: c.bool()?,
            bits: c.u8()?.min(9),
            data: c.u8()?,
            quarters: c.u8()?.min(3),
            zero: c.bool()?,
        };
        let has = self.media.is_some();
        self.play = play && has && !record;
        self.record = record && has && !self.protected;
        self.motor = motor;
        self.block = block.min(self.blocks.len());
        self.tick = tick;
        self.phase = phase % self.cpu_hz;
        self.synced = synced;
        self.decoder = decoder;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;
    use std::sync::{Arc, Mutex};

    const HZ: u32 = 3_579_545;

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl MediaBackend for Shared {
        fn len(&self) -> u64 {
            self.0.lock().unwrap().len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let d = self.0.lock().unwrap();
            let o = offset as usize;
            buf.copy_from_slice(&d[o..o + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
            let mut d = self.0.lock().unwrap();
            let o = offset as usize;
            d[o..o + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
        fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
            self.0.lock().unwrap().resize(len as usize, 0);
            Ok(())
        }
    }

    fn deck(file: &Shared) -> MsxTape {
        let mut d = MsxTape::new(HZ);
        d.insert(Media {
            name: "t.cas".into(),
            kind: MediaKind::Tape,
            backend: Box::new(file.clone()),
        })
        .unwrap();
        d
    }

    fn file_of(blocks: &[Vec<u8>]) -> Shared {
        Shared(Arc::new(Mutex::new(msx_cas::image(blocks))))
    }

    fn sample_blocks() -> Vec<Vec<u8>> {
        let mut header = vec![0xd3; 10];
        header.extend_from_slice(b"HELLO ");
        vec![header, vec![0x00, 0xff, 0x55, 0xaa, 0x01, 0x80, 0x42]]
    }

    #[test]
    fn the_motor_runs_the_tape_and_the_header_is_2400_hz() {
        let mut d = deck(&file_of(&sample_blocks()));
        assert!(d.has_tape() && !d.running(), "PLAY is down, the motor off");
        d.sync(1_000_000);
        assert_eq!(d.counter(), (0, 2));
        assert!(!d.level());
        d.set_motor(true, 1_000_000);
        assert!(d.status().active);
        // One second of gap, then the tone: count edges over a second.
        let start = 1_000_000 + u64::from(HZ) * 3 / 2;
        let mut edges = 0;
        let mut prev = false;
        for i in 0..u64::from(HZ) / 10 {
            d.sync(start + i * 10);
            let l = d.level();
            edges += u32::from(l != prev);
            prev = l;
        }
        assert!((4795..=4805).contains(&edges), "{edges} edges a second");
        // Without the motor the tape stands.
        d.set_motor(false, start + u64::from(HZ));
        let at = d.tick;
        d.sync(start + 10 * u64::from(HZ));
        assert_eq!(d.tick, at);
        // To the end: PLAY springs up.
        d.set_motor(true, start + 10 * u64::from(HZ));
        d.sync(start + 40 * u64::from(HZ));
        assert_eq!(d.counter(), (2, 2));
        assert!(!d.running());
        d.control("play").unwrap();
        assert!(!d.running(), "nothing left to play");
        d.control("rewind").unwrap();
        d.control("play").unwrap();
        assert!(d.running());
        assert!(d.control("fly").is_err());
    }

    /// Play `from` into `to`'s recording input.
    fn copy(from: &mut MsxTape, to: &mut MsxTape) {
        from.set_motor(true, 0);
        to.set_motor(true, 0);
        let mut now = 0;
        while from.running() {
            now += 50;
            from.sync(now);
            to.sync(now);
            to.write(from.level(), now);
        }
    }

    #[test]
    fn a_recording_decodes_what_another_deck_plays() {
        let blocks = sample_blocks();
        let mut player = deck(&file_of(&blocks));
        let file = Shared::default();
        let mut recorder = deck(&file);
        recorder.control("record").unwrap();
        copy(&mut player, &mut recorder);
        assert_eq!(recorder.blocks(), blocks);
        assert!(recorder.dirty());
        assert!(file.0.lock().unwrap().is_empty(), "RECORD is still down");
        recorder.control("stop").unwrap();
        assert!(!recorder.dirty());
        assert_eq!(*file.0.lock().unwrap(), msx_cas::image(&blocks));

        // Recording from the second block on keeps the first.
        let mut player = deck(&file_of(&[vec![1, 2, 3]]));
        let mut recorder = deck(&file);
        recorder.set_motor(true, 0);
        recorder.sync(u64::from(HZ) * 9);
        assert_eq!(recorder.counter().0, 1);
        recorder.set_motor(false, u64::from(HZ) * 9);
        recorder.control("record").unwrap();
        recorder.set_time(0);
        copy(&mut player, &mut recorder);
        assert_eq!(recorder.blocks(), [blocks[0].clone(), vec![1, 2, 3]]);
        assert!(recorder.eject().is_some());
        assert_eq!(
            msx_cas::blocks(&file.0.lock().unwrap()).unwrap(),
            [blocks[0].clone(), vec![1, 2, 3]]
        );
        // A protected tape refuses RECORD.
        let mut d = deck(&file);
        d.write_protect(true);
        assert!(d.control("record").is_err());
    }

    #[test]
    fn savestate_keeps_the_place() {
        let mut d = deck(&file_of(&sample_blocks()));
        d.set_motor(true, 0);
        d.sync(u64::from(HZ) * 8);
        let mut w = StateWriter::new();
        d.save(&mut w);
        let data = w.into_bytes();
        let mut e = deck(&file_of(&sample_blocks()));
        e.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!((e.block, e.tick, e.phase), (d.block, d.tick, d.phase));
        assert!(e.running());
        for t in 0..2000 {
            let now = u64::from(HZ) * 8 + t * 97;
            d.sync(now);
            e.sync(now);
            assert_eq!(d.level(), e.level());
        }
        // Without a tape nothing plays.
        let mut empty = MsxTape::new(HZ);
        empty.load(&mut StateReader::new(&data)).unwrap();
        assert!(!empty.running());
        assert!(empty.load(&mut StateReader::new(&data[..9])).is_err());
    }
}
