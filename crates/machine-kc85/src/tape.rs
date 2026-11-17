//! The cassette recorder at the KC85's tape socket (playback).
//!
//! **The signal.** Every bit is one full wave: a 0 at 2400 Hz, a 1 at
//! 1200 Hz; a wave at 600 Hz separates the bytes. A block is a lead of
//! 1-waves (8000 before a file's first block, 160 before the others), a
//! separator, the block number, 128 data bytes and their sum (modulo
//! 256); every byte goes lowest bit first and ends with a separator.
//! Blocks count from 1; a file's last block has number `$FF`.
//!
//! The base unit turns every zero crossing into a strobe at PIO port A;
//! CAOS measures the half waves with CTC channel 2. The half waves here
//! have the lengths CAOS itself writes (its save routine loads CTC
//! channel 1 with 23, 47 and 93 at prescaler 16: 368, 752 and 1488 CPU
//! clocks). The tape moves while PLAY is down and the motor line (PIO A
//! bit 6) is on.
//!
//! **Images.**
//! - KC-TAP (`.tap`): the 16 bytes `$C3 "KC-TAPE by AF. "`, then records
//!   of a block number and 128 bytes; a further file starts with the 16
//!   bytes again.
//! - KCC (`.kcc`): the 128-byte header block (name, address count, load,
//!   end and start address) and the data in blocks of 128 bytes; the
//!   block numbers are made here (1, 2, … and `$FF` for the last).
//!
//! Sources: KC85/3 and KC85/4 Systemhandbuch (tape format: frequencies,
//! block layout); the half-wave lengths and the lead CAOS needs are read
//! from the user's CAOS 4.2 ROM (save routine at `$E56B`, load routine at
//! `$E58E`); the KC-TAP layout from the files themselves (the format of
//! Arne Fitzenreiter's KC emulator, used by KCemu and JKCEMU as well).

use mimic_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use mimic_core::{StateError, StateReader, StateWriter};

const TAP_MAGIC: &[u8; 16] = b"\xc3KC-TAPE by AF. ";

/// Half waves: of a 0, of a 1, of a separator (CPU clocks).
pub const HALF: [u64; 3] = [368, 752, 1488];
const ZERO: u8 = 0;
const ONE: u8 = 1;
const SEPARATOR: u8 = 2;

/// 1-waves before a file's first block and before the others.
pub const LEAD_FIRST: usize = 8000;
pub const LEAD: usize = 160;

/// One block on the tape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub number: u8,
    pub data: [u8; 128],
    /// A file's first block (the long lead).
    pub first: bool,
}

/// The blocks of an image.
pub fn parse(image: &[u8]) -> Result<Vec<Block>, MediaError> {
    let bad = |m: &str| MediaError::Format(m.into());
    let mut blocks = Vec::new();
    if image.starts_with(TAP_MAGIC) {
        let mut rest = image;
        let mut first = false;
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix(TAP_MAGIC) {
                rest = r;
                first = true;
                continue;
            }
            if rest.len() < 129 {
                // Padding at the end of some files.
                if rest.iter().all(|&b| b == 0) {
                    break;
                }
                return Err(bad("KC-TAP: a cut-off block"));
            }
            blocks.push(Block {
                number: rest[0],
                data: rest[1..129].try_into().expect("128 bytes"),
                first,
            });
            first = false;
            rest = &rest[129..];
        }
    } else if image.len() >= 256 && image.len().is_multiple_of(128) {
        let n = image.len() / 128;
        if n > 255 {
            return Err(bad("KCC: more than 255 blocks"));
        }
        for (i, data) in image.as_chunks::<128>().0.iter().enumerate() {
            blocks.push(Block {
                number: if i + 1 == n { 0xff } else { i as u8 + 1 },
                data: *data,
                first: i == 0,
            });
        }
    } else {
        return Err(bad("neither a KC-TAP nor a KCC file"));
    }
    if blocks.is_empty() {
        return Err(bad("no blocks on the tape"));
    }
    Ok(blocks)
}

/// The tape's half waves ([`HALF`] indices), block after block.
fn half_waves(blocks: &[Block]) -> Vec<u8> {
    let mut out = Vec::new();
    let byte = |out: &mut Vec<u8>, b: u8| {
        for bit in 0..8 {
            let kind = if b >> bit & 1 != 0 { ONE } else { ZERO };
            out.extend([kind; 2]);
        }
        out.extend([SEPARATOR; 2]);
    };
    for b in blocks {
        let lead = if b.first { LEAD_FIRST } else { LEAD };
        out.resize(out.len() + 2 * lead, ONE);
        out.extend([SEPARATOR; 2]);
        byte(&mut out, b.number);
        let mut sum = 0u8;
        for &d in &b.data {
            byte(&mut out, d);
            sum = sum.wrapping_add(d);
        }
        byte(&mut out, sum);
    }
    // The tape runs on a little.
    out.resize(out.len() + 2 * LEAD, ONE);
    out
}

#[derive(Default)]
pub struct Tape {
    media: Option<Media>,
    halves: Vec<u8>,
    /// The half wave under the head and the CPU clocks left of it.
    pos: usize,
    left: u64,
    /// PLAY is down.
    pub playing: bool,
    /// The motor line, as of the last [`Tape::run`].
    motor: bool,
}

impl Tape {
    pub fn inserted(&self) -> bool {
        self.media.is_some()
    }

    fn rewind(&mut self) {
        self.pos = 0;
        self.left = self.halves.first().map_or(0, |&k| HALF[usize::from(k)]);
    }

    /// At the end of the tape.
    pub fn at_end(&self) -> bool {
        self.pos >= self.halves.len()
    }

    /// Run `t` CPU clocks with the motor line at `motor`. Returns the
    /// zero crossings that passed the head.
    pub fn run(&mut self, t: u64, motor: bool) -> u32 {
        self.motor = motor;
        if !motor || !self.playing || self.at_end() {
            return 0;
        }
        let mut t = t;
        let mut edges = 0;
        while t >= self.left {
            t -= self.left;
            edges += 1;
            self.pos += 1;
            match self.halves.get(self.pos) {
                Some(&k) => self.left = HALF[usize::from(k)],
                None => {
                    self.left = 0;
                    return edges;
                }
            }
        }
        self.left -= t;
        edges
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u64(self.pos as u64);
        w.u64(self.left);
        w.bool(self.playing);
        w.bool(self.motor);
    }

    /// What [`Tape::save`] wrote, for [`Tape::restore`].
    pub fn load(r: &mut StateReader<'_>) -> Result<(u64, u64, bool, bool), StateError> {
        Ok((r.u64()?, r.u64()?, r.bool()?, r.bool()?))
    }

    /// The tape itself stays; its position loads (within the tape that
    /// is inserted now).
    pub fn restore(&mut self, (pos, left, playing, motor): (u64, u64, bool, bool)) {
        let pos = (pos as usize).min(self.halves.len());
        self.left = match self.halves.get(pos) {
            Some(&k) => left.clamp(1, HALF[usize::from(k)]),
            None => 0,
        };
        self.pos = pos;
        self.playing = playing;
        self.motor = motor;
    }
}

impl MediaSlot for Tape {
    /// A tape goes in with PLAY down: LOAD starts the motor.
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        self.halves = half_waves(&parse(&bytes)?);
        self.media = Some(media);
        self.rewind();
        self.playing = true;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.halves.clear();
        self.rewind();
        self.playing = false;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        if self.media.is_none() {
            return Err(MediaError::NoMedia);
        }
        match command {
            "play" => self.playing = true,
            "stop" => self.playing = false,
            "rewind" => self.rewind(),
            other => {
                return Err(MediaError::Control(format!(
                    "'{other}': the recorder knows play, stop, rewind"
                )))
            }
        }
        Ok(())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.motor && self.playing && !self.at_end(),
            error: false,
            track: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kcc(blocks: usize) -> Vec<u8> {
        (0..blocks * 128)
            .map(|i| (i / 128 + i % 128) as u8)
            .collect()
    }

    #[test]
    fn a_kcc_file_gets_its_block_numbers() {
        let b = parse(&kcc(3)).unwrap();
        let numbers: Vec<u8> = b.iter().map(|b| b.number).collect();
        assert_eq!(numbers, [1, 2, 0xff]);
        assert!(b[0].first && !b[1].first);
        assert!(parse(&[0; 100]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn a_tap_file_carries_its_block_numbers_and_may_hold_several_files() {
        let mut tap = TAP_MAGIC.to_vec();
        for n in [1u8, 0xff] {
            tap.push(n);
            tap.extend([n; 128]);
        }
        tap.extend(TAP_MAGIC);
        tap.push(1);
        tap.extend([7; 128]);
        let b = parse(&tap).unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!((b[1].number, b[1].data[5], b[1].first), (0xff, 0xff, false));
        assert!(b[2].first);
        tap.extend([1, 2, 3]);
        assert!(parse(&tap).is_err());
    }

    #[test]
    fn a_block_is_lead_separator_number_data_and_sum() {
        let blocks = [Block {
            number: 0x01,
            data: [0x80; 128],
            first: false,
        }];
        let h = half_waves(&blocks);
        assert!(h[..2 * LEAD].iter().all(|&k| k == ONE));
        let body = &h[2 * LEAD..];
        assert_eq!(body[..2], [SEPARATOR; 2]);
        // Block number 1: bit 0 set, lowest bit first.
        assert_eq!(body[2..6], [ONE, ONE, ZERO, ZERO]);
        assert_eq!(body[18..20], [SEPARATOR; 2]);
        // 130 bytes of 9 waves, then the run-out.
        assert_eq!(body.len(), 2 + 130 * 18 + 2 * LEAD);
        // The sum of 128 times $80 is 0.
        let sum = &body[2 + 129 * 18..][..16];
        assert!(sum.iter().all(|&k| k == ZERO));
    }

    #[test]
    fn the_tape_moves_only_with_play_and_motor() {
        let mut t = Tape {
            halves: vec![ONE, ONE, ZERO],
            playing: true,
            ..Default::default()
        };
        t.rewind();
        assert_eq!(t.run(10_000, false), 0);
        assert_eq!(t.run(751, true), 0);
        assert_eq!(t.run(1, true), 1);
        t.playing = false;
        assert_eq!(t.run(10_000, true), 0);
        t.playing = true;
        assert_eq!(t.run(752 + 368, true), 2);
        assert!(t.at_end());
        assert_eq!(t.run(10_000, true), 0);
    }
}
