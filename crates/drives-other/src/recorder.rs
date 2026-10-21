//! Atari program recorder (410, 1010) playing and recording CAS images.
//!
//! Lines as on the SIO connector: MOTOR CONTROL (PIA CA2, low = motor on),
//! DATA IN to the computer (POKEY's serial input; the recorder's FSK
//! decoder turns the tones into a 600 baud serial signal) and DATA OUT
//! from the computer (POKEY's serial output in two-tone mode). The tape
//! only moves while a key is down and the motor runs.
//!
//! Playing: each record waits out its gap (the inter-record tone, a mark
//! level), then its bytes go out 8N1 at the record's baud rate — the OS
//! measures that rate from the edges of the two `$55` sync bytes.
//! Recording (PLAY + RECORD): bytes the computer sends while the motor runs
//! form records; a pause of more than 50 ms starts a new one, the pause
//! becoming its gap. Recording replaces the tape from the current record
//! on and is written back when it stops (motor off, STOP, flush, eject) —
//! into the padding of a blank tape (`media::cas::blank`), the image keeps
//! its size.
//!
//! Keys ([`MediaSlot::control`]): "play", "record", "stop", "rewind",
//! "forward". Inserting a tape presses PLAY.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::cas::{self, Cas, Record};

use crate::sio::{SerialIn, SerialTx};

/// A pause in the computer's output longer than this ends a record.
const RECORD_PAUSE_MS: u64 = 50;

struct Tape {
    media: Media,
    cas: Cas,
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

pub struct Recorder {
    hz: u32,
    tape: Option<Tape>,
    keys: Keys,
    motor: bool,
    /// Index of the next record under the head.
    position: usize,
    /// Cycles of the current record's gap still to pass (playing).
    gap_left: Option<u64>,
    tx: SerialTx,
    /// Recording: the record being written and when its last byte ended.
    current: Option<Record>,
    last_byte: u64,
}

impl Recorder {
    pub fn new(hz: u32) -> Self {
        Self {
            hz,
            tape: None,
            keys: Keys::Stop,
            motor: false,
            position: 0,
            gap_left: None,
            tx: SerialTx::with_baud(hz, u32::from(cas::BAUD)),
            current: None,
            last_byte: 0,
        }
    }

    pub fn has_tape(&self) -> bool {
        self.tape.is_some()
    }

    /// (record under the head, records on the tape).
    pub fn counter(&self) -> (usize, usize) {
        (
            self.position,
            self.tape.as_ref().map_or(0, |t| t.cas.records.len()),
        )
    }

    fn running(&self) -> bool {
        self.motor && self.keys != Keys::Stop && self.tape.is_some()
    }

    /// MOTOR CONTROL from the computer (true = run).
    pub fn set_motor(&mut self, on: bool, now: u64) {
        if on == self.motor {
            return;
        }
        self.motor = on;
        if self.keys == Keys::Record {
            if on {
                self.last_byte = now;
            } else {
                self.finish_record();
                let _ = self.write_back();
            }
        }
    }

    /// A byte the computer sent (recorded while recording).
    pub fn byte_from_computer(&mut self, v: u8, now: u64) {
        if !(self.running() && self.keys == Keys::Record) {
            return;
        }
        let pause = now.saturating_sub(self.last_byte);
        let pause_ms = pause * 1000 / u64::from(self.hz);
        if self.current.is_none() || pause_ms > RECORD_PAUSE_MS {
            self.finish_record();
            self.current = Some(Record {
                gap_ms: pause_ms.min(u64::from(u16::MAX)) as u16,
                baud: cas::BAUD,
                data: Vec::new(),
            });
        }
        if let Some(r) = &mut self.current {
            r.data.push(v);
        }
        self.last_byte = now;
    }

    fn finish_record(&mut self) {
        let Some(rec) = self.current.take() else {
            return;
        };
        if let Some(t) = &mut self.tape {
            t.cas.records.truncate(self.position);
            t.cas.records.push(rec);
            t.dirty = true;
            self.position += 1;
        }
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        let Some(t) = &mut self.tape else {
            return Ok(());
        };
        if !t.dirty || t.protected {
            return Ok(());
        }
        let mut bytes = t.cas.to_bytes();
        let len = t.media.backend.len() as usize;
        if bytes.len() > len {
            return Err(MediaError::Format(
                "recording does not fit the tape (use a blank tape)".into(),
            ));
        }
        bytes.resize(len, 0);
        t.media.backend.write(0, &bytes)?;
        t.dirty = false;
        Ok(())
    }

    /// One cycle; the serial signal towards the computer.
    pub fn tick(&mut self, now: u64) -> SerialIn {
        if self.running() && self.keys == Keys::Play && self.tx.is_idle() {
            let next = self
                .tape
                .as_ref()
                .and_then(|t| t.cas.records.get(self.position))
                .map(|r| (r.gap_ms, r.baud));
            if let Some((gap_ms, baud)) = next {
                let left = self
                    .gap_left
                    .get_or_insert(u64::from(gap_ms) * u64::from(self.hz) / 1000);
                if *left == 0 {
                    self.gap_left = None;
                    let data = self.tape.as_ref().expect("tape").cas.records[self.position]
                        .data
                        .clone();
                    self.tx.set_baud(u32::from(baud));
                    self.tx.send(now, &data);
                    self.position += 1;
                } else {
                    *left -= 1;
                }
            }
        }
        if self.running() || !self.tx.is_idle() {
            self.tx.tick(now)
        } else {
            SerialIn {
                level: true,
                busy: false,
                byte: None,
            }
        }
    }

    fn press(&mut self, keys: Keys) {
        if self.keys == Keys::Record && keys != Keys::Record {
            self.finish_record();
            let _ = self.write_back();
        }
        self.keys = keys;
        self.gap_left = None;
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"A410", 1, |w| {
            w.u8(match self.keys {
                Keys::Stop => 0,
                Keys::Play => 1,
                Keys::Record => 2,
            });
            w.bool(self.motor);
            w.u32(self.position as u32);
            w.bool(self.gap_left.is_some());
            w.u64(self.gap_left.unwrap_or(0));
            w.u64(self.last_byte);
            let queued: Vec<(u64, u8)> = self.tx.queued().collect();
            w.u32(queued.len() as u32);
            for (t, b) in queued {
                w.u64(t);
                w.u8(b);
            }
        });
    }

    /// Restores the mechanics and the transfer; the tape stays as inserted
    /// (a recording in progress is closed first).
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"A410", 1)?;
        let keys = match c.u8()? {
            0 => Keys::Stop,
            1 => Keys::Play,
            2 => Keys::Record,
            _ => return Err(StateError::Invalid("recorder keys")),
        };
        let motor = c.bool()?;
        let position = c.u32()? as usize;
        let has_gap = c.bool()?;
        let gap = c.u64()?;
        let last_byte = c.u64()?;
        let n = c.u32()? as usize;
        if n > 0x10000 {
            return Err(StateError::Invalid("recorder queue"));
        }
        let mut queue = Vec::with_capacity(n);
        for _ in 0..n {
            queue.push((c.u64()?, c.u8()?));
        }
        self.finish_record();
        self.keys = keys;
        self.motor = motor;
        self.position = position.min(self.counter().1);
        self.gap_left = has_gap.then_some(gap);
        self.last_byte = last_byte;
        self.tx.clear();
        for (t, b) in queue {
            self.tx.send(t, &[b]);
        }
        Ok(())
    }
}

impl MediaSlot for Recorder {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let cas = Cas::parse(&bytes)?;
        let _ = self.write_back();
        self.tape = Some(Tape {
            media,
            cas,
            dirty: false,
            protected: false,
        });
        self.position = 0;
        self.gap_left = None;
        self.tx.clear();
        self.keys = Keys::Play;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.finish_record();
        let _ = self.write_back();
        self.keys = Keys::Stop;
        self.tx.clear();
        self.tape.take().map(|t| t.media)
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(t) = &mut self.tape {
            t.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.finish_record();
        self.write_back()?;
        match &mut self.tape {
            Some(t) => t.media.backend.commit(),
            None => Ok(()),
        }
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        if self.tape.is_none() && command != "stop" {
            return Err(MediaError::NoMedia);
        }
        match command {
            "play" => self.press(Keys::Play),
            "record" => self.press(Keys::Record),
            "stop" => self.press(Keys::Stop),
            "rewind" | "forward" => {
                self.press(Keys::Stop);
                self.tx.clear();
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

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;

    const HZ: u32 = 1_773_447;

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
            self.0
                .get_mut(o..o + data.len())
                .ok_or(MediaError::OutOfRange)?
                .copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn tape(bytes: Vec<u8>) -> Media {
        Media {
            name: "t.cas".into(),
            kind: MediaKind::Tape,
            backend: Box::new(Mem(bytes)),
        }
    }

    fn ms(n: u64) -> u64 {
        n * u64::from(HZ) / 1000
    }

    #[test]
    fn plays_records_after_their_gaps_only_with_the_motor() {
        let cas = Cas {
            description: vec![],
            records: vec![Record {
                gap_ms: 100,
                baud: 600,
                data: vec![0x55, 0x55],
            }],
        };
        let mut r = Recorder::new(HZ);
        r.insert(tape(cas.to_bytes())).unwrap();
        // Motor off: nothing.
        assert!((0..ms(200)).all(|t| r.tick(t).byte.is_none()));
        r.set_motor(true, ms(200));
        let got: Vec<(u64, u8)> = (ms(200)..ms(400))
            .filter_map(|t| r.tick(t).byte.map(|b| (t, b)))
            .collect();
        assert_eq!(got.len(), 2);
        // Gap 100 ms, then 10 bits at 600 baud ≈ 16.7 ms per byte.
        let first = got[0].0 - ms(200);
        assert!((ms(116)..ms(118)).contains(&first), "{first}");
        assert_eq!(r.counter(), (1, 1));
    }

    #[test]
    fn records_and_plays_back() {
        let mut r = Recorder::new(HZ);
        r.insert(tape(cas::blank(4096))).unwrap();
        r.control("record").unwrap();
        r.set_motor(true, 0);
        // Leader of 1 s, then a record; a 200 ms pause, then another.
        let mut t = ms(1000);
        for b in [0x55, 0x55, 0xfc] {
            r.byte_from_computer(b, t);
            t += ms(17);
        }
        t += ms(200);
        r.byte_from_computer(0x01, t);
        r.set_motor(false, t + ms(10));
        let mut media = r.eject().unwrap();
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes).unwrap();
        assert_eq!(bytes.len(), 4096, "the tape keeps its size");
        let cas = Cas::parse(&bytes).unwrap();
        assert_eq!(cas.records.len(), 2);
        assert_eq!(cas.records[0].data, [0x55, 0x55, 0xfc]);
        assert!((999..=1001).contains(&cas.records[0].gap_ms));
        assert!((200..=218).contains(&cas.records[1].gap_ms));
        // Too much for a full-size tape without padding.
        let mut r = Recorder::new(HZ);
        r.insert(tape(Cas::parse(&cas::blank(0)).unwrap().to_bytes()))
            .unwrap();
        r.control("record").unwrap();
        r.set_motor(true, 0);
        r.byte_from_computer(1, 10);
        assert!(r.flush().is_err());
    }

    #[test]
    fn keys_and_counter() {
        let mut r = Recorder::new(HZ);
        assert!(r.control("play").is_err(), "no tape");
        assert!(r.control("stop").is_ok());
        r.insert(tape(cas::blank(100))).unwrap();
        assert!(r.control("eject").is_err());
        r.control("forward").unwrap();
        assert_eq!(r.counter(), (0, 0));
    }
}
