//! Swap lists and the disk change buttons (sd2iec README, "Changing Disk
//! Images"): a text file with one `CD` argument per line (images,
//! directories, `//PATH/:IMAGE.D64`), `;` starts a comment, empty lines
//! are skipped, ASCII unless the first line is exactly `#PETSCII`.
//!
//! `XS:name` activates a list (and mounts its first entry), `XS` ends it.
//! NEXT and PREV step through it, wrapping at both ends; both buttons
//! together (HOME) go to the first entry. Without a list the buttons look
//! for `AUTOSWAP.LST` in the current directory (then all three act as
//! HOME); HOME without one writes `AUTOSWAP.GEN` with the directory's disk
//! images and uses that. Such a found list ends with the next `CD`. The
//! lines are taken relative to the directory the list is in.

use emu_core::host::lock;
use emu_core::{StateError, StateReader, StateWriter};

use super::super::names;
use super::{fs_error, DosResult, Sd2iecDos, NOT_FOUND, OK, SYNTAX};

/// The disk change buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapButton {
    Next,
    Prev,
    /// Both buttons at once.
    Home,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Swap {
    /// The directory the list is in (lines are relative to it).
    pub(crate) dir: String,
    /// The entries as `CD` arguments (PETSCII).
    pub(crate) lines: Vec<Vec<u8>>,
    pub(crate) index: usize,
    /// Found by the buttons (`AUTOSWAP.LST`/`.GEN`): ends with a `CD`.
    pub(crate) auto: bool,
}

const AUTO_LIST: &[u8] = b"AUTOSWAP.LST";
const AUTO_GEN: &str = "AUTOSWAP.GEN";

/// The entries of a swap list file.
fn parse(data: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = data
        .split(|&c| c == b'\r' || c == b'\n')
        .filter(|l| !l.is_empty())
        .peekable();
    let petscii = lines.peek() == Some(&&b"#PETSCII"[..]);
    if petscii {
        lines.next();
    }
    lines
        .filter(|l| l.first() != Some(&b';'))
        .map(|l| {
            if petscii {
                l.to_vec()
            } else {
                names::petscii_from_host(&String::from_utf8_lossy(l))
            }
        })
        .collect()
}

pub(super) fn save(swap: Option<&Swap>, w: &mut StateWriter) {
    let Some(s) = swap else {
        w.bool(false);
        return;
    };
    w.bool(true);
    w.str(&s.dir);
    w.u32(s.lines.len() as u32);
    for l in &s.lines {
        w.bytes(l);
    }
    w.u32(s.index as u32);
    w.bool(s.auto);
}

pub(super) fn load(c: &mut StateReader<'_>) -> Result<Option<Swap>, StateError> {
    if !c.bool()? {
        return Ok(None);
    }
    let dir = c.string()?;
    emu_core::host::check_path(&dir).map_err(|_| StateError::Invalid("sd2iec swap list"))?;
    let n = c.u32()? as usize;
    let mut lines = Vec::new();
    for _ in 0..n {
        lines.push(c.bytes()?.to_vec());
    }
    let index = c.u32()? as usize;
    if lines.is_empty() || index >= lines.len() {
        return Err(StateError::Invalid("sd2iec swap list"));
    }
    Ok(Some(Swap {
        dir,
        lines,
        index,
        auto: c.bool()?,
    }))
}

impl Sd2iecDos {
    /// `XS:name` / `XS`.
    pub(super) fn swap_command(&mut self, arg: &[u8]) -> DosResult<()> {
        let name = match arg {
            [] => {
                self.swap = None;
                self.set(OK);
                return Ok(());
            }
            [b':', name @ ..] if !name.is_empty() => name,
            _ => return Err(SYNTAX),
        };
        let swap = self.read_list(name, false)?.ok_or(NOT_FOUND)?;
        self.swap = Some(swap);
        self.set(OK);
        self.swap_mount();
        Ok(())
    }

    /// A swap list file `name` in the current directory (`None`: no such
    /// file; an empty list counts as none).
    fn read_list(&mut self, name: &[u8], auto: bool) -> DosResult<Option<Swap>> {
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.cwd.clone();
        let Ok(e) = self.find_file(&mut *fs, &dir, name) else {
            return Ok(None);
        };
        let data = Self::file_data(&mut *fs, &dir, &e)?;
        let lines = parse(&data);
        Ok((!lines.is_empty()).then_some(Swap {
            dir,
            lines,
            index: 0,
            auto,
        }))
    }

    /// `AUTOSWAP.GEN` with the current directory's disk images.
    fn generate_list(&mut self) -> DosResult<Option<Swap>> {
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.cwd.clone();
        let lines: Vec<Vec<u8>> = self
            .scan(&mut *fs, &dir)?
            .into_iter()
            .filter(|e| e.mountable && !e.hidden)
            .map(|e| e.name)
            .collect();
        if lines.is_empty() {
            return Ok(None);
        }
        if !self.protected {
            let mut text = b"#PETSCII\r\n".to_vec();
            for l in &lines {
                text.extend_from_slice(l);
                text.extend_from_slice(b"\r\n");
            }
            Self::write_all(&mut *fs, &Self::join(&dir, AUTO_GEN), &text).map_err(fs_error)?;
        }
        Ok(Some(Swap {
            dir,
            lines,
            index: 0,
            auto: true,
        }))
    }

    /// A press of the disk change buttons.
    pub fn swap_button(&mut self, button: SwapButton) {
        if let Some(s) = &mut self.swap {
            let n = s.lines.len();
            s.index = match button {
                SwapButton::Next => (s.index + 1) % n,
                SwapButton::Prev => (s.index + n - 1) % n,
                SwapButton::Home => 0,
            };
        } else {
            let found = match self.read_list(AUTO_LIST, true) {
                Ok(None) if button == SwapButton::Home => self.generate_list(),
                other => other,
            };
            match found {
                Ok(Some(s)) => self.swap = Some(s),
                Ok(None) => return,
                Err(e) => {
                    self.set(e);
                    return;
                }
            }
        }
        self.swap_mount();
    }

    /// Change to the list's current entry (as `CD` from the list's
    /// directory); a disk in the media slot stays.
    fn swap_mount(&mut self) {
        let Some(s) = &self.swap else {
            return;
        };
        if self.has_media() {
            return;
        }
        let line = s.lines[s.index].clone();
        self.cwd = s.dir.clone();
        self.mount = None;
        match self.cd(&line) {
            Ok(()) => self.set(OK),
            Err(e) => self.set(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn lists_skip_comments_and_blank_lines() {
        let l = parse(b"foo.d64\r\n\r\n; save disk\n//GAMES/:Bar.D64\n");
        assert_eq!(l, vec![b"FOO.D64".to_vec(), b"//GAMES/:BAR.D64".to_vec()]);
        let p = parse(b"#PETSCII\rDISK\xc1\r");
        assert_eq!(p, vec![b"DISK\xc1".to_vec()]);
    }
}
