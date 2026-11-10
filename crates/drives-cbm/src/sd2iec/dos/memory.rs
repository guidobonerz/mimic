//! `M-R`: the sd2iec has no 1541 memory, but answers the reads programs
//! make to tell drives apart (sd2iec README, "M-R, M-W, M-E"):
//! - With `XR:name`, reads of `$8000`–`$FFFF` come from that file, a dump
//!   of a drive ROM (looked for in the current directory, then the root;
//!   a header in front of the ROM is skipped, the ROM ends at `$FFFF`).
//! - Otherwise a small table: the DOS version string of the drive the
//!   mounted image belongs to (a D71 a 1571, a D81 a 1581, anything else a
//!   1541) and the 1541's listen/talk addresses `$77`/`$78`. The strings
//!   and addresses are those of the Commodore drive ROMs (1541 325302-01/
//!   901229-05, 1571 310654-05, 1581 318045-02: "CBM DOS V2.6 1541" at
//!   `$E5B7`, "CBM DOS V3.0 1571" at `$E5B7`, "CBM DOS V10 1581" at
//!   `$A6DB`, first and last character with bit 7 set), as software reads
//!   `$E5C6` ('4'/'7') and `$A6E9` ('8').
//! - Everything else reads as zero (the sd2iec answers "more-or-less
//!   random data").

use emu_core::host::lock;
use media::Format;

use super::Sd2iecDos;

const ID_1541: (u16, &[u8]) = (0xe5b7, b"\xc3BM DOS V2.6 154\xb1");
const ID_1571: (u16, &[u8]) = (0xe5b7, b"\xc3BM DOS V3.0 157\xb1");
const ID_1581: (u16, &[u8]) = (0xa6db, b"CBM DOS V10 158\xb1");

impl Sd2iecDos {
    /// `n` bytes of "drive memory" from `addr`.
    pub(super) fn memory_read(&mut self, addr: u16, n: usize) -> Vec<u8> {
        let rom = if self.xr.is_some() {
            self.rom_file()
        } else {
            None
        };
        let format = if self.mount.is_some() {
            self.image().ok().map(|i| i.format)
        } else {
            None
        };
        let id = match format {
            Some(Format::D71) => ID_1571,
            Some(Format::D81) => ID_1581,
            _ => ID_1541,
        };
        (0..n)
            .map(|i| {
                let a = addr.wrapping_add(i as u16);
                if let Some(rom) = rom.as_ref().filter(|_| a >= 0x8000) {
                    let base = 0x1_0000 - rom.len();
                    if usize::from(a) >= base {
                        return rom[usize::from(a) - base];
                    }
                }
                match a {
                    0x77 => 0x20 + self.device,
                    0x78 => 0x40 + self.device,
                    _ => {
                        let (start, text) = id;
                        a.checked_sub(start)
                            .and_then(|o| text.get(usize::from(o)))
                            .copied()
                            .unwrap_or(0)
                    }
                }
            })
            .collect()
    }

    /// The `XR` ROM (at most 32 KB, ending at `$FFFF`), read once per
    /// directory and name.
    fn rom_file(&mut self) -> Option<Vec<u8>> {
        let name = self.xr.clone()?;
        if let Some((dir, n, data)) = &self.rom_cache {
            if *dir == self.cwd && *n == name {
                return Some(data.clone());
            }
        }
        let files = self.files().ok()?;
        let data = {
            let mut fs = lock(&files);
            [self.cwd.clone(), "/".to_string()].iter().find_map(|dir| {
                let e = self.find_file(&mut *fs, dir, &name).ok()?;
                Self::file_data(&mut *fs, dir, &e).ok()
            })?
        };
        // A header is what does not fill a whole KB.
        let rom = data[data.len() % 1024..].to_vec();
        let rom = rom[rom.len().saturating_sub(0x8000)..].to_vec();
        if rom.is_empty() {
            return None;
        }
        self.rom_cache = Some((self.cwd.clone(), name, rom.clone()));
        Some(rom)
    }
}
