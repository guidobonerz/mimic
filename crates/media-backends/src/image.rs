//! Image backends: an in-memory buffer, an image file, and the copy-on-write
//! overlay that is the default for everything users insert. The original
//! image is only modified when the overlay is committed.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};

fn range(len: u64, offset: u64, n: usize) -> Result<std::ops::Range<usize>, MediaError> {
    let end = offset.checked_add(n as u64).ok_or(MediaError::OutOfRange)?;
    if end > len {
        return Err(MediaError::OutOfRange);
    }
    Ok(offset as usize..end as usize)
}

/// Bytes in memory (tests, wasm, generated images).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryBackend {
    pub data: Vec<u8>,
}

impl MediaBackend for MemoryBackend {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let r = range(self.len(), offset, buf.len())?;
        buf.copy_from_slice(&self.data[r]);
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let r = range(self.len(), offset, data.len())?;
        self.data[r].copy_from_slice(data);
        Ok(())
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// An image file on the host. The content is read once; writes go straight
/// to the file.
#[derive(Debug)]
pub struct FileBackend {
    path: PathBuf,
    data: Vec<u8>,
}

fn io_err(e: std::io::Error) -> MediaError {
    MediaError::Backend(e.to_string())
}

impl FileBackend {
    pub fn open(path: &Path) -> Result<Self, MediaError> {
        Ok(Self {
            path: path.to_path_buf(),
            data: fs::read(path).map_err(io_err)?,
        })
    }
}

impl MediaBackend for FileBackend {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let r = range(self.len(), offset, buf.len())?;
        buf.copy_from_slice(&self.data[r]);
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let r = range(self.len(), offset, data.len())?;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(io_err)?;
        f.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        f.write_all(data).map_err(io_err)?;
        self.data[r].copy_from_slice(data);
        Ok(())
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

const BLOCK: u64 = 256;

/// Writes collect in memory, in 256-byte blocks over the base; `commit`
/// applies them to the base, `discard` forgets them.
pub struct CowOverlay {
    base: Box<dyn MediaBackend>,
    blocks: BTreeMap<u64, Vec<u8>>,
}

impl CowOverlay {
    pub fn new(base: Box<dyn MediaBackend>) -> Self {
        Self {
            base,
            blocks: BTreeMap::new(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        !self.blocks.is_empty()
    }

    pub fn discard(&mut self) {
        self.blocks.clear();
    }

    fn block(&mut self, index: u64) -> Result<&mut Vec<u8>, MediaError> {
        if !self.blocks.contains_key(&index) {
            let start = index * BLOCK;
            let len = BLOCK.min(self.base.len() - start) as usize;
            let mut buf = vec![0; len];
            self.base.read(start, &mut buf)?;
            self.blocks.insert(index, buf);
        }
        Ok(self
            .blocks
            .get_mut(&index)
            .unwrap_or_else(|| unreachable!()))
    }
}

impl MediaBackend for CowOverlay {
    fn len(&self) -> u64 {
        self.base.len()
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        range(self.len(), offset, buf.len())?;
        self.base.read(offset, buf)?;
        for (&index, data) in self
            .blocks
            .range(offset / BLOCK..=(offset + buf.len() as u64) / BLOCK)
        {
            let start = index * BLOCK;
            for (i, &b) in data.iter().enumerate() {
                let at = start + i as u64;
                if at >= offset && at < offset + buf.len() as u64 {
                    buf[(at - offset) as usize] = b;
                }
            }
        }
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        range(self.len(), offset, data.len())?;
        for (i, &b) in data.iter().enumerate() {
            let at = offset + i as u64;
            let block = self.block(at / BLOCK)?;
            block[(at % BLOCK) as usize] = b;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        for (index, data) in std::mem::take(&mut self.blocks) {
            self.base.write(index * BLOCK, &data)?;
        }
        self.base.commit()
    }
}

/// Media kind from a file name extension.
pub fn kind_for(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "d64" | "d71" | "d81" | "g64" => Some(MediaKind::Disk),
        "prg" | "p00" | "t64" => Some(MediaKind::Program),
        "tap" => Some(MediaKind::Tape),
        "crt" => Some(MediaKind::Cartridge),
        _ => None,
    }
}

/// Open an image file as media behind a copy-on-write overlay.
pub fn open(path: &Path) -> Result<Media, MediaError> {
    let kind = kind_for(path)
        .ok_or_else(|| MediaError::Format(format!("unknown image type: {}", path.display())))?;
    let file = FileBackend::open(path)?;
    Ok(Media {
        name: path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        kind,
        backend: Box::new(CowOverlay::new(Box::new(file))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(n: usize) -> Box<MemoryBackend> {
        Box::new(MemoryBackend {
            data: (0..n).map(|i| i as u8).collect(),
        })
    }

    #[test]
    fn overlay_keeps_the_base_until_commit() {
        let mut cow = CowOverlay::new(mem(1000));
        cow.write(250, &[0xaa; 10]).unwrap();
        let mut buf = [0u8; 20];
        cow.read(245, &mut buf).unwrap();
        assert_eq!(&buf[..5], &[245, 246, 247, 248, 249]);
        assert_eq!(&buf[5..15], &[0xaa; 10]);
        assert_eq!(buf[15], 4, "byte 260 untouched");
        assert!(cow.is_dirty());

        let mut base = [0u8; 1];
        cow.base.read(250, &mut base).unwrap();
        assert_eq!(base[0], 250, "base unchanged before commit");
        cow.commit().unwrap();
        cow.base.read(250, &mut base).unwrap();
        assert_eq!(base[0], 0xaa);
        assert!(!cow.is_dirty());
    }

    #[test]
    fn discard_and_bounds() {
        let mut cow = CowOverlay::new(mem(300));
        cow.write(299, &[1]).unwrap();
        cow.discard();
        let mut b = [0u8; 1];
        cow.read(299, &mut b).unwrap();
        assert_eq!(b[0], 43);
        assert_eq!(cow.write(299, &[1, 2]), Err(MediaError::OutOfRange));
        assert_eq!(cow.read(u64::MAX, &mut b), Err(MediaError::OutOfRange));
    }

    #[test]
    fn file_backend_through_overlay() {
        let dir = std::env::temp_dir().join(format!("polytrace-media-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.d64");
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let mut media = open(&path).unwrap();
        assert_eq!(media.kind, MediaKind::Disk);
        assert_eq!(media.name, "game.d64");
        media.backend.write(1, &[9]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), [1, 2, 3, 4], "original untouched");
        media.backend.commit().unwrap();
        assert_eq!(fs::read(&path).unwrap(), [1, 9, 3, 4]);
        assert!(open(&dir.join("x.unknown")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
