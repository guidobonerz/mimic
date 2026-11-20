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
//! A BASIC program (CSAVE) whose image lacks the end mark behind the
//! program gets it when the tape goes in ([`mend_basic`]).
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
    mend_basic(&mut blocks);
    Ok(blocks)
}

/// The byte CSAVE's close call leaves behind the program.
const BASIC_END: u8 = 0x03;

/// Give BASIC programs the end mark their image lacks.
///
/// A CSAVE file starts with three bytes `$D3` (`$D7` with the list
/// protection), the name (8 bytes) and the program's length (2 bytes);
/// the program follows. BASIC closes the file with the data byte `$03`,
/// which lands right behind the program in the last block. After CLOAD
/// BASIC goes on reading its input from the tape, and it is this `$03`
/// (BRK) that closes the tape and gives the keyboard back; without it
/// BASIC uses up the block and CAOS waits for another one for ever.
/// Images made by tools (not recorded from a KC) often hold zeros there.
/// So a file without any `$03` behind its program gets one behind the
/// last byte that is not zero (text a tape may carry there to start the
/// program stays); in a new last block if there is no room.
///
/// Source: the user's BASIC ROM (CLOAD at `$DC43`, the input loop at
/// `$C5AE` with the end test at `$DF27`, CSAVE's close at `$DD5C`) and
/// CAOS 4.2 (the tape drivers at `$C72B` and `$C7A7`).
fn mend_basic(blocks: &mut Vec<Block>) {
    let mut start = 0;
    while start < blocks.len() {
        // A file ends with block $FF or before the next file's lead.
        let mut end = start + 1;
        while end < blocks.len() && blocks[end - 1].number != 0xff && !blocks[end].first {
            end += 1;
        }
        let head = &blocks[start].data;
        if (head[0] == 0xd3 || head[0] == 0xd7) && head[1] == head[0] && head[2] == head[0] {
            let len = usize::from(head[11]) | usize::from(head[12]) << 8;
            let total = (end - start) * 128;
            let tail = 13 + len;
            // The file's bytes from `tail` on, by position in the file.
            let at = |blocks: &[Block], i: usize| blocks[start + i / 128].data[i % 128];
            if tail <= total && !(tail..total).any(|i| at(blocks, i) == BASIC_END) {
                let mark = (tail..total)
                    .rev()
                    .find(|&i| at(blocks, i) != 0)
                    .map_or(tail, |i| i + 1);
                if mark < total {
                    blocks[start + mark / 128].data[mark % 128] = BASIC_END;
                } else {
                    // No room: the last block gets its number in the
                    // row, a new last block follows.
                    let last = &mut blocks[end - 1];
                    if last.number == 0xff {
                        last.number = (end - start) as u8;
                    }
                    let mut data = [0; 128];
                    data[0] = BASIC_END;
                    blocks.insert(
                        end,
                        Block {
                            number: 0xff,
                            data,
                            first: false,
                        },
                    );
                    end += 1;
                }
            }
        }
        start = end;
    }
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

    /// A CSAVE file of `blocks` blocks with a program of `len` bytes
    /// (all `$41`), `tail` behind it, then zeros.
    fn csave(blocks: usize, len: usize, tail: &[u8]) -> Vec<u8> {
        let mut data = vec![0xd3; 3];
        data.extend(b"TEST    ");
        data.extend((len as u16).to_le_bytes());
        data.extend(vec![0x41; len]);
        data.extend(tail);
        assert!(data.len() <= blocks * 128);
        data.resize(blocks * 128, 0);
        let mut tap = TAP_MAGIC.to_vec();
        for (i, block) in data.chunks(128).enumerate() {
            tap.push(if i + 1 == blocks { 0xff } else { i as u8 + 1 });
            tap.extend(block);
        }
        tap
    }

    #[test]
    fn a_basic_program_gets_the_end_mark_csave_writes() {
        // Zeros behind the program: the mark goes right behind it.
        let b = parse(&csave(2, 150, &[])).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].data[13 + 150 - 128..][..2], [BASIC_END, 0]);
        assert_eq!(b[1].data[13 + 150 - 128 - 1], 0x41);
        // A mark that is there stays alone, wherever it is.
        let b = parse(&csave(2, 150, b"RUN\r\x03")).unwrap();
        let tail = &b[1].data[13 + 150 - 128..];
        assert_eq!(tail[..6], *b"RUN\r\x03\0");
        assert_eq!(tail.iter().filter(|&&c| c == BASIC_END).count(), 1);
        // Text without a mark keeps its place, the mark follows it.
        let b = parse(&csave(2, 150, b"RUN\r")).unwrap();
        assert_eq!(b[1].data[13 + 150 - 128..][..6], *b"RUN\r\x03\0");
        // No room in the last block: a new last block.
        let b = parse(&csave(2, 2 * 128 - 13, &[])).unwrap();
        let numbers: Vec<u8> = b.iter().map(|b| b.number).collect();
        assert_eq!(numbers, [1, 2, 0xff]);
        assert_eq!(b[2].data[..2], [BASIC_END, 0]);
        assert!(!b[2].first);
        // A second file on the tape is found; a machine program is not
        // touched.
        let mut tap = csave(1, 20, &[]);
        tap.extend(&csave(1, 30, &[]));
        tap.extend(TAP_MAGIC);
        tap.push(0xff);
        tap.extend([0; 128]);
        let b = parse(&tap).unwrap();
        assert_eq!(b[0].data[33], BASIC_END);
        assert_eq!(b[1].data[43], BASIC_END);
        assert_eq!(b[2].data, [0; 128]);
        // A length beyond the file: left alone.
        let mut tap = csave(1, 20, &[]);
        tap[16 + 1 + 12] = 0x40;
        assert!(!parse(&tap).unwrap()[0].data.contains(&BASIC_END));
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
