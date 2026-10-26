//! CDT: Amstrad CPC tape images, which are TZX files (TZX format 1.20,
//! World of Spectrum). Timings are in T-states of the ZX Spectrum's
//! 3.5 MHz clock, whatever the machine.
//!
//! A file is the header `ZXTape!` 0x1A, major/minor version, then blocks
//! (an ID byte and its data). The tape is expanded into [`Segment`]s: the
//! signal level and how long it stays. Each pulse of a block toggles the
//! level at its start; the tape starts low.
//!
//! | ID   | Block                | Played as                                        |
//! | ---- | -------------------- | ------------------------------------------------ |
//! | 0x10 | standard speed data  | ROM timings (pilot 2168, sync 667/735, bits 855/1710) |
//! | 0x11 | turbo speed data     | its own pilot, sync and bit timings              |
//! | 0x12 | pure tone            | N pulses of one length                           |
//! | 0x13 | pulse sequence       | up to 255 pulses of their own lengths            |
//! | 0x14 | pure data            | bits only                                        |
//! | 0x15 | direct recording     | one level per sample                             |
//! | 0x20 | pause (0 = stop)     | low level                                        |
//! | 0x24/0x25 | loop start / end | repeated                                        |
//! | 0x2B | set signal level     | level                                            |
//! | 0x21, 0x22, 0x23, 0x26–0x28, 0x2A, 0x30–0x35, 0x5A | info, groups, jumps | skipped |
//!
//! A data bit is two pulses of the zero or one length, bytes MSB first;
//! the last byte has only its "used bits". The pause after a data block
//! (and block 0x20) begins with an edge — the opposite of the last
//! pulse's level, held for 1 ms if high — then stays low. Blocks 0x18 (CSW) and 0x19 (generalized data) are not supported.
//!
//! Writing: the whole tape is stored as pulse sequences — exact to the
//! T-state — with 0x2B before each run, padded to the file's size with a
//! 0x35 custom-info block (so blank tapes have room to record).
//!
//! For the ZX Spectrum the tape also lists its standard-speed data blocks
//! ([`Tape::blocks`], which a ROM loading trap can copy directly) and the
//! places where it stops ([`Tape::stops`]: block 0x20 with pause 0, and
//! 0x2A "stop the tape if in 48K mode" in [`Tape::stops_48k`]).
//! [`Tape::from_tap`] reads a Spectrum `.tap` file (blocks of a 16-bit
//! length and the bytes from the flag to the checksum) as standard-speed
//! blocks with a second's pause each.

use emu_core::media::MediaError;

/// TZX T-states per second.
pub const TZX_HZ: u32 = 3_500_000;
const HEADER: &[u8; 8] = b"ZXTape!\x1a";
const PAD_ID: &[u8; 16] = b"mimic blank     ";
/// T-states in a millisecond.
const MS: u32 = TZX_HZ / 1000;

/// The level for `len` T-states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub level: bool,
    pub len: u32,
}

/// A tape as level segments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tape {
    pub segments: Vec<Segment>,
    /// Standard-speed data blocks (0x10) in tape order.
    pub blocks: Vec<DataBlock>,
    /// Segment indices where the tape stops (0x20 with pause 0).
    pub stops: Vec<usize>,
    /// Segment indices where a 48K machine stops the tape (0x2A).
    pub stops_48k: Vec<usize>,
}

/// A standard-speed data block: its bytes (flag, data, checksum) and the
/// segments it covers, pilot to pause.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataBlock {
    pub start: usize,
    pub end: usize,
    pub data: Vec<u8>,
}

fn fmt(msg: &str) -> MediaError {
    MediaError::Format(format!("CDT: {msg}"))
}

fn tap(msg: &str) -> MediaError {
    MediaError::Format(format!("TAP: {msg}"))
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], MediaError> {
        let s = self
            .b
            .get(self.i..self.i + n)
            .ok_or_else(|| fmt("block truncated"))?;
        self.i += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, MediaError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, MediaError> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn u24(&mut self) -> Result<usize, MediaError> {
        let s = self.take(3)?;
        Ok(usize::from(s[0]) | usize::from(s[1]) << 8 | usize::from(s[2]) << 16)
    }

    fn u32(&mut self) -> Result<usize, MediaError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
    }
}

/// Builds the segments while blocks are played.
struct Player {
    segments: Vec<Segment>,
    level: bool,
}

impl Player {
    fn hold(&mut self, level: bool, len: u32) {
        if len == 0 {
            return;
        }
        match self.segments.last_mut() {
            Some(s) if s.level == level => s.len = s.len.saturating_add(len),
            _ => self.segments.push(Segment { level, len }),
        }
    }

    fn pulse(&mut self, len: u32) {
        self.level = !self.level;
        self.hold(self.level, len);
    }

    fn data(&mut self, zero: u32, one: u32, used_last: u8, data: &[u8]) {
        for (k, &byte) in data.iter().enumerate() {
            let bits = if k + 1 == data.len() {
                used_last.clamp(1, 8)
            } else {
                8
            };
            for b in 0..bits {
                let len = if byte & (0x80 >> b) != 0 { one } else { zero };
                self.pulse(len);
                self.pulse(len);
            }
        }
    }

    /// A block at the Spectrum ROM's timings: pilot (8063 pulses before a
    /// header, flag < $80, 3223 before data) of 2168, syncs 667/735, bits
    /// 855/1710.
    fn standard_block(&mut self, data: &[u8], pause: u16) {
        let pilot = if data.first().is_some_and(|&f| f < 0x80) {
            8063
        } else {
            3223
        };
        for _ in 0..pilot {
            self.pulse(2168);
        }
        self.pulse(667);
        self.pulse(735);
        self.data(855, 1710, 8, data);
        self.pause(pause);
    }

    /// The end of the last pulse is an edge: the pause starts with the
    /// opposite level, held for 1 ms if that is high, then low (TZX 1.20,
    /// "pause"). Without that edge the last half-pulse would run into the
    /// pause and its bit read as a one.
    fn pause(&mut self, ms: u16) {
        if ms == 0 {
            return;
        }
        let total = u32::from(ms) * MS;
        let after = !self.level && !self.segments.is_empty();
        if after {
            self.hold(true, MS.min(total));
            self.hold(false, total.saturating_sub(MS));
        } else {
            self.hold(false, total);
        }
        self.level = false;
    }
}

impl Tape {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.len() < 10 || &bytes[..8] != HEADER {
            return Err(fmt("not a TZX/CDT image"));
        }
        let mut r = Reader { b: bytes, i: 10 };
        let mut p = Player {
            segments: Vec::new(),
            level: false,
        };
        let mut blocks = Vec::new();
        let mut stops = Vec::new();
        let mut stops_48k = Vec::new();
        // Loop start: segment index and repetitions left.
        let mut repeat: Option<(usize, u16)> = None;
        while r.i < bytes.len() {
            let id = r.u8()?;
            match id {
                0x10 => {
                    let pause = r.u16()?;
                    let len = usize::from(r.u16()?);
                    let data = r.take(len)?.to_vec();
                    let start = p.segments.len();
                    p.standard_block(&data, pause);
                    blocks.push(DataBlock {
                        start,
                        end: p.segments.len(),
                        data,
                    });
                }
                0x11 => {
                    let pilot = u32::from(r.u16()?);
                    let sync1 = u32::from(r.u16()?);
                    let sync2 = u32::from(r.u16()?);
                    let zero = u32::from(r.u16()?);
                    let one = u32::from(r.u16()?);
                    let pilots = r.u16()?;
                    let used = r.u8()?;
                    let pause = r.u16()?;
                    let len = r.u24()?;
                    let data = r.take(len)?.to_vec();
                    for _ in 0..pilots {
                        p.pulse(pilot);
                    }
                    p.pulse(sync1);
                    p.pulse(sync2);
                    p.data(zero, one, used, &data);
                    p.pause(pause);
                }
                0x12 => {
                    let len = u32::from(r.u16()?);
                    for _ in 0..r.u16()? {
                        p.pulse(len);
                    }
                }
                0x13 => {
                    for _ in 0..r.u8()? {
                        let len = u32::from(r.u16()?);
                        p.pulse(len);
                    }
                }
                0x14 => {
                    let zero = u32::from(r.u16()?);
                    let one = u32::from(r.u16()?);
                    let used = r.u8()?;
                    let pause = r.u16()?;
                    let len = r.u24()?;
                    let data = r.take(len)?.to_vec();
                    p.data(zero, one, used, &data);
                    p.pause(pause);
                }
                0x15 => {
                    let per_sample = u32::from(r.u16()?);
                    let pause = r.u16()?;
                    let used = r.u8()?;
                    let len = r.u24()?;
                    let data = r.take(len)?.to_vec();
                    for (k, &byte) in data.iter().enumerate() {
                        let bits = if k + 1 == data.len() {
                            used.clamp(1, 8)
                        } else {
                            8
                        };
                        for b in 0..bits {
                            let level = byte & (0x80 >> b) != 0;
                            p.level = level;
                            p.hold(level, per_sample);
                        }
                    }
                    p.pause(pause);
                }
                0x18 | 0x19 => return Err(fmt(&format!("block {id:#04x} not supported"))),
                0x20 => {
                    let ms = r.u16()?;
                    if ms == 0 {
                        stops.push(p.segments.len());
                    }
                    p.pause(ms);
                }
                0x21 | 0x30 => {
                    let n = usize::from(r.u8()?);
                    r.take(n)?;
                }
                0x22 | 0x27 => {}
                0x23 => {
                    r.u16()?;
                }
                0x24 => {
                    let n = r.u16()?;
                    repeat = Some((p.segments.len(), n.saturating_sub(1)));
                }
                0x25 => {
                    if let Some((from, n)) = repeat.take() {
                        let body = p.segments[from..].to_vec();
                        for _ in 0..n {
                            for s in &body {
                                p.hold(s.level, s.len);
                            }
                        }
                        p.level = p.segments.last().is_some_and(|s| s.level);
                    }
                }
                0x26 => {
                    let n = usize::from(r.u16()?);
                    r.take(2 * n)?;
                }
                0x28 | 0x32 => {
                    let n = usize::from(r.u16()?);
                    r.take(n)?;
                }
                0x2a => {
                    r.u32()?;
                    stops_48k.push(p.segments.len());
                }
                0x2b => {
                    let n = r.u32()?;
                    let body = r.take(n)?;
                    p.level = body.first().is_some_and(|&v| v != 0);
                }
                0x31 => {
                    r.u8()?;
                    let n = usize::from(r.u8()?);
                    r.take(n)?;
                }
                0x33 => {
                    let n = usize::from(r.u8()?);
                    r.take(3 * n)?;
                }
                0x35 => {
                    r.take(16)?;
                    let n = r.u32()?;
                    r.take(n)?;
                }
                0x5a => {
                    r.take(9)?;
                }
                other => return Err(fmt(&format!("unknown block {other:#04x}"))),
            }
        }
        Ok(Self {
            segments: p.segments,
            blocks,
            stops,
            stops_48k,
        })
    }

    /// A Spectrum `.tap` file: each block a 16-bit length and that many
    /// bytes, played at standard speed with a 1000 ms pause.
    pub fn from_tap(bytes: &[u8]) -> Result<Self, MediaError> {
        let mut p = Player {
            segments: Vec::new(),
            level: false,
        };
        let mut blocks = Vec::new();
        let mut r = Reader { b: bytes, i: 0 };
        while r.i < bytes.len() {
            let len = usize::from(r.u16().map_err(|_| tap("block length truncated"))?);
            let data = r.take(len).map_err(|_| tap("block truncated"))?.to_vec();
            if data.is_empty() {
                continue;
            }
            let start = p.segments.len();
            p.standard_block(&data, 1000);
            blocks.push(DataBlock {
                start,
                end: p.segments.len(),
                data,
            });
        }
        Ok(Self {
            segments: p.segments,
            blocks,
            stops: Vec::new(),
            stops_48k: Vec::new(),
        })
    }

    /// A TZX image (by its header) or else a Spectrum `.tap` file.
    pub fn parse_spectrum(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.starts_with(HEADER) {
            Self::parse(bytes)
        } else {
            Self::from_tap(bytes)
        }
    }

    /// The tape as a TZX file of exactly `capacity` bytes (padded), or an
    /// error if it does not fit.
    pub fn to_bytes(&self, capacity: usize) -> Result<Vec<u8>, MediaError> {
        let mut out = Vec::new();
        out.extend_from_slice(HEADER);
        out.extend_from_slice(&[1, 20]);
        let mut pulses: Vec<u16> = Vec::new();
        let flush = |out: &mut Vec<u8>, pulses: &mut Vec<u16>| {
            for chunk in pulses.chunks(255) {
                out.push(0x13);
                out.push(chunk.len() as u8);
                for &p in chunk {
                    out.extend_from_slice(&p.to_le_bytes());
                }
            }
            pulses.clear();
        };
        for s in &self.segments {
            // Each piece starts with a pulse; the level before it is the
            // opposite, set explicitly.
            let mut left = s.len;
            while left > 0 {
                let piece = left.min(u32::from(u16::MAX));
                flush(&mut out, &mut pulses);
                out.push(0x2b);
                out.extend_from_slice(&1u32.to_le_bytes());
                out.push(u8::from(!s.level));
                pulses.push(piece as u16);
                left -= piece;
            }
        }
        flush(&mut out, &mut pulses);
        pad(out, capacity)
    }
}

/// Pad a TZX file to `capacity` bytes with a custom-info block.
fn pad(mut out: Vec<u8>, capacity: usize) -> Result<Vec<u8>, MediaError> {
    let overhead = 1 + 16 + 4;
    if out.len() == capacity {
        return Ok(out);
    }
    if out.len() + overhead > capacity {
        return Err(MediaError::OutOfRange);
    }
    let n = capacity - out.len() - overhead;
    out.push(0x35);
    out.extend_from_slice(PAD_ID);
    out.extend_from_slice(&(n as u32).to_le_bytes());
    out.resize(capacity, 0);
    Ok(out)
}

/// A blank tape of `capacity` bytes.
pub fn blank(capacity: usize) -> Vec<u8> {
    Tape::default().to_bytes(capacity).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(blocks: &[u8]) -> Vec<u8> {
        let mut f = HEADER.to_vec();
        f.extend_from_slice(&[1, 20]);
        f.extend_from_slice(blocks);
        f
    }

    #[test]
    fn turbo_block_pulses() {
        // Pilot 2 × 1000, syncs 300/400, zero 500, one 900, one byte $81
        // (all 8 bits), no pause.
        let mut b = vec![0x11];
        for v in [1000u16, 300, 400, 500, 900, 2] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(8);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&[1, 0, 0, 0x81]);
        let t = Tape::parse(&file(&b)).unwrap();
        let lens: Vec<u32> = t.segments.iter().map(|s| s.len).collect();
        let mut want = vec![1000, 1000, 300, 400, 900, 900];
        want.extend([500; 12]);
        want.extend([900, 900]);
        assert_eq!(lens, want);
        // Levels alternate, the first pulse goes high.
        assert!(t.segments[0].level);
        assert!(t.segments.windows(2).all(|w| w[0].level != w[1].level));
    }

    #[test]
    fn tap_blocks_and_stops() {
        let tap = [2u8, 0, 0x00, 0x00, 3, 0, 0xff, 0x12, 0xed];
        let t = Tape::parse_spectrum(&tap).unwrap();
        assert_eq!(t.blocks.len(), 2);
        assert_eq!(t.blocks[1].data, [0xff, 0x12, 0xed]);
        assert_eq!(t.blocks[0].start, 0);
        assert_eq!(t.blocks[0].end, t.blocks[1].start);
        assert_eq!(t.blocks[1].end, t.segments.len());
        // Header pilot: 8063 pulses of 2168.
        assert!(t.segments[..8063].iter().all(|s| s.len == 2168));
        assert!(Tape::parse_spectrum(&[5, 0, 1]).is_err());
        // TZX: a standard block, stop, stop-if-48K.
        let mut b = vec![0x10, 0, 0, 2, 0, 0xff, 0xff, 0x20, 0, 0, 0x2a, 0, 0, 0, 0];
        b.extend_from_slice(&[0x10, 0xe8, 0x03, 1, 0, 0x00]);
        let t = Tape::parse(&file(&b)).unwrap();
        assert_eq!(t.blocks.len(), 2);
        assert_eq!(t.stops, [t.blocks[0].end]);
        assert_eq!(t.stops_48k, [t.blocks[0].end]);
    }

    #[test]
    fn pause_starts_with_an_edge() {
        let seg = |level, len| Segment { level, len };
        // One pulse (high): the pause's edge goes low at once.
        let mut b = vec![0x12];
        b.extend_from_slice(&700u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.push(0x20);
        b.extend_from_slice(&5u16.to_le_bytes());
        let t = Tape::parse(&file(&b)).unwrap();
        assert_eq!(t.segments, vec![seg(true, 700), seg(false, 5 * MS)]);
        // Two pulses (high, low): the edge goes high for 1 ms, then low.
        b[3] = 2;
        let t = Tape::parse(&file(&b)).unwrap();
        assert_eq!(
            t.segments,
            vec![
                seg(true, 700),
                seg(false, 700),
                seg(true, MS),
                seg(false, 4 * MS)
            ]
        );
    }

    #[test]
    fn loops_and_info_blocks() {
        let mut b = vec![0x30, 3, b'a', b'b', b'c', 0x24];
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&[0x12, 0x64, 0, 2, 0]); // 2 pulses of 100
        b.push(0x25);
        let t = Tape::parse(&file(&b)).unwrap();
        assert_eq!(t.segments.len(), 6);
    }

    #[test]
    fn written_tape_reads_back_exactly() {
        let t = Tape {
            segments: vec![
                Segment {
                    level: true,
                    len: 1234,
                },
                Segment {
                    level: false,
                    len: 200_000,
                },
                Segment {
                    level: true,
                    len: 7,
                },
                Segment {
                    level: true,
                    len: 70_000,
                },
            ],
            ..Tape::default()
        };
        let bytes = t.to_bytes(4096).unwrap();
        assert_eq!(bytes.len(), 4096);
        let back = Tape::parse(&bytes).unwrap();
        assert_eq!(
            back.segments,
            vec![
                Segment {
                    level: true,
                    len: 1234
                },
                Segment {
                    level: false,
                    len: 200_000
                },
                Segment {
                    level: true,
                    len: 70_007
                },
            ]
        );
        assert!(t.to_bytes(20).is_err(), "does not fit");
        assert_eq!(Tape::parse(&blank(1000)).unwrap().segments, vec![]);
    }

    #[test]
    fn rejects_other_files() {
        assert!(Tape::parse(b"Hello, world").is_err());
        assert!(Tape::parse(&file(&[0x19, 0, 0, 0, 0])).is_err());
    }
}
