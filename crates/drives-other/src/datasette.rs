//! Commodore datasette (C2N/1530, 1531) playing and recording TAP images.
//!
//! Lines as on the cassette port: MOTOR (from the computer, on = the tape
//! moves while a key is down), SENSE (to the computer, active while PLAY,
//! RECORD or a spool key is down), READ (to the computer) and WRITE (from
//! the computer). Machines map these onto their pins and tick the
//! datasette once per cycle of `host_hz`.
//!
//! Playing: a full-wave recording (TAP versions 0/1) gives one falling
//! edge of READ at the end of every pulse and a rising one halfway; a
//! half-wave recording (version 2, C16/Plus/4) toggles READ at the end of
//! every pulse. Recording (PLAY and RECORD down, motor on) measures the
//! time between rising edges of WRITE, where the KERNALs start each wave
//! (between any two edges for half
//! waves) and replaces the tape from the current position on; it is written
//! back to the media when the motor stops, on STOP, flush and eject.
//!
//! Keys ([`MediaSlot::control`]): "play", "record" (with PLAY), "stop",
//! "rewind" and "forward" (to the start or the end at once). Inserting a
//! tape presses PLAY.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};
use media::tap::{Tap, TapMachine, Video};

struct Cassette {
    media: Media,
    tap: Tap,
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

pub struct Datasette {
    cassette: Option<Cassette>,
    /// The machine whose tapes a blank recording is for.
    machine: TapMachine,
    host_hz: u32,
    keys: Keys,
    motor: bool,
    /// Index of the pulse under the head.
    position: usize,
    /// Tape-clock cycles into that pulse (playing) or since the last edge
    /// (recording).
    elapsed: u32,
    /// Host-to-tape clock accumulator.
    acc: u64,
    /// READ for half waves.
    half_level: bool,
    write_level: bool,
    /// A recording has started (the first edge was seen).
    recording: bool,
}

impl Datasette {
    /// A datasette next to a computer clocked at `host_hz`, recording tapes
    /// for `machine` (half waves for the C16/Plus/4).
    pub fn new(machine: TapMachine, host_hz: u32) -> Self {
        Self {
            cassette: None,
            machine,
            host_hz: host_hz.max(1),
            keys: Keys::Stop,
            motor: false,
            position: 0,
            elapsed: 0,
            acc: 0,
            half_level: true,
            write_level: true,
            recording: false,
        }
    }

    pub fn has_tape(&self) -> bool {
        self.cassette.is_some()
    }

    /// SENSE: a key is down.
    pub fn sense(&self) -> bool {
        self.keys != Keys::Stop
    }

    pub fn set_motor(&mut self, on: bool) {
        if self.motor && !on {
            let _ = self.write_back();
        }
        self.motor = on;
    }

    /// The tape position in pulses and their total.
    pub fn counter(&self) -> (usize, usize) {
        (
            self.position,
            self.cassette.as_ref().map_or(0, |c| c.tap.pulses.len()),
        )
    }

    fn moving(&self) -> bool {
        self.motor && self.keys != Keys::Stop && self.cassette.is_some()
    }

    fn half_waves(&self) -> bool {
        self.cassette
            .as_ref()
            .map_or(self.machine == TapMachine::C16, |c| c.tap.half_waves)
    }

    /// READ (true = high).
    pub fn read(&self) -> bool {
        if self.half_waves() {
            return self.half_level;
        }
        let Some(c) = &self.cassette else {
            return true;
        };
        if self.keys != Keys::Play {
            return true;
        }
        // The falling edge at the end of the last pulse leaves READ low.
        match c.tap.pulses.get(self.position) {
            Some(&len) => self.elapsed >= len / 2,
            None => false,
        }
    }

    /// WRITE from the computer.
    pub fn set_write(&mut self, level: bool) {
        let edge = if self.half_waves() {
            level != self.write_level
        } else {
            !self.write_level && level
        };
        self.write_level = level;
        if !edge || self.keys != Keys::Record || !self.moving() {
            return;
        }
        let Some(c) = &mut self.cassette else {
            return;
        };
        if c.protected {
            return;
        }
        if !self.recording {
            // The recording replaces the tape from here on.
            self.recording = true;
            c.tap.pulses.truncate(self.position);
        } else {
            c.tap.pulses.push(self.elapsed.max(1));
            self.position = c.tap.pulses.len();
        }
        c.dirty = true;
        self.elapsed = 0;
    }

    /// One cycle of the computer's clock.
    pub fn tick(&mut self) {
        if !self.moving() {
            return;
        }
        let Some(c) = &self.cassette else {
            return;
        };
        self.acc += u64::from(c.tap.clock_hz());
        let n = (self.acc / u64::from(self.host_hz)) as u32;
        self.acc %= u64::from(self.host_hz);
        if n == 0 {
            return;
        }
        self.elapsed = self.elapsed.saturating_add(n);
        if self.keys == Keys::Record {
            return;
        }
        let pulses = &c.tap.pulses;
        while let Some(&len) = pulses.get(self.position) {
            if self.elapsed < len {
                break;
            }
            self.elapsed -= len;
            self.position += 1;
            self.half_level = !self.half_level;
        }
        if self.position >= pulses.len() {
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
        let bytes = c.tap.to_bytes()?;
        c.media.backend.write(0, &bytes)
    }

    /// A key: what was recorded goes to the media; a new recording starts
    /// with the next edge (a motor stop in between does not end it).
    fn press(&mut self, keys: Keys) {
        let _ = self.write_back();
        self.recording = false;
        self.keys = keys;
        self.elapsed = 0;
    }
}

impl MediaSlot for Datasette {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let tap = Tap::parse(&bytes)?;
        self.cassette = Some(Cassette {
            media,
            tap,
            dirty: false,
            protected: false,
        });
        self.position = 0;
        self.elapsed = 0;
        self.half_level = true;
        self.keys = Keys::Play;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.recording = false;
        self.keys = Keys::Stop;
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

    /// Active while the tape moves (motor on, PLAY or RECORD down).
    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.moving(),
            error: false,
            track: None,
        }
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

/// A blank tape for `machine` with room for about `minutes` of recording.
pub fn blank_tape(machine: TapMachine, video: Video, minutes: u32) -> Vec<u8> {
    // About 3000 pulses (bytes) per second of CBM recording.
    let capacity = 20 + minutes as usize * 60 * 3000;
    Tap::blank(machine, video, capacity)
        .to_bytes()
        .unwrap_or_default()
}

const TAG: [u8; 4] = *b"TAPE";
const VERSION: u16 = 1;

impl Savestate for Datasette {
    /// The tape is media, not state: a savestate keeps the keys, the motor
    /// and the position on whatever tape is inserted.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.keys as u8);
            w.bool(self.motor);
            w.u32(self.position as u32);
            w.u32(self.elapsed);
            w.u64(self.acc);
            w.bool(self.half_level);
            w.bool(self.write_level);
            w.bool(self.recording);
        });
    }

    /// Atomic: nothing changes unless the whole chunk is valid.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let keys = match c.u8()? {
            0 => Keys::Stop,
            1 => Keys::Play,
            2 => Keys::Record,
            _ => return Err(StateError::Invalid("datasette keys")),
        };
        let motor = c.bool()?;
        let position = c.u32()? as usize;
        let elapsed = c.u32()?;
        let acc = c.u64()?;
        if acc >= u64::from(self.host_hz) {
            return Err(StateError::Invalid("datasette clock"));
        }
        let (half_level, write_level, recording) = (c.bool()?, c.bool()?, c.bool()?);
        self.half_level = half_level;
        self.write_level = write_level;
        self.recording = recording;
        self.keys = keys;
        self.motor = motor;
        self.position = position.min(self.counter().1);
        self.elapsed = elapsed;
        self.acc = acc;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
