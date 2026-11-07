//! The Phonograph, the Logophor 16's decoder for compressed audio
//! (SPEC.md 9.1, FFFF:3200): MP3 (MPEG-1/2/2.5 Layer III) and AAC-LC in
//! ADTS frames, taken from a ring buffer in chip RAM, resampled to 48 kHz
//! stereo and added to Calliope's mix.
//!
//! The decoders are Symphonia's (`symphonia-bundle-mp3`,
//! `symphonia-codec-aac`, MPL-2.0, version 0.5.5,
//! <https://github.com/pdeljanov/Symphonia>); the chip finds the frames
//! itself and hands the decoder one frame a packet. Frame headers after
//! ISO/IEC 11172-3 2.4.1.3 and ISO/IEC 13818-3 2.4.1.3 (MPEG audio:
//! sync, version, layer, bit rate, sample rate, padding, channel mode,
//! frame length 144 000 × kbit/s / sample rate + padding, half that for
//! MPEG-2/2.5) and ISO/IEC 13818-7 6.2 / ISO/IEC 14496-3 1.A.2 (ADTS:
//! sync word, layer, protection_absent with its 16-bit CRC, profile,
//! sampling frequency index, channel configuration, frame length,
//! number of raw data blocks; the AudioSpecificConfig built from them).
//!
//! **[S]** (decided here, recorded in SPEC.md 9.1):
//!
//! - Accepted headers: MP3 only Layer III with a bit-rate index 1–14
//!   (free format is refused) and a sample-rate index 0–2; AAC only
//!   profile LC, sampling frequency index 0–12, channel configuration 1
//!   or 2 and one raw data block. The CRC of a protected ADTS frame is
//!   skipped, not checked. Anything else is bad data: the chip skips one
//!   byte and looks again (PHSTAT bit 2). A frame longer than the ring can
//!   hold is skipped the same way.
//! - The chip works once a line, at slot 640 (with the fixed DMA): when
//!   less than one frame of decoded samples is left it takes one whole
//!   frame (and any bad bytes before it) from the ring and decodes it.
//!   Every byte taken (bad bytes too) is charged to the fixed DMA of the
//!   following lines, after Calliope: one slot for each 4 bytes, at most
//!   one slot a line (128 kbit/s: a slot about every 9 lines).
//! - A frame always yields the samples its header promises (1 152 or 576
//!   for MP3, 1 024 for AAC): a frame the decoder refuses plays as
//!   silence of that length and sets PHSTAT bit 2. So timing (PHRD, DMA,
//!   interrupts) never depends on the decoder.
//! - PHSTAT bit 0: the chip runs (PHCON bit 0 and DMACON bit 7). Bit 1:
//!   the last output sample found no decoded data (silence); it clears
//!   when a frame is decoded. PHLEVL/PHLEVR hold the peak of the
//!   resampled stream before PHVOL. PHFRM counts every frame taken,
//!   decodable or not. PHKBPS of an AAC frame: its whole length (header
//!   included) × 8 × sample rate / 1 024, in kbit/s, rounded.
//! - PHSIZE takes a power of two from 256 to 32 768; other values are
//!   rounded down to one and clamped (reset value 256); PHWR and PHRD are
//!   kept modulo the size. PHVOL resets to 0. Setting PHCON bit 0 clears
//!   the frame count, PHRATE, PHCHAN, PHKBPS, the decoded samples and the
//!   decoder; PHRD and PHWR stay.
//! - A ring that does not lie wholly in chip RAM stops the chip (PHCON
//!   bit 0 clears) with DMAERR bit 7 and the DMA-error interrupt.
//! - Interrupt (PHCON bit 2): when the frames taken in a line move the
//!   free space from below half of PHSIZE to half or more.
//! - Resampling: linear, the source position advancing by sample rate /
//!   48 000 a sample (an exact fraction, no rounding drift).
//! - PHSCOPE (3240-32BF): the last 64 output sample pairs before PHVOL,
//!   left in a word's high byte, right in its low byte (each the high
//!   byte of the 16-bit value), one every 4th output sample (12 kHz:
//!   5.3 ms), oldest first; for an oscilloscope picture (the Radio's
//!   Lissajous figure: left across, right up). Like the sound, not saved:
//!   it fills again after a load.
//! - Savestate: the registers, indices, counters and the number of
//!   decoded samples still waiting with the resampler's phase; after
//!   loading those samples play as silence and the decoder starts afresh,
//!   so everything but the sound (and the peaks PHLEVL/PHLEVR) stays
//!   exactly as without the load. Clearing PHCON bit 0 drops the decoded
//!   samples too.

use std::fmt;

use emu_core::{StateError, StateReader, StateWriter};
use symphonia_core::audio::{Channels, SampleBuffer};
use symphonia_core::codecs::{
    CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_AAC, CODEC_TYPE_MP3,
};
use symphonia_core::formats::Packet;

use crate::lumiere::ChipRam;

pub const PHID: u16 = 0x3200;
pub const PHCON: u16 = 0x3202;
pub const PHSEG: u16 = 0x3204;
pub const PHOFS: u16 = 0x3206;
pub const PHSIZE: u16 = 0x3208;
pub const PHWR: u16 = 0x320a;
pub const PHRD: u16 = 0x320c;
pub const PHSTAT: u16 = 0x320e;
pub const PHRATE: u16 = 0x3210;
pub const PHCHAN: u16 = 0x3212;
pub const PHKBPS: u16 = 0x3214;
pub const PHVOL: u16 = 0x3216;
pub const PHLEVL: u16 = 0x3218;
pub const PHLEVR: u16 = 0x321a;
pub const PHFRMH: u16 = 0x321c;
pub const PHFRML: u16 = 0x321e;
/// The scope window: 64 pairs of signed 8-bit samples of the output
/// (left in the high byte, right in the low byte, before PHVOL), one
/// every 4th output sample (12 kHz, 5.3 ms), the oldest first: 3240-32BF.
pub const PHSCOPE: u16 = 0x3240;
pub const SCOPE_SAMPLES: usize = 64;
const SCOPE_EVERY: u8 = 4;

/// PHCON bits.
pub const CON_ON: u16 = 1 << 0;
pub const CON_AAC: u16 = 1 << 1;
pub const CON_IRQ: u16 = 1 << 2;

/// PHSTAT bits.
pub const STAT_RUNNING: u16 = 1 << 0;
pub const STAT_STARVED: u16 = 1 << 1;
pub const STAT_BAD: u16 = 1 << 2;
pub const STAT_IRQ: u16 = 1 << 3;

const OUT_RATE: u32 = 48_000;
const CHIP_END: u64 = 0x0040_0000;
const MIN_SIZE: u16 = 256;
const MAX_SIZE: u16 = 32_768;
/// Decoded samples kept in a savestate at most (more than two frames).
const MAX_PENDING: u32 = 8192;

/// What a line's work gave the machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Line {
    /// Fixed-DMA slots taken this line.
    pub slots: u16,
    /// The half-empty interrupt is new.
    pub irq: bool,
    /// The ring lies outside chip RAM: DMAERR bit 7, the chip stopped.
    pub error: bool,
}

/// A frame header found in the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    /// Whole frame in bytes, header included.
    len: usize,
    /// Bytes before the payload the decoder gets (AAC: the ADTS header
    /// and CRC; MP3: 0, the decoder reads the header itself).
    skip: usize,
    rate: u32,
    channels: u8,
    /// Samples per channel in the frame.
    samples: u32,
    kbps: u16,
    /// AAC: sampling frequency index and channel configuration.
    sfi: u8,
}

const MP3_KBPS_V1: [u16; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const MP3_KBPS_V2: [u16; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];
const AAC_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// An MPEG audio Layer III header (ISO/IEC 11172-3, 13818-3).
fn mp3_header(h: [u8; 4]) -> Option<Header> {
    if h[0] != 0xff || h[1] & 0xe0 != 0xe0 {
        return None;
    }
    let version = h[1] >> 3 & 3; // 0: 2.5, 1: reserved, 2: MPEG-2, 3: MPEG-1
    let layer = h[1] >> 1 & 3; // 1: Layer III
    if version == 1 || layer != 1 {
        return None;
    }
    let br = usize::from(h[2] >> 4);
    let sr = usize::from(h[2] >> 2 & 3);
    if br == 0 || br == 15 || sr == 3 {
        return None;
    }
    let base = [44_100, 48_000, 32_000][sr];
    let (rate, kbps, factor, samples) = match version {
        3 => (base, MP3_KBPS_V1[br], 144_000, 1152),
        2 => (base / 2, MP3_KBPS_V2[br], 72_000, 576),
        _ => (base / 4, MP3_KBPS_V2[br], 72_000, 576),
    };
    let pad = u32::from(h[2] >> 1 & 1);
    let len = (factor * u32::from(kbps) / rate + pad) as usize;
    Some(Header {
        len,
        skip: 0,
        rate,
        channels: if h[3] >> 6 == 3 { 1 } else { 2 },
        samples,
        kbps,
        sfi: 0,
    })
}

/// An ADTS header (ISO/IEC 13818-7, 14496-3).
fn adts_header(h: [u8; 7]) -> Option<Header> {
    if h[0] != 0xff || h[1] & 0xf6 != 0xf0 {
        return None;
    }
    let protected = h[1] & 1 == 0;
    let profile = h[2] >> 6; // 1: LC
    let sfi = h[2] >> 2 & 0xf;
    let chan = (h[2] & 1) << 2 | h[3] >> 6;
    let len = usize::from(h[3] & 3) << 11 | usize::from(h[4]) << 3 | usize::from(h[5] >> 5);
    let blocks = h[6] & 3;
    let skip = if protected { 9 } else { 7 };
    if profile != 1 || sfi > 12 || !(1..=2).contains(&chan) || blocks != 0 || len <= skip {
        return None;
    }
    let rate = AAC_RATES[usize::from(sfi)];
    let bits = len as u64 * 8 * u64::from(rate);
    Some(Header {
        len,
        skip,
        rate,
        channels: chan,
        samples: 1024,
        kbps: ((bits + 512_000) / 1_024_000).min(0xffff) as u16,
        sfi,
    })
}

/// The decoder in use; a copy of the chip (savestate loading) has none
/// and makes a fresh one at its next frame.
#[derive(Default)]
struct Codec(Option<(bool, u32, u8, Box<dyn Decoder>)>);

impl Clone for Codec {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl fmt::Debug for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "Codec(..)"
        } else {
            "Codec(none)"
        })
    }
}

impl Codec {
    /// Decode one frame into `out` (exactly `h.samples` stereo samples);
    /// `false` when the decoder refused it (`out` then holds silence).
    fn decode(&mut self, aac: bool, h: &Header, frame: &[u8], out: &mut Vec<[i16; 2]>) -> bool {
        let n = h.samples as usize;
        let start = out.len();
        let ok = self.try_decode(aac, h, frame, out);
        if !ok {
            out.truncate(start);
        }
        out.resize(start + n, [0, 0]);
        out.truncate(start + n);
        ok
    }

    fn try_decode(&mut self, aac: bool, h: &Header, frame: &[u8], out: &mut Vec<[i16; 2]>) -> bool {
        let fits =
            matches!(&self.0, Some((a, r, c, _)) if *a == aac && *r == h.rate && *c == h.channels);
        if !fits {
            self.0 = make_decoder(aac, h).map(|d| (aac, h.rate, h.channels, d));
        }
        let Some((_, _, _, dec)) = &mut self.0 else {
            return false;
        };
        let packet = Packet::new_from_slice(0, 0, u64::from(h.samples), &frame[h.skip..]);
        let Ok(buf) = dec.decode(&packet) else {
            return false;
        };
        let spec = *buf.spec();
        let chans = spec.channels.count();
        if chans == 0 || buf.frames() == 0 {
            return false;
        }
        let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
        sb.copy_interleaved_ref(buf);
        let conv = |x: f32| (x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
        out.extend(
            sb.samples()
                .chunks_exact(chans)
                .take(h.samples as usize)
                .map(|s| {
                    let l = conv(s[0]);
                    [l, if chans > 1 { conv(s[1]) } else { l }]
                }),
        );
        true
    }
}

fn make_decoder(aac: bool, h: &Header) -> Option<Box<dyn Decoder>> {
    let mut p = CodecParameters::new();
    p.with_sample_rate(h.rate)
        .with_channels(if h.channels == 1 {
            Channels::FRONT_LEFT
        } else {
            Channels::FRONT_LEFT | Channels::FRONT_RIGHT
        });
    let opts = DecoderOptions::default();
    if aac {
        // AudioSpecificConfig: object type 2 (LC), the sampling frequency
        // index, the channel configuration, GASpecificConfig all 0.
        let asc = 2u16 << 11 | u16::from(h.sfi) << 7 | u16::from(h.channels) << 3;
        p.for_codec(CODEC_TYPE_AAC)
            .with_extra_data(asc.to_be_bytes().to_vec().into_boxed_slice());
        symphonia_codec_aac::AacDecoder::try_new(&p, &opts)
            .ok()
            .map(|d| Box::new(d) as Box<dyn Decoder>)
    } else {
        p.for_codec(CODEC_TYPE_MP3);
        symphonia_bundle_mp3::MpaDecoder::try_new(&p, &opts)
            .ok()
            .map(|d| Box::new(d) as Box<dyn Decoder>)
    }
}

#[derive(Clone, Debug)]
pub struct Phonograph {
    con: u16,
    seg: u16,
    ofs: u16,
    size: u16,
    wr: u16,
    rd: u16,
    bad: bool,
    irq: bool,
    starved: bool,
    rate: u32,
    channels: u8,
    kbps: u16,
    vol: u16,
    level: [u16; 2],
    frames: u32,
    /// DMACON bit 7 (and the master bit).
    dma: bool,
    /// Bytes taken from the ring and not yet charged to a DMA slot.
    owed: u32,
    /// Samples a frame of the stream gives (the decode threshold).
    frame_samples: u32,
    /// Decoded samples at the stream's rate; `pos` is the one being
    /// played, `frac` the position between it and the next in 1/48 000.
    pcm: Vec<[i16; 2]>,
    pos: usize,
    frac: u32,
    codec: Codec,
    /// The scope window's ring, its next slot and the output samples
    /// until the next is kept (like the sound, not machine state).
    scope: [[i8; 2]; SCOPE_SAMPLES],
    scope_at: usize,
    scope_wait: u8,
}

impl Default for Phonograph {
    fn default() -> Self {
        Self::new()
    }
}

impl Phonograph {
    pub fn new() -> Self {
        Self {
            con: 0,
            seg: 0,
            ofs: 0,
            size: MIN_SIZE,
            wr: 0,
            rd: 0,
            bad: false,
            irq: false,
            starved: false,
            rate: 0,
            channels: 0,
            kbps: 0,
            vol: 0,
            level: [0; 2],
            frames: 0,
            dma: false,
            owed: 0,
            frame_samples: 0,
            pcm: Vec::new(),
            pos: 0,
            frac: 0,
            codec: Codec::default(),
            scope: [[0; 2]; SCOPE_SAMPLES],
            scope_at: 0,
            scope_wait: 0,
        }
    }

    /// Keeps every 4th output sample (both sides) for the scope.
    fn keep(&mut self, s: [i32; 2]) {
        if self.scope_wait == 0 {
            self.scope_wait = SCOPE_EVERY;
            self.scope[self.scope_at] = s.map(|v| (v >> 8).clamp(-128, 127) as i8);
            self.scope_at = (self.scope_at + 1) % SCOPE_SAMPLES;
        }
        self.scope_wait -= 1;
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn running(&self) -> bool {
        self.con & CON_ON != 0 && self.dma
    }

    fn mask(&self) -> u16 {
        self.size - 1
    }

    fn used(&self) -> u16 {
        self.wr.wrapping_sub(self.rd) & self.mask()
    }

    fn free(&self) -> u16 {
        self.size - 1 - self.used()
    }

    fn status(&self) -> u16 {
        u16::from(self.running())
            | u16::from(self.starved) << 1
            | u16::from(self.bad) << 2
            | u16::from(self.irq) << 3
    }

    /// DMACON bit 7 (with the master bit), from the machine.
    pub fn set_dma(&mut self, on: bool) {
        self.dma = on;
    }

    /// The interrupt is pending (PHSTAT bit 3).
    pub fn irq(&self) -> bool {
        self.irq
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            PHID => 0xf0e1,
            PHCON => self.con,
            PHSEG => self.seg,
            PHOFS => self.ofs,
            PHSIZE => self.size,
            PHWR => self.wr,
            PHRD => self.rd,
            PHSTAT => self.status(),
            PHRATE => (self.rate / 10) as u16,
            PHCHAN => u16::from(self.channels),
            PHKBPS => self.kbps,
            PHVOL => self.vol,
            PHLEVL => self.level[0],
            PHLEVR => self.level[1],
            PHFRMH => (self.frames >> 16) as u16,
            PHFRML => self.frames as u16,
            o if (PHSCOPE..PHSCOPE + 2 * SCOPE_SAMPLES as u16).contains(&o) => {
                let [l, r] =
                    self.scope[(self.scope_at + usize::from(o - PHSCOPE) / 2) % SCOPE_SAMPLES];
                u16::from(l as u8) << 8 | u16::from(r as u8)
            }
            _ => 0,
        }
    }

    /// Register read: PHSTAT clears bits 2 and 3, PHLEVL/PHLEVR their peak.
    pub fn read(&mut self, off: u16) -> u16 {
        let v = self.peek(off);
        match off & !1 {
            PHSTAT => {
                self.bad = false;
                self.irq = false;
            }
            PHLEVL => self.level[0] = 0,
            PHLEVR => self.level[1] = 0,
            _ => {}
        }
        v
    }

    pub fn write(&mut self, off: u16, v: u16) {
        match off & !1 {
            PHCON => {
                let start = v & CON_ON != 0 && self.con & CON_ON == 0;
                self.con = v & 7;
                if start {
                    self.frames = 0;
                    self.rate = 0;
                    self.channels = 0;
                    self.kbps = 0;
                    self.starved = false;
                    self.flush();
                } else if v & CON_ON == 0 {
                    self.flush();
                }
            }
            PHSEG => self.seg = v,
            PHOFS => self.ofs = v,
            PHSIZE => {
                let v = v.clamp(MIN_SIZE, MAX_SIZE);
                self.size = 1 << (15 - v.leading_zeros());
                self.wr &= self.mask();
                self.rd &= self.mask();
            }
            PHWR => self.wr = v & self.mask(),
            PHVOL => self.vol = v & 0x7f,
            _ => {}
        }
    }

    /// Drop the decoded samples and the decoder.
    fn flush(&mut self) {
        self.pcm.clear();
        self.pos = 0;
        self.frac = 0;
        self.frame_samples = 0;
        self.codec = Codec::default();
    }

    /// Ring byte `i` after PHRD.
    fn byte(&self, mem: &dyn ChipRam, base: u32, i: u16) -> u8 {
        mem.chip_byte(base + u32::from(self.rd.wrapping_add(i) & self.mask()))
    }

    /// Decoded samples not yet played (beyond the one sounding).
    fn pending(&self) -> u32 {
        self.pcm.len().saturating_sub(self.pos + 1) as u32
    }

    /// Decoded samples from the one sounding on (for the savestate).
    fn held(&self) -> u32 {
        self.pcm.len().saturating_sub(self.pos) as u32
    }

    /// Slot 640 of every line: charge a DMA slot for bytes taken, and
    /// take and decode a frame when less than one frame is left.
    pub fn line(&mut self, mem: &dyn ChipRam) -> Line {
        let mut res = Line::default();
        if !self.running() {
            return res;
        }
        let base = u32::from(self.seg) << 16 | u32::from(self.ofs);
        if u64::from(base) + u64::from(self.size) > CHIP_END {
            self.con &= !CON_ON;
            self.flush();
            res.error = true;
            return res;
        }
        if self.owed > 0 {
            res.slots = 1;
            self.owed = self.owed.saturating_sub(4);
        }
        if self.pending() >= self.frame_samples.max(1) {
            return res;
        }
        let free_before = self.free();
        let aac = self.con & CON_AAC != 0;
        let hlen: u16 = if aac { 7 } else { 4 };
        loop {
            let used = self.used();
            if used < hlen {
                break;
            }
            let header = if aac {
                let mut h = [0; 7];
                for (i, b) in h.iter_mut().enumerate() {
                    *b = self.byte(mem, base, i as u16);
                }
                adts_header(h)
            } else {
                let mut h = [0; 4];
                for (i, b) in h.iter_mut().enumerate() {
                    *b = self.byte(mem, base, i as u16);
                }
                mp3_header(h)
            };
            let Some(h) = header.filter(|h| h.len < usize::from(self.size)) else {
                // Bad data: one byte on.
                self.rd = self.rd.wrapping_add(1) & self.mask();
                self.owed += 1;
                self.bad = true;
                continue;
            };
            if usize::from(used) < h.len {
                break; // the frame is not whole yet
            }
            let frame: Vec<u8> = (0..h.len as u16).map(|i| self.byte(mem, base, i)).collect();
            self.rd = self.rd.wrapping_add(h.len as u16) & self.mask();
            self.owed += h.len as u32;
            self.take_frame(aac, &h, &frame);
            break;
        }
        if self.con & CON_IRQ != 0 && free_before < self.size / 2 && self.free() >= self.size / 2 {
            res.irq = !self.irq;
            self.irq = true;
        }
        res
    }

    fn take_frame(&mut self, aac: bool, h: &Header, frame: &[u8]) {
        if self.pos > 0 {
            self.pcm.drain(..self.pos);
            self.pos = 0;
        }
        let mut pcm = std::mem::take(&mut self.pcm);
        if !self.codec.decode(aac, h, frame, &mut pcm) {
            self.bad = true;
        }
        self.pcm = pcm;
        self.rate = h.rate;
        self.channels = h.channels;
        self.kbps = h.kbps;
        self.frame_samples = h.samples;
        self.frames = self.frames.wrapping_add(1);
        self.starved = false;
    }

    /// The next 48 kHz output sample, each side × PHVOL (Calliope adds it
    /// to its mix and scales by 1/127 and AUDMASTER/127).
    pub fn output(&mut self) -> [i32; 2] {
        if !self.running() {
            return [0, 0];
        }
        if self.pos + 1 >= self.pcm.len() || self.rate == 0 {
            self.starved = true;
            self.keep([0, 0]);
            return [0, 0];
        }
        let (a, b) = (self.pcm[self.pos], self.pcm[self.pos + 1]);
        let f = i64::from(self.frac);
        let lerp = |x: i16, y: i16| {
            (i64::from(x) + (i64::from(y) - i64::from(x)) * f / i64::from(OUT_RATE)) as i32
        };
        let s = [lerp(a[0], b[0]), lerp(a[1], b[1])];
        for (lev, v) in self.level.iter_mut().zip(s) {
            *lev = (*lev).max(v.unsigned_abs().min(32_767) as u16);
        }
        self.keep(s);
        self.frac += self.rate;
        while self.frac >= OUT_RATE {
            self.frac -= OUT_RATE;
            self.pos += 1;
        }
        let vol = i32::from(self.vol);
        [s[0] * vol, s[1] * vol]
    }

    pub fn save(&self, w: &mut StateWriter) {
        for v in [self.con, self.seg, self.ofs, self.size, self.wr, self.rd] {
            w.u16(v);
        }
        w.bool(self.bad);
        w.bool(self.irq);
        w.bool(self.starved);
        w.u32(self.rate);
        w.u8(self.channels);
        w.u16(self.kbps);
        w.u16(self.vol);
        w.u16(self.level[0]);
        w.u16(self.level[1]);
        w.u32(self.frames);
        w.bool(self.dma);
        w.u32(self.owed);
        w.u32(self.frame_samples);
        w.u32(self.held());
        w.u32(self.frac);
    }

    /// Corrupt data gives an error, never a panic; `self` is unchanged
    /// then.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut s = Phonograph::new();
        s.con = r.u16()? & 7;
        s.seg = r.u16()?;
        s.ofs = r.u16()?;
        let size = r.u16()?;
        if !(MIN_SIZE..=MAX_SIZE).contains(&size) || !size.is_power_of_two() {
            return Err(StateError::Invalid("Phonograph ring size"));
        }
        s.size = size;
        s.wr = r.u16()? & s.mask();
        s.rd = r.u16()? & s.mask();
        s.bad = r.bool()?;
        s.irq = r.bool()?;
        s.starved = r.bool()?;
        s.rate = r.u32()?;
        s.channels = r.u8()?;
        s.kbps = r.u16()?;
        s.vol = r.u16()? & 0x7f;
        s.level = [r.u16()?.min(32_767), r.u16()?.min(32_767)];
        s.frames = r.u32()?;
        s.dma = r.bool()?;
        s.owed = r.u32()?;
        s.frame_samples = r.u32()?;
        let held = r.u32()?;
        s.frac = r.u32()?;
        if s.rate > 96_000 || s.channels > 2 || s.frame_samples > 1152 || s.frac >= OUT_RATE {
            return Err(StateError::Invalid("Phonograph stream"));
        }
        if held > MAX_PENDING || (held > 1 && s.rate == 0) {
            return Err(StateError::Invalid("Phonograph samples"));
        }
        // The samples that were waiting play as silence.
        s.pcm = vec![[0, 0]; held as usize];
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MP3: &[u8] = include_bytes!("../../../tests/data/logophor16/tone.mp3");
    const AAC: &[u8] = include_bytes!("../../../tests/data/logophor16/tone.aac");
    /// CPU cycles a line, output samples come every 250.
    const LINE: u32 = 318;
    const RING: u32 = 0x1_0000;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    /// A chip with its ring at 0001:0000 of `size` bytes, on, volume 127.
    fn chip(size: u16, con: u16) -> (Phonograph, Ram) {
        let mut p = Phonograph::new();
        p.write(PHSEG, 1);
        p.write(PHSIZE, size);
        p.write(PHVOL, 127);
        p.set_dma(true);
        p.write(PHCON, con | CON_ON);
        (p, Ram(vec![0; 0x2_0000]))
    }

    /// The CPU's side: copy as much of `src` as fits into the ring.
    fn feed(p: &mut Phonograph, ram: &mut Ram, src: &[u8], at: &mut usize) {
        while *at < src.len() && p.free() > 0 {
            let wr = p.peek(PHWR);
            ram.0[(RING + u32::from(wr)) as usize] = src[*at];
            p.write(PHWR, wr.wrapping_add(1));
            *at += 1;
        }
    }

    /// Run `samples` output samples, a line every 318 cycles, feeding the
    /// ring from `src` when `feed_it`; the output (stereo, before ×127
    /// scaling removed) and the slots and interrupts.
    fn run(
        p: &mut Phonograph,
        ram: &mut Ram,
        src: &[u8],
        at: &mut usize,
        samples: u32,
        feed_it: bool,
    ) -> (Vec<[i32; 2]>, u32, u32) {
        let (mut out, mut slots, mut irqs) = (Vec::new(), 0, 0);
        for k in 0..samples * 250 {
            if k % LINE == 0 {
                if feed_it {
                    feed(p, ram, src, at);
                }
                let l = p.line(ram);
                slots += u32::from(l.slots);
                irqs += u32::from(l.irq);
            }
            if k % 250 == 249 {
                let s = p.output();
                out.push([s[0] / 127, s[1] / 127]);
            }
        }
        (out, slots, irqs)
    }

    /// Frequency from the rising zero crossings of one side.
    fn freq(out: &[[i32; 2]], side: usize) -> f64 {
        let x: Vec<i32> = out.iter().map(|s| s[side]).collect();
        let ups: Vec<usize> = (1..x.len())
            .filter(|&i| x[i - 1] < 0 && x[i] >= 0)
            .collect();
        let (first, last) = (ups[0], *ups.last().unwrap());
        (ups.len() - 1) as f64 * 48_000.0 / (last - first) as f64
    }

    fn check_tone(src: &[u8], con: u16, kbps: std::ops::RangeInclusive<u16>) {
        let (mut p, mut ram) = chip(4096, con);
        let mut at = 0;
        assert_eq!(p.read(PHID), 0xf0e1);
        // 1.5 s of output.
        let (out, slots, _) = run(&mut p, &mut ram, src, &mut at, 72_000, true);
        assert_eq!(p.peek(PHRATE), 4410);
        assert_eq!(p.peek(PHCHAN), 2);
        assert!(kbps.contains(&p.peek(PHKBPS)), "{}", p.peek(PHKBPS));
        let frames = u32::from(p.peek(PHFRMH)) << 16 | u32::from(p.peek(PHFRML));
        // 1.5 s at 44.1 kHz: about 57 frames of 1152 or 65 of 1024, plus
        // up to two frames buffered ahead (and an MP3 info frame).
        let per = if con & CON_AAC != 0 { 1024 } else { 1152 };
        let expect = 72_000 * 44_100 / 48_000 / per;
        assert!((expect..=expect + 3).contains(&frames), "{frames} frames");
        // Every byte taken was charged: a slot for 4 bytes of each frame
        // (the last frame's partly still owed).
        let taken = at as u32 - u32::from(p.used());
        assert!(
            slots * 4 >= taken - 4 * 1100 && slots * 4 <= taken + 3 * frames,
            "{slots} {taken}"
        );
        let tail = &out[24_000..];
        let (l, r) = (freq(tail, 0), freq(tail, 1));
        assert!((l - 440.0).abs() < 1.0, "left {l} Hz");
        assert!((r - 660.0).abs() < 1.0, "right {r} Hz");
        let peak = tail.iter().map(|s| s[0].abs()).max().unwrap();
        assert!(peak > 3000, "audible: {peak}");
        // The scope window: every 4th of the last 256 output samples, left
        // and right, before PHVOL (the test's PHVOL is 127, which `run`
        // divides out), the high byte.
        let scope: Vec<[i8; 2]> = (0..SCOPE_SAMPLES as u16)
            .map(|k| p.peek(PHSCOPE + 2 * k).to_be_bytes().map(|b| b as i8))
            .collect();
        let last = &out[out.len() - 4 * SCOPE_SAMPLES..];
        let kept = |phase: usize| -> Vec<[i8; 2]> {
            (0..SCOPE_SAMPLES)
                .map(|k| last[4 * k + phase].map(|v| (v >> 8).clamp(-128, 127) as i8))
                .collect()
        };
        assert!(
            (0..4).any(|ph| kept(ph) == scope),
            "the scope shows the output"
        );
        assert!(scope
            .iter()
            .any(|v| v[0].unsigned_abs() > 10 && v[1] != v[0]));
        assert_eq!(p.peek(PHSTAT) & (STAT_STARVED | STAT_BAD), 0);
        assert!(p.read(PHLEVL) > 3000);
        assert_eq!(p.read(PHLEVL), 0, "cleared on read");
        // The stream ends: the ring runs dry, silence and starved.
        let (out, _, _) = run(&mut p, &mut ram, src, &mut at, 60_000, true);
        assert_eq!(at, src.len());
        assert!(out[out.len() - 100..].iter().all(|s| *s == [0, 0]));
        assert_eq!(p.peek(PHSTAT), STAT_RUNNING | STAT_STARVED);
        assert_eq!(p.used(), 0, "everything taken");
    }

    #[test]
    fn mp3_plays_its_tone_at_48_khz() {
        check_tone(MP3, 0, 128..=128);
    }

    #[test]
    fn aac_plays_its_tone_at_48_khz() {
        check_tone(AAC, CON_AAC, 80..=112);
    }

    #[test]
    fn headers_are_parsed() {
        let h = mp3_header([0xff, 0xfb, 0x90, 0x44]).unwrap();
        assert_eq!((h.len, h.rate, h.channels, h.kbps), (417, 44_100, 2, 128));
        let h = mp3_header([0xff, 0xfb, 0x92, 0xc4]).unwrap();
        assert_eq!((h.len, h.channels), (418, 1), "padding, mono");
        let h = mp3_header([0xff, 0xf3, 0x84, 0x44]).unwrap();
        assert_eq!((h.len, h.rate, h.samples), (192, 24_000, 576), "MPEG-2");
        assert!(
            mp3_header([0xff, 0xfb, 0x00, 0x44]).is_none(),
            "free format"
        );
        assert!(mp3_header([0xff, 0xfd, 0x90, 0x44]).is_none(), "layer II");
        assert!(
            mp3_header([0xff, 0xfb, 0x9c, 0x44]).is_none(),
            "rate index 3"
        );
        // ADTS: LC, 44.1 kHz, stereo, 300 bytes, no CRC / with CRC.
        // Length 300: bits 12-11 in byte 3, 10-3 in byte 4, 2-0 on top of byte 5.
        let adts = |b1: u8| [0xff, b1, 0x50, 0x80, (300u16 >> 3) as u8, 0x9f, 0xfc];
        let h = adts_header(adts(0xf1)).unwrap();
        assert_eq!(
            (h.len, h.skip, h.rate, h.channels, h.sfi),
            (300, 7, 44_100, 2, 4)
        );
        assert_eq!(h.kbps, 103);
        assert_eq!(adts_header(adts(0xf0)).unwrap().skip, 9, "CRC");
        assert!(adts_header(adts(0xf3)).is_none(), "layer 1");
        let mut main = adts(0xf1);
        main[2] = 0x10;
        assert!(adts_header(main).is_none(), "profile Main");
    }

    #[test]
    fn junk_is_skipped_and_a_frame_wraps_round_the_ring() {
        // 1000 bytes of junk before the stream, a 2 KiB ring: frames lie
        // across its end again and again.
        let mut src = vec![0x55u8; 1000];
        src.extend_from_slice(MP3);
        let (mut p, mut ram) = chip(2048, 0);
        let mut at = 0;
        let (out, slots, _) = run(&mut p, &mut ram, &src, &mut at, 48_000, true);
        assert!(slots >= 250, "junk bytes charged too: {slots}");
        assert_eq!(p.read(PHSTAT) & STAT_BAD, STAT_BAD);
        assert_eq!(p.read(PHSTAT) & STAT_BAD, 0, "cleared on read");
        let frames = u32::from(p.peek(PHFRML));
        assert!(frames > 30, "{frames}");
        assert!((freq(&out[20_000..], 0) - 440.0).abs() < 1.0);
        // No more bad data once in sync, though the ring wrapped.
        let (out, _, _) = run(&mut p, &mut ram, &src, &mut at, 12_000, true);
        assert_eq!(p.peek(PHSTAT) & STAT_BAD, 0);
        assert!((freq(&out, 1) - 660.0).abs() < 1.5);
    }

    #[test]
    fn starving_and_the_half_empty_interrupt() {
        let (mut p, mut ram) = chip(8192, CON_IRQ);
        let mut at = 0;
        // Without data: silence, starved.
        let (out, slots, irqs) = run(&mut p, &mut ram, MP3, &mut at, 100, false);
        assert!(out.iter().all(|s| *s == [0, 0]));
        assert_eq!((slots, irqs), (0, 0));
        assert_eq!(p.peek(PHSTAT), STAT_RUNNING | STAT_STARVED);
        // The ring filled once (8191 bytes): the frames taken bring the
        // free space to half once, then it stays above.
        feed(&mut p, &mut ram, MP3, &mut at);
        assert_eq!(p.free(), 0);
        let (_, _, irqs) = run(&mut p, &mut ram, MP3, &mut at, 48_000, false);
        assert_eq!(irqs, 1);
        assert_eq!(p.peek(PHSTAT) & STAT_IRQ, STAT_IRQ);
        assert!(p.irq());
        p.read(PHSTAT);
        assert!(!p.irq(), "cleared on read");
        // Without DMACON bit 7 nothing happens.
        p.set_dma(false);
        let rd = p.peek(PHRD);
        let (out, slots, _) = run(&mut p, &mut ram, MP3, &mut at, 10_000, false);
        assert_eq!((p.peek(PHRD), slots), (rd, 0));
        assert!(out.iter().all(|s| *s == [0, 0]));
        assert_eq!(p.peek(PHSTAT) & STAT_RUNNING, 0);
    }

    #[test]
    fn registers_and_a_ring_outside_chip_ram() {
        let mut p = Phonograph::new();
        assert_eq!(p.peek(PHSIZE), 256);
        p.write(PHSIZE, 3000);
        assert_eq!(p.peek(PHSIZE), 2048, "rounded down");
        p.write(PHSIZE, 0xffff);
        assert_eq!(p.peek(PHSIZE), 32_768);
        p.write(PHSIZE, 5);
        assert_eq!(p.peek(PHSIZE), 256);
        p.write(PHWR, 0x1234);
        assert_eq!(p.peek(PHWR), 0x34);
        p.write(PHVOL, 0x1ff);
        assert_eq!(p.peek(PHVOL), 0x7f);
        p.write(PHRD, 9);
        assert_eq!(p.peek(PHRD), 0, "read only");
        assert_eq!(p.peek(0x3220), 0);
        p.write(PHSEG, 0x3f);
        p.write(PHOFS, 0xff80);
        p.set_dma(true);
        p.write(PHCON, CON_ON);
        let l = p.line(&Ram(Vec::new()));
        assert!(l.error);
        assert_eq!(p.peek(PHCON) & CON_ON, 0, "stopped");
    }

    #[test]
    fn savestate_round_trip_and_corrupt_data() {
        let (mut p, mut ram) = chip(4096, CON_IRQ);
        let mut at = 0;
        run(&mut p, &mut ram, MP3, &mut at, 5_000, true);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        let mut q = Phonograph::new();
        q.load(&mut StateReader::new(&bytes)).unwrap();
        let mut w = StateWriter::new();
        q.save(&mut w);
        assert_eq!(w.into_bytes(), bytes);
        // Both go on alike but for the sound of the samples that were
        // waiting and the frame after (the decoder starts afresh).
        let mut ram2 = Ram(ram.0.clone());
        let mut at2 = at;
        let (a, sa, ia) = run(&mut p, &mut ram, MP3, &mut at, 20_000, true);
        let (b, sb, ib) = run(&mut q, &mut ram2, MP3, &mut at2, 20_000, true);
        assert_eq!((sa, ia, at), (sb, ib, at2));
        assert!(b[..1000].iter().all(|s| *s == [0, 0]), "silence first");
        assert_eq!(&a[6000..], &b[6000..], "then the same sound");
        // The peaks differ with the sound: read them away.
        for c in [&mut p, &mut q] {
            c.read(PHLEVL);
            c.read(PHLEVR);
        }
        let (mut w1, mut w2) = (StateWriter::new(), StateWriter::new());
        p.save(&mut w1);
        q.save(&mut w2);
        assert_eq!(w1.into_bytes(), w2.into_bytes());
        // Corrupt data: errors, no panic, nothing changed.
        let before = format!("{q:?}");
        for n in 0..bytes.len() {
            assert!(q.load(&mut StateReader::new(&bytes[..n])).is_err());
        }
        let mut bad = bytes.clone();
        bad[6] = 0x33; // size $1033
        assert!(q.load(&mut StateReader::new(&bad)).is_err());
        for k in 0..bytes.len() {
            let mut b = bytes.clone();
            b[k] ^= 0xff;
            let _ = Phonograph::new().load(&mut StateReader::new(&b));
        }
        assert_eq!(format!("{q:?}"), before);
    }
}
