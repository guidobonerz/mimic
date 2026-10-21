//! The CPC's cassette deck (built into the 464, a recorder on the 664 and
//! 6128) playing and recording CDT images ([`media::cdt`]).
//!
//! Lines: MOTOR (PPI port C bit 4, from the computer), READ (PPI port B
//! bit 7, to the computer) and WRITE (PPI port C bit 5). The machine ticks
//! the deck once per microsecond. Tape time is kept in 28 MHz units: a
//! microsecond is 28, a TZX T-state (3.5 MHz) 8.
//!
//! Playing gives the tape's level on READ while the motor runs and PLAY
//! is down. Recording (PLAY and RECORD, motor on) replaces the tape from
//! the current position with the levels of WRITE; it is written back to
//! the media when the motor stops, on a key, flush and eject.
//!
//! Keys ([`MediaSlot::control`]): "play", "record", "stop", "rewind" and
//! "forward" (to the start or the end at once). Inserting a tape presses
//! PLAY.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};
use media::cdt::{Segment, Tape};

/// 28 MHz units per microsecond and per TZX T-state.
const UNITS_PER_US: u64 = 28;
const UNITS_PER_TZX: u64 = 8;

struct Cassette {
    media: Media,
    tape: Tape,
    dirty: bool,
    protected: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Keys {
    #[default]
    Stop,
    Play,
    Record,
}

#[derive(Default)]
pub struct CpcTape {
    cassette: Option<Cassette>,
    keys: Keys,
    motor: bool,
    /// Segment under the head and units into it (playing), or units since
    /// the last WRITE edge (recording).
    position: usize,
    elapsed: u64,
    write_level: bool,
    /// A recording replaces the tape from `position`.
    recording: bool,
}

impl CpcTape {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_tape(&self) -> bool {
        self.cassette.is_some()
    }

    pub fn set_motor(&mut self, on: bool) {
        if self.motor && !on {
            self.end_recording();
            let _ = self.write_back();
        }
        self.motor = on;
    }

    pub fn motor(&self) -> bool {
        self.motor
    }

    /// Segment position and the tape's segment count.
    pub fn counter(&self) -> (usize, usize) {
        (
            self.position,
            self.cassette.as_ref().map_or(0, |c| c.tape.segments.len()),
        )
    }

    fn moving(&self) -> bool {
        self.motor && self.keys != Keys::Stop && self.cassette.is_some()
    }

    /// READ (true = high).
    pub fn read(&self) -> bool {
        match (&self.cassette, self.keys) {
            (Some(c), Keys::Play) => c.tape.segments.get(self.position).is_some_and(|s| s.level),
            _ => false,
        }
    }

    /// WRITE from the computer.
    pub fn set_write(&mut self, level: bool) {
        if level == self.write_level {
            return;
        }
        if self.keys == Keys::Record && self.moving() {
            self.record_edge();
        }
        self.write_level = level;
    }

    /// The level that ends now goes onto the tape.
    fn record_edge(&mut self) {
        let Some(c) = &mut self.cassette else {
            return;
        };
        if c.protected {
            return;
        }
        if !self.recording {
            self.recording = true;
            c.tape.segments.truncate(self.position);
        } else {
            let len = (self.elapsed / UNITS_PER_TZX).max(1) as u32;
            let level = self.write_level;
            match c.tape.segments.last_mut() {
                Some(s) if s.level == level => s.len = s.len.saturating_add(len),
                _ => c.tape.segments.push(Segment { level, len }),
            }
            self.position = c.tape.segments.len();
        }
        c.dirty = true;
        self.elapsed = 0;
    }

    fn end_recording(&mut self) {
        if self.recording {
            self.record_edge();
            self.recording = false;
        }
    }

    /// One microsecond.
    pub fn tick(&mut self) {
        if !self.moving() {
            return;
        }
        self.elapsed += UNITS_PER_US;
        if self.keys == Keys::Record {
            return;
        }
        let Some(c) = &self.cassette else {
            return;
        };
        while let Some(s) = c.tape.segments.get(self.position) {
            let len = u64::from(s.len) * UNITS_PER_TZX;
            if self.elapsed < len {
                break;
            }
            self.elapsed -= len;
            self.position += 1;
        }
        if self.position >= c.tape.segments.len() {
            self.elapsed = 0;
        }
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        let Some(c) = &mut self.cassette else {
            return Ok(());
        };
        if !std::mem::take(&mut c.dirty) {
            return Ok(());
        }
        let bytes = c.tape.to_bytes(c.media.backend.len() as usize)?;
        c.media.backend.write(0, &bytes)
    }

    fn press(&mut self, keys: Keys) {
        self.end_recording();
        let _ = self.write_back();
        self.keys = keys;
        self.elapsed = 0;
    }
}

impl MediaSlot for CpcTape {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let tape = Tape::parse(&bytes)?;
        self.cassette = Some(Cassette {
            media,
            tape,
            dirty: false,
            protected: false,
        });
        self.position = 0;
        self.elapsed = 0;
        self.recording = false;
        self.keys = Keys::Play;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.press(Keys::Stop);
        self.cassette.take().map(|c| c.media)
    }

    /// The write-protect tab: broken off (`on`), RECORD records nothing.
    fn write_protect(&mut self, on: bool) {
        if let Some(c) = &mut self.cassette {
            c.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        if self.cassette.is_none() && command != "stop" {
            return Err(MediaError::NoMedia);
        }
        match command {
            "play" => self.press(Keys::Play),
            "record" => self.press(Keys::Record),
            "stop" => self.press(Keys::Stop),
            "rewind" | "forward" => {
                self.press(Keys::Stop);
                self.position = if command == "rewind" {
                    0
                } else {
                    self.counter().1
                };
            }
            other => {
                return Err(MediaError::Control(format!(
                    "'{other}' (play, record, stop, rewind, forward)"
                )))
            }
        }
        Ok(())
    }
}

/// A blank CDT with room for about `minutes` of recording.
pub fn blank_tape(minutes: u32) -> Vec<u8> {
    // A CPC recording is about 2 × 2000 pulses per second at 3 bytes per
    // pulse.
    media::cdt::blank(64 + minutes as usize * 60 * 12_000)
}

const TAG: [u8; 4] = *b"CTAP";
const VERSION: u16 = 1;

impl Savestate for CpcTape {
    /// The tape is media, not state: a savestate keeps the keys, the motor
    /// and the position on whatever tape is inserted.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.keys as u8);
            w.bool(self.motor);
            w.u32(self.position as u32);
            w.u64(self.elapsed);
            w.bool(self.write_level);
            w.bool(self.recording);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let keys = match c.u8()? {
            0 => Keys::Stop,
            1 => Keys::Play,
            2 => Keys::Record,
            _ => return Err(StateError::Invalid("tape keys")),
        };
        let motor = c.bool()?;
        let position = c.u32()? as usize;
        let elapsed = c.u64()?;
        let (write_level, recording) = (c.bool()?, c.bool()?);
        self.keys = keys;
        self.motor = motor;
        self.position = position.min(self.counter().1);
        self.elapsed = elapsed;
        self.write_level = write_level;
        self.recording = recording;
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

    fn deck(bytes: Vec<u8>) -> CpcTape {
        let mut d = CpcTape::new();
        d.insert(Media {
            name: "t".into(),
            kind: MediaKind::Tape,
            backend: Box::new(Mem(bytes)),
        })
        .unwrap();
        d
    }

    /// Record a square wave of `periods` half-waves of `us` microseconds.
    fn record(d: &mut CpcTape, periods: usize, us: usize) {
        let mut level = false;
        for _ in 0..periods {
            level = !level;
            d.set_write(level);
            for _ in 0..us {
                d.tick();
            }
        }
    }

    #[test]
    fn records_and_plays_back() {
        let mut d = deck(blank_tape(1));
        d.control("record").unwrap();
        d.set_motor(true);
        record(&mut d, 21, 250);
        d.set_motor(false); // written back
        d.control("rewind").unwrap();
        d.control("play").unwrap();
        d.set_motor(true);
        // The recording starts at the first edge: 20 half-waves of 250 µs
        // (the last level ends with the motor).
        let mut edges = 0;
        let mut prev = d.read();
        for _ in 0..(25 * 250) {
            d.tick();
            if d.read() != prev {
                edges += 1;
                prev = d.read();
            }
        }
        assert!((19..=21).contains(&edges), "{edges}");
        let media = d.eject().unwrap();
        let mut bytes = vec![0; media.backend.len() as usize];
        let mut b = media.backend;
        b.read(0, &mut bytes).unwrap();
        let t = Tape::parse(&bytes).unwrap();
        assert!(
            t.segments[1..t.segments.len() - 1]
                .iter()
                .all(|s| s.len == 875),
            "250 µs = 875 TZX T-states"
        );
    }

    #[test]
    fn stopped_or_protected_records_nothing() {
        let mut d = deck(blank_tape(1));
        d.control("record").unwrap();
        d.write_protect(true);
        d.set_motor(true);
        record(&mut d, 10, 100);
        d.set_motor(false);
        assert_eq!(d.counter(), (0, 0));
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = deck(blank_tape(1));
        d.set_motor(true);
        let mut w = StateWriter::new();
        d.save(&mut w);
        let bytes = w.into_bytes();
        let mut e = deck(blank_tape(1));
        e.load(&mut StateReader::new(&bytes)).unwrap();
        assert!(e.motor());
    }
}
