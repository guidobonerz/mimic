//! The ZX Spectrum's cassette recorder playing TAP and TZX images
//! ([`media::cdt`]).
//!
//! The Spectrum has no motor control: the tape runs while PLAY is down.
//! The machine moves it by CPU T-states ([`SpectrumTape::advance`]); TZX
//! timings are in 3.5 MHz T-states, so on the 128K (3.5469 MHz) the tape
//! is scaled to keep its real speed. The level goes to the EAR input
//! ([`SpectrumTape::level`]).
//!
//! The tape stops by itself at TZX "stop the tape" blocks (and "stop if in
//! 48K mode" on a 48K machine). For the ROM's loading routine the machine
//! may take the next standard-speed block whole ([`SpectrumTape::take_block`])
//! instead of playing it.
//!
//! Keys ([`MediaSlot::control`]): "play", "stop", "rewind", "forward".
//! Inserting a tape leaves it stopped at its start.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};
use media::cdt::{Tape, TZX_HZ};

pub struct SpectrumTape {
    tape: Option<(Media, Tape)>,
    playing: bool,
    /// Segment under the head and the time into it, in units of
    /// 1 / (TZX_HZ · cpu_hz) seconds.
    position: usize,
    elapsed: u64,
    cpu_hz: u64,
    /// A 48K machine (stops at 0x2A blocks).
    is_48k: bool,
}

impl SpectrumTape {
    pub fn new(cpu_hz: u32, is_48k: bool) -> Self {
        Self {
            tape: None,
            playing: false,
            position: 0,
            elapsed: 0,
            cpu_hz: u64::from(cpu_hz.max(1)),
            is_48k,
        }
    }

    pub fn has_tape(&self) -> bool {
        self.tape.is_some()
    }

    pub fn playing(&self) -> bool {
        self.playing && self.tape.is_some()
    }

    /// PLAY (the ROM's loader presses it when the tape is stopped).
    pub fn play(&mut self) {
        if self.tape.is_some() && !self.at_end() {
            self.playing = true;
        }
    }

    /// Segment position and the tape's segment count.
    pub fn counter(&self) -> (usize, usize) {
        (
            self.position,
            self.tape.as_ref().map_or(0, |(_, t)| t.segments.len()),
        )
    }

    fn at_end(&self) -> bool {
        let (p, n) = self.counter();
        p >= n
    }

    /// The EAR level (true = high).
    #[inline]
    pub fn level(&self) -> bool {
        match &self.tape {
            Some((_, t)) if self.playing => t.segments.get(self.position).is_some_and(|s| s.level),
            _ => false,
        }
    }

    /// Move the tape on by `t` CPU T-states.
    pub fn advance(&mut self, t: u32) {
        if !self.playing {
            return;
        }
        let Some((_, tape)) = &self.tape else {
            return;
        };
        self.elapsed += u64::from(t) * u64::from(TZX_HZ);
        while let Some(s) = tape.segments.get(self.position) {
            let len = u64::from(s.len) * self.cpu_hz;
            if self.elapsed < len {
                break;
            }
            self.elapsed -= len;
            self.position += 1;
            let stops = |v: &[usize]| v.binary_search(&self.position).is_ok();
            if stops(&tape.stops) || (self.is_48k && stops(&tape.stops_48k)) {
                self.playing = false;
                self.elapsed = 0;
                return;
            }
        }
        if self.position >= tape.segments.len() {
            self.playing = false;
            self.elapsed = 0;
        }
    }

    /// The next standard-speed block (flag, data, checksum) from the
    /// head's position on; the tape is wound to the block's end and keeps
    /// its PLAY state.
    pub fn take_block(&mut self) -> Option<Vec<u8>> {
        let (_, tape) = self.tape.as_ref()?;
        let b = tape.blocks.iter().find(|b| b.end > self.position)?;
        self.position = b.end;
        self.elapsed = 0;
        Some(b.data.clone())
    }

    fn wind(&mut self, to: usize) {
        self.playing = false;
        self.position = to;
        self.elapsed = 0;
    }
}

impl MediaSlot for SpectrumTape {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let tape = Tape::parse_spectrum(&bytes)?;
        self.tape = Some((media, tape));
        self.wind(0);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.wind(0);
        self.tape.take().map(|(m, _)| m)
    }

    /// Playing only: nothing is written.
    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }

    /// Active while the tape plays.
    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.playing(),
            error: false,
            track: None,
        }
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        if self.tape.is_none() && command != "stop" {
            return Err(MediaError::NoMedia);
        }
        match command {
            "play" => self.play(),
            "stop" => self.playing = false,
            "rewind" => self.wind(0),
            "forward" => self.wind(self.counter().1),
            other => {
                return Err(MediaError::Control(format!(
                    "'{other}' (play, stop, rewind, forward)"
                )))
            }
        }
        Ok(())
    }
}

const TAG: [u8; 4] = *b"STAP";

impl Savestate for SpectrumTape {
    /// The tape is media, not state: a savestate keeps PLAY and the
    /// position on whatever tape is inserted.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.bool(self.playing);
            w.u32(self.position as u32);
            w.u64(self.elapsed);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let playing = c.bool()?;
        let position = c.u32()? as usize;
        let elapsed = c.u64()?;
        self.position = position.min(self.counter().1);
        self.playing = playing && self.tape.is_some();
        self.elapsed = elapsed;
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

        fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
            Ok(())
        }

        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn deck(bytes: &[u8], hz: u32) -> SpectrumTape {
        let mut d = SpectrumTape::new(hz, true);
        d.insert(Media {
            name: "t".into(),
            kind: MediaKind::Tape,
            backend: Box::new(Mem(bytes.to_vec())),
        })
        .unwrap();
        d
    }

    const TAP: [u8; 9] = [2, 0, 0x00, 0x00, 3, 0, 0xff, 0x12, 0xed];

    #[test]
    fn plays_the_pilot_at_tzx_speed() {
        let mut d = deck(&TAP, 3_500_000);
        assert!(!d.level(), "stopped");
        d.play();
        // Pilot pulses of 2168 T-states: the first goes high.
        d.advance(10);
        assert!(d.level());
        d.advance(2168);
        assert!(!d.level());
        // On the 128K the same pulse is 2168 · 3.5469 / 3.5 CPU T-states.
        let mut d = deck(&TAP, 3_546_900);
        d.play();
        d.advance(2190);
        assert!(d.level());
        d.advance(10);
        assert!(!d.level());
    }

    #[test]
    fn take_block_winds_on() {
        let mut d = deck(&TAP, 3_500_000);
        assert_eq!(d.take_block().unwrap(), [0x00, 0x00]);
        assert_eq!(d.take_block().unwrap(), [0xff, 0x12, 0xed]);
        assert_eq!(d.take_block(), None);
        d.control("rewind").unwrap();
        assert_eq!(d.counter().0, 0);
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = deck(&TAP, 3_500_000);
        d.play();
        d.advance(100_000);
        let mut w = StateWriter::new();
        d.save(&mut w);
        let bytes = w.into_bytes();
        let mut e = deck(&TAP, 3_500_000);
        e.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(e.counter(), d.counter());
        assert!(e.playing());
    }
}
