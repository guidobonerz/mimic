//! What the Elite II board's boot ROM does, and how an update file is
//! laid out.
//!
//! The firmware's images are sequences of records: load address, length
//! and start address (32 bits each, little-endian), then the data, padded
//! to a multiple of four bytes. Only the last record of an image carries a
//! start address; the others have 0. A `.cfw` file has a header of 36
//! bytes before the records (`DE AD 15 41`, product family and product
//! name, 16 bytes each); a `.ue2` file has none.
//!
//! The boot ROM (inside the FPGA, not available here) reads one such
//! record from the flash at the application's place and jumps to its
//! start address; it copies the data only if the length is below
//! `$180000`. An update file's record is the updater, a program of its
//! own that carries the FPGA image, the application and the flash's files
//! and writes them.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/01-cpu-boot-memory.md` (sections
//! "Boot ROM" and "Image format"), `docs/status/install.md` §5; the
//! user's update files 3.15a (record layout looked at).

/// The `.cfw` header's first bytes.
const CFW_MAGIC: [u8; 4] = [0xde, 0xad, 0x15, 0x41];
const CFW_HEADER: usize = 36;
/// The boot ROM copies an application only below this length.
pub const BOOT_MAX_LEN: u32 = 0x18_0000;

/// One record of an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record<'a> {
    pub load: u32,
    /// 0: not the last record.
    pub start: u32,
    pub data: &'a [u8],
}

/// The product family and name of a `.cfw` file, if it is one.
pub fn cfw_header(file: &[u8]) -> Option<(String, String)> {
    if file.len() < CFW_HEADER || file[..4] != CFW_MAGIC {
        return None;
    }
    let text = |b: &[u8]| {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..end]).into_owned()
    };
    Some((text(&file[4..20]), text(&file[20..36])))
}

/// The records of an image up to and with the first that has a start
/// address. `None`: the bytes are no image (a record runs past the end,
/// or none has a start address).
pub fn records(file: &[u8]) -> Option<Vec<Record<'_>>> {
    let mut rest = if cfw_header(file).is_some() {
        &file[CFW_HEADER..]
    } else {
        file
    };
    let mut out = Vec::new();
    loop {
        let word = |n: usize| {
            rest.get(n..n + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let (load, len, start) = (word(0)?, word(4)?, word(8)?);
        let len = usize::try_from(len).ok()?;
        let data = rest.get(12..12usize.checked_add(len)?)?;
        out.push(Record { load, start, data });
        if start != 0 {
            return Some(out);
        }
        let padded = len.checked_add(3)? & !3;
        rest = rest.get(12usize.checked_add(padded)?..)?;
    }
}

/// Copy an image's records into the board's memory (`ram.len()` a power
/// of two; addresses wrap as on the board). The start address.
pub fn load_records(ram: &mut [u8], recs: &[Record<'_>]) -> Option<u32> {
    let mask = ram.len() - 1;
    for r in recs {
        let at = r.load as usize & mask;
        let end = at.checked_add(r.data.len())?;
        ram.get_mut(at..end)?.copy_from_slice(r.data);
    }
    recs.last().map(|r| r.start)
}

/// The boot ROM's flash boot: the application record at `offset` of the
/// flash copied into memory. The address to jump to; `None`: the flash
/// holds no application there (erased).
pub fn boot_from_flash(ram: &mut [u8], flash: &[u8], offset: usize) -> Option<u32> {
    let word = |n: usize| {
        flash
            .get(offset + n..offset + n + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (load, len, start) = (word(0)?, word(4)?, word(8)?);
    if len == u32::MAX || start == u32::MAX {
        return None;
    }
    if len > 0 && len < BOOT_MAX_LEN {
        let data = flash.get(offset + 12..offset + 12 + len as usize)?;
        let mask = ram.len() - 1;
        let at = load as usize & mask;
        ram.get_mut(at..at.checked_add(data.len())?)?
            .copy_from_slice(data);
    }
    Some(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(load: u32, start: u32, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(load.to_le_bytes());
        v.extend((data.len() as u32).to_le_bytes());
        v.extend(start.to_le_bytes());
        v.extend(data);
        v.resize(v.len().div_ceil(4) * 4, 0);
        v
    }

    #[test]
    fn records_end_at_the_start_address() {
        let mut f = record(0x100, 0, &[1, 2, 3]);
        f.extend(record(0x200, 0x100, &[4, 5, 6, 7, 8]));
        f.extend([0xaa; 40]);
        let r = records(&f).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].data, [1, 2, 3]);
        assert_eq!((r[1].load, r[1].start), (0x200, 0x100));
        let mut ram = vec![0u8; 0x1000];
        assert_eq!(load_records(&mut ram, &r), Some(0x100));
        assert_eq!(ram[0x100..0x103], [1, 2, 3]);
        assert_eq!(ram[0x204], 8);
    }

    #[test]
    fn cfw_header_is_skipped() {
        let mut f = vec![0xde, 0xad, 0x15, 0x41];
        f.extend(b"U64MK2\0\0\0\0\0\0\0\0\0\0");
        f.extend(b"Ultimate 64-II\0\0");
        f.extend(record(0x300_0000, 0x300_0000, &[9; 8]));
        assert_eq!(
            cfw_header(&f),
            Some(("U64MK2".into(), "Ultimate 64-II".into()))
        );
        assert_eq!(records(&f).unwrap()[0].load, 0x300_0000);
    }

    #[test]
    fn broken_images_are_refused() {
        assert!(records(&[]).is_none());
        assert!(records(&[0; 11]).is_none());
        // A length past the end.
        let mut f = record(0, 4, &[1, 2, 3, 4]);
        f[4] = 0xff;
        assert!(records(&f).is_none());
        // No record with a start address.
        assert!(records(&record(0, 0, &[1, 2, 3, 4])).is_none());
    }

    #[test]
    fn flash_boot() {
        let mut flash = vec![0xffu8; 0x2000];
        let mut ram = vec![0u8; 0x1000];
        assert_eq!(boot_from_flash(&mut ram, &flash, 0x1000), None);
        let rec = record(0x80, 0x84, &[1, 2, 3, 4, 5]);
        flash[0x1000..0x1000 + rec.len()].copy_from_slice(&rec);
        assert_eq!(boot_from_flash(&mut ram, &flash, 0x1000), Some(0x84));
        assert_eq!(ram[0x80..0x85], [1, 2, 3, 4, 5]);
    }
}
