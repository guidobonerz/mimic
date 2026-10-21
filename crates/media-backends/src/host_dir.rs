//! A host folder as a disk: the PRG files in it appear as a D64 image, or a
//! D81 when they do not fit a D64 (more than 664 blocks or 144 files)
//! (built when the folder is opened), and what the drive writes goes back
//! into the folder as files. Works with the true drive and the virtual one,
//! since both only see the image.
//!
//! Names: a file's stem, upper-cased, becomes its PETSCII name (16
//! characters at most); a file the machine saves gets the lower-cased name
//! plus `.prg`, or overwrites the host file it came from. Files are never
//! deleted from the folder (SCRATCH only removes them from the image).
//! Folders with more than 3160 blocks of PRGs show the files that fit. A
//! D81 needs the 1581 (or the virtual drive) in the slot.
//!
//! Unlike image files (behind a copy-on-write overlay until committed), the
//! folder is written through: a SAVE lands in the folder when the drive
//! writes the image back.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
use media::D64;

fn io_err(e: std::io::Error) -> MediaError {
    MediaError::Backend(e.to_string())
}

/// The PETSCII name for a host file stem (printable ASCII, upper case).
fn cbm_name(stem: &str) -> Vec<u8> {
    stem.chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .map(|c| c.to_ascii_uppercase() as u8)
        .take(16)
        .collect()
}

#[derive(Debug)]
pub struct HostDirBackend {
    dir: PathBuf,
    image: Vec<u8>,
    /// Host file for each name in the image, and the bytes last synced.
    files: BTreeMap<Vec<u8>, (PathBuf, Vec<u8>)>,
}

impl HostDirBackend {
    pub fn open(dir: &Path) -> Result<Self, MediaError> {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(io_err)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("prg"))
            })
            .collect();
        entries.sort();
        let title = dir
            .file_name()
            .map_or_else(|| "HOST".into(), |n| n.to_string_lossy().into_owned());
        let mut programs = Vec::new();
        for path in entries {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = cbm_name(&stem);
            if name.is_empty() || programs.iter().any(|(n, _, _)| *n == name) {
                continue;
            }
            let data = fs::read(&path).map_err(io_err)?;
            programs.push((name, path, data));
        }
        let blocks: usize = programs
            .iter()
            .map(|(_, _, d)| d.len().div_ceil(254).max(1))
            .sum();
        let mut d64 = if blocks > 664 || programs.len() > 144 {
            D64::blank_d81(&cbm_name(&title), *b"PT")
        } else {
            D64::blank(&cbm_name(&title), *b"PT")
        };
        let mut files = BTreeMap::new();
        for (name, path, data) in programs {
            // A full disk shows the files that fit.
            if d64.write_file(&name, 2, &data).is_ok() {
                files.insert(name, (path, data));
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            image: d64.to_bytes(),
            files,
        })
    }

    /// Write files that are new or changed in the image into the folder.
    fn sync(&mut self) -> Result<(), MediaError> {
        let d64 = D64::parse(&self.image)?;
        for entry in d64.directory()? {
            if entry.file_type & 7 != 2 || entry.file_type & 0x80 == 0 {
                continue;
            }
            let Ok((_, data)) = d64.read_file(&entry.name) else {
                continue;
            };
            let path = match self.files.get(&entry.name) {
                Some((_, old)) if *old == data => continue,
                Some((path, _)) => path.clone(),
                None => {
                    let stem: String = entry
                        .name
                        .iter()
                        .map(|&c| match c {
                            b'/' | b'\\' | b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|' => '_',
                            c if c.is_ascii_graphic() || c == b' ' => {
                                c.to_ascii_lowercase() as char
                            }
                            _ => '_',
                        })
                        .collect();
                    self.dir.join(format!("{}.prg", stem.trim()))
                }
            };
            fs::write(&path, &data).map_err(io_err)?;
            self.files.insert(entry.name, (path, data));
        }
        Ok(())
    }
}

impl MediaBackend for HostDirBackend {
    fn len(&self) -> u64 {
        self.image.len() as u64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let start = usize::try_from(offset).map_err(|_| MediaError::OutOfRange)?;
        let end = start.checked_add(buf.len()).ok_or(MediaError::OutOfRange)?;
        buf.copy_from_slice(self.image.get(start..end).ok_or(MediaError::OutOfRange)?);
        Ok(())
    }

    /// Updates the image and writes new or changed files to the folder.
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let start = usize::try_from(offset).map_err(|_| MediaError::OutOfRange)?;
        let end = start
            .checked_add(data.len())
            .ok_or(MediaError::OutOfRange)?;
        self.image
            .get_mut(start..end)
            .ok_or(MediaError::OutOfRange)?
            .copy_from_slice(data);
        self.sync()
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        self.sync()
    }
}

/// A host folder as disk media.
pub fn open(dir: &Path) -> Result<Media, MediaError> {
    let backend = HostDirBackend::open(dir)?;
    Ok(Media {
        name: dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        kind: MediaKind::Disk,
        backend: Box::new(backend),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mimic-hostdir-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folder_files_appear_and_saves_come_back() {
        let dir = temp_dir("rw");
        fs::write(dir.join("Hello.prg"), [0x01, 0x08, 1, 2, 3]).unwrap();
        fs::write(dir.join("notes.txt"), b"not a program").unwrap();
        let mut media = open(&dir).unwrap();
        assert_eq!(media.kind, MediaKind::Disk);
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes).unwrap();
        let mut d = D64::parse(&bytes).unwrap();
        let names: Vec<_> = d.directory().unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, [b"HELLO".to_vec()]);

        // The machine saves a new file and changes the old one.
        d.write_file(b"NEW GAME", 2, &[0x01, 0x10, 9]).unwrap();
        d.scratch(b"HELLO").unwrap();
        d.write_file(b"HELLO", 2, &[0x01, 0x08, 7]).unwrap();
        media.backend.write(0, &d.to_bytes()).unwrap();
        assert_eq!(fs::read(dir.join("new game.prg")).unwrap(), [0x01, 0x10, 9]);
        assert_eq!(fs::read(dir.join("Hello.prg")).unwrap(), [0x01, 0x08, 7]);
        assert!(dir.join("notes.txt").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn large_folders_become_d81() {
        let dir = temp_dir("big");
        for i in 0..4 {
            fs::write(dir.join(format!("part{i}.prg")), vec![i as u8; 200 * 254]).unwrap();
        }
        let mut media = open(&dir).unwrap();
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes).unwrap();
        let d = D64::parse(&bytes).unwrap();
        assert_eq!(d.format, media::Format::D81);
        assert_eq!(d.directory().unwrap().len(), 4);
        assert_eq!(d.read_file(b"PART3").unwrap().1, vec![3u8; 200 * 254]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
