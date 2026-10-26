//! Snapshot files: SNA (48K and 128K) and Z80 (versions 1, 2 and 3).
//!
//! **SNA** (Mirage Microdrive format): a 27-byte header — I, HL', DE', BC',
//! AF', HL, DE, BC, IY, IX, IFF2 (bit 2), R, AF, SP, IM, border — then the
//! 48 KB from `$4000`. The 48K file keeps PC on the stack (taken off as by
//! RETN). The 128K file adds PC, the `$7FFD` value, a TR-DOS flag and the
//! other five (or six) banks in ascending order; its first 48 KB are banks
//! 5, 2 and the paged one.
//!
//! **Z80** (Gerton Lunter's format): a 30-byte header (A, F, BC, HL, PC,
//! SP, I, R, flags with R bit 7, the border and "compressed", DE, the
//! alternate set, IY, IX, IFF1, IFF2, IM). Version 1 (PC ≠ 0) is a 48K
//! image, compressed with `ED ED n b` runs and ended by `00 ED ED 00`.
//! Versions 2 and 3 have an extra header (23, 54 or 55 bytes: PC, hardware
//! mode, `$7FFD`, AY registers, …) and then 16 KB pages, each with its
//! length (`$FFFF` = stored uncompressed) and number (48K: 8 = `$4000`, 4
//! = `$8000`, 5 = `$C000`; 128K: bank + 3).
//!
//! Sources: World of Spectrum, "Z80 file format" and "SNA file format"
//! descriptions (worldofspectrum.org/faq/reference/z80format.htm,
//! formats.htm). The implementation is this project's own.

use cpu_z80::Z80;
use emu_core::media::MediaError;

use crate::memory::BANK;

/// What a snapshot sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cpu: Z80,
    /// RAM banks (0–7) and their 16 KB.
    pub banks: Vec<(usize, Vec<u8>)>,
    /// Port $7FFD for a 128K snapshot.
    pub paging: Option<u8>,
    pub border: u8,
    /// AY: selected register and the 16 registers.
    pub ay: Option<(u8, [u8; 16])>,
    pub issue2: bool,
}

fn err(msg: &str) -> MediaError {
    MediaError::Format(format!("snapshot: {msg}"))
}

fn w16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn set_af(z: &mut Z80, v: u16) {
    [z.a, z.f] = v.to_be_bytes();
}

/// A snapshot file by its contents (SNA by its size, otherwise Z80).
pub fn parse(bytes: &[u8]) -> Result<Snapshot, MediaError> {
    match bytes.len() {
        49179 | 131103 | 147487 => sna(bytes),
        _ => z80(bytes),
    }
}

fn sna(b: &[u8]) -> Result<Snapshot, MediaError> {
    let mut z = Z80::new();
    z.i = b[0];
    z.hl_ = w16(b, 1);
    z.de_ = w16(b, 3);
    z.bc_ = w16(b, 5);
    z.af_ = w16(b, 7);
    z.set_hl(w16(b, 9));
    z.set_de(w16(b, 11));
    z.set_bc(w16(b, 13));
    z.iy = w16(b, 15);
    z.ix = w16(b, 17);
    z.iff2 = b[19] & 4 != 0;
    z.iff1 = z.iff2;
    z.r = b[20];
    set_af(&mut z, w16(b, 21));
    z.sp = w16(b, 23);
    z.im = b[25].min(2);
    let border = b[26] & 7;
    let ram = &b[27..27 + 3 * BANK];
    let lower = |k: usize| ram[k * BANK..(k + 1) * BANK].to_vec();
    if b.len() == 49179 {
        // PC on the stack.
        let at = |a: u16| ram[usize::from(a.wrapping_sub(0x4000)) % (3 * BANK)];
        z.pc = u16::from_le_bytes([at(z.sp), at(z.sp.wrapping_add(1))]);
        z.sp = z.sp.wrapping_add(2);
        return Ok(Snapshot {
            cpu: z,
            banks: vec![(5, lower(0)), (2, lower(1)), (0, lower(2))],
            paging: None,
            border,
            ay: None,
            issue2: false,
        });
    }
    let x = 27 + 3 * BANK;
    z.pc = w16(b, x);
    let paging = b[x + 2] & 0x3f;
    let current = usize::from(paging & 7);
    let mut banks = vec![(5, lower(0)), (2, lower(1)), (current, lower(2))];
    let mut at = x + 4;
    for n in 0..8 {
        if n == 5 || n == 2 || n == current {
            continue;
        }
        let page = b
            .get(at..at + BANK)
            .ok_or_else(|| err("128K SNA truncated"))?;
        banks.push((n, page.to_vec()));
        at += BANK;
    }
    Ok(Snapshot {
        cpu: z,
        banks,
        paging: Some(paging),
        border,
        ay: None,
        issue2: false,
    })
}

/// Expand `ED ED n b` runs from `src` into exactly `len` bytes (stopping
/// at the version 1 end marker).
fn unrle(src: &[u8], len: usize) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    while out.len() < len {
        if src.get(i..i + 2) == Some(&[0xed, 0xed]) {
            let (n, v) = match (src.get(i + 2), src.get(i + 3)) {
                (Some(&n), Some(&v)) => (n, v),
                _ => return Err(err("run truncated")),
            };
            out.extend(std::iter::repeat_n(v, usize::from(n)));
            i += 4;
        } else {
            let &v = src.get(i).ok_or_else(|| err("page truncated"))?;
            out.push(v);
            i += 1;
        }
    }
    if out.len() != len {
        return Err(err("run past the page"));
    }
    Ok(out)
}

fn z80(b: &[u8]) -> Result<Snapshot, MediaError> {
    if b.len() < 30 {
        return Err(err("not an SNA or Z80 file"));
    }
    let mut z = Z80::new();
    z.a = b[0];
    z.f = b[1];
    z.set_bc(w16(b, 2));
    z.set_hl(w16(b, 4));
    z.pc = w16(b, 6);
    z.sp = w16(b, 8);
    z.i = b[10];
    let flags = if b[12] == 0xff { 1 } else { b[12] };
    z.r = (b[11] & 0x7f) | (flags & 1) << 7;
    let border = flags >> 1 & 7;
    z.set_de(w16(b, 13));
    z.bc_ = w16(b, 15);
    z.de_ = w16(b, 17);
    z.hl_ = w16(b, 19);
    z.af_ = u16::from_be_bytes([b[21], b[22]]);
    z.iy = w16(b, 23);
    z.ix = w16(b, 25);
    z.iff1 = b[27] != 0;
    z.iff2 = b[28] != 0;
    z.im = (b[29] & 3).min(2);
    let issue2 = b[29] & 4 != 0;
    if z.pc != 0 {
        // Version 1: one 48 KB block.
        let ram = if flags & 0x20 != 0 {
            unrle(&b[30..], 3 * BANK)?
        } else {
            b.get(30..30 + 3 * BANK)
                .ok_or_else(|| err("48K image truncated"))?
                .to_vec()
        };
        let lower = |k: usize| ram[k * BANK..(k + 1) * BANK].to_vec();
        return Ok(Snapshot {
            cpu: z,
            banks: vec![(5, lower(0)), (2, lower(1)), (0, lower(2))],
            paging: None,
            border,
            ay: None,
            issue2,
        });
    }
    let extra = usize::from(*b.get(30).ok_or_else(|| err("header truncated"))?)
        | usize::from(*b.get(31).ok_or_else(|| err("header truncated"))?) << 8;
    if !matches!(extra, 23 | 54 | 55) || b.len() < 32 + extra {
        return Err(err("unknown Z80 version"));
    }
    z.pc = w16(b, 32);
    let hw = b[34];
    let v2 = extra == 23;
    let is128 = if v2 {
        match hw {
            0 | 1 => false,
            3 | 4 => true,
            _ => return Err(err(&format!("hardware mode {hw} not supported"))),
        }
    } else {
        match hw {
            0 | 1 | 3 => false,
            4..=6 | 12 => true,
            _ => return Err(err(&format!("hardware mode {hw} not supported"))),
        }
    };
    let paging = is128.then_some(b[35] & 0x3f);
    let ay = is128.then(|| {
        let mut regs = [0; 16];
        regs.copy_from_slice(&b[39..55]);
        (b[38], regs)
    });
    let mut banks = Vec::new();
    let mut at = 32 + extra;
    while at < b.len() {
        let len = usize::from(w16(b, at));
        let page = b
            .get(at + 2)
            .copied()
            .ok_or_else(|| err("page header truncated"))?;
        at += 3;
        let data = if len == 0xffff {
            let d = b.get(at..at + BANK).ok_or_else(|| err("page truncated"))?;
            at += BANK;
            d.to_vec()
        } else {
            let d = b.get(at..at + len).ok_or_else(|| err("page truncated"))?;
            at += len;
            unrle(d, BANK)?
        };
        let bank = if is128 {
            match page {
                3..=10 => usize::from(page - 3),
                _ => continue,
            }
        } else {
            match page {
                8 => 5,
                4 => 2,
                5 => 0,
                _ => continue,
            }
        };
        banks.push((bank, data));
    }
    Ok(Snapshot {
        cpu: z,
        banks,
        paging,
        border,
        ay,
        issue2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sna_48k_takes_pc_off_the_stack() {
        let mut f = vec![0u8; 49179];
        f[23..25].copy_from_slice(&0x8000u16.to_le_bytes());
        f[26] = 3;
        // $8000 is offset $4000 of the RAM image.
        f[27 + 0x4000] = 0x34;
        f[27 + 0x4001] = 0x12;
        let s = parse(&f).unwrap();
        assert_eq!(s.cpu.pc, 0x1234);
        assert_eq!(s.cpu.sp, 0x8002);
        assert_eq!(s.border, 3);
        assert_eq!(s.banks.len(), 3);
    }

    #[test]
    fn z80_v1_compressed() {
        let mut f = vec![0u8; 30];
        f[6] = 0x00;
        f[7] = 0x80; // PC $8000
        f[12] = 0x20 | 2 << 1; // compressed, border 2
                               // 48 KB of $AA as runs of 255 plus the rest.
        let mut left = 3 * BANK;
        while left > 0 {
            let n = left.min(255);
            f.extend_from_slice(&[0xed, 0xed, n as u8, 0xaa]);
            left -= n;
        }
        f.extend_from_slice(&[0, 0xed, 0xed, 0]);
        let s = parse(&f).unwrap();
        assert_eq!(s.cpu.pc, 0x8000);
        assert_eq!(s.border, 2);
        assert!(s.banks.iter().all(|(_, d)| d.iter().all(|&v| v == 0xaa)));
    }

    #[test]
    fn z80_v3_128k_pages() {
        let mut f = vec![0u8; 30 + 2 + 54];
        f[30] = 54;
        f[32] = 0x00;
        f[33] = 0x60; // PC $6000
        f[34] = 4; // 128K
        f[35] = 0x13;
        f[39 + 7] = 0x38; // AY mixer
                          // Page 3 (bank 0) uncompressed, page 8 (bank 5) as one run.
        f.extend_from_slice(&[0xff, 0xff, 3]);
        f.extend(std::iter::repeat_n(0x11u8, BANK));
        // 256 runs of 64 bytes = 16 KB.
        let run: Vec<u8> = std::iter::repeat_n([0xed, 0xed, 0x40, 0x22], 256)
            .flatten()
            .collect();
        f.extend_from_slice(&(run.len() as u16).to_le_bytes());
        f.push(8);
        f.extend_from_slice(&run);
        let s = parse(&f).unwrap();
        assert_eq!(s.cpu.pc, 0x6000);
        assert_eq!(s.paging, Some(0x13));
        assert_eq!(s.ay.unwrap().1[7], 0x38);
        assert_eq!(s.banks[0].0, 0);
        assert_eq!(s.banks[1].0, 5);
        assert!(s.banks[1].1.iter().all(|&v| v == 0x22));
        assert!(parse(&[0; 10]).is_err());
    }
}
