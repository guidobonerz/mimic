//! DMS (DISK-Masher System) archives: a compressed Amiga disk, unpacked
//! into a plain ADF image. Read-only: a DMS is never written back.
//!
//! Layout (big endian; the format as documented by the public-domain xDMS
//! unpacker's description of it, see `docs/sources.md`):
//!
//! - a 56-byte file header: `DMS!`, four unused bytes, `u32` info bits
//!   (bit 1: encrypted — not supported), `u32` date, `u16` first and last
//!   track, `u32` packed and unpacked sizes, machine and version words,
//!   `u16` disk type and a `u16` CRC over bytes 4–53;
//! - then tracks, each with a 20-byte header: `TR`, `u16` track number,
//!   `u16` unused, `u16` packed length, `u16` length after the LZ stage
//!   (before RLE), `u16` unpacked length, `u8` flags, `u8` mode, `u16`
//!   checksum of the unpacked bytes (their sum), `u16` CRC of the packed
//!   bytes, `u16` CRC of the header's first 18 bytes; then the packed bytes.
//!
//! Track numbers 0–79 are cylinders (11 264 bytes: both sides); track 80
//! holds a FILE_ID.DIZ text and $FFFF a banner, both skipped, as is a
//! 1024-byte "track 0" that only carries an advertisement.
//!
//! Modes: 0 NOCOMP (stored), 1 SIMPLE (RLE), 2 QUICK, 3 MEDIUM, 4 DEEP and
//! 5/6 HEAVY1/2 (an LZ stage, then RLE; HEAVY applies RLE only with flag
//! bit 2). Flags: bit 0 — the LZ state (windows, write positions, the
//! adaptive tree) carries over to the next track (else it starts afresh
//! after this one), bit 1 — HEAVY: new Huffman trees precede the data
//! (else the previous ones stay in use), bit 2 — HEAVY: RLE follows. RLE: `$90, 0` is a literal `$90`,
//! `$90, n, b` repeats `b` n times, `$90, $FF, b, nn` with a 16-bit count.
//! QUICK: a 256-byte window, one bit literal/match, 8-bit literals, 2-bit
//! length (+2) and 8-bit distance. MEDIUM: a 16 KB window, matches coded
//! with Okumura's LZHUF position tables. DEEP: LZHUF, the adaptive Huffman
//! coder with a 16 KB window and 60-byte matches. HEAVY: LHA-style static
//! Huffman trees (510 symbols, 5-bit code lengths; positions with 4-bit
//! lengths; the highest position code repeats the previous position), a
//! 4 KB (HEAVY1) or 8 KB (HEAVY2) window. CRCs are CRC-16/ARC.
//! Every track's checksum is verified, so a decoding error is reported
//! rather than silently producing a wrong disk.

use emu_core::media::MediaError;

use crate::adf;

pub const SIGNATURE: &[u8; 4] = b"DMS!";
const HEADER: usize = 56;
const TRACK_HEADER: usize = 20;

/// Bytes of one cylinder (both sides) on a double-density disk.
const CYLINDER_DD: usize = 2 * adf::SECTORS * adf::SECTOR_SIZE;

pub fn is_dms(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER && &bytes[..4] == SIGNATURE
}

/// CRC-16/ARC (reflected polynomial $A001) of `data`.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xa001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn err(msg: impl Into<String>) -> MediaError {
    MediaError::Format(msg.into())
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}

/// MSB-first bit reader (reads zeros past the end).
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    count: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut b = Self {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        };
        b.fill();
        b
    }

    fn fill(&mut self) {
        while self.count <= 24 {
            let byte = self.data.get(self.pos).copied().unwrap_or(0);
            self.pos += 1;
            self.buf |= u32::from(byte) << (24 - self.count);
            self.count += 8;
        }
    }

    /// The next `n` bits (n ≤ 16) without consuming them.
    fn peek(&self, n: u32) -> u16 {
        if n == 0 {
            return 0;
        }
        (self.buf >> (32 - n)) as u16
    }

    fn drop(&mut self, n: u32) {
        self.buf <<= n;
        self.count -= n;
        self.fill();
    }

    fn get(&mut self, n: u32) -> u16 {
        let v = self.peek(n);
        self.drop(n);
        v
    }

    fn bit(&mut self) -> bool {
        self.get(1) != 0
    }
}

fn unpack_rle(input: &[u8], len: usize) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    let next = |i: &mut usize| -> Result<u8, MediaError> {
        let b = *input.get(*i).ok_or_else(|| err("DMS RLE data truncated"))?;
        *i += 1;
        Ok(b)
    };
    while out.len() < len {
        let a = next(&mut i)?;
        if a != 0x90 {
            out.push(a);
            continue;
        }
        let b = next(&mut i)?;
        if b == 0 {
            out.push(0x90);
            continue;
        }
        let v = next(&mut i)?;
        let n = if b == 0xff {
            let hi = next(&mut i)?;
            let lo = next(&mut i)?;
            usize::from(u16::from_be_bytes([hi, lo]))
        } else {
            usize::from(b)
        };
        if out.len() + n > len {
            return Err(err("DMS RLE run past the track"));
        }
        out.extend(std::iter::repeat_n(v, n));
    }
    Ok(out)
}

/// Okumura's LZHUF position tables: the high part of a position from a
/// byte, and the extra bits that follow.
fn d_code(c: u8) -> u8 {
    const RUNS: [(u8, u8); 6] = [(32, 1), (16, 3), (8, 8), (4, 12), (2, 24), (1, 16)];
    let mut i = u16::from(c);
    let mut code = 0u8;
    for (width, count) in RUNS {
        let span = u16::from(width) * u16::from(count);
        if i < span {
            return code + (i / u16::from(width)) as u8;
        }
        i -= span;
        code += count;
    }
    0
}

fn d_len(c: u8) -> u32 {
    match c {
        0..=31 => 3,
        32..=79 => 4,
        80..=143 => 5,
        144..=191 => 6,
        192..=239 => 7,
        _ => 8,
    }
}

/// State the LZ stages keep between tracks.
pub struct Decruncher {
    text: Vec<u8>,
    quick_loc: usize,
    medium_loc: usize,
    deep_loc: usize,
    heavy_loc: usize,
    deep: Lzhuf,
    heavy: Heavy,
}

impl Default for Decruncher {
    fn default() -> Self {
        Self::new()
    }
}

impl Decruncher {
    pub fn new() -> Self {
        let mut d = Self {
            text: Vec::new(),
            quick_loc: 0,
            medium_loc: 0,
            deep_loc: 0,
            heavy_loc: 0,
            deep: Lzhuf::new(),
            heavy: Heavy::new(),
        };
        d.reset();
        d
    }

    /// The state a fresh archive starts with (and a track whose flag bit
    /// 0 is clear leaves behind): the windows cleared, the write positions
    /// at the values the cruncher used, the adaptive tree new. The HEAVY
    /// trees stay: a later track may reuse them.
    fn reset(&mut self) {
        self.text = vec![0; 0x4000];
        self.quick_loc = 251;
        self.medium_loc = 0x3fbe;
        self.deep_loc = 0x3fc4;
        self.heavy_loc = 0;
        self.deep = Lzhuf::new();
    }

    fn quick(&mut self, input: &[u8], len: usize) -> Vec<u8> {
        let mut bits = Bits::new(input);
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            if bits.bit() {
                let b = bits.get(8) as u8;
                self.text[self.quick_loc & 0xff] = b;
                self.quick_loc += 1;
                out.push(b);
            } else {
                let n = usize::from(bits.get(2)) + 2;
                let from = self.quick_loc.wrapping_sub(usize::from(bits.get(8)) + 1);
                for i in (from..).take(n) {
                    let b = self.text[i & 0xff];
                    self.text[self.quick_loc & 0xff] = b;
                    self.quick_loc += 1;
                    out.push(b);
                }
            }
        }
        self.quick_loc = (self.quick_loc + 5) & 0xff;
        out.truncate(len);
        out
    }

    fn medium(&mut self, input: &[u8], len: usize) -> Vec<u8> {
        let mut bits = Bits::new(input);
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            if bits.bit() {
                let b = bits.get(8) as u8;
                self.text[self.medium_loc & 0x3fff] = b;
                self.medium_loc += 1;
                out.push(b);
            } else {
                let c = bits.get(8) as u8;
                let n = usize::from(d_code(c)) + 3;
                let u = d_len(c);
                let c = ((u16::from(c) << u) | bits.get(u)) as u8;
                let u = d_len(c);
                let pos =
                    (u16::from(d_code(c)) << 8) | (((u16::from(c) << u) | bits.get(u)) & 0xff);
                let from = self.medium_loc.wrapping_sub(usize::from(pos) + 1);
                for i in (from..).take(n) {
                    let b = self.text[i & 0x3fff];
                    self.text[self.medium_loc & 0x3fff] = b;
                    self.medium_loc += 1;
                    out.push(b);
                }
            }
        }
        self.medium_loc = (self.medium_loc + 66) & 0x3fff;
        out.truncate(len);
        out
    }

    fn deep(&mut self, input: &[u8], len: usize) -> Vec<u8> {
        let mut bits = Bits::new(input);
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let c = self.deep.decode_char(&mut bits);
            if c < 256 {
                let b = c as u8;
                self.text[self.deep_loc & 0x3fff] = b;
                self.deep_loc += 1;
                out.push(b);
            } else {
                let n = usize::from(c) - 255 + LZHUF_THRESHOLD;
                let pos = self.deep.decode_position(&mut bits);
                let from = self.deep_loc.wrapping_sub(usize::from(pos) + 1);
                for i in (from..).take(n) {
                    let b = self.text[i & 0x3fff];
                    self.text[self.deep_loc & 0x3fff] = b;
                    self.deep_loc += 1;
                    out.push(b);
                }
            }
        }
        self.deep_loc = (self.deep_loc + 60) & 0x3fff;
        out.truncate(len);
        out
    }

    fn heavy(&mut self, input: &[u8], len: usize, flags: u8) -> Result<Vec<u8>, MediaError> {
        let (np, mask) = if flags & 8 != 0 {
            (15, 0x1fff)
        } else {
            (14, 0x0fff)
        };
        self.heavy.np = np;
        let mut bits = Bits::new(input);
        if flags & 2 != 0 {
            self.heavy.read_tree_c(&mut bits)?;
            self.heavy.read_tree_p(&mut bits)?;
        }
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let c = self.heavy.decode_c(&mut bits);
            if c < 256 {
                let b = c as u8;
                self.text[self.heavy_loc & mask] = b;
                self.heavy_loc += 1;
                out.push(b);
            } else {
                let n = usize::from(c) - 253;
                let pos = self.heavy.decode_p(&mut bits);
                let from = self.heavy_loc.wrapping_sub(usize::from(pos) + 1);
                for i in (from..).take(n) {
                    let b = self.text[i & mask];
                    self.text[self.heavy_loc & mask] = b;
                    self.heavy_loc += 1;
                    out.push(b);
                }
            }
        }
        out.truncate(len);
        Ok(out)
    }

    /// Unpack one track's data (`mode`, `flags` from its header) to
    /// `unpacked` bytes; `lz_len` is the length after the LZ stage.
    pub fn track(
        &mut self,
        packed: &[u8],
        mode: u8,
        flags: u8,
        lz_len: usize,
        unpacked: usize,
    ) -> Result<Vec<u8>, MediaError> {
        let out = match mode {
            0 => packed
                .get(..unpacked)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| err("DMS stored track short"))?,
            1 => unpack_rle(packed, unpacked)?,
            2 => unpack_rle(&self.quick(packed, lz_len), unpacked)?,
            3 => unpack_rle(&self.medium(packed, lz_len), unpacked)?,
            4 => unpack_rle(&self.deep(packed, lz_len), unpacked)?,
            5 | 6 => {
                let f = if mode == 6 { flags | 8 } else { flags & 7 };
                let lz = self.heavy(packed, lz_len, f)?;
                if flags & 4 != 0 {
                    unpack_rle(&lz, unpacked)?
                } else {
                    lz
                }
            }
            m => return Err(err(format!("DMS compression mode {m}"))),
        };
        if flags & 1 == 0 {
            self.reset();
        }
        Ok(out)
    }
}

// ----- LZHUF (DEEP): adaptive Huffman coding after Okumura.

const LZHUF_F: usize = 60;
const LZHUF_THRESHOLD: usize = 2;
const LZHUF_N_CHAR: usize = 256 - LZHUF_THRESHOLD + LZHUF_F;
const LZHUF_T: usize = LZHUF_N_CHAR * 2 - 1;
const LZHUF_R: usize = LZHUF_T - 1;
const LZHUF_MAX_FREQ: u16 = 0x8000;

struct Lzhuf {
    freq: Vec<u16>,
    prnt: Vec<u16>,
    son: Vec<u16>,
}

impl Lzhuf {
    fn new() -> Self {
        let mut h = Self {
            freq: vec![0; LZHUF_T + 1],
            prnt: vec![0; LZHUF_T + LZHUF_N_CHAR],
            son: vec![0; LZHUF_T],
        };
        for i in 0..LZHUF_N_CHAR {
            h.freq[i] = 1;
            h.son[i] = (i + LZHUF_T) as u16;
            h.prnt[i + LZHUF_T] = i as u16;
        }
        let (mut i, mut j) = (0, LZHUF_N_CHAR);
        while j <= LZHUF_R {
            h.freq[j] = h.freq[i] + h.freq[i + 1];
            h.son[j] = i as u16;
            h.prnt[i] = j as u16;
            h.prnt[i + 1] = j as u16;
            i += 2;
            j += 1;
        }
        h.freq[LZHUF_T] = 0xffff;
        h.prnt[LZHUF_R] = 0;
        h
    }

    fn reconstruct(&mut self) {
        let mut j = 0;
        for i in 0..LZHUF_T {
            if usize::from(self.son[i]) >= LZHUF_T {
                self.freq[j] = self.freq[i].div_ceil(2);
                self.son[j] = self.son[i];
                j += 1;
            }
        }
        let (mut i, mut j) = (0, LZHUF_N_CHAR);
        while j < LZHUF_T {
            let k = i + 1;
            let f = self.freq[i] + self.freq[k];
            self.freq[j] = f;
            let mut k = j - 1;
            while f < self.freq[k] {
                k -= 1;
            }
            k += 1;
            self.freq.copy_within(k..j, k + 1);
            self.freq[k] = f;
            self.son.copy_within(k..j, k + 1);
            self.son[k] = i as u16;
            i += 2;
            j += 1;
        }
        for i in 0..LZHUF_T {
            let k = usize::from(self.son[i]);
            if k >= LZHUF_T {
                self.prnt[k] = i as u16;
            } else {
                self.prnt[k] = i as u16;
                self.prnt[k + 1] = i as u16;
            }
        }
    }

    fn update(&mut self, c: usize) {
        if self.freq[LZHUF_R] == LZHUF_MAX_FREQ {
            self.reconstruct();
        }
        let mut c = usize::from(self.prnt[c + LZHUF_T]);
        loop {
            self.freq[c] += 1;
            let k = self.freq[c];
            let mut l = c + 1;
            if k > self.freq[l] {
                while k > self.freq[l + 1] {
                    l += 1;
                }
                self.freq[c] = self.freq[l];
                self.freq[l] = k;
                let i = usize::from(self.son[c]);
                self.prnt[i] = l as u16;
                if i < LZHUF_T {
                    self.prnt[i + 1] = l as u16;
                }
                let j = usize::from(self.son[l]);
                self.son[l] = i as u16;
                self.prnt[j] = c as u16;
                if j < LZHUF_T {
                    self.prnt[j + 1] = c as u16;
                }
                self.son[c] = j as u16;
                c = l;
            }
            c = usize::from(self.prnt[c]);
            if c == 0 {
                break;
            }
        }
    }

    fn decode_char(&mut self, bits: &mut Bits<'_>) -> u16 {
        let mut c = usize::from(self.son[LZHUF_R]);
        while c < LZHUF_T {
            c += usize::from(bits.bit());
            c = usize::from(self.son[c]);
        }
        let c = c - LZHUF_T;
        self.update(c);
        c as u16
    }

    fn decode_position(&mut self, bits: &mut Bits<'_>) -> u16 {
        let i = bits.get(8) as u8;
        let c = u16::from(d_code(i)) << 8;
        let j = d_len(i);
        let i = ((u16::from(i) << j) | bits.get(j)) & 0xff;
        c | i
    }
}

// ----- HEAVY: LHA-style static Huffman trees.

const HEAVY_NC: usize = 510;
const HEAVY_NPT: usize = 15;

struct Heavy {
    np: usize,
    /// The last position: code `np - 1` repeats it.
    last_pos: u16,
    c_len: [u8; HEAVY_NC],
    pt_len: [u8; HEAVY_NPT],
    c_table: Vec<u16>,
    pt_table: Vec<u16>,
    left: Vec<u16>,
    right: Vec<u16>,
}

impl Heavy {
    fn new() -> Self {
        Self {
            np: 14,
            last_pos: 0,
            c_len: [0; HEAVY_NC],
            pt_len: [0; HEAVY_NPT],
            c_table: vec![0; 4096],
            pt_table: vec![0; 256],
            left: vec![0; 2 * HEAVY_NC - 1],
            right: vec![0; 2 * HEAVY_NC - 1],
        }
    }

    fn read_tree_c(&mut self, bits: &mut Bits<'_>) -> Result<(), MediaError> {
        let n = usize::from(bits.get(9));
        if n > 0 {
            if n > HEAVY_NC {
                return Err(err("DMS HEAVY code table"));
            }
            for i in 0..n {
                self.c_len[i] = bits.get(5) as u8;
            }
            for i in n..HEAVY_NC {
                self.c_len[i] = 0;
            }
            let (table, left, right) =
                make_table(HEAVY_NC, &self.c_len, 12).ok_or_else(|| err("DMS HEAVY code table"))?;
            self.c_table = table;
            self.left = left;
            self.right = right;
        } else {
            let n = bits.get(9);
            self.c_len = [0; HEAVY_NC];
            self.c_table.fill(n);
        }
        Ok(())
    }

    fn read_tree_p(&mut self, bits: &mut Bits<'_>) -> Result<(), MediaError> {
        let n = usize::from(bits.get(5));
        if n > 0 {
            if n > self.np {
                return Err(err("DMS HEAVY position table"));
            }
            for i in 0..n {
                self.pt_len[i] = bits.get(4) as u8;
            }
            for i in n..self.np {
                self.pt_len[i] = 0;
            }
            let (table, left, right) = make_table(self.np, &self.pt_len[..self.np], 8)
                .ok_or_else(|| err("DMS HEAVY position table"))?;
            self.pt_table = table;
            // The position tree shares the node arrays above the code
            // tree's leaves as LHA does; the tables are small, so keep
            // separate copies merged by index range.
            for (i, (l, r)) in left.iter().zip(&right).enumerate() {
                if i >= self.np {
                    self.left[i] = *l;
                    self.right[i] = *r;
                }
            }
        } else {
            let n = bits.get(5);
            self.pt_len = [0; HEAVY_NPT];
            self.pt_table.fill(n);
        }
        Ok(())
    }

    fn decode_c(&self, bits: &mut Bits<'_>) -> u16 {
        let mut j = self.c_table[usize::from(bits.peek(12))];
        if usize::from(j) < HEAVY_NC {
            bits.drop(u32::from(self.c_len[usize::from(j)]));
        } else {
            bits.drop(12);
            let i = bits.peek(16);
            let mut m = 0x8000;
            while usize::from(j) >= HEAVY_NC {
                j = if i & m != 0 {
                    self.right[usize::from(j)]
                } else {
                    self.left[usize::from(j)]
                };
                m >>= 1;
            }
            bits.drop(u32::from(self.c_len[usize::from(j)]) - 12);
        }
        j
    }

    fn decode_p(&mut self, bits: &mut Bits<'_>) -> u16 {
        let mut j = self.pt_table[usize::from(bits.peek(8))];
        if usize::from(j) < self.np {
            bits.drop(u32::from(self.pt_len[usize::from(j)]));
        } else {
            bits.drop(8);
            let i = bits.peek(16);
            let mut m = 0x8000;
            while usize::from(j) >= self.np {
                j = if i & m != 0 {
                    self.right[usize::from(j)]
                } else {
                    self.left[usize::from(j)]
                };
                m >>= 1;
            }
            bits.drop(u32::from(self.pt_len[usize::from(j)]) - 8);
        }
        if usize::from(j) != self.np - 1 {
            self.last_pos = if j == 0 {
                0
            } else {
                let extra = u32::from(j) - 1;
                (1 << extra) | bits.get(extra)
            };
        }
        self.last_pos
    }
}

/// A canonical-Huffman table builder in the manner of LHA's: a lookup
/// table of `tablebits` bits for the codes no longer than that, and a
/// binary tree (left/right, nodes numbered from `nchar`) for the longer
/// ones. Codes are assigned in order of length, then symbol; lengths up
/// to 31 bits are allowed (the cruncher writes 5-bit lengths). `None` if
/// the lengths do not form a complete prefix code.
fn make_table(
    nchar: usize,
    bitlen: &[u8],
    tablebits: u32,
) -> Option<(Vec<u16>, Vec<u16>, Vec<u16>)> {
    const CODE_BITS: u32 = 32;
    let mut count = [0u64; 33];
    for &l in &bitlen[..nchar] {
        if l > 31 {
            return None;
        }
        count[usize::from(l)] += 1;
    }
    let mut start = [0u64; 34];
    for i in 1..=32 {
        start[i + 1] = start[i] + (count[i] << (CODE_BITS - i as u32));
    }
    if start[33] != 1 << CODE_BITS {
        return None;
    }
    let jutbits = CODE_BITS - tablebits;
    let mut weight = [0u64; 33];
    for i in 1..=tablebits as usize {
        start[i] >>= jutbits;
        weight[i] = 1 << (tablebits as usize - i);
    }
    for (i, w) in weight.iter_mut().enumerate().skip(tablebits as usize + 1) {
        *w = 1 << (CODE_BITS as usize - i);
    }
    let size = 1usize << tablebits;
    let mut table = vec![0u16; size];
    let mut left = vec![0u16; 2 * nchar - 1];
    let mut right = vec![0u16; 2 * nchar - 1];
    let mut avail = nchar;
    let mask = 1u64 << (CODE_BITS - 1 - tablebits);
    for (ch, &bits) in bitlen.iter().enumerate().take(nchar) {
        let len = usize::from(bits);
        if len == 0 {
            continue;
        }
        let nextcode = start[len] + weight[len];
        if len <= tablebits as usize {
            for i in start[len]..nextcode {
                *table.get_mut(i as usize)? = ch as u16;
            }
        } else {
            let mut k = start[len];
            let mut p = Slot::Table((k >> jutbits) as usize);
            for _ in 0..len - tablebits as usize {
                let node = match p {
                    Slot::Table(i) => &mut table[i],
                    Slot::Left(i) => &mut left[i],
                    Slot::Right(i) => &mut right[i],
                };
                if *node == 0 {
                    if avail >= 2 * nchar - 1 {
                        return None;
                    }
                    *node = avail as u16;
                    avail += 1;
                }
                let n = usize::from(*node);
                p = if k & mask != 0 {
                    Slot::Right(n)
                } else {
                    Slot::Left(n)
                };
                k <<= 1;
            }
            match p {
                Slot::Table(i) => table[i] = ch as u16,
                Slot::Left(i) => left[i] = ch as u16,
                Slot::Right(i) => right[i] = ch as u16,
            }
        }
        start[len] = nextcode;
    }
    Some((table, left, right))
}

enum Slot {
    Table(usize),
    Left(usize),
    Right(usize),
}

/// Unpack a DMS archive into an ADF image.
pub fn unpack(bytes: &[u8]) -> Result<Vec<u8>, MediaError> {
    if !is_dms(bytes) {
        return Err(err("not a DMS archive"));
    }
    if crc16(&bytes[4..HEADER - 2]) != u16_at(bytes, HEADER - 2) {
        return Err(err("DMS header CRC"));
    }
    let info = u16_at(bytes, 10);
    if info & 2 != 0 {
        return Err(err("encrypted DMS archives are not supported"));
    }
    let mut image = vec![0u8; adf::ADF_SIZE];
    let mut cylinder = CYLINDER_DD;
    let mut dec = Decruncher::new();
    let mut pos = HEADER;
    let mut got = 0;
    while pos + TRACK_HEADER <= bytes.len() {
        let h = &bytes[pos..pos + TRACK_HEADER];
        if &h[..2] != b"TR" {
            return Err(err(format!("DMS track header at {pos}")));
        }
        if crc16(&h[..18]) != u16_at(h, 18) {
            return Err(err(format!("DMS track header CRC at {pos}")));
        }
        let number = u16_at(h, 2);
        let packed_len = usize::from(u16_at(h, 6));
        let lz_len = usize::from(u16_at(h, 8));
        let unpacked = usize::from(u16_at(h, 10));
        let flags = h[12];
        let mode = h[13];
        let usum = u16_at(h, 14);
        let dcrc = u16_at(h, 16);
        let packed = bytes
            .get(pos + TRACK_HEADER..pos + TRACK_HEADER + packed_len)
            .ok_or_else(|| err("DMS track data truncated"))?;
        pos += TRACK_HEADER + packed_len;
        if crc16(packed) != dcrc {
            return Err(err(format!("DMS track {number}: data CRC")));
        }
        // FILE_ID.DIZ, the banner and a 1024-byte advertisement "track 0"
        // are no disk data.
        if number >= 80 || unpacked <= 2048 {
            continue;
        }
        let data = dec.track(packed, mode, flags, lz_len, unpacked)?;
        let sum = data.iter().fold(0u16, |a, &b| a.wrapping_add(u16::from(b)));
        if sum != usum {
            return Err(err(format!("DMS track {number}: checksum (mode {mode})")));
        }
        if data.len() == 2 * CYLINDER_DD && cylinder == CYLINDER_DD && got == 0 {
            // High density.
            image = vec![0u8; adf::ADF_HD_SIZE];
            cylinder = 2 * CYLINDER_DD;
        }
        if data.len() != cylinder {
            return Err(err(format!("DMS track {number}: {} bytes", data.len())));
        }
        let off = usize::from(number) * cylinder;
        image
            .get_mut(off..off + cylinder)
            .ok_or_else(|| err(format!("DMS track {number} out of range")))?
            .copy_from_slice(&data);
        got += 1;
    }
    if got == 0 {
        return Err(err("DMS archive without disk tracks"));
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RLE-pack like the DMS cruncher's SIMPLE mode.
    fn pack_rle(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            let mut n = 1;
            while i + n < data.len() && data[i + n] == b && n < 0xfffe {
                n += 1;
            }
            if b == 0x90 {
                out.extend([0x90, 0]);
                i += 1;
            } else if n >= 4 {
                if n > 254 {
                    out.extend([0x90, 0xff, b, (n >> 8) as u8, n as u8]);
                } else {
                    out.extend([0x90, n as u8, b]);
                }
                i += n;
            } else {
                out.push(b);
                i += 1;
            }
        }
        out
    }

    /// (track number, mode, flags, packed bytes, LZ length, unpacked length).
    type TrackSpec = (u16, u8, u8, Vec<u8>, usize, usize);

    fn header(tracks: &[TrackSpec]) -> Vec<u8> {
        let mut file = b"DMS!".to_vec();
        file.extend([0; 52]);
        let crc = crc16(&file[4..54]);
        file[54..56].copy_from_slice(&crc.to_be_bytes());
        for (number, mode, flags, packed, lz_len, data_len) in tracks {
            let mut h = b"TR".to_vec();
            h.extend(number.to_be_bytes());
            h.extend(0u16.to_be_bytes());
            h.extend((packed.len() as u16).to_be_bytes());
            h.extend((*lz_len as u16).to_be_bytes());
            h.extend((*data_len as u16).to_be_bytes());
            h.push(*flags);
            h.push(*mode);
            h.extend(0u16.to_be_bytes()); // usum filled by the caller
            h.extend(crc16(packed).to_be_bytes());
            h.extend(0u16.to_be_bytes()); // hcrc filled by the caller
            file.extend(h);
            file.extend(packed);
        }
        file
    }

    /// Fix usum and header CRC of every track in a built file.
    fn fix_sums(file: &mut [u8], sums: &[u16]) {
        let mut pos = HEADER;
        for s in sums {
            file[pos + 14..pos + 16].copy_from_slice(&s.to_be_bytes());
            let crc = crc16(&file[pos..pos + 18]);
            file[pos + 18..pos + 20].copy_from_slice(&crc.to_be_bytes());
            let len = usize::from(u16_at(file, pos + 6));
            pos += TRACK_HEADER + len;
        }
    }

    fn sum(d: &[u8]) -> u16 {
        d.iter().fold(0u16, |a, &b| a.wrapping_add(u16::from(b)))
    }

    #[test]
    fn crc16_arc() {
        assert_eq!(crc16(b"123456789"), 0xbb3d);
    }

    #[test]
    fn stored_and_rle_tracks_make_a_disk() {
        let t0: Vec<u8> = (0..CYLINDER_DD).map(|i| (i % 7) as u8).collect();
        let mut t1 = vec![0x90u8; CYLINDER_DD];
        t1[100..].fill(0x33);
        let packed1 = pack_rle(&t1);
        let banner = b"hello".to_vec();
        let mut file = header(&[
            (0, 0, 0, t0.clone(), CYLINDER_DD, CYLINDER_DD),
            (1, 1, 0, packed1.clone(), packed1.len(), CYLINDER_DD),
            (0xffff, 0, 0, banner.clone(), 5, 5),
        ]);
        fix_sums(&mut file, &[sum(&t0), sum(&t1), sum(&banner)]);
        let img = unpack(&file).unwrap();
        assert_eq!(img.len(), adf::ADF_SIZE);
        assert_eq!(img[..CYLINDER_DD], t0[..]);
        assert_eq!(img[CYLINDER_DD..2 * CYLINDER_DD], t1[..]);
        // A wrong checksum is an error, not a silent disk.
        let mut bad = file.clone();
        fix_sums(
            &mut bad,
            &[sum(&t0).wrapping_add(1), sum(&t1), sum(&banner)],
        );
        assert!(unpack(&bad).is_err());
        bad = file.clone();
        bad[HEADER + 20] ^= 1;
        assert!(matches!(unpack(&bad), Err(MediaError::Format(m)) if m.contains("data CRC")));
        assert!(unpack(b"nope").is_err());
    }

    #[test]
    fn quick_mode_decodes_literals_and_matches() {
        // Bits: literal 'A' (1 + 8), literal 'B', match len 4 (0 + 2 bits
        // "10") distance 1 (8 bits 00000001): A B B A B A? distance counts
        // back from the write position: i = loc - dist - 1.
        let mut bits: Vec<bool> = Vec::new();
        let mut push = |v: u32, n: u32| {
            for i in (0..n).rev() {
                bits.push(v >> i & 1 != 0);
            }
        };
        push(1, 1);
        push(b'A' as u32, 8);
        push(1, 1);
        push(b'B' as u32, 8);
        push(0, 1);
        push(2, 2); // length 4
        push(1, 8); // i = 2 - 1 - 1 = 0 → copies from "AB"
        let mut packed = vec![0u8; bits.len().div_ceil(8) + 2];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                packed[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let mut d = Decruncher::new();
        let out = d.quick(&packed, 6);
        assert_eq!(out, b"ABABAB");
    }

    #[test]
    fn deep_mode_walks_the_initial_tree() {
        // With all frequencies 1 the all-zero path from the root reaches
        // leaf 116 (the pairing order of LZHUF's start-up tree); after the
        // update the tree changes, so a run of zero bits yields a
        // deterministic but varying sequence.
        let packed = vec![0u8; 64];
        let mut d = Decruncher::new();
        let out = d.deep(&packed, 20);
        assert_eq!(out.len(), 20);
        assert_eq!(out[0], 116);
        let mut again = Decruncher::new();
        assert_eq!(again.deep(&packed, 20), out);
    }

    #[test]
    fn heavy_tables_follow_lha() {
        // Two symbols of length 1: a complete code.
        let lens = [1u8, 1];
        let (table, _, _) = make_table(2, &lens, 8).unwrap();
        assert_eq!(table[0], 0);
        assert_eq!(table[127], 0);
        assert_eq!(table[128], 1);
        assert!(make_table(2, &[1, 2], 8).is_none(), "incomplete");
        assert!(make_table(3, &[1, 2, 2], 8).is_some());
        // A code longer than the table goes into the tree.
        let lens = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 10];
        let (table, left, right) = make_table(11, &lens, 8).unwrap();
        assert_eq!(table[0], 0);
        assert_eq!(table[0xff] as usize, 11, "node");
        assert_eq!(left[11], 8);
        assert_ne!(right[11], 0);
        assert_eq!(left[usize::from(right[11])], 9);
        assert_eq!(right[usize::from(right[11])], 10);
    }

    #[test]
    fn position_tables_match_lzhuf() {
        assert_eq!((d_code(0), d_len(0)), (0, 3));
        assert_eq!((d_code(31), d_len(31)), (0, 3));
        assert_eq!((d_code(32), d_len(32)), (1, 4));
        assert_eq!((d_code(79), d_len(79)), (3, 4));
        assert_eq!((d_code(80), d_len(80)), (4, 5));
        assert_eq!((d_code(143), d_len(143)), (11, 5));
        assert_eq!((d_code(144), d_len(144)), (12, 6));
        assert_eq!((d_code(191), d_len(191)), (23, 6));
        assert_eq!((d_code(192), d_len(192)), (24, 7));
        assert_eq!((d_code(239), d_len(239)), (47, 7));
        assert_eq!((d_code(240), d_len(240)), (48, 8));
        assert_eq!((d_code(255), d_len(255)), (63, 8));
    }
}
