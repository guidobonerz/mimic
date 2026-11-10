//! File names between the C64 (PETSCII) and the host folder, x00 headers
//! and name matching.
//!
//! Letters swap case on the way, as on an SD2IEC with a FAT card: a host
//! name in lower case reads as upper case (unshifted PETSCII) on the C64,
//! host upper case as shifted PETSCII. Characters a host folder cannot
//! hold become `_`. Names longer than 16 characters appear under a short
//! alias in the style of a FAT 8.3 name (`LONGNA~1.PRG`), as the SD2IEC
//! shows the short name for long file names (sd2iec README, "Long File
//! Names").
//!
//! x00 files (`.P00`, `.S00`, `.U00`, `.R00`): a 26-byte header,
//! `C64File\0`, the 16-byte PETSCII name padded with zeros, a zero and the
//! REL record length; the file data follows (sd2iec README, "x00 files";
//! PC64 file format).

/// The first bytes of an x00 file.
pub const X00_MAGIC: &[u8; 8] = b"C64File\0";
/// Bytes in front of the data of an x00 file.
pub const X00_HEADER: usize = 26;

/// File type codes as in a CBM directory entry.
pub const SEQ: u8 = 1;
pub const PRG: u8 = 2;
pub const USR: u8 = 3;
pub const REL: u8 = 4;

/// The C64's view of a host file name. Both cases of a letter become the
/// unshifted PETSCII letter (what `LOAD"GAME"` types): host file systems
/// mostly ignore case and PC names are often upper case (`GAME.D64`),
/// which as shifted PETSCII would show as graphics and could not be typed.
/// A shifted letter the C64 writes ([`host_from_petscii`]: upper case on
/// the host) therefore reads back unshifted.
pub fn petscii_from_host(name: &str) -> Vec<u8> {
    name.chars()
        .map(|c| match c {
            'a'..='z' => c as u8 - 0x20,
            'A'..='Z' => c as u8,
            ' '..='~' => c as u8,
            _ => b'?',
        })
        .collect()
}

/// The host file name for a PETSCII name. Never empty; no path
/// separators, wildcards or characters Windows refuses; no trailing dot or
/// space.
pub fn host_from_petscii(name: &[u8]) -> String {
    let mut s: String = name
        .iter()
        .map(|&b| match b {
            0x41..=0x5a => (b + 0x20) as char,
            0xc1..=0xda => (b - 0x80) as char,
            0x61..=0x7a => (b - 0x20) as char,
            b'/' | b'\\' | b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|' => '_',
            0x20..=0x5f => b as char,
            _ => '_',
        })
        .collect();
    if s.ends_with(['.', ' ']) {
        s.pop();
        s.push('_');
    }
    if s.is_empty() {
        s.push('_');
    }
    s
}

/// A name's stem and extension (without the dot), if it has one.
pub fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// The file type of an x00 extension (`P00`–`P99`, `S..`, `U..`, `R..`).
pub fn x00_type(ext: &str) -> Option<u8> {
    let b = ext.as_bytes();
    if b.len() != 3 || !b[1].is_ascii_digit() || !b[2].is_ascii_digit() {
        return None;
    }
    match b[0].to_ascii_lowercase() {
        b'p' => Some(PRG),
        b's' => Some(SEQ),
        b'u' => Some(USR),
        b'r' => Some(REL),
        _ => None,
    }
}

/// The file type a `.PRG`/`.SEQ`/`.USR`/`.REL` extension names.
pub fn type_ext(ext: &str) -> Option<u8> {
    match ext.to_ascii_lowercase().as_str() {
        "prg" => Some(PRG),
        "seq" => Some(SEQ),
        "usr" => Some(USR),
        "rel" => Some(REL),
        _ => None,
    }
}

/// The extension for a file type.
pub fn ext_for(file_type: u8) -> &'static str {
    match file_type {
        SEQ => "seq",
        USR => "usr",
        REL => "rel",
        _ => "prg",
    }
}

/// The extension letter of an x00 file of `file_type`.
pub fn x00_letter(file_type: u8) -> char {
    match file_type {
        SEQ => 's',
        USR => 'u',
        REL => 'r',
        _ => 'p',
    }
}

/// A disk image the SD2IEC mounts by its extension (`.D41` is a D64).
pub fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "d64" | "d41" | "d71" | "d81"
    )
}

/// Extensions a PRG keeps without an x00 header or `.PRG` (images written
/// from the C64).
pub fn is_raw_ext(ext: &str) -> bool {
    is_image_ext(ext) || matches!(ext.to_ascii_lowercase().as_str(), "dnp" | "m2i")
}

/// The internal name of an x00 file from its first bytes, `None` if they
/// are not an x00 header.
pub fn parse_x00(header: &[u8]) -> Option<Vec<u8>> {
    if header.len() < X00_HEADER || &header[..8] != X00_MAGIC {
        return None;
    }
    let name = &header[8..24];
    let len = name.iter().position(|&c| c == 0).unwrap_or(16);
    Some(name[..len].to_vec())
}

/// An x00 header for `name` (at most 16 characters are kept).
pub fn x00_header(name: &[u8], record_len: u8) -> Vec<u8> {
    let mut h = X00_MAGIC.to_vec();
    let mut field = [0u8; 16];
    let n = name.len().min(16);
    field[..n].copy_from_slice(&name[..n]);
    h.extend_from_slice(&field);
    h.push(0);
    h.push(record_len);
    h
}

/// The stem and extension of an 8.3 alias for a long host name, before the
/// `~n` is added: up to six letters or digits of the stem and three of the
/// extension, in upper case.
pub fn alias_parts(host: &str) -> (String, String) {
    let (stem, ext) = split_ext(host);
    let pick = |s: &str, n: usize| -> String {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .take(n)
            .map(|c| c.to_ascii_uppercase())
            .collect()
    };
    let mut base = pick(stem, 6);
    if base.is_empty() {
        base.push('_');
    }
    (base, ext.map(|e| pick(e, 3)).unwrap_or_default())
}

/// The alias `BASE~n.EXT` as the C64 shows it.
pub fn alias(base: &str, ext: &str, n: usize) -> Vec<u8> {
    let mut s = format!("{base}~{n}");
    if !ext.is_empty() {
        s.push('.');
        s.push_str(ext);
    }
    // Upper-case ASCII as unshifted PETSCII: the same bytes.
    s.into_bytes()
}

/// CBM DOS name matching: `?` matches any one character; `*` matches the
/// rest, or with `star_1581` (SD2IEC's default, `X*+`) the characters
/// after it must match the end of the name.
pub fn matches(pattern: &[u8], name: &[u8], star_1581: bool) -> bool {
    let one = |p: &[u8], n: &[u8]| -> bool {
        p.len() == n.len() && p.iter().zip(n).all(|(&a, &b)| a == b'?' || a == b)
    };
    let Some(star) = pattern.iter().position(|&c| c == b'*') else {
        return one(pattern, name);
    };
    let (head, tail) = (&pattern[..star], &pattern[star + 1..]);
    if name.len() < head.len() || !one(head, &name[..head.len()]) {
        return false;
    }
    if !star_1581 || tail.is_empty() {
        return true;
    }
    // A further `*` in the tail: match what comes before it at the end.
    let tail = match tail.iter().position(|&c| c == b'*') {
        Some(i) => &tail[..i],
        None => tail,
    };
    let rest = &name[head.len()..];
    rest.len() >= tail.len() && one(tail, &rest[rest.len() - tail.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_swap_case_both_ways() {
        assert_eq!(petscii_from_host("game.prg"), b"GAME.PRG".to_vec());
        assert_eq!(petscii_from_host("Ab"), b"AB".to_vec());
        assert_eq!(petscii_from_host("TOOL.D64"), b"TOOL.D64".to_vec());
        assert_eq!(host_from_petscii(b"GAME"), "game");
        assert_eq!(host_from_petscii(&[0xc1, b'B']), "Ab");
        assert_eq!(host_from_petscii(b"A/B:C*"), "a_b_c_");
        assert_eq!(host_from_petscii(b"END."), "end_");
        assert_eq!(host_from_petscii(b""), "_");
    }

    #[test]
    fn x00_headers_round_trip() {
        let h = x00_header(b"HELLO", 0);
        assert_eq!(h.len(), X00_HEADER);
        assert_eq!(parse_x00(&h), Some(b"HELLO".to_vec()));
        assert_eq!(parse_x00(b"not an x00 header at all.."), None);
        assert_eq!(x00_type("P00"), Some(PRG));
        assert_eq!(x00_type("s17"), Some(SEQ));
        assert_eq!(x00_type("prg"), None);
    }

    #[test]
    fn aliases_look_like_fat_short_names() {
        let (b, e) = alias_parts("a very long file name.prg");
        assert_eq!(alias(&b, &e, 1), b"AVERYL~1.PRG".to_vec());
        let (b, e) = alias_parts("no_extension_but_long_enough");
        assert_eq!(alias(&b, &e, 2), b"NOEXTE~2".to_vec());
    }

    #[test]
    fn matching_with_and_without_1581_stars() {
        assert!(matches(b"GA*", b"GAME", true));
        assert!(matches(b"G?ME", b"GAME", true));
        assert!(!matches(b"GAM", b"GAME", true));
        assert!(matches(b"*.D64", b"DISK.D64", true));
        assert!(!matches(b"*.D64", b"DISK.D71", true));
        assert!(matches(b"*.D64", b"DISK.D71", false));
        assert!(matches(b"*", b"", true));
    }
}
