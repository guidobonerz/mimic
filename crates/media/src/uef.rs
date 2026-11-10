//! BBC Micro / Acorn Electron cassettes: UEF files (gzip-compressed or
//! not) and WAV recordings, both as a timeline of [`Event`]s at the tape's
//! baud rate (1200 or 300).
//!
//! **UEF** (Thomas Harte's Unified Emulator Format, as documented by its
//! author): the magic `UEF File!` and a zero byte, a version (two bytes),
//! then chunks: a 16-bit id, a 32-bit length (both little endian), the
//! body. Read: `&0100` bytes with a start and a stop bit, `&0110` carrier
//! tone (its length in cycles of 2400 Hz), `&0111` carrier with a dummy
//! byte in it, `&0112` a gap (in 1/(2 × baud) s), `&0116` a gap (seconds,
//! a 32-bit float), `&0117` the baud rate. Other chunks are skipped.
//!
//! **WAV**: the tones of the cassette interface, a 0 bit one cycle of
//! 1200 Hz, a 1 bit two cycles of 2400 Hz (at 1200 baud; 300 baud: four
//! and eight cycles); a byte is a 0 start bit, eight data bits (lowest
//! first) and a 1 stop bit; 1 bits outside a byte are carrier. Half cycles
//! are measured between zero crossings.
//!
//! Writing: an uncompressed UEF with `&0110` carrier and `&0100` data
//! chunks.

use emu_core::media::MediaError;
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Carrier tone: this many bit times of 1s.
    Carrier(u32),
    /// A byte (with its start and stop bit: 10 bit times).
    Byte(u8),
    /// Silence: this many bit times.
    Gap(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tape {
    pub baud: u32,
    pub events: Vec<Event>,
}

fn err(m: &str) -> MediaError {
    MediaError::Format(m.into())
}

const MAGIC: &[u8] = b"UEF File!\0";

impl Tape {
    /// A UEF (gzip or not) or a WAV recording.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.starts_with(b"RIFF") {
            let wav = crate::wav::Wav::parse(bytes).map_err(MediaError::Format)?;
            return Ok(Self::from_wav(&wav));
        }
        let raw;
        let data = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes)
                .read_to_end(&mut out)
                .map_err(|e| MediaError::Format(format!("UEF gzip: {e}")))?;
            raw = out;
            &raw[..]
        } else {
            bytes
        };
        let rest = data
            .strip_prefix(MAGIC)
            .ok_or_else(|| err("not a UEF file"))?;
        let mut rest = rest.get(2..).ok_or_else(|| err("UEF header cut short"))?;
        let mut tape = Self {
            baud: 1200,
            events: Vec::new(),
        };
        while rest.len() >= 6 {
            let id = u16::from_le_bytes([rest[0], rest[1]]);
            let len = u32::from_le_bytes([rest[2], rest[3], rest[4], rest[5]]) as usize;
            let body = rest
                .get(6..6 + len)
                .ok_or_else(|| err("UEF chunk cut short"))?;
            rest = &rest[6 + len..];
            let u16_at = |o: usize| {
                body.get(o..o + 2)
                    .map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])))
            };
            match id {
                0x0100 => tape.events.extend(body.iter().map(|&b| Event::Byte(b))),
                0x0110 => {
                    let cycles = u16_at(0).unwrap_or(0);
                    tape.events
                        .push(Event::Carrier(tape.bits_of_cycles(cycles)));
                }
                0x0111 => {
                    let before = u16_at(0).unwrap_or(0);
                    let after = u16_at(2).unwrap_or(0);
                    tape.events
                        .push(Event::Carrier(tape.bits_of_cycles(before)));
                    tape.events.push(Event::Byte(0xaa));
                    tape.events.push(Event::Carrier(tape.bits_of_cycles(after)));
                }
                0x0112 => {
                    let n = u16_at(0).unwrap_or(0);
                    tape.events.push(Event::Gap(n.div_ceil(2).max(1)));
                }
                0x0116 => {
                    let s = body
                        .get(0..4)
                        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .unwrap_or(0.0);
                    let bits = (s.max(0.0) * tape.baud as f32) as u32;
                    tape.events.push(Event::Gap(bits.max(1)));
                }
                0x0117 => {
                    if let Some(b) = u16_at(0).filter(|&b| b == 300 || b == 1200) {
                        tape.baud = b;
                    }
                }
                _ => {}
            }
        }
        Ok(tape)
    }

    /// Bit times of a carrier of `cycles` cycles of 2400 Hz.
    fn bits_of_cycles(&self, cycles: u32) -> u32 {
        (cycles * self.baud / 2400).max(1)
    }

    /// Decode the tones of a recording.
    pub fn from_wav(wav: &crate::wav::Wav) -> Self {
        let rate = wav.rate.max(1) as f32;
        // Half cycles in seconds between zero crossings.
        let mut halves = Vec::new();
        let mut last = 0usize;
        let mut positive = wav.samples.first().is_some_and(|&s| s >= 0);
        for (i, &s) in wav.samples.iter().enumerate() {
            let p = s >= 0;
            if p != positive {
                halves.push((i - last) as f32 / rate);
                last = i;
                positive = p;
            }
        }
        // 1200 Hz half cycle 417 µs, 2400 Hz 208 µs: the border at 312 µs.
        let mut bits = Vec::new();
        let mut i = 0;
        while i < halves.len() {
            let h = halves[i];
            if h > 0.0015 {
                bits.push(None);
                i += 1;
            } else if h > 0.000_312 {
                // Two long halves: a 0.
                bits.push(Some(false));
                i += 2;
            } else {
                // Four short halves: a 1.
                bits.push(Some(true));
                i += 4;
            }
        }
        let mut tape = Self {
            baud: 1200,
            events: Vec::new(),
        };
        let mut carrier = 0u32;
        let mut k = 0;
        let flush = |tape: &mut Self, carrier: &mut u32| {
            if *carrier > 0 {
                tape.events.push(Event::Carrier(*carrier));
                *carrier = 0;
            }
        };
        while k < bits.len() {
            match bits[k] {
                None => {
                    flush(&mut tape, &mut carrier);
                    tape.events.push(Event::Gap(1));
                    k += 1;
                }
                Some(true) => {
                    carrier += 1;
                    k += 1;
                }
                Some(false) => {
                    // A start bit: eight data bits and a stop bit follow.
                    if k + 9 < bits.len() && bits[k + 9] == Some(true) {
                        flush(&mut tape, &mut carrier);
                        let mut v = 0u8;
                        for n in 0..8 {
                            if bits[k + 1 + n] == Some(true) {
                                v |= 1 << n;
                            }
                        }
                        tape.events.push(Event::Byte(v));
                        k += 10;
                    } else {
                        k += 1;
                    }
                }
            }
        }
        flush(&mut tape, &mut carrier);
        tape
    }

    /// An uncompressed UEF.
    pub fn to_uef(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&[0x0a, 0x00]);
        let chunk = |id: u16, body: &[u8], out: &mut Vec<u8>| {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(body);
        };
        if self.baud != 1200 {
            chunk(0x0117, &(self.baud as u16).to_le_bytes(), &mut out);
        }
        let mut data = Vec::new();
        for e in &self.events {
            if !matches!(e, Event::Byte(_)) && !data.is_empty() {
                chunk(0x0100, &data, &mut out);
                data.clear();
            }
            match *e {
                Event::Byte(b) => data.push(b),
                Event::Carrier(bits) => {
                    let cycles = (bits * 2400 / self.baud).clamp(1, 0xffff) as u16;
                    chunk(0x0110, &cycles.to_le_bytes(), &mut out);
                }
                Event::Gap(bits) => {
                    let halves = (bits * 2).clamp(1, 0xffff) as u16;
                    chunk(0x0112, &halves.to_le_bytes(), &mut out);
                }
            }
        }
        if !data.is_empty() {
            chunk(0x0100, &data, &mut out);
        }
        out
    }

    /// Bit times of an event.
    pub fn bits(e: &Event) -> u32 {
        match *e {
            Event::Carrier(n) | Event::Gap(n) => n,
            Event::Byte(_) => 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uef_round_trip_and_gzip() {
        let tape = Tape {
            baud: 1200,
            events: vec![
                Event::Carrier(600),
                Event::Byte(0x2a),
                Event::Byte(0x41),
                Event::Gap(4),
            ],
        };
        let bytes = tape.to_uef();
        assert_eq!(Tape::parse(&bytes).unwrap(), tape);
        let mut gz = Vec::new();
        {
            use std::io::Write;
            let mut e = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            e.write_all(&bytes).unwrap();
        }
        assert_eq!(Tape::parse(&gz).unwrap(), tape);
        assert!(Tape::parse(b"nonsense").is_err());
    }

    #[test]
    fn wav_tones_decode_to_bytes() {
        // Carrier, then the byte &A5 at 1200 baud, at 44.1 kHz.
        let rate = 44_100.0f32;
        let mut samples = Vec::new();
        let mut t = 0.0f32;
        let mut tone = |cycles: u32, hz: f32, samples: &mut Vec<i16>| {
            let n = (cycles as f32 * rate / hz) as usize;
            for _ in 0..n {
                samples.push(((t * hz * std::f32::consts::TAU).sin() * 20_000.0) as i16);
                t += 1.0 / rate;
            }
        };
        let mut bit = |b: bool, s: &mut Vec<i16>| {
            if b {
                tone(2, 2400.0, s)
            } else {
                tone(1, 1200.0, s)
            }
        };
        for _ in 0..50 {
            bit(true, &mut samples);
        }
        bit(false, &mut samples);
        for n in 0..8 {
            bit(0xa5 >> n & 1 != 0, &mut samples);
        }
        bit(true, &mut samples);
        for _ in 0..20 {
            bit(true, &mut samples);
        }
        let tape = Tape::from_wav(&crate::wav::Wav {
            rate: 44_100,
            samples,
        });
        assert!(
            tape.events.contains(&Event::Byte(0xa5)),
            "{:?}",
            tape.events
        );
        assert!(matches!(tape.events.first(), Some(Event::Carrier(n)) if *n > 40));
    }
}
