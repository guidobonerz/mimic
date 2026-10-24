//! Apple II 5.25" disks: 16-sector images (DSK/DO/PO) and WOZ bit-stream
//! images, both turned into one bit stream per track for the Disk II.
//!
//! **Track format** (Don Worth, Pieter Lechner, *Beneath Apple DOS*,
//! chapter 3 "Disk II formatting"): a track is a stream of 4 µs bit cells.
//! The controller shifts bits into a latch until its bit 7 is set, so a
//! valid "disk byte" (nibble) always has the high bit set. DOS 3.3 writes
//! sixteen sectors per track, each as
//!
//! - an **address field**: prologue `D5 AA 96`, volume, track, sector and
//!   checksum (volume ^ track ^ sector), each in "4-and-4" (odd-even)
//!   encoding as two nibbles `(v >> 1) | $AA`, `v | $AA`, epilogue
//!   `DE AA EB`;
//! - a gap of self-sync bytes (`FF` followed by two 0 bits, ten bit
//!   cells: whatever bit the latch starts on, it falls into step within a
//!   few of them);
//! - a **data field**: prologue `D5 AA AD`, 342 data nibbles plus a
//!   checksum nibble, epilogue `DE AA EB`;
//! - another self-sync gap.
//!
//! **6-and-2 encoding:** the 256 bytes of a sector become 342 six-bit
//! values. The first 86 hold the low two bits of the bytes (value `i`:
//! bits 1–0 from byte `i`, bits 3–2 from byte `i + 86`, bits 5–4 from byte
//! `i + 172`, each pair with its two bits swapped), the other 256 the
//! upper six bits of the bytes in order. Each value is written XORed with
//! the one before (the first with 0), followed by the last value as the
//! checksum; a six-bit value goes on disk through the 64-entry write
//! table. *Beneath Apple DOS* gives its rule: the high bit set, at least
//! two adjacent 1 bits in bits 6–0, no more than one pair of consecutive
//! 0 bits; `$D5` and `$AA` stay reserved for the field marks. The table
//! here is computed from that rule ([`write_table`]) — the result is the
//! book's table, `$96 … $FF`.
//!
//! **Sector order:** the address field carries the *physical* sector
//! number; images store sectors in a logical order. DOS 3.3 order
//! (.dsk, .do): physical sector `p` holds the image's sector
//! `[0, 7, 14, 6, 13, 5, 12, 4, 11, 3, 10, 2, 9, 1, 8, 15][p]` of the
//! track (*Beneath Apple DOS*, chapter 3, the DOS 3.3 interleave).
//! ProDOS order (.po): the image is a row of 512-byte blocks, eight per
//! track; block `b` of a track lies on physical sectors
//! `[0, 4, 8, 12, 1, 5, 9, 13][b]` and that + 2, in its first and
//! second half (Don Worth, Pieter Lechner, *Beneath Apple
//! ProDOS*, chapter 3, block-to-sector mapping), i.e. physical sector `p`
//! holds the image's 256-byte half-block `[0, 8, 1, 9, 2, 10, 3, 11, 4,
//! 12, 5, 13, 6, 14, 7, 15][p]` of the track.
//!
//! Generated tracks are [`TRACK_BITS`] long: a gap before sector 0, then
//! per sector the address field, 6 self-sync bytes, the data field and 16
//! self-sync bytes. [`decode_dsk`] does not rely on this layout: it reads
//! the bit stream the way the controller's latch does (from anywhere,
//! wrapping around the end of the track) and finds the fields wherever the
//! guest's RWTS wrote them.
//!
//! **WOZ** (John K. Morris, "WOZ Disk Image Reference" 1.0 and 2.0/2.1,
//! applesaucefdc.com/woz/reference/ and /reference2/): a 12-byte header
//! (`WOZ1`/`WOZ2`, `FF 0A 0D 0A`, CRC32 of the rest of the file, 0 = not
//! computed) followed by chunks (4-byte id, 32-bit little-endian size,
//! data). INFO: version, disk type (1 = 5.25", 2 = 3.5"), write protected,
//! …, in version 2 also the largest track in 512-byte blocks (offset 44).
//! TMAP: 160 bytes, one per quarter-track position, the TRKS index or
//! `FF`. TRKS in WOZ1: 6656 bytes per track (6646 bytes of bits, bytes
//! used, bit count, splice point, splice nibble, splice bit count,
//! reserved); in WOZ2: 160 entries of starting block, block count (16 bits
//! each) and bit count (32 bits), the bits stored at the given 512-byte
//! blocks of the file. Flux tracks of WOZ 2.1 (FLUX chunk) are not read;
//! [`store_woz`] copies them unchanged. The CRC is the common CRC-32
//! (polynomial $EDB88320, reflected, initial and final value $FFFFFFFF),
//! implemented here.

/// Tracks on a standard 5.25" disk image.
pub const TRACKS: usize = 35;
/// Quarter-track positions the head can reach (WOZ TMAP size).
pub const QUARTER_TRACKS: usize = 160;
/// Bytes in a 16-sector DSK/DO/PO image (35 × 16 × 256).
pub const DSK_BYTES: usize = 143_360;
/// Nominal bit cells of an encoded track (4 µs cells, ~300 rpm): use 51_200 (6400 bytes) for tracks you generate.
pub const TRACK_BITS: u32 = 51_200;

/// Sectors per track of a 16-sector disk.
pub const SECTORS: usize = 16;
/// Volume number written into generated address fields (DOS 3.3's default).
pub const VOLUME: u8 = 254;

/// Image sector (within the track) held by physical sector `p`, DOS 3.3 order.
const DOS_ORDER: [usize; SECTORS] = [0, 7, 14, 6, 13, 5, 12, 4, 11, 3, 10, 2, 9, 1, 8, 15];
/// Image half-block (within the track) held by physical sector `p`, ProDOS order.
const PRODOS_ORDER: [usize; SECTORS] = [0, 8, 1, 9, 2, 10, 3, 11, 4, 12, 5, 13, 6, 14, 7, 15];

/// Self-sync bytes between an address field and its data field.
const GAP2: usize = 6;
/// Self-sync bytes after a data field.
const GAP3: usize = 16;

const ADDR_PROLOGUE: [u8; 3] = [0xd5, 0xaa, 0x96];
const DATA_PROLOGUE: [u8; 3] = [0xd5, 0xaa, 0xad];
const EPILOGUE: [u8; 3] = [0xde, 0xaa, 0xeb];
/// Nibbles after an address field in which its data field must start.
const DATA_SEARCH: usize = 48;

const WOZ1_TRACK: usize = 6656;
const WOZ1_BITS_BYTES: usize = 6646;
const WOZ2_TABLE: usize = QUARTER_TRACKS * 8;
const BLOCK: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectorOrder {
    Dos,
    ProDos,
}

impl SectorOrder {
    /// Index of the 256-byte image sector (within its track) that physical sector `p` holds.
    pub fn image_sector(self, p: usize) -> usize {
        match self {
            SectorOrder::Dos => DOS_ORDER[p],
            SectorOrder::ProDos => PRODOS_ORDER[p],
        }
    }
}

/// A 5.25" disk as one bit stream per stored track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NibbleDisk {
    /// Bit streams, MSB first: bit i of a track is `tracks[t][i / 8] & (0x80 >> (i % 8))`.
    pub tracks: Vec<Vec<u8>>,
    /// Valid bit cells per stored track (same length as `tracks`).
    pub bits: Vec<u32>,
    /// Which stored track a quarter-track position reads (`None` = no track / random bits). Index = quarter track (track n = 4n).
    pub quarter: [Option<u8>; QUARTER_TRACKS],
    /// The image says the disk is write-protected (WOZ INFO).
    pub write_protected: bool,
}

/// Why [`decode_dsk`] could not recover the whole image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The image buffer has the wrong size; nothing was stored.
    BadImage(String),
    /// These sectors, as `(track, physical sector)`, were not found or had
    /// a bad checksum; their bytes in the image are unchanged, every other
    /// sector was stored.
    Sectors(Vec<(u8, u8)>),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::BadImage(m) => write!(f, "DSK: {m}"),
            DecodeError::Sectors(s) => {
                write!(f, "DSK: {} unreadable sector(s):", s.len())?;
                for (t, p) in s {
                    write!(f, " T{t} S{p}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<DecodeError> for String {
    fn from(e: DecodeError) -> String {
        e.to_string()
    }
}

// ---------------------------------------------------------------------------
// 6-and-2 tables

/// The *Beneath Apple DOS* rule for a disk byte that can carry 6-and-2 data.
const fn valid_disk_byte(b: u8) -> bool {
    if b & 0x80 == 0 || b == 0xd5 || b == 0xaa {
        return false;
    }
    let mut zero_pairs = 0;
    let mut adjacent_ones = false;
    let mut i = 0;
    while i < 7 {
        let pair = (b >> i) & 3;
        if pair == 0 {
            zero_pairs += 1;
        }
        // Bits i and i + 1, both below bit 7.
        if pair == 3 && i < 6 {
            adjacent_ones = true;
        }
        i += 1;
    }
    zero_pairs <= 1 && adjacent_ones
}

const fn build_write_table() -> [u8; 64] {
    let mut t = [0u8; 64];
    let mut n = 0;
    let mut b = 0x80usize;
    while b < 0x100 {
        if valid_disk_byte(b as u8) {
            t[n] = b as u8; // exactly 64 valid bytes; a different count fails the build
            n += 1;
        }
        b += 1;
    }
    assert!(n == 64);
    t
}

const fn build_read_table(w: &[u8; 64]) -> [u8; 256] {
    let mut r = [0xffu8; 256];
    let mut i = 0;
    while i < 64 {
        r[w[i] as usize] = i as u8;
        i += 1;
    }
    r
}

const WRITE: [u8; 64] = build_write_table();
/// Disk byte → six-bit value, `$FF` for bytes that are not in the table.
const READ: [u8; 256] = build_read_table(&WRITE);

/// The 6-and-2 write table: six-bit value → disk byte.
pub fn write_table() -> [u8; 64] {
    WRITE
}

/// A disk byte → its six-bit value, `None` for bytes outside the table.
pub fn read_nibble(b: u8) -> Option<u8> {
    let v = READ[usize::from(b)];
    (v != 0xff).then_some(v)
}

/// Encode a sector's 256 bytes into the 343 disk bytes of a data field (without prologue/epilogue).
pub fn encode_6and2(data: &[u8; 256]) -> [u8; 343] {
    let mut v = [0u8; 342];
    for (i, aux) in v.iter_mut().enumerate().take(86) {
        let mut a = 0u8;
        for k in 0..3 {
            if let Some(&b) = data.get(i + 86 * k) {
                a |= ((b & 1) << 1 | (b >> 1) & 1) << (2 * k);
            }
        }
        *aux = a;
    }
    for (i, &b) in data.iter().enumerate() {
        v[86 + i] = b >> 2;
    }
    let mut out = [0u8; 343];
    let mut prev = 0u8;
    for (o, &x) in out.iter_mut().zip(v.iter()) {
        *o = WRITE[usize::from(x ^ prev)];
        prev = x;
    }
    out[342] = WRITE[usize::from(prev)];
    out
}

/// Decode the 343 disk bytes of a data field; `None` for a byte outside
/// the table or a bad checksum.
pub fn decode_6and2(nibbles: &[u8]) -> Option<[u8; 256]> {
    if nibbles.len() < 343 {
        return None;
    }
    let mut v = [0u8; 342];
    let mut prev = 0u8;
    for (x, &n) in v.iter_mut().zip(nibbles) {
        let r = READ[usize::from(n)];
        if r == 0xff {
            return None;
        }
        *x = r ^ prev;
        prev = *x;
    }
    if READ[usize::from(nibbles[342])] != prev {
        return None;
    }
    let mut out = [0u8; 256];
    for (i, o) in out.iter_mut().enumerate() {
        let pair = (v[i % 86] >> (2 * (i / 86))) & 3;
        *o = v[86 + i] << 2 | (pair & 1) << 1 | pair >> 1;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Track encoding

/// Builds a bit stream MSB first.
struct BitWriter {
    bytes: Vec<u8>,
    len: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(TRACK_BITS as usize / 8),
            len: 0,
        }
    }

    fn bit(&mut self, one: bool) {
        if self.len.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if one {
            if let Some(last) = self.bytes.last_mut() {
                *last |= 0x80 >> (self.len % 8);
            }
        }
        self.len += 1;
    }

    fn byte(&mut self, v: u8) {
        for i in (0..8).rev() {
            self.bit(v >> i & 1 != 0);
        }
    }

    fn bytes(&mut self, v: &[u8]) {
        for &b in v {
            self.byte(b);
        }
    }

    /// `n` self-sync bytes: `FF` and two 0 bits.
    fn syncs(&mut self, n: usize) {
        for _ in 0..n {
            self.byte(0xff);
            self.bit(false);
            self.bit(false);
        }
    }

    /// Fill `bits` (even, ≥ 8 or 0) bit cells with self-sync bytes and plain `FF`s.
    fn fill(&mut self, bits: u32) {
        let mut syncs = bits / 10;
        let mut rest = bits % 10;
        while !rest.is_multiple_of(8) && syncs > 0 {
            syncs -= 1;
            rest += 10;
        }
        self.syncs(syncs as usize);
        for _ in 0..rest / 8 {
            self.byte(0xff);
        }
    }

    fn odd_even(&mut self, v: u8) {
        self.byte(v >> 1 | 0xaa);
        self.byte(v | 0xaa);
    }

    /// Address field, `gap2` self-sync bytes, data field.
    fn sector(&mut self, volume: u8, track: u8, phys: u8, data: &[u8; 256], gap2: usize) {
        self.bytes(&ADDR_PROLOGUE);
        self.odd_even(volume);
        self.odd_even(track);
        self.odd_even(phys);
        self.odd_even(volume ^ track ^ phys);
        self.bytes(&EPILOGUE);
        self.syncs(gap2);
        self.bytes(&DATA_PROLOGUE);
        self.bytes(&encode_6and2(data));
        self.bytes(&EPILOGUE);
    }
}

/// Bit cells of one sector in the generated layout (fields and gaps 2/3).
const SECTOR_BITS: u32 = ((14 + 349) * 8 + (GAP2 + GAP3) * 10) as u32;

/// Encode one track from its sectors in physical order: exactly
/// [`TRACK_BITS`] bit cells (6400 bytes).
pub fn encode_track(volume: u8, track: u8, sectors: &[[u8; 256]; SECTORS]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.fill(TRACK_BITS - SECTORS as u32 * SECTOR_BITS);
    for (p, data) in sectors.iter().enumerate() {
        w.sector(volume, track, p as u8, data, GAP2);
        w.syncs(GAP3);
    }
    debug_assert_eq!(w.len, TRACK_BITS);
    w.bytes
}

/// Encode a 16-sector image (DOS 3.3 order for .dsk/.do, ProDOS order for .po) into GCR 6-and-2 tracks
/// with address fields (volume 254, track, sector, checksum, prologue D5 AA 96, epilogue DE AA EB),
/// data fields (D5 AA AD, 342+1 nibbles, epilogue DE AA EB) and 10-bit self-sync FF gaps (an FF followed by two 0 bits),
/// physical sectors in the standard interleave. Quarter positions 4t-1..=4t+1 (clamped) map to track t (like WOZ files made from DSKs; neighbouring half tracks readable as the real head would).
pub fn encode_dsk(image: &[u8], order: SectorOrder) -> Result<NibbleDisk, String> {
    if image.len() != DSK_BYTES {
        return Err(format!(
            "DSK: image has {} bytes, expected {DSK_BYTES}",
            image.len()
        ));
    }
    let mut tracks = Vec::with_capacity(TRACKS);
    let mut quarter = [None; QUARTER_TRACKS];
    for t in 0..TRACKS {
        let mut sectors = [[0u8; 256]; SECTORS];
        for (p, s) in sectors.iter_mut().enumerate() {
            let off = (t * SECTORS + order.image_sector(p)) * 256;
            s.copy_from_slice(&image[off..off + 256]);
        }
        tracks.push(encode_track(VOLUME, t as u8, &sectors));
        let q = 4 * t;
        let near = q.saturating_sub(1)..=(q + 1).min(QUARTER_TRACKS - 1);
        quarter[near].fill(Some(t as u8));
    }
    Ok(NibbleDisk {
        tracks,
        bits: vec![TRACK_BITS; TRACKS],
        quarter,
        write_protected: false,
    })
}

// ---------------------------------------------------------------------------
// Track decoding

/// The disk bytes the controller's latch sees over two revolutions,
/// starting at bit 0: each with the bit index (counted over both
/// revolutions) of its first bit. Leading 0 bits are skipped, a byte is
/// complete when bit 7 of the latch is set.
fn latch_nibbles(track: &[u8], bits: u32) -> Vec<(u8, u32)> {
    let bits = bits.min((track.len() * 8).min(u32::MAX as usize) as u32);
    let mut out = Vec::with_capacity(bits as usize / 8 * 2 + 1);
    let mut reg = 0u8;
    let mut start = 0u32;
    for k in 0..u64::from(bits) * 2 {
        let i = (k % u64::from(bits)) as usize;
        let b = track[i / 8] >> (7 - i % 8) & 1;
        if reg == 0 {
            if b == 0 {
                continue;
            }
            start = k as u32;
        }
        reg = reg << 1 | b;
        if reg & 0x80 != 0 {
            out.push((reg, start));
            reg = 0;
        }
    }
    out
}

fn odd_even(a: u8, b: u8) -> u8 {
    (a << 1 | 1) & b
}

/// Find the sectors of one track: indexed by physical sector, each the
/// first copy whose address field (with this track number and a good
/// checksum) and data field read back correctly. Address fields are
/// searched from every bit position of one revolution; fields may wrap
/// around the end of the track.
pub fn decode_track(track: &[u8], bits: u32, track_no: u8) -> [Option<[u8; 256]>; SECTORS] {
    let mut found = [None; SECTORS];
    if bits == 0 || track.is_empty() {
        return found;
    }
    let nibs = latch_nibbles(track, bits);
    let n: Vec<u8> = nibs.iter().map(|&(v, _)| v).collect();
    let mut i = 0;
    while i + 14 <= n.len() {
        if nibs[i].1 >= bits || n[i..i + 3] != ADDR_PROLOGUE {
            i += 1;
            continue;
        }
        let a = &n[i + 3..i + 14];
        let (vol, trk, sec, chk) = (
            odd_even(a[0], a[1]),
            odd_even(a[2], a[3]),
            odd_even(a[4], a[5]),
            odd_even(a[6], a[7]),
        );
        i += 3;
        // RWTS checks only the first two epilogue bytes.
        if vol ^ trk ^ sec ^ chk != 0 || a[8..10] != EPILOGUE[..2] {
            continue;
        }
        if trk != track_no || usize::from(sec) >= SECTORS || found[usize::from(sec)].is_some() {
            continue;
        }
        let from = i + 11;
        let to = (from + DATA_SEARCH).min(n.len());
        let Some(d) = (from..to).find(|&j| j + 3 <= n.len() && n[j..j + 3] == DATA_PROLOGUE) else {
            continue;
        };
        let body = d + 3;
        if body + 345 > n.len() || n[body + 343..body + 345] != EPILOGUE[..2] {
            continue;
        }
        if let Some(data) = decode_6and2(&n[body..body + 343]) {
            found[usize::from(sec)] = Some(data);
        }
    }
    found
}

/// Decode the tracks back into a 16-sector image (for write-back after the guest wrote). Finds address fields anywhere in the bit stream (bit-aligned search, wrapping around the end of the track; tolerate any gap layout written by DOS 3.3/ProDOS RWTS), decodes the data field and stores it at the logical sector per `order`.
///
/// Track `t` is read from the stored track at quarter position `4t`.
/// Returns [`DecodeError::Sectors`] with `(track, physical sector)` of
/// every sector that was missing or had a bad checksum: those bytes stay
/// as they were in `image`, all other sectors are stored.
/// [`DecodeError::BadImage`] if `image` is not [`DSK_BYTES`] long.
/// `DecodeError` converts into `String` (`?` in functions returning
/// `Result<_, String>`).
pub fn decode_dsk(
    disk: &NibbleDisk,
    order: SectorOrder,
    image: &mut [u8],
) -> Result<(), DecodeError> {
    if image.len() != DSK_BYTES {
        return Err(DecodeError::BadImage(format!(
            "image has {} bytes, expected {DSK_BYTES}",
            image.len()
        )));
    }
    let mut bad = Vec::new();
    for t in 0..TRACKS {
        let stored = disk.quarter[4 * t].map(usize::from);
        let sectors = match stored.and_then(|s| Some((disk.tracks.get(s)?, *disk.bits.get(s)?))) {
            Some((data, bits)) => decode_track(data, bits, t as u8),
            None => [None; SECTORS],
        };
        for (p, s) in sectors.iter().enumerate() {
            match s {
                Some(data) => {
                    let off = (t * SECTORS + order.image_sector(p)) * 256;
                    image[off..off + 256].copy_from_slice(data);
                }
                None => bad.push((t as u8, p as u8)),
            }
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(DecodeError::Sectors(bad))
    }
}

// ---------------------------------------------------------------------------
// Sector order guess

/// The DOS 3.3 VTOC: tracks per disk 35, sectors per track 16, 256 bytes per sector.
fn is_vtoc(s: &[u8]) -> bool {
    s.len() >= 0x38 && s[0x34] == 35 && s[0x35] == 16 && s[0x36] == 0 && s[0x37] == 1
}

/// The start of a ProDOS volume directory key block: no previous block,
/// storage type $F with a name, entry length $27, 13 entries per block.
fn is_prodos_volume_dir(s: &[u8]) -> bool {
    s.len() >= 0x25
        && s[0] == 0
        && s[1] == 0
        && s[4] >> 4 == 0xf
        && s[4] & 0xf != 0
        && s[0x23] == 0x27
        && s[0x24] == 0x0d
}

/// Sector order from the file name extension (".po" → ProDos, ".do"/".dsk" → Dos) — for ".dsk" also detect a ProDOS-ordered image by content.
///
/// Heuristic for `.dsk` (and unknown extensions), in this order:
/// 1. a DOS 3.3 VTOC at track 17 sector 0 (offset `$11000`, the same in
///    both orders) → `Dos`;
/// 2. a ProDOS volume directory header (block 2) at offset `$400`, where
///    a ProDOS-ordered image keeps it → `ProDos`;
/// 3. the same header at track 0 image sector 11 (`$B00`), where block 2's
///    first half lands in a DOS-ordered image → `Dos`;
/// 4. otherwise `Dos`, the usual order of `.dsk` files.
pub fn guess_order(name: &str, image: &[u8]) -> SectorOrder {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".po") {
        return SectorOrder::ProDos;
    }
    if lower.ends_with(".do") {
        return SectorOrder::Dos;
    }
    let at = |off: usize| image.get(off..(off + 256).min(image.len())).unwrap_or(&[]);
    if is_vtoc(at(17 * SECTORS * 256)) {
        return SectorOrder::Dos;
    }
    if is_prodos_volume_dir(at(2 * BLOCK)) {
        return SectorOrder::ProDos;
    }
    SectorOrder::Dos
}

// ---------------------------------------------------------------------------
// WOZ

/// CRC-32 (IEEE 802.3, reflected polynomial $EDB88320), as the WOZ header uses it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                crc >> 1 ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

struct Chunk {
    id: [u8; 4],
    /// File offset of the chunk header and of its data.
    head: usize,
    data: usize,
    size: usize,
}

struct WozFile<'a> {
    bytes: &'a [u8],
    v2: bool,
    chunks: Vec<Chunk>,
}

impl<'a> WozFile<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < 12 {
            return Err("WOZ: file too short".into());
        }
        let v2 = match &bytes[..4] {
            b"WOZ1" => false,
            b"WOZ2" => true,
            _ => return Err("WOZ: not a WOZ file".into()),
        };
        if bytes[4..8] != [0xff, 0x0a, 0x0d, 0x0a] {
            return Err("WOZ: bad header bytes (file mangled by a text transfer?)".into());
        }
        let crc = u32_at(bytes, 8).unwrap_or(0);
        if crc != 0 && crc32(&bytes[12..]) != crc {
            return Err("WOZ: CRC32 mismatch".into());
        }
        let mut chunks = Vec::new();
        let mut pos = 12;
        while pos + 8 <= bytes.len() {
            let mut id = [0u8; 4];
            id.copy_from_slice(&bytes[pos..pos + 4]);
            let size = u32_at(bytes, pos + 4).unwrap_or(0) as usize;
            let data = pos + 8;
            let end = data
                .checked_add(size)
                .filter(|&e| e <= bytes.len())
                .ok_or_else(|| format!("WOZ: chunk {} truncated", String::from_utf8_lossy(&id)))?;
            chunks.push(Chunk {
                id,
                head: pos,
                data,
                size,
            });
            pos = end;
        }
        Ok(Self { bytes, v2, chunks })
    }

    fn chunk(&self, id: &[u8; 4]) -> Option<&Chunk> {
        self.chunks.iter().find(|c| &c.id == id)
    }

    fn body(&self, c: &Chunk) -> &'a [u8] {
        &self.bytes[c.data..c.data + c.size]
    }

    fn required(&self, id: &[u8; 4]) -> Result<&'a [u8], String> {
        let c = self
            .chunk(id)
            .ok_or_else(|| format!("WOZ: no {} chunk", String::from_utf8_lossy(id)))?;
        Ok(self.body(c))
    }

    /// TRKS indices that hold flux data (WOZ 2.1 FLUX chunk).
    fn flux_tracks(&self) -> [bool; QUARTER_TRACKS] {
        let mut flux = [false; QUARTER_TRACKS];
        if let Some(c) = self.chunk(b"FLUX") {
            for &v in self.body(c).iter().take(QUARTER_TRACKS) {
                if let Some(f) = flux.get_mut(usize::from(v)) {
                    *f = true;
                }
            }
        }
        flux
    }

    /// WOZ2 TRK entry `i`: starting block, block count, bit count.
    fn trk(trks: &[u8], i: usize) -> (usize, usize, u32) {
        let o = i * 8;
        (
            usize::from(u16_at(trks, o).unwrap_or(0)),
            usize::from(u16_at(trks, o + 2).unwrap_or(0)),
            u32_at(trks, o + 4).unwrap_or(0),
        )
    }
}

fn bytes_for(bits: u32) -> usize {
    (bits as usize).div_ceil(8)
}

/// WOZ 1.0 and 2.0 (5.25" only; reject 3.5" disks): INFO (write protect), TMAP, TRKS; check the CRC32 header if non-zero.
///
/// `tracks[i]` is TRKS entry `i` (so TMAP values carry over unchanged),
/// trimmed to the bytes its bit count needs. WOZ2 entries without data
/// and flux tracks (WOZ 2.1) are empty with 0 bits; the list ends at the
/// last entry with data.
pub fn parse_woz(data: &[u8]) -> Result<NibbleDisk, String> {
    let f = WozFile::parse(data)?;
    let info = f.required(b"INFO")?;
    if info.len() < 3 {
        return Err("WOZ: INFO chunk too short".into());
    }
    match info[1] {
        1 => {}
        2 => return Err("WOZ: 3.5\" disks are not supported".into()),
        t => return Err(format!("WOZ: unknown disk type {t}")),
    }
    let write_protected = info[2] == 1;
    let tmap = f.required(b"TMAP")?;
    if tmap.len() < QUARTER_TRACKS {
        return Err("WOZ: TMAP chunk too short".into());
    }
    let trks = f.required(b"TRKS")?;
    let mut tracks = Vec::new();
    let mut bits = Vec::new();
    if f.v2 {
        if trks.len() < WOZ2_TABLE {
            return Err("WOZ: TRKS chunk too short".into());
        }
        let flux = f.flux_tracks();
        let n = (0..QUARTER_TRACKS)
            .rev()
            .find(|&i| WozFile::trk(trks, i).0 != 0)
            .map_or(0, |i| i + 1);
        for (i, &is_flux) in flux.iter().enumerate().take(n) {
            let (start, count, bit_count) = WozFile::trk(trks, i);
            if start == 0 || is_flux || bit_count == 0 {
                tracks.push(Vec::new());
                bits.push(0);
                continue;
            }
            let need = bytes_for(bit_count);
            let off = start * BLOCK;
            if need > count * BLOCK || off + need > data.len() {
                return Err(format!("WOZ: track {i} lies outside the file"));
            }
            tracks.push(data[off..off + need].to_vec());
            bits.push(bit_count);
        }
    } else {
        for (i, t) in trks.as_chunks::<WOZ1_TRACK>().0.iter().enumerate() {
            let bit_count = u32::from(u16_at(t, WOZ1_BITS_BYTES + 2).unwrap_or(0));
            let need = bytes_for(bit_count);
            if need > WOZ1_BITS_BYTES {
                return Err(format!("WOZ: track {i} has too many bits"));
            }
            tracks.push(t[..need].to_vec());
            bits.push(bit_count);
        }
    }
    let mut quarter = [None; QUARTER_TRACKS];
    for (q, &v) in quarter.iter_mut().zip(tmap) {
        if v == 0xff {
            continue;
        }
        match bits.get(usize::from(v)) {
            Some(&b) if b > 0 => *q = Some(v),
            Some(_) => {}
            None => return Err(format!("WOZ: TMAP points to missing track {v}")),
        }
    }
    Ok(NibbleDisk {
        tracks,
        bits,
        quarter,
        write_protected,
    })
}

/// Write tracks back into a WOZ file: rebuild the file from `original` with the new track bits (keep INFO/META and the rest; WOZ2: TRKS blocks can be re-laid-out; recompute CRC32). Must round-trip: parse_woz(&store_woz(orig, &parse_woz(orig)?)?) == parse_woz(orig).
///
/// Chunks keep their order; INFO gets the write-protect flag (and in
/// version 2 the largest track's block count), TMAP is rewritten from
/// `disk.quarter`. WOZ1: a track may hold at most 6646 bytes; splice
/// information is kept for tracks whose bits did not change and cleared
/// ($FFFF) otherwise. WOZ2: the track data is laid out again from the
/// first block after the TRK table; flux tracks are copied unchanged.
pub fn store_woz(original: &[u8], disk: &NibbleDisk) -> Result<Vec<u8>, String> {
    let orig = parse_woz(original)?;
    let f = WozFile::parse(original)?;
    if disk.tracks.len() != disk.bits.len() {
        return Err("WOZ: tracks and bit counts differ in length".into());
    }
    for (i, (t, &b)) in disk.tracks.iter().zip(&disk.bits).enumerate() {
        if t.len() < bytes_for(b) {
            return Err(format!("WOZ: track {i} is shorter than its bit count"));
        }
    }
    if disk.tracks.len() > if f.v2 { QUARTER_TRACKS } else { 255 } {
        return Err("WOZ: too many tracks".into());
    }
    let flux = f.flux_tracks();
    let orig_trks = f.required(b"TRKS")?;

    // WOZ2: block count of every TRK entry in the new file.
    let mut largest = 0usize;
    if f.v2 {
        for (i, &is_flux) in flux.iter().enumerate() {
            let blocks = if is_flux {
                WozFile::trk(orig_trks, i).1
            } else {
                disk.bits
                    .get(i)
                    .map_or(0, |&b| bytes_for(b).div_ceil(BLOCK))
            };
            largest = largest.max(blocks);
        }
    }

    let mut out = Vec::with_capacity(original.len());
    out.extend_from_slice(&original[..8]);
    out.extend_from_slice(&[0; 4]);
    for c in &f.chunks {
        let body = f.body(c);
        match &c.id {
            b"INFO" => {
                let mut info = body.to_vec();
                info[2] = u8::from(disk.write_protected);
                if f.v2 && info[0] >= 2 && info.len() >= 46 {
                    let l = u16::try_from(largest).map_err(|_| "WOZ: track too long")?;
                    info[44..46].copy_from_slice(&l.to_le_bytes());
                }
                push_chunk(&mut out, b"INFO", &info);
            }
            b"TMAP" => {
                let mut tmap = body.to_vec();
                for (t, q) in tmap.iter_mut().zip(&disk.quarter) {
                    *t = match q {
                        Some(v) if usize::from(*v) < disk.tracks.len() => *v,
                        Some(v) => {
                            return Err(format!("WOZ: quarter track maps to missing track {v}"))
                        }
                        None => 0xff,
                    };
                }
                push_chunk(&mut out, b"TMAP", &tmap);
            }
            b"TRKS" if f.v2 => {
                let data_abs = out.len() + 8;
                let first = (data_abs + WOZ2_TABLE).div_ceil(BLOCK) * BLOCK;
                let mut table = vec![0u8; first - data_abs];
                let mut blobs = Vec::new();
                for i in 0..QUARTER_TRACKS {
                    let start = (first + blobs.len()) / BLOCK;
                    let (bytes, count, bit_field): (&[u8], usize, u32) = if flux[i] {
                        let (s, n, b) = WozFile::trk(orig_trks, i);
                        let off = s * BLOCK;
                        let src = original
                            .get(off..off + n * BLOCK)
                            .ok_or_else(|| format!("WOZ: flux track {i} lies outside the file"))?;
                        (src, n, b)
                    } else {
                        match (disk.tracks.get(i), disk.bits.get(i)) {
                            (Some(t), Some(&b)) if b > 0 => {
                                let need = bytes_for(b);
                                (&t[..need], need.div_ceil(BLOCK), b)
                            }
                            _ => continue,
                        }
                    };
                    if count == 0 {
                        continue;
                    }
                    let start = u16::try_from(start).map_err(|_| "WOZ: file too large")?;
                    let count16 = u16::try_from(count).map_err(|_| "WOZ: track too long")?;
                    table[i * 8..i * 8 + 2].copy_from_slice(&start.to_le_bytes());
                    table[i * 8 + 2..i * 8 + 4].copy_from_slice(&count16.to_le_bytes());
                    table[i * 8 + 4..i * 8 + 8].copy_from_slice(&bit_field.to_le_bytes());
                    let before = blobs.len();
                    blobs.extend_from_slice(bytes);
                    blobs.resize(before + count * BLOCK, 0);
                }
                table.extend_from_slice(&blobs);
                push_chunk(&mut out, b"TRKS", &table);
            }
            b"TRKS" => {
                let mut trks = Vec::with_capacity(disk.tracks.len() * WOZ1_TRACK);
                for (i, (t, &b)) in disk.tracks.iter().zip(&disk.bits).enumerate() {
                    let need = bytes_for(b);
                    if need > WOZ1_BITS_BYTES {
                        return Err(format!("WOZ: track {i} does not fit a WOZ1 track"));
                    }
                    let mut block = vec![0u8; WOZ1_TRACK];
                    block[..need].copy_from_slice(&t[..need]);
                    block[WOZ1_BITS_BYTES..WOZ1_BITS_BYTES + 2]
                        .copy_from_slice(&(need as u16).to_le_bytes());
                    block[WOZ1_BITS_BYTES + 2..WOZ1_BITS_BYTES + 4]
                        .copy_from_slice(&(b as u16).to_le_bytes());
                    let unchanged = orig.tracks.get(i).is_some_and(|o| o[..] == t[..need])
                        && orig.bits.get(i) == Some(&b);
                    match orig_trks.get(i * WOZ1_TRACK + 6650..(i + 1) * WOZ1_TRACK) {
                        Some(splice) if unchanged => block[6650..].copy_from_slice(splice),
                        _ => block[6650..6652].copy_from_slice(&[0xff, 0xff]),
                    }
                    trks.extend_from_slice(&block);
                }
                push_chunk(&mut out, b"TRKS", &trks);
            }
            _ => out.extend_from_slice(&original[c.head..c.data + c.size]),
        }
    }
    let crc = crc32(&out[12..]);
    out[8..12].copy_from_slice(&crc.to_le_bytes());
    Ok(out)
}

fn push_chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random image (xorshift).
    fn image(seed: u32) -> Vec<u8> {
        let mut x = seed | 1;
        (0..DSK_BYTES)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    #[test]
    fn write_table_follows_the_documented_rule() {
        let t = write_table();
        assert_eq!(t.len(), 64);
        // The table as printed in Beneath Apple DOS, chapter 3.
        let book: [u8; 64] = [
            0x96, 0x97, 0x9a, 0x9b, 0x9d, 0x9e, 0x9f, 0xa6, 0xa7, 0xab, 0xac, 0xad, 0xae, 0xaf,
            0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb9, 0xba, 0xbb, 0xbc, 0xbd, 0xbe, 0xbf, 0xcb,
            0xcd, 0xce, 0xcf, 0xd3, 0xd6, 0xd7, 0xd9, 0xda, 0xdb, 0xdc, 0xdd, 0xde, 0xdf, 0xe5,
            0xe6, 0xe7, 0xe9, 0xea, 0xeb, 0xec, 0xed, 0xee, 0xef, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6,
            0xf7, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff,
        ];
        assert_eq!(t, book);
        for (i, &b) in t.iter().enumerate() {
            assert!(b & 0x80 != 0);
            assert!(b != 0xd5 && b != 0xaa);
            assert!((0..6).any(|k| b >> k & 3 == 3), "{b:02X} adjacent ones");
            assert!((0..7).filter(|k| b >> k & 3 == 0).count() <= 1, "{b:02X}");
            assert_eq!(READ[usize::from(b)], i as u8);
            if i > 0 {
                assert!(b > t[i - 1]);
            }
        }
        assert_eq!(READ.iter().filter(|&&r| r != 0xff).count(), 64);
    }

    #[test]
    fn six_and_two_round_trip_and_checksum() {
        let mut data = [0u8; 256];
        for (i, d) in data.iter_mut().enumerate() {
            *d = (i * 37 + 11) as u8;
        }
        let enc = encode_6and2(&data);
        assert_eq!(decode_6and2(&enc), Some(data));
        let mut bad = enc;
        bad[100] = WRITE[usize::from(READ[usize::from(bad[100])] ^ 1)];
        assert_eq!(decode_6and2(&bad), None);
        // All zero data: every value 0, so every nibble is $96.
        assert!(encode_6and2(&[0; 256]).iter().all(|&n| n == 0x96));
    }

    #[test]
    fn encode_decode_round_trip_both_orders() {
        for (order, seed) in [(SectorOrder::Dos, 1), (SectorOrder::ProDos, 77)] {
            let img = image(seed);
            let disk = encode_dsk(&img, order).unwrap();
            assert_eq!(disk.tracks.len(), TRACKS);
            assert!(disk
                .tracks
                .iter()
                .all(|t| t.len() * 8 == TRACK_BITS as usize));
            assert_eq!(disk.quarter[0], Some(0));
            assert_eq!(disk.quarter[1], Some(0));
            assert_eq!(disk.quarter[2], None);
            assert_eq!(disk.quarter[3], Some(1));
            assert_eq!(disk.quarter[137], Some(34));
            assert_eq!(disk.quarter[138], None);
            let mut out = vec![0u8; DSK_BYTES];
            decode_dsk(&disk, order, &mut out).unwrap();
            assert!(out == img);
            // Decoding in the other order shuffles sectors within the track.
            let other = match order {
                SectorOrder::Dos => SectorOrder::ProDos,
                SectorOrder::ProDos => SectorOrder::Dos,
            };
            decode_dsk(&disk, other, &mut out).unwrap();
            assert!(out != img);
        }
    }

    #[test]
    fn dos_and_prodos_mappings() {
        // T0: DOS logical sector 1 sits on physical 13; ProDOS block 1 on
        // physical 4 and 6.
        assert_eq!(SectorOrder::Dos.image_sector(13), 1);
        assert_eq!(SectorOrder::ProDos.image_sector(4), 2);
        assert_eq!(SectorOrder::ProDos.image_sector(6), 3);
        for o in [SectorOrder::Dos, SectorOrder::ProDos] {
            let mut seen = [false; SECTORS];
            for p in 0..SECTORS {
                seen[o.image_sector(p)] = true;
            }
            assert!(seen.iter().all(|&s| s));
        }
    }

    fn rotate(track: &[u8], bits: u32, by: u32) -> Vec<u8> {
        let mut out = vec![0u8; track.len()];
        for i in 0..bits {
            let src = ((i + by) % bits) as usize;
            if track[src / 8] & (0x80 >> (src % 8)) != 0 {
                out[i as usize / 8] |= 0x80 >> (i % 8);
            }
        }
        out
    }

    #[test]
    fn other_gap_layouts_and_wrapping_still_decode() {
        let img = image(5);
        let mut disk = encode_dsk(&img, SectorOrder::Dos).unwrap();
        // Track 3 rewritten in another layout: sectors in reverse order,
        // 40-bit syncs and plain FFs as gaps, a stray header with a bad
        // checksum, then rotated by an odd bit count so a sector wraps.
        let mut sectors = [[0u8; 256]; SECTORS];
        for (p, s) in sectors.iter_mut().enumerate() {
            let off = (3 * SECTORS + SectorOrder::Dos.image_sector(p)) * 256;
            s.copy_from_slice(&img[off..off + 256]);
        }
        let mut w = BitWriter::new();
        w.bytes(&[0xd5, 0xaa, 0x96, 0xff, 0xfe, 0xaa, 0xab, 0xaa, 0xaa, 0x00]);
        for p in (0..SECTORS).rev() {
            for _ in 0..5 {
                w.byte(0xff);
                for _ in 0..32 {
                    w.bit(false);
                }
            }
            w.bytes(&[0xff; 3]);
            w.sector(VOLUME - 3, 3, p as u8, &sectors[p], 9);
            w.byte(0xff);
        }
        let bits = w.len;
        disk.tracks[3] = rotate(&w.bytes, bits, 1234);
        disk.bits[3] = bits;
        let mut out = vec![0u8; DSK_BYTES];
        decode_dsk(&disk, SectorOrder::Dos, &mut out).unwrap();
        assert!(out == img);
    }

    #[test]
    fn unreadable_sectors_are_reported_and_left_alone() {
        let img = image(9);
        let mut disk = encode_dsk(&img, SectorOrder::Dos).unwrap();
        // Corrupt the data field of physical sector 0 of track 2.
        let start = (TRACK_BITS - SECTORS as u32 * SECTOR_BITS) as usize / 8;
        disk.tracks[2][start + 14 + 1 + 100] ^= 0x10;
        disk.quarter[40] = None; // track 10 gone
        let mut out = vec![0xeeu8; DSK_BYTES];
        let err = decode_dsk(&disk, SectorOrder::Dos, &mut out).unwrap_err();
        let DecodeError::Sectors(bad) = &err else {
            panic!("{err}")
        };
        assert!(bad.contains(&(2, 0)));
        assert_eq!(bad.iter().filter(|s| s.0 == 10).count(), 16);
        assert_eq!(bad.len(), 17);
        assert!(out[2 * 4096..2 * 4096 + 256].iter().all(|&b| b == 0xee));
        assert!(out[2 * 4096 + 256..3 * 4096] == img[2 * 4096 + 256..3 * 4096]);
        assert!(String::from(err).contains("T2 S0"));
        let mut short = vec![0u8; 100];
        assert!(matches!(
            decode_dsk(&disk, SectorOrder::Dos, &mut short),
            Err(DecodeError::BadImage(_))
        ));
        assert!(encode_dsk(&short, SectorOrder::Dos).is_err());
    }

    fn header(magic: &[u8; 4]) -> Vec<u8> {
        let mut f = magic.to_vec();
        f.extend_from_slice(&[0xff, 0x0a, 0x0d, 0x0a, 0, 0, 0, 0]);
        f
    }

    fn info(version: u8, disk_type: u8, wp: bool) -> Vec<u8> {
        let mut i = vec![0u8; 60];
        i[0] = version;
        i[1] = disk_type;
        i[2] = u8::from(wp);
        i[5..37].copy_from_slice(&[b' '; 32]);
        i
    }

    fn tmap(disk: &NibbleDisk) -> Vec<u8> {
        disk.quarter.iter().map(|q| q.unwrap_or(0xff)).collect()
    }

    fn finish(mut f: Vec<u8>) -> Vec<u8> {
        let crc = crc32(&f[12..]);
        f[8..12].copy_from_slice(&crc.to_le_bytes());
        f
    }

    /// A minimal WOZ2 file: INFO, TMAP, TRKS (data from block 3), META.
    fn build_woz2(disk: &NibbleDisk) -> Vec<u8> {
        let mut f = header(b"WOZ2");
        push_chunk(&mut f, b"INFO", &info(2, 1, disk.write_protected));
        push_chunk(&mut f, b"TMAP", &tmap(disk));
        let mut trks = vec![0u8; WOZ2_TABLE];
        let mut block = 3usize;
        let mut data = Vec::new();
        for (i, (t, &b)) in disk.tracks.iter().zip(&disk.bits).enumerate() {
            let n = bytes_for(b).div_ceil(BLOCK);
            trks[i * 8..i * 8 + 2].copy_from_slice(&(block as u16).to_le_bytes());
            trks[i * 8 + 2..i * 8 + 4].copy_from_slice(&(n as u16).to_le_bytes());
            trks[i * 8 + 4..i * 8 + 8].copy_from_slice(&b.to_le_bytes());
            let mut d = t[..bytes_for(b)].to_vec();
            d.resize(n * BLOCK, 0);
            data.extend_from_slice(&d);
            block += n;
        }
        trks.extend_from_slice(&data);
        push_chunk(&mut f, b"TRKS", &trks);
        push_chunk(&mut f, b"META", b"title\tTest disk\n");
        finish(f)
    }

    fn build_woz1(disk: &NibbleDisk) -> Vec<u8> {
        let mut f = header(b"WOZ1");
        push_chunk(&mut f, b"INFO", &info(1, 1, disk.write_protected));
        push_chunk(&mut f, b"TMAP", &tmap(disk));
        let mut trks = Vec::new();
        for (t, &b) in disk.tracks.iter().zip(&disk.bits) {
            let mut blk = vec![0u8; WOZ1_TRACK];
            let need = bytes_for(b);
            blk[..need].copy_from_slice(&t[..need]);
            blk[6646..6648].copy_from_slice(&(need as u16).to_le_bytes());
            blk[6648..6650].copy_from_slice(&(b as u16).to_le_bytes());
            blk[6650..6652].copy_from_slice(&0x1234u16.to_le_bytes());
            trks.extend_from_slice(&blk);
        }
        push_chunk(&mut f, b"TRKS", &trks);
        finish(f)
    }

    #[test]
    fn woz2_and_woz1_parse_back_to_the_same_tracks() {
        let mut disk = encode_dsk(&image(3), SectorOrder::Dos).unwrap();
        disk.write_protected = true;
        let w2 = build_woz2(&disk);
        // TRKS at 248, its table at 256, the first track at block 3.
        assert_eq!(&w2[248..252], b"TRKS");
        assert_eq!(u16_at(&w2, 256), Some(3));
        assert_eq!(parse_woz(&w2).unwrap(), disk);
        let w1 = build_woz1(&disk);
        assert_eq!(parse_woz(&w1).unwrap(), disk);
        // An odd bit count survives too.
        disk.bits[5] = TRACK_BITS - 3;
        disk.tracks[5].truncate(bytes_for(TRACK_BITS - 3));
        *disk.tracks[5].last_mut().unwrap() &= 0xf8;
        assert_eq!(parse_woz(&build_woz2(&disk)).unwrap(), disk);
    }

    #[test]
    fn woz_store_round_trips_and_writes_new_bits() {
        let disk = encode_dsk(&image(4), SectorOrder::Dos).unwrap();
        for orig in [build_woz2(&disk), build_woz1(&disk)] {
            let parsed = parse_woz(&orig).unwrap();
            let stored = store_woz(&orig, &parsed).unwrap();
            assert_eq!(parse_woz(&stored).unwrap(), parsed);
            assert!(
                stored.windows(9).any(|w| w == b"Test disk")
                    == orig.windows(9).any(|w| w == b"Test disk")
            );
            // Change track 7's bits (a longer track in WOZ2) and the write protect.
            let mut changed = parsed.clone();
            changed.tracks[7][100] ^= 0xff;
            changed.write_protected = true;
            if &orig[..4] == b"WOZ2" {
                changed.tracks[7].extend_from_slice(&[0xff; 600]);
                changed.bits[7] += 600 * 8;
            }
            let stored = store_woz(&orig, &changed).unwrap();
            assert_eq!(parse_woz(&stored).unwrap(), changed);
            if &orig[..4] == b"WOZ1" {
                // Splice info kept for unchanged tracks, cleared for track 7.
                let trks = 12 + 68 + 168 + 8;
                assert_eq!(&stored[trks + 6650..trks + 6652], &[0x34, 0x12]);
                let t7 = trks + 7 * WOZ1_TRACK;
                assert_eq!(&stored[t7 + 6650..t7 + 6652], &[0xff, 0xff]);
            } else {
                assert_eq!(u16_at(&stored, 12 + 8 + 44), Some(14));
            }
        }
    }

    #[test]
    fn woz_rejects_bad_files() {
        let disk = encode_dsk(&image(6), SectorOrder::ProDos).unwrap();
        let mut w = build_woz2(&disk);
        let last = w.len() - 1;
        w[last] ^= 1;
        assert!(parse_woz(&w).unwrap_err().contains("CRC"));
        // A zero CRC means "not computed".
        w[8..12].copy_from_slice(&[0; 4]);
        assert!(parse_woz(&w).is_ok());
        // 3.5" disks.
        let mut w = build_woz2(&disk);
        w[12 + 8 + 1] = 2;
        let w = finish(w);
        assert!(parse_woz(&w).unwrap_err().contains("3.5"));
        // Truncated files and garbage never panic.
        let w = build_woz1(&disk);
        for n in [0, 5, 12, 20, 100, 300, 7000] {
            assert!(parse_woz(&w[..n]).is_err());
        }
        assert!(parse_woz(b"WOZ2\xff\x0a\x0d\x0a\0\0\0\0INFO\xff\xff\xff\xff").is_err());
    }

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn guess_order_cases() {
        let blank = vec![0u8; DSK_BYTES];
        assert_eq!(guess_order("game.PO", &blank), SectorOrder::ProDos);
        assert_eq!(guess_order("game.do", &blank), SectorOrder::Dos);
        assert_eq!(guess_order("game.dsk", &blank), SectorOrder::Dos);
        // DOS 3.3 VTOC.
        let mut dos = blank.clone();
        let v = 17 * 16 * 256;
        dos[v + 1] = 17;
        dos[v + 2] = 15;
        dos[v + 3] = 3;
        dos[v + 0x34] = 35;
        dos[v + 0x35] = 16;
        dos[v + 0x37] = 1;
        assert_eq!(guess_order("x.dsk", &dos), SectorOrder::Dos);
        // ProDOS volume directory at block 2 of a ProDOS-ordered image.
        let mut pro = blank.clone();
        let mut dir = [0u8; 0x25];
        dir[2] = 3;
        dir[4] = 0xf0 | 5;
        dir[5..10].copy_from_slice(b"HELLO");
        dir[0x23] = 0x27;
        dir[0x24] = 0x0d;
        pro[1024..1024 + 0x25].copy_from_slice(&dir);
        assert_eq!(guess_order("x.dsk", &pro), SectorOrder::ProDos);
        // The same volume in DOS order: block 2's first half at sector 11.
        let disk = encode_dsk(&pro, SectorOrder::ProDos).unwrap();
        let mut as_dos = vec![0u8; DSK_BYTES];
        decode_dsk(&disk, SectorOrder::Dos, &mut as_dos).unwrap();
        assert!(is_prodos_volume_dir(&as_dos[11 * 256..]));
        assert_eq!(guess_order("x.dsk", &as_dos), SectorOrder::Dos);
        // Short or empty data does not panic.
        assert_eq!(guess_order("x.dsk", &[]), SectorOrder::Dos);
    }
}
