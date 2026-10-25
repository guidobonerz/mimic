//! MFM from flux: IBM-style tracks (1581, PC and Atari ST disks) and Amiga
//! tracks.
//!
//! MFM writes each data bit as a clock and a data cell; the clock cell is
//! 1 only between two 0 data bits. IBM tracks mark their address fields
//! with `$A1` written with a missing clock (the cell pattern `$4489`);
//! this module aligns the byte grid at every such mark and hands the
//! bytes to [`media::mfm::decode_track`] (ID field C/H/R/N, data field,
//! CRC-CCITT). Amiga tracks use the same `$4489` sync word but split each
//! long into odd and even bits; [`media::adf::decode_track`] reads the
//! raw cells at any bit offset. Double density is a 2 µs cell at 300 rpm
//! (250 kbit/s), high density 1 µs.
//! Sources: WD1772 and µPD765 data sheets (MFM, address marks, CRC);
//! IBM "Diskette General Information Manual" track format; Amiga
//! Hardware Reference Manual chapter 8 "Disk"; Laurent Clévy, "The .ADF
//! (Amiga Disk File) format FAQ".

use media::adf;
use media::mfm::{self, MARK};

use super::{decode_bits, Bits, Flux};

/// The cell pattern of `$A1` with a missing clock.
pub const SYNC_A1: u16 = 0x4489;

/// The data byte in a 16-cell MFM word.
fn data_bits(word: u16) -> u8 {
    (0..8).fold(0u8, |b, i| b << 1 | ((word >> (14 - 2 * i)) & 1) as u8)
}

/// Bytes of an IBM MFM track: the byte grid restarts at every `$A1` mark
/// (flagged with [`MARK`]); cells before the first mark are dropped.
pub fn ibm_stream(bits: &Bits) -> Vec<u16> {
    let mut out = Vec::with_capacity(bits.bits.len() / 16 + 1);
    let (mut window, mut acc, mut n, mut aligned) = (0u16, 0u16, 0u8, false);
    for &b in &bits.bits {
        window = window << 1 | u16::from(b);
        if aligned {
            acc = acc << 1 | u16::from(b);
            n += 1;
            if n == 16 {
                out.push(if acc == SYNC_A1 {
                    MARK | 0xa1
                } else {
                    u16::from(data_bits(acc))
                });
                n = 0;
                acc = 0;
                continue;
            }
        }
        if window == SYNC_A1 {
            out.push(MARK | 0xa1);
            aligned = true;
            n = 0;
            acc = 0;
        }
    }
    out
}

/// MFM cells for a stream of bytes and marks (the reverse of
/// [`ibm_stream`]).
pub fn ibm_encode(stream: &[u16]) -> Vec<bool> {
    let mut out = Vec::with_capacity(stream.len() * 16);
    let mut prev = false;
    for &v in stream {
        if v & MARK != 0 {
            out.extend((0..16).rev().map(|i| SYNC_A1 >> i & 1 != 0));
            prev = true;
            continue;
        }
        for i in (0..8).rev() {
            let d = v >> i & 1 != 0;
            out.push(!prev && !d);
            out.push(d);
            prev = d;
        }
    }
    out
}

fn cell_ticks(flux: &Flux, cell_seconds: f64) -> f64 {
    let rpm = flux.rpm().unwrap_or(300.0);
    cell_seconds * f64::from(flux.sample_freq) * 300.0 / rpm
}

/// Double-density cell at 300 rpm.
pub const DD_CELL: f64 = 2e-6;
/// High-density cell at 300 rpm.
pub const HD_CELL: f64 = 1e-6;

/// The sectors with good CRCs on an IBM MFM track (cell at 300 rpm).
pub fn decode_ibm(flux: &Flux, cell_seconds: f64) -> Vec<mfm::Sector> {
    let bits = decode_bits(flux, cell_ticks(flux, cell_seconds));
    let mut sectors: Vec<mfm::Sector> = Vec::new();
    for s in mfm::decode_track(&ibm_stream(&bits)) {
        if !sectors
            .iter()
            .any(|o| (o.track, o.side, o.sector) == (s.track, s.side, s.sector))
        {
            sectors.push(s);
        }
    }
    sectors
}

/// The sectors with good checksums on an Amiga track (cell at 300 rpm).
pub fn decode_amiga(flux: &Flux, cell_seconds: f64) -> Vec<adf::Sector> {
    let bits = decode_bits(flux, cell_ticks(flux, cell_seconds));
    adf::decode_track(&bits.packed())
}

#[cfg(test)]
mod tests {
    use super::super::{encode_bits, unpack, Rng};
    use super::*;
    use media::D64;

    #[test]
    fn mark_and_bytes_round_trip() {
        let stream = [0x4e, 0x00, MARK | 0xa1, MARK | 0xa1, 0xfe, 0x12, 0x00];
        let bits = ibm_encode(&stream);
        assert_eq!(bits.len(), 7 * 16);
        let back = ibm_stream(&Bits {
            bits,
            index: vec![],
        });
        assert_eq!(back, [MARK | 0xa1, MARK | 0xa1, 0xfe, 0x12, 0x00]);
    }

    #[test]
    fn d81_track_from_flux() {
        let mut d = D64::blank_d81(b"MFM", *b"MF");
        d.sector_mut(5, 22).unwrap().fill(0x77);
        let stream = mfm::encode_track(&d, 4, 1);
        let mut rng = Rng::new(3);
        let flux = encode_bits(
            &ibm_encode(&stream),
            144.0,
            72_000_000,
            2,
            0.12,
            700,
            &mut rng,
        );
        let sectors = decode_ibm(&flux, DD_CELL);
        assert_eq!(sectors.len(), 10);
        let s = sectors.iter().find(|s| s.sector == 2).unwrap();
        assert_eq!((s.track, s.side), (4, 1));
        assert_eq!(&s.data[..256], &[0x77; 256]);
    }

    #[test]
    fn amiga_track_from_flux() {
        let img: Vec<u8> = (0..adf::ADF_SIZE)
            .map(|i| (i / 512) as u8 ^ i as u8)
            .collect();
        let bits = unpack(&adf::encode_track(&img, 33));
        let mut rng = Rng::new(9);
        let flux = encode_bits(&bits, 144.0, 72_000_000, 2, 0.12, 1234, &mut rng);
        let sectors = decode_amiga(&flux, DD_CELL);
        assert_eq!(sectors.len(), 11);
        assert!(sectors.iter().all(|s| s.track == 33));
        let s = &sectors[0];
        let off = (33 * 11 + usize::from(s.sector)) * 512;
        assert_eq!(s.data, img[off..off + 512]);
    }
}
