//! Apple II cassette recorder: plays and records WAV files.
//!
//! The Apple's cassette interface has no motor control: the input ($C060
//! bit 7) is a comparator on the recorder's earphone signal, the output
//! a flip-flop toggled by each access to $C020 (Apple II Reference
//! Manual, "The Cassette Interface"). The monitor writes and reads tones
//! of about 2 kHz (1 bits) and 1 kHz (0 bits) after a 770 Hz header.
//!
//! **Playing.** The recording is read as one mono channel; the input is
//! high above +[`THRESHOLD`] and low below −[`THRESHOLD`] (hysteresis, so
//! noise near zero does not flip it). As a convenience for a recorder the
//! machine cannot stop, the tape only moves while PLAY is down *and* the
//! program listens: for [`LISTEN_WINDOW`] cycles after each read of the
//! input. So `LOAD` (or the monitor's `R` command) finds the start of the
//! recording whenever it is typed, without pressing PLAY at the right
//! moment.
//!
//! **Recording** (PLAY + RECORD): every toggle of the output is an edge;
//! on STOP, flush or eject the edges become a square wave at
//! [`RECORD_RATE`] that replaces the tape from where recording started,
//! and the file is written back as a 16-bit mono WAV. An empty file is a
//! blank tape.
//!
//! Keys ([`MediaSlot::control`]): "play", "record", "stop", "rewind".
//! Inserting a tape presses PLAY.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::wav::Wav;

/// Input hysteresis (16-bit sample units).
pub const THRESHOLD: i16 = 1024;
/// Cycles the tape keeps moving after the program read the input.
pub const LISTEN_WINDOW: u64 = 500_000;
/// Sample rate of recordings.
pub const RECORD_RATE: u32 = 44_100;
/// Most output edges kept while recording (about 20 minutes of 2 kHz).
const MAX_EDGES: usize = 5_000_000;

struct Tape {
    media: Media,
    wav: Wav,
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

pub struct AppleTape {
    hz: u32,
    tape: Option<Tape>,
    keys: Keys,
    /// Cycles of tape played from its start.
    pos: u64,
    level: bool,
    listened_until: u64,
    /// Recording: the tape position where it started and the cycles (since
    /// then) of each output edge.
    rec_start: u64,
    edges: Vec<u64>,
    /// The cycle of the last tick (for the status display only).
    now: u64,
}

impl std::fmt::Debug for AppleTape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleTape")
            .field("keys", &self.keys)
            .field("pos", &self.pos)
            .field("tape", &self.tape.as_ref().map(|t| &t.media.name))
            .finish()
    }
}

impl AppleTape {
    /// A recorder for a machine running at `hz`.
    pub fn new(hz: u32) -> Self {
        Self {
            hz: hz.max(1),
            tape: None,
            keys: Keys::Stop,
            pos: 0,
            level: false,
            listened_until: 0,
            rec_start: 0,
            edges: Vec::new(),
            now: 0,
        }
    }

    pub fn has_tape(&self) -> bool {
        self.tape.is_some()
    }

    fn sample_at(&self, pos: u64) -> Option<i16> {
        let t = self.tape.as_ref()?;
        let i = pos * u64::from(t.wav.rate) / u64::from(self.hz);
        t.wav.samples.get(i as usize).copied()
    }

    /// The input as the program reads it at cycle `now`.
    pub fn read(&mut self, now: u64) -> bool {
        self.listened_until = now + LISTEN_WINDOW;
        self.level
    }

    /// The input without moving the tape (debugger peeks).
    pub fn peek(&self) -> bool {
        self.level
    }

    /// The output flip-flop toggled at cycle `now` (an access to $C020).
    pub fn toggle_output(&mut self, _now: u64) {
        if self.keys == Keys::Record && self.edges.len() < MAX_EDGES {
            self.edges.push(self.pos - self.rec_start);
        }
    }

    /// One CPU cycle at `now`.
    pub fn tick(&mut self, now: u64) {
        self.now = now;
        match self.keys {
            Keys::Play if now < self.listened_until && self.tape.is_some() => {
                self.pos += 1;
                if let Some(s) = self.sample_at(self.pos) {
                    if s > THRESHOLD {
                        self.level = true;
                    } else if s < -THRESHOLD {
                        self.level = false;
                    }
                }
            }
            Keys::Record => self.pos += 1,
            _ => {}
        }
    }

    /// Recording stops: the edges replace the tape from where it started.
    fn finish_recording(&mut self) {
        if self.keys != Keys::Record {
            return;
        }
        self.keys = Keys::Stop;
        let edges = std::mem::take(&mut self.edges);
        let (rec_start, end, hz) = (self.rec_start, self.pos, self.hz);
        let Some(t) = &mut self.tape else {
            return;
        };
        if t.protected {
            return;
        }
        // Resample the tape to the recording rate before splicing.
        let keep = (rec_start * u64::from(RECORD_RATE) / u64::from(hz)) as usize;
        let mut samples: Vec<i16> = (0..keep)
            .map(|i| {
                let src = i as u64 * u64::from(t.wav.rate) / u64::from(RECORD_RATE);
                t.wav.samples.get(src as usize).copied().unwrap_or(0)
            })
            .collect();
        let len = ((end - rec_start) * u64::from(RECORD_RATE) / u64::from(hz)) as usize;
        let mut level = false;
        let mut e = edges.iter().peekable();
        for i in 0..len {
            let cycle = i as u64 * u64::from(hz) / u64::from(RECORD_RATE);
            while e.next_if(|&&c| c <= cycle).is_some() {
                level = !level;
            }
            samples.push(if level { 16_000 } else { -16_000 });
        }
        t.wav = Wav {
            rate: RECORD_RATE,
            samples,
        };
        t.dirty = true;
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        self.finish_recording();
        if let Some(t) = &mut self.tape {
            if std::mem::take(&mut t.dirty) {
                t.media.backend.write(0, &t.wav.to_bytes())?;
            }
        }
        Ok(())
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"A2TP", 1, |w| {
            w.u8(match self.keys {
                Keys::Stop => 0,
                Keys::Play => 1,
                Keys::Record => 2,
            });
            w.u64(self.pos);
            w.bool(self.level);
            w.u64(self.listened_until);
            w.u64(self.rec_start);
            w.u32(self.edges.len() as u32);
            for &e in &self.edges {
                w.u64(e);
            }
        });
    }

    /// Atomic: on error the recorder is unchanged. The tape stays.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"A2TP", 1)?;
        let keys = match c.u8()? {
            0 => Keys::Stop,
            1 => Keys::Play,
            2 => Keys::Record,
            _ => return Err(StateError::Invalid("tape keys")),
        };
        let pos = c.u64()?;
        let level = c.bool()?;
        let listened_until = c.u64()?;
        let rec_start = c.u64()?;
        let n = c.u32()? as usize;
        if n > MAX_EDGES || rec_start > pos {
            return Err(StateError::Invalid("tape recording"));
        }
        let edges = (0..n).map(|_| c.u64()).collect::<Result<Vec<_>, _>>()?;
        self.keys = keys;
        self.pos = pos;
        self.level = level;
        self.listened_until = listened_until;
        self.rec_start = rec_start;
        self.edges = edges;
        Ok(())
    }
}

impl MediaSlot for AppleTape {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut data = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut data)?;
        let wav = if data.is_empty() {
            Wav {
                rate: RECORD_RATE,
                samples: Vec::new(),
            }
        } else {
            Wav::parse(&data).map_err(MediaError::Format)?
        };
        let _ = self.eject();
        self.tape = Some(Tape {
            media,
            wav,
            dirty: false,
            protected: false,
        });
        self.pos = 0;
        self.keys = Keys::Play;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.keys = Keys::Stop;
        self.tape.take().map(|t| t.media)
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(t) = &mut self.tape {
            t.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        let recording = self.keys == Keys::Record;
        self.write_back()?;
        if recording {
            // Go on recording after the flushed part.
            self.keys = Keys::Record;
            self.rec_start = self.pos;
        }
        match &mut self.tape {
            Some(t) => t.media.backend.commit(),
            None => Ok(()),
        }
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        if self.tape.is_none() {
            return Err(MediaError::NoMedia);
        }
        match command {
            "play" => {
                self.finish_recording();
                self.keys = Keys::Play;
            }
            "record" => {
                if self.tape.as_ref().is_some_and(|t| t.protected) {
                    return Err(MediaError::WriteProtected);
                }
                self.finish_recording();
                self.keys = Keys::Record;
                self.rec_start = self.pos;
                self.edges.clear();
            }
            "stop" => {
                self.write_back()?;
                self.keys = Keys::Stop;
            }
            "rewind" => {
                self.write_back()?;
                self.keys = Keys::Stop;
                self.pos = 0;
            }
            other => {
                return Err(MediaError::Control(format!(
                    "'{other}' (play, record, stop or rewind)"
                )))
            }
        }
        Ok(())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.keys == Keys::Record
                || (self.keys == Keys::Play && self.now < self.listened_until),
            error: false,
            track: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
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
            let o = offset as usize;
            let n = d.len().max(o + data.len());
            d.resize(n, 0);
            d[o..o + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn tape(data: Vec<u8>) -> (Media, Shared) {
        let s = Shared(Arc::new(Mutex::new(data)));
        (
            Media {
                name: "t.wav".into(),
                kind: MediaKind::Tape,
                backend: Box::new(s.clone()),
            },
            s,
        )
    }

    #[test]
    fn plays_only_while_listened_to() {
        // 1 kHz square wave at 8 kHz.
        let wav = Wav {
            rate: 8000,
            samples: (0..8000)
                .map(|i| if i % 8 < 4 { 8000 } else { -8000 })
                .collect(),
        };
        let mut t = AppleTape::new(1_000_000);
        t.insert(tape(wav.to_bytes()).0).unwrap();
        for now in 0..1000 {
            t.tick(now);
        }
        assert_eq!(t.pos, 0, "nobody listens");
        let mut flips = 0;
        let mut last = t.read(1000);
        for now in 1000..11_000 {
            t.tick(now);
            let l = t.read(now);
            flips += usize::from(l != last);
            last = l;
        }
        // 10 ms of 1 kHz: 20 edges.
        assert!((19..=21).contains(&flips), "{flips}");
    }

    #[test]
    fn records_edges_into_a_wav() {
        let (m, store) = tape(Vec::new());
        let mut t = AppleTape::new(1_000_000);
        t.insert(m).unwrap();
        t.control("record").unwrap();
        for now in 0..10_000u64 {
            if now % 500 == 0 {
                t.toggle_output(now);
            }
            t.tick(now);
        }
        t.control("stop").unwrap();
        let w = Wav::parse(&store.0.lock().unwrap()).unwrap();
        assert_eq!(w.rate, RECORD_RATE);
        assert_eq!(w.samples.len(), 441);
        let flips = w.samples.windows(2).filter(|p| p[0] != p[1]).count();
        assert!((18..=20).contains(&flips), "{flips}");
    }
}
