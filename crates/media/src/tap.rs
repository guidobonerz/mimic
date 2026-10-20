//! TAP: datasette recordings as pulse lengths.
//!
//! Header (20 bytes): "C64-TAPE-RAW" (or "C16-TAPE-RAW"), version, machine
//! (0 C64, 1 VIC-20, 2 C16/Plus/4, 3 PET, 4 C5x0, 5 C6x0/7x0), video
//! standard (0 PAL, 1 NTSC, 2 old NTSC, 3 PAL-N), a reserved byte and the
//! data size (u32 LE). Data: one byte per pulse, its length in machine
//! cycles / 8; a 0 byte is an overflow (version 0: "longer than 255 × 8") or
//! introduces the exact length in the next three bytes (versions 1 and 2,
//! LE). Versions 0 and 1 hold full waves (the time between two falling
//! edges), version 2 half waves (the time between any two edges; C16 and
//! Plus/4 tapes). Bytes after the data size are slack: a blank tape is a
//! file with room for its recording. Source: the TAP format as written by
//! the C64S emulator and documented with the VICE tools.
//!
//! [`encode_cbm`] records a program in the format of the Commodore
//! KERNALs of the PET, VIC-20, C64 and C128; [`encode_ted`] in that of the
//! C16 and Plus/4.

use emu_core::media::MediaError;

const HEADER: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapMachine {
    C64,
    Vic20,
    /// C16 and Plus/4.
    C16,
    Pet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Video {
    Pal,
    Ntsc,
}

impl TapMachine {
    fn code(self) -> u8 {
        match self {
            Self::C64 => 0,
            Self::Vic20 => 1,
            Self::C16 => 2,
            Self::Pet => 3,
        }
    }

    fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            0 => Self::C64,
            1 => Self::Vic20,
            2 => Self::C16,
            3 => Self::Pet,
            _ => return None,
        })
    }

    /// The clock TAP lengths count in: the CPU clock (the TED's single
    /// clock on the C16/Plus/4).
    pub fn clock_hz(self, video: Video) -> u32 {
        match (self, video) {
            (Self::C64, Video::Pal) => 985_248,
            (Self::C64, Video::Ntsc) => 1_022_727,
            (Self::Vic20, Video::Pal) => 1_108_405,
            (Self::Vic20, Video::Ntsc) => 1_022_727,
            (Self::C16, Video::Pal) => 886_724,
            (Self::C16, Video::Ntsc) => 894_886,
            (Self::Pet, _) => 1_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tap {
    pub machine: TapMachine,
    pub video: Video,
    /// Version 2 recordings hold half waves.
    pub half_waves: bool,
    /// Pulse lengths in cycles of [`Tap::clock_hz`].
    pub pulses: Vec<u32>,
    /// File size: what a write back must fit into (0 = no limit).
    pub capacity: usize,
}

impl Tap {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let err = |m: &str| MediaError::Format(format!("TAP: {m}"));
        if bytes.len() < HEADER || !matches!(&bytes[..12], b"C64-TAPE-RAW" | b"C16-TAPE-RAW") {
            return Err(err("no TAP header"));
        }
        let version = bytes[12];
        if version > 2 {
            return Err(err("unknown version"));
        }
        let machine = TapMachine::from_code(bytes[13]).ok_or_else(|| err("unknown machine"))?;
        let video = if bytes[14] == 0 || bytes[14] == 3 {
            Video::Pal
        } else {
            Video::Ntsc
        };
        let size = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
        let data = bytes
            .get(HEADER..HEADER + size)
            .ok_or_else(|| err("data size beyond the file"))?;
        let mut pulses = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            i += 1;
            if b != 0 {
                pulses.push(u32::from(b) * 8);
            } else if version == 0 {
                pulses.push(256 * 8);
            } else {
                let Some(l) = data.get(i..i + 3) else {
                    break;
                };
                pulses.push(u32::from_le_bytes([l[0], l[1], l[2], 0]));
                i += 3;
            }
        }
        Ok(Self {
            machine,
            video,
            half_waves: version == 2,
            pulses,
            capacity: bytes.len(),
        })
    }

    /// An empty tape with room for `capacity` file bytes (0: no limit).
    pub fn blank(machine: TapMachine, video: Video, capacity: usize) -> Self {
        Self {
            machine,
            video,
            half_waves: machine == TapMachine::C16,
            pulses: Vec::new(),
            capacity: if capacity == 0 {
                0
            } else {
                capacity.max(HEADER)
            },
        }
    }

    pub fn clock_hz(&self) -> u32 {
        self.machine.clock_hz(self.video)
    }

    /// The data bytes (version 1, or 2 for half waves).
    fn data(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pulses.len());
        for &p in &self.pulses {
            let v = (p + 4) / 8;
            if (1..256).contains(&v) {
                out.push(v as u8);
            } else {
                let l = p.min(0xff_ffff).to_le_bytes();
                out.extend_from_slice(&[0, l[0], l[1], l[2]]);
            }
        }
        out
    }

    /// The file: header, data and, up to the capacity, the slack.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MediaError> {
        let data = self.data();
        let mut out = Vec::with_capacity(HEADER + data.len());
        out.extend_from_slice(if self.machine == TapMachine::C16 {
            b"C16-TAPE-RAW"
        } else {
            b"C64-TAPE-RAW"
        });
        out.push(if self.half_waves { 2 } else { 1 });
        out.push(self.machine.code());
        out.push(match self.video {
            Video::Pal => 0,
            Video::Ntsc => 1,
        });
        out.push(0);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        if self.capacity > 0 {
            if out.len() > self.capacity {
                return Err(MediaError::Format("TAP: the tape is full".into()));
            }
            out.resize(self.capacity, 0);
        }
        Ok(out)
    }
}

/// Pulse lengths of the CBM format in C64 PAL cycles: short (2840 Hz),
/// medium (1953 Hz), long (1488 Hz).
const SHORT: u32 = 0x30 * 8;
const MEDIUM: u32 = 0x42 * 8;
const LONG: u32 = 0x56 * 8;
const C64_PAL_HZ: u64 = 985_248;

/// A recording in the making, in C64 PAL cycles.
struct Recorder {
    pulses: Vec<u32>,
}

impl Recorder {
    fn shorts(&mut self, n: usize) {
        self.pulses.extend(std::iter::repeat_n(SHORT, n));
    }

    /// A byte: marker (long, medium), 8 bits LSB first (0 = short, medium;
    /// 1 = medium, short), odd parity bit.
    fn byte(&mut self, b: u8) {
        self.pulses.extend([LONG, MEDIUM]);
        let mut parity = 1;
        for i in 0..8 {
            let bit = (b >> i) & 1;
            parity ^= bit;
            self.bit(bit);
        }
        self.bit(parity);
    }

    fn bit(&mut self, bit: u8) {
        if bit == 0 {
            self.pulses.extend([SHORT, MEDIUM]);
        } else {
            self.pulses.extend([MEDIUM, SHORT]);
        }
    }

    /// A block recorded twice: countdown sync, payload, XOR checksum, end
    /// marker; 79 short pulses between the copies, 78 after.
    fn block(&mut self, leader: usize, payload: &[u8]) {
        self.shorts(leader);
        for copy in 0..2u8 {
            let base = if copy == 0 { 0x89 } else { 0x09 };
            for k in 0..9u8 {
                self.byte(base - k);
            }
            let mut sum = 0;
            for &b in payload {
                self.byte(b);
                sum ^= b;
            }
            self.byte(sum);
            self.pulses.extend([LONG, SHORT]);
            self.shorts(if copy == 0 { 79 } else { 78 });
        }
    }
}

/// Convert C64 PAL cycles to another clock.
fn scale(pulses: Vec<u32>, hz: u32) -> Vec<u32> {
    pulses
        .into_iter()
        .map(|p| ((u64::from(p) * u64::from(hz) + C64_PAL_HZ / 2) / C64_PAL_HZ) as u32)
        .collect()
}

/// The header block: type (1 = relocatable, i.e. loads to BASIC; 3 = to
/// its address), start, end + 1, 16-character name padded with spaces, then
/// spaces up to 192 bytes.
fn header_block(file_type: u8, start: u16, end: u16, name: &[u8]) -> Vec<u8> {
    let mut h = vec![file_type];
    h.extend_from_slice(&start.to_le_bytes());
    h.extend_from_slice(&end.to_le_bytes());
    let n = name.len().min(16);
    h.extend_from_slice(&name[..n]);
    h.resize(192, b' ');
    h
}

/// Record a PRG (load address + data) as the PET/VIC-20/C64/C128 KERNAL
/// saves it: a header file (leader 27 136 short pulses) and the data file
/// (leader 6656), each block twice. `relocatable` makes a type 1 header.
pub fn encode_cbm(
    prg: &[u8],
    name: &[u8],
    relocatable: bool,
    machine: TapMachine,
    video: Video,
) -> Result<Tap, MediaError> {
    let (start, data) = split_prg(prg)?;
    let end = start.wrapping_add(data.len() as u16);
    let mut r = Recorder { pulses: Vec::new() };
    r.block(
        0x6a00,
        &header_block(if relocatable { 1 } else { 3 }, start, end, name),
    );
    r.block(0x1a00, data);
    let hz = machine.clock_hz(video);
    Ok(Tap {
        machine,
        video,
        half_waves: false,
        pulses: scale(r.pulses, hz),
        capacity: 0,
    })
}

/// Half-wave lengths of the C16/Plus/4 KERNAL in TED single-clock cycles:
/// the timer periods it loads (`$E452`: 208, `$E45D`: 420, `$E447`: 846)
/// plus the time its write loop takes to restart the timer, as measured on
/// the emulated Plus/4's own recordings.
const TED_SHORT: u32 = 216;
const TED_MEDIUM: u32 = 428;
const TED_LONG: u32 = 856;
/// The half wave before the first leader wave.
const TED_LEAD_IN: u32 = 496;

/// A C16/Plus/4 recording: every wave as two equal half waves.
struct TedRecorder {
    halves: Vec<u32>,
}

impl TedRecorder {
    fn wave(&mut self, half: u32) {
        self.halves.extend([half, half]);
    }

    fn waves(&mut self, half: u32, n: usize) {
        for _ in 0..n {
            self.wave(half);
        }
    }

    /// A byte (KERNAL `$E48C`): marker (long, medium), 8 bits LSB first
    /// (0 = short, medium; 1 = medium, short), then a parity bit that is
    /// 1 for an even number of zero bits.
    fn byte(&mut self, b: u8) {
        self.wave(TED_LONG);
        self.wave(TED_MEDIUM);
        let mut count = 1u8;
        for i in 0..8 {
            let one = (b >> i) & 1 != 0;
            if !one {
                count += 1;
            }
            self.bit(one);
        }
        self.bit(count & 1 != 0);
    }

    fn bit(&mut self, one: bool) {
        if one {
            self.wave(TED_MEDIUM);
            self.wave(TED_SHORT);
        } else {
            self.wave(TED_SHORT);
            self.wave(TED_MEDIUM);
        }
    }

    /// A block written twice (KERNAL `$E4BA`): leader of short waves
    /// (16 382, then 256 before the repeat), sync `$89–$81` / `$09–$01`,
    /// the block type if any, the payload, the XOR checksum (over type and
    /// payload), a medium wave and 194 short waves.
    fn block(&mut self, block_type: Option<u8>, payload: &[u8]) {
        for copy in 0..2u8 {
            self.waves(TED_SHORT, if copy == 0 { 0x3ffe } else { 0x100 });
            let base = if copy == 0 { 0x80 } else { 0 };
            for k in (1..=9u8).rev() {
                self.byte(base | k);
            }
            let mut sum = 0;
            if let Some(t) = block_type {
                self.byte(t);
                sum = t;
            }
            for &b in payload {
                self.byte(b);
                sum ^= b;
            }
            self.byte(sum);
            self.wave(TED_MEDIUM);
            self.waves(TED_SHORT, 194);
        }
    }
}

/// Record a PRG (load address + data) as the C16/Plus/4 KERNAL saves it:
/// a header block (type 1 relocatable / 3 absolute, start, end + 1, name)
/// and the data block, each twice, as half waves.
pub fn encode_ted(
    prg: &[u8],
    name: &[u8],
    relocatable: bool,
    video: Video,
) -> Result<Tap, MediaError> {
    let (start, data) = split_prg(prg)?;
    let end = start.wrapping_add(data.len() as u16);
    let header = header_block(if relocatable { 1 } else { 3 }, start, end, name);
    // The write line rests low before the KERNAL starts: the recording
    // begins with that half (a moment of silence), so every wave after it
    // starts with a rising edge, as the reader expects.
    let mut r = TedRecorder {
        halves: vec![TED_LEAD_IN],
    };
    r.block(Some(header[0]), &header[1..]);
    r.block(None, data);
    Ok(Tap {
        machine: TapMachine::C16,
        video,
        half_waves: true,
        pulses: r.halves,
        capacity: 0,
    })
}

fn split_prg(prg: &[u8]) -> Result<(u16, &[u8]), MediaError> {
    if prg.len() < 3 {
        return Err(MediaError::Format("PRG: too short".into()));
    }
    Ok((u16::from_le_bytes([prg[0], prg[1]]), &prg[2..]))
}

#[cfg(test)]
mod tests;
