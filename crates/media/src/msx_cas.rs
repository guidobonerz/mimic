//! MSX cassette images (`.cas`): the bytes of a tape's blocks without
//! their timing.
//!
//! The file is a sequence of blocks, each introduced by the eight bytes
//! `1F A6 DE BA CC 13 7D 74` at a file offset that is a multiple of eight
//! (the block before is filled up with zeros); the bytes up to the next
//! such mark are what the tape carries after a header tone. A BASIC file
//! is two blocks or more: a file header of ten equal type bytes ($D3
//! tokenised BASIC, $D0 binary, $EA ASCII) and a six-character name, then
//! the data. Bytes before the first mark are ignored.
//!
//! Source: the format as MSX emulators and tools use it since fMSX,
//! written from memory of it (see `docs/sources.md`); MSX BASIC's own
//! `CSAVE` output, recorded into such an image, loads again.

use emu_core::media::MediaError;

/// The mark before each block.
pub const MARK: [u8; 8] = [0x1f, 0xa6, 0xde, 0xba, 0xcc, 0x13, 0x7d, 0x74];

/// The blocks of a CAS image.
pub fn blocks(bytes: &[u8]) -> Result<Vec<Vec<u8>>, MediaError> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if at % 8 == 0 && bytes[at..].starts_with(&MARK) {
            out.push(Vec::new());
            at += 8;
            continue;
        }
        if let Some(block) = out.last_mut() {
            block.push(bytes[at]);
        }
        at += 1;
    }
    if out.is_empty() && !bytes.is_empty() {
        return Err(MediaError::Format(
            "not an MSX CAS image: no block mark".into(),
        ));
    }
    Ok(out)
}

/// The CAS image of `blocks`.
pub fn image(blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for b in blocks {
        out.resize(out.len().next_multiple_of(8), 0);
        out.extend_from_slice(&MARK);
        out.extend_from_slice(b);
    }
    out
}

/// A block that opens a file: ten equal type bytes ($D3, $D0 or $EA) and
/// the name. The tape gives it a long header tone.
pub fn is_file_header(block: &[u8]) -> bool {
    block.len() >= 10
        && matches!(block[0], 0xd3 | 0xd0 | 0xea)
        && block[..10].iter().all(|&b| b == block[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip_with_padding() {
        let mut header = vec![0xd3; 10];
        header.extend_from_slice(b"TEST  ");
        let data = vec![1, 2, 3, 4, 5];
        let img = image(&[header.clone(), data.clone()]);
        assert_eq!(img.len(), 8 + 16 + 8 + 5);
        let back = blocks(&img).unwrap();
        assert_eq!(back, [header.clone(), data]);
        assert!(is_file_header(&back[0]));
        assert!(!is_file_header(&back[1]));
        // A mark off the eight-byte grid is data; padding stays with its block.
        let mut odd = image(&[vec![9; 3]]);
        odd.extend_from_slice(&MARK);
        let b = blocks(&odd).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].len(), 3 + 8);
        let padded = image(&[vec![7; 3], vec![8]]);
        assert_eq!(blocks(&padded).unwrap()[0], [7, 7, 7, 0, 0, 0, 0, 0]);
        assert!(blocks(b"hello").is_err());
        assert!(blocks(&[]).unwrap().is_empty());
    }
}
