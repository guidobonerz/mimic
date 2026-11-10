//! REL files: fixed-length records in a file of the folder, opened with
//! `OPEN n,dev,sa,"name,L,"+CHR$(length)` (or by name once they exist) and
//! positioned with `P`+CHR$(96+sa)+CHR$(record low)+CHR$(record high)+
//! CHR$(byte). `P` on a plain file's channel seeks to a byte offset (up to
//! four bytes, low first). As on the sd2iec, REL files live in the folder
//! only, not in disk images; with an x00 header (`.R00`, the default) the
//! record length is header byte 25, without one the first byte of the file
//! (sd2iec README, "REL files", "P").
//!
//! Record rules as on CBM drives (*Commodore 1541 User's Guide*, relative
//! files): an empty record holds `$FF` and zeros; a read ends after the
//! record's last non-zero byte (EOI there) and goes on with the next
//! record; a write fills the record from the position, the rest becomes
//! zeros when the data ends (EOI), more than the record holds is
//! `51,OVERFLOW IN RECORD`; positioning past the end is `50,RECORD NOT
//! PRESENT`, and writing there adds the records up to it.

use emu_core::host::lock;
use emu_core::{StateError, StateReader, StateWriter};

use super::super::names::{self, REL};
use super::{
    fs_error, Channel, DosResult, Sd2iecDos, BAD_NAME, NOT_FOUND, NO_CHANNEL, NO_RECORD, OK,
    PROTECTED, SYNTAX, TYPE_MISMATCH,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RelFile {
    /// The host file.
    pub(crate) path: String,
    /// The bytes in front of the records (x00 header or the length byte).
    header: Vec<u8>,
    reclen: u8,
    /// The records, a whole number of them.
    body: Vec<u8>,
    /// Current record (0-based) and byte in it.
    record: usize,
    pos: usize,
    changed: bool,
    /// The current write ran past the record.
    overflow: bool,
}

/// `name,L,`+CHR$(length) or `name,L`: the part before `,L` and the length.
pub(super) fn rel_spec(spec: &[u8]) -> Option<(&[u8], Option<u8>)> {
    let i = spec
        .windows(2)
        .position(|w| w == b",L")
        .filter(|&i| matches!(spec.get(i + 2), None | Some(b',')))?;
    Some((&spec[..i], spec.get(i + 3).copied()))
}

impl RelFile {
    fn len(&self) -> usize {
        usize::from(self.reclen)
    }

    fn records(&self) -> usize {
        self.body.len() / self.len()
    }

    /// The current record's bytes up to its last non-zero byte.
    fn used(&self) -> Option<&[u8]> {
        let start = self.record.checked_mul(self.len())?;
        let rec = self.body.get(start..start + self.len())?;
        let used = rec.iter().rposition(|&b| b != 0).map_or(1, |i| i + 1);
        Some(&rec[..used])
    }

    /// The byte to send and whether it ends the record.
    fn current(&self) -> Option<(u8, bool)> {
        let rec = self.used()?;
        let i = self.pos.min(rec.len() - 1);
        Some((rec[i], i + 1 >= rec.len()))
    }

    pub(super) fn advance(&mut self) {
        let Some(used) = self.used().map(<[u8]>::len) else {
            return;
        };
        self.pos += 1;
        if self.pos >= used {
            self.record += 1;
            self.pos = 0;
        }
    }

    /// Records up to the current one exist (new ones empty).
    fn extend(&mut self) {
        while self.records() <= self.record {
            self.body.push(0xff);
            self.body
                .extend(std::iter::repeat_n(0, self.len().saturating_sub(1)));
        }
    }

    /// One byte from the computer; `false`: it did not fit the record.
    pub(super) fn write(&mut self, byte: u8, eoi: bool) -> bool {
        if self.pos == 0 {
            self.overflow = false;
        }
        self.extend();
        let start = self.record * self.len();
        let fits = self.pos < self.len() && !self.overflow;
        if fits {
            self.body[start + self.pos] = byte;
            self.pos += 1;
        } else {
            self.overflow = true;
        }
        if eoi {
            let (from, to) = (start + self.pos, start + self.len());
            self.body[from..to].fill(0);
            self.record += 1;
            self.pos = 0;
        }
        self.changed = true;
        fits
    }

    pub(super) fn save(&self, w: &mut StateWriter) {
        w.str(&self.path);
        w.bytes(&self.header);
        w.u8(self.reclen);
        w.bytes(&self.body);
        w.u32(self.record as u32);
        w.u32(self.pos as u32);
        w.bool(self.changed);
        w.bool(self.overflow);
    }

    pub(super) fn load(c: &mut StateReader<'_>) -> Result<Self, StateError> {
        let path = c.string()?;
        emu_core::host::check_path(&path).map_err(|_| StateError::Invalid("sd2iec REL path"))?;
        let f = Self {
            path,
            header: c.bytes()?.to_vec(),
            reclen: c.u8()?,
            body: c.bytes()?.to_vec(),
            record: c.u32()? as usize,
            pos: c.u32()? as usize,
            changed: c.bool()?,
            overflow: c.bool()?,
        };
        if f.reclen == 0 || !f.body.len().is_multiple_of(f.len()) || f.pos >= f.len() {
            return Err(StateError::Invalid("sd2iec REL file"));
        }
        Ok(f)
    }
}

impl Sd2iecDos {
    /// Opens (or, with a length, creates) the REL file `name` in `path`.
    pub(super) fn open_rel(
        &mut self,
        path: &[u8],
        name: &[u8],
        len: Option<u8>,
    ) -> DosResult<Channel> {
        if self.mount.is_some() {
            return Err(TYPE_MISMATCH);
        }
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        let (path, header, reclen, mut body) = match self.find_file(&mut *fs, &dir, name) {
            Ok(e) => {
                if e.file_type != REL {
                    return Err(TYPE_MISMATCH);
                }
                let path = Self::join(&dir, &e.host);
                let mut all = Self::read_all(&mut *fs, &path).map_err(fs_error)?;
                let (head, reclen) = if e.x00 {
                    (names::X00_HEADER, all.get(25).copied())
                } else {
                    (1, all.first().copied())
                };
                let reclen = reclen.filter(|&l| l != 0).ok_or(TYPE_MISMATCH)?;
                if len.is_some_and(|l| l != reclen) {
                    return Err(NO_RECORD);
                }
                let body = all.split_off(head.min(all.len()));
                (path, all, reclen, body)
            }
            Err(e) if e == NOT_FOUND => {
                let reclen = len.ok_or(NOT_FOUND)?;
                if reclen == 0 || reclen == 255 {
                    return Err(SYNTAX);
                }
                if self.protected {
                    return Err(PROTECTED);
                }
                if name.iter().any(|&c| c == b'*' || c == b'?') {
                    return Err(BAD_NAME);
                }
                let (path, mut header) = self.new_file(&mut *fs, &dir, name, REL)?;
                if header.len() == names::X00_HEADER {
                    header[25] = reclen;
                } else {
                    header = vec![reclen];
                }
                Self::write_all(&mut *fs, &path, &header).map_err(fs_error)?;
                (path, header, reclen, Vec::new())
            }
            Err(e) => return Err(e),
        };
        // A torn last record is filled up.
        let n = usize::from(reclen);
        body.resize(body.len().div_ceil(n) * n, 0);
        Ok(Channel::Rel(RelFile {
            path,
            header,
            reclen,
            body,
            ..RelFile::default()
        }))
    }

    /// CLOSE of a REL channel: the records go to the folder.
    pub(super) fn close_rel(&mut self, f: RelFile) -> DosResult<()> {
        if !f.changed {
            return Ok(());
        }
        let files = self.files()?;
        let mut fs = lock(&files);
        let mut bytes = f.header;
        bytes.extend_from_slice(&f.body);
        Self::write_all(&mut *fs, &f.path, &bytes).map_err(fs_error)
    }

    /// The next byte of a REL channel; past the last record a CR with EOI
    /// and `50,RECORD NOT PRESENT`.
    pub(super) fn rel_next_byte(&mut self, ch: usize) -> Option<(u8, bool)> {
        let Channel::Rel(f) = &self.channels[ch] else {
            return None;
        };
        match f.current() {
            Some(b) => Some(b),
            None => {
                self.set(NO_RECORD);
                Some((b'\r', true))
            }
        }
    }

    /// `P`: the channel's byte (after its secondary address, `$60` + n).
    pub(super) fn position(&mut self, args: &[u8]) -> DosResult<()> {
        let ch = usize::from(args.first().copied().ok_or(SYNTAX)? & 0x0f);
        let arg = |i: usize| args.get(i).copied().unwrap_or(0);
        let missing = match &mut self.channels[ch] {
            Channel::Rel(f) => {
                let record = usize::from(u16::from_le_bytes([arg(1), arg(2)]).max(1) - 1);
                let pos = usize::from(args.get(3).copied().unwrap_or(1).max(1) - 1);
                f.record = record;
                f.pos = pos.min(f.len() - 1);
                f.overflow = false;
                record >= f.records()
            }
            Channel::Read { data, pos } if ch != 15 => {
                let offset = u32::from_le_bytes([arg(1), arg(2), arg(3), arg(4)]) as usize;
                *pos = offset;
                offset > data.len()
            }
            _ => return Err(NO_CHANNEL),
        };
        self.set(if missing { NO_RECORD } else { OK });
        Ok(())
    }
}
