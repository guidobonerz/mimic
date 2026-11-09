//! The Model I cassette at 500 baud (Level II). Each bit is a cell of
//! [`CELL`] CPU clocks with a clock pulse at its start and, for a 1, a data
//! pulse in its middle; bytes go most significant bit first. A pulse sets
//! the input latch (port `$FF` bit 7), any write to port `$FF` clears it.
//! The tape only moves while the motor relay (port `$FF` bit 2) is on.
//!
//! Images: ".cas" files hold the bytes as the ROM writes them (leader of
//! zeros, `$A5`, then the data). Playback turns them into pulses;
//! recording (CSAVE) times the ROM's output pulses and decodes them back
//! into bytes, appended to the tape (the leader's zero bits up to the first
//! 1 are written as whole zero bytes).
//!
//! Source: TRS-80 Model I Technical Reference Manual (cassette interface);
//! the cell length and pulse spacing are the Level II ROM's own output,
//! measured in this emulator (see the tests).

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};

/// Clocks per bit cell at 500 baud: the Level II ROM's clock pulses come
/// this far apart (measured: 3 560–3 580 within a byte, about 3 700 across
/// a byte boundary, where the ROM takes the next byte).
pub const CELL: u64 = 3_570;
/// The data pulse of a 1 comes this far after the clock pulse (measured
/// 1 760–1 790).
pub const DATA_AT: u64 = 1_775;
/// The last cell of a byte is this much longer (the ROM's byte handling).
pub const BYTE_GAP: u64 = 130;
/// Clocks per byte on the tape.
pub const BYTE_T: u64 = 8 * CELL + BYTE_GAP;
/// After the sync byte `$A5` that ends a leader the ROM pauses before the
/// first data byte (measured: 5 594 clocks from its last clock pulse to
/// the next instead of 3 700); CLOAD needs that time to show its stars.
pub const SYNC_GAP: u64 = 2_024;

#[derive(Default)]
pub struct Cassette {
    media: Option<Media>,
    /// The tape's bytes.
    pub bytes: Vec<u8>,
    /// Ones before each bit (prefix sums for counting pulses).
    ones: Vec<u32>,
    /// Tape time each byte starts at (one more: the end).
    starts: Vec<u64>,
    /// Tape time: clocks the motor has run since the tape was inserted.
    pub tape_t: u64,
    /// Tape time of the last latch clear.
    cleared_at: u64,
    pub motor: bool,
    /// Output level (port `$FF` bits 0–1 non-zero) and the recording.
    level: bool,
    pulses: Vec<u64>,
    dirty: bool,
}

impl Cassette {
    fn rebuild(&mut self) {
        self.ones.clear();
        self.starts.clear();
        let mut n = 0u32;
        let mut t = 0u64;
        for (i, b) in self.bytes.iter().enumerate() {
            for bit in (0..8).rev() {
                self.ones.push(n);
                n += u32::from(b >> bit & 1);
            }
            self.starts.push(t);
            let sync = *b == 0xa5 && i > 0 && self.bytes[i - 1] == 0;
            t += BYTE_T + if sync { SYNC_GAP } else { 0 };
        }
        self.ones.push(n);
        self.starts.push(t);
    }

    /// Pulses on the tape up to tape time `t` (inclusive).
    fn pulses_until(&self, t: u64) -> u64 {
        let bits = self.ones.len().saturating_sub(1) as u64;
        if bits == 0 {
            return 0;
        }
        let bytes = self.bytes.len();
        // The byte playing at `t`.
        let byte = self.starts.partition_point(|&s| s <= t).saturating_sub(1);
        if byte >= bytes {
            return bits + u64::from(self.ones[bits as usize]);
        }
        let into = t - self.starts[byte];
        let byte = byte as u64;
        let n = (into / CELL).min(7);
        let into = into - n * CELL;
        let cell = byte * 8 + n;
        let bit = self.bytes[byte as usize] >> (7 - n) & 1;
        cell + u64::from(self.ones[cell as usize]) + 1 + u64::from(bit == 1 && into >= DATA_AT)
    }

    /// Advance the tape by `clocks` if the motor runs.
    pub fn run(&mut self, clocks: u64) {
        if self.motor {
            self.tape_t += clocks;
        }
    }

    /// The input latch: a pulse since the last clear.
    pub fn latch(&self) -> bool {
        self.motor && self.pulses_until(self.tape_t) > self.pulses_until(self.cleared_at)
    }

    /// A write to port `$FF`: clears the latch, sets the output level and
    /// the motor. `now` is the machine clock (for recording).
    pub fn write_port(&mut self, v: u8, now: u64) {
        self.cleared_at = self.tape_t;
        let motor = v & 0x04 != 0;
        let level = v & 0x03 != 0;
        if motor && level && !self.level {
            self.pulses.push(now);
        }
        self.level = level;
        if self.motor && !motor {
            self.finish_recording();
        }
        self.motor = motor;
    }

    /// Decode the recorded pulses into bytes and append them.
    fn finish_recording(&mut self) {
        let pulses = std::mem::take(&mut self.pulses);
        let bits = decode(&pulses);
        let Some(first) = bits.iter().position(|&b| b) else {
            return;
        };
        let mut out = vec![0u8; first / 8];
        for chunk in bits[first..].chunks(8) {
            let mut b = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                b |= u8::from(bit) << (7 - i);
            }
            out.push(b);
        }
        self.bytes.extend_from_slice(&out);
        self.rebuild();
        self.dirty = true;
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        match &mut self.media {
            Some(m) => {
                m.backend.set_len(self.bytes.len() as u64)?;
                m.backend.write(0, &self.bytes)
            }
            None => Ok(()),
        }
    }

    /// Take the position and recording from a loaded copy (the tape's
    /// contents stay).
    pub fn restore(&mut self, from: Cassette) {
        self.tape_t = from.tape_t;
        self.cleared_at = from.cleared_at;
        self.motor = from.motor;
        self.level = from.level;
        self.pulses = from.pulses;
    }

    pub fn inserted(&self) -> bool {
        self.media.is_some()
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u64(self.tape_t);
        w.u64(self.cleared_at);
        w.bool(self.motor);
        w.bool(self.level);
        w.u32(self.pulses.len() as u32);
        for p in &self.pulses {
            w.u64(*p);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.tape_t = r.u64()?;
        self.cleared_at = r.u64()?;
        self.motor = r.bool()?;
        self.level = r.bool()?;
        let n = r.u32()? as usize;
        if n > 1 << 24 {
            return Err(StateError::Invalid("cassette pulses"));
        }
        self.pulses = (0..n).map(|_| r.u64()).collect::<Result<_, _>>()?;
        Ok(())
    }
}

/// Bits from pulse times: a pulse less than three quarters of a cell after
/// the clock pulse is a data pulse (a 1), otherwise it is the next clock.
pub fn decode(pulses: &[u64]) -> Vec<bool> {
    let mut bits = Vec::new();
    let mut i = 0;
    while i < pulses.len() {
        let clock = pulses[i];
        match pulses.get(i + 1) {
            Some(&next) if next - clock < CELL * 3 / 4 => {
                bits.push(true);
                i += 2;
            }
            _ => {
                bits.push(false);
                i += 1;
            }
        }
    }
    bits
}

impl MediaSlot for Cassette {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let _ = self.eject();
        self.media = Some(media);
        self.bytes = bytes;
        self.rebuild();
        self.tape_t = 0;
        self.cleared_at = 0;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.bytes.clear();
        self.rebuild();
        self.tape_t = 0;
        self.cleared_at = 0;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        self.write_back()
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.motor,
            error: false,
            track: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulses_follow_the_bits() {
        let mut c = Cassette {
            bytes: vec![0x80],
            ..Default::default()
        };
        c.rebuild();
        // Bit 0 is a 1: clock and data pulse; the other seven only clocks.
        assert_eq!(c.pulses_until(0), 1);
        assert_eq!(c.pulses_until(DATA_AT), 2);
        assert_eq!(c.pulses_until(CELL), 3);
        assert_eq!(c.pulses_until(BYTE_T * 2), 9);
    }

    #[test]
    fn sync_byte_after_a_leader_is_longer() {
        let mut c = Cassette {
            bytes: vec![0, 0xa5, 0x00],
            ..Default::default()
        };
        c.rebuild();
        assert_eq!(
            c.starts,
            vec![0, BYTE_T, 2 * BYTE_T + SYNC_GAP, 3 * BYTE_T + SYNC_GAP]
        );
        // The last byte's first clock pulse comes after the pause.
        let before = c.pulses_until(2 * BYTE_T + SYNC_GAP - 1);
        assert_eq!(c.pulses_until(2 * BYTE_T + SYNC_GAP), before + 1);
    }

    #[test]
    fn decode_round_trip() {
        let bits = [false, true, true, false, true];
        let mut pulses = Vec::new();
        for (i, &b) in bits.iter().enumerate() {
            let t = 1000 + i as u64 * CELL;
            pulses.push(t);
            if b {
                pulses.push(t + DATA_AT);
            }
        }
        assert_eq!(decode(&pulses), bits);
    }
}
