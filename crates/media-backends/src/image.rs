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

    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        self.data
            .resize(usize::try_from(len).map_err(|_| MediaError::OutOfRange)?, 0);
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

    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        let n = usize::try_from(len).map_err(|_| MediaError::OutOfRange)?;
        fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .and_then(|f| f.set_len(len))
            .map_err(io_err)?;
        self.data.resize(n, 0);
        Ok(())
    }
}

const BLOCK: u64 = 256;

/// Writes collect in memory, in 256-byte blocks over the base; `commit`
/// applies them to the base, `discard` forgets them.
/// The overlay can also change the medium's size (`set_len`); `commit`
/// then resizes the base first.
pub struct CowOverlay {
    base: Box<dyn MediaBackend>,
    /// Full blocks; bytes past the base (a grown medium) start as 0.
    blocks: BTreeMap<u64, Vec<u8>>,
    /// The medium's size as the guest sees it.
    len: u64,
}

impl CowOverlay {
    pub fn new(base: Box<dyn MediaBackend>) -> Self {
        let len = base.len();
        Self {
            base,
            blocks: BTreeMap::new(),
            len,
        }
    }

    pub fn is_dirty(&self) -> bool {
        !self.blocks.is_empty() || self.len != self.base.len()
    }

    pub fn discard(&mut self) {
        self.blocks.clear();
        self.len = self.base.len();
    }

    fn block(&mut self, index: u64) -> Result<&mut Vec<u8>, MediaError> {
        if !self.blocks.contains_key(&index) {
            let start = index * BLOCK;
            let mut buf = vec![0; BLOCK as usize];
            let from_base = BLOCK.min(self.base.len().saturating_sub(start)) as usize;
            if from_base > 0 {
                self.base.read(start, &mut buf[..from_base])?;
            }
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
        self.len
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        range(self.len(), offset, buf.len())?;
        // The part the base holds; a grown medium reads 0 past it.
        let in_base = self.base.len().saturating_sub(offset).min(buf.len() as u64) as usize;
        buf[in_base..].fill(0);
        if in_base > 0 {
            self.base.read(offset, &mut buf[..in_base])?;
        }
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
        if self.len != self.base.len() {
            self.base.set_len(self.len)?;
        }
        for (index, data) in std::mem::take(&mut self.blocks) {
            let start = index * BLOCK;
            let n = BLOCK.min(self.len.saturating_sub(start)) as usize;
            if n > 0 {
                self.base.write(start, &data[..n])?;
            }
        }
        self.base.commit()
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        if len < self.len {
            // Bytes cut off read 0 if the medium grows again.
            self.blocks.retain(|&i, _| i * BLOCK < len);
            if let Some((&i, data)) = self.blocks.iter_mut().next_back() {
                let keep = (len - i * BLOCK).min(BLOCK) as usize;
                data[keep..].fill(0);
            }
            // A base byte past a cut that grows back must read 0 as well:
            // cover the base's rest with zero blocks.
            let mut at = len;
            while at < self.base.len() {
                self.block(at / BLOCK)?;
                at = (at / BLOCK + 1) * BLOCK;
            }
            for (&i, data) in &mut self.blocks {
                let start = i * BLOCK;
                for (k, b) in data.iter_mut().enumerate() {
                    if start + k as u64 >= len {
                        *b = 0;
                    }
                }
            }
        }
        self.len = len;
        Ok(())
    }
}

/// Media kind from a file name's extension (a name without a folder).
pub fn kind_for_name(name: &str) -> Option<MediaKind> {
    kind_for(Path::new(name))
}

/// Media kind from a file name extension.
pub fn kind_for(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "d64" | "d71" | "d81" | "d80" | "d82" | "g64" | "atr" | "dsk" | "adf" | "dms" => {
            Some(MediaKind::Disk)
        }
        // Apple II images; Macintosh floppy (raw, DiskCopy 4.2) and hard
        // disk images.
        "do" | "po" | "nib" | "woz" | "img" | "image" | "dc42" | "diskcopy42" | "hda" | "hdv"
        | "hfv" => Some(MediaKind::Disk),
        // ZX Microdrive cartridges, TR-DOS disks.
        "mdr" | "trd" => Some(MediaKind::Disk),
        // ZX Spectrum snapshots.
        "prg" | "p00" | "t64" | "xex" | "com" | "exe" | "sna" | "z80" => Some(MediaKind::Program),
        // Binaries and Intel HEX for machines loaded straight into memory.
        "bin" | "hex" => Some(MediaKind::Program),
        // "ptp": paper tape.
        "tap" | "cas" | "cdt" | "tzx" | "p" | "81" | "ptp" => Some(MediaKind::Tape),
        "crt" | "car" | "a26" | "vec" => Some(MediaKind::Cartridge),
        // Punched-card decks: binary columns, text, hex columns.
        "deck" | "crd" | "chx" => Some(MediaKind::Cards),
        _ => None,
    }
}

/// A card deck file for a reader's hopper or a punch's stacker. A punch
/// writes into a deck that does not exist yet: a missing file is created
/// empty. The deck sits behind a copy-on-write overlay like every image;
/// committing writes the punched cards to the file.
pub fn open_cards(path: &Path) -> Result<Media, MediaError> {
    if kind_for(path) != Some(MediaKind::Cards) {
        return Err(MediaError::Format(
            "not a card deck (.deck, .crd, .chx)".into(),
        ));
    }
    if !path.exists() {
        fs::write(path, []).map_err(io_err)?;
    }
    open(path)
}

/// A tape for the datasette of machine `machine_id`: a TAP image (behind
/// a copy-on-write overlay), or a PRG recorded as the machine's KERNAL
/// saves it (C16/Plus/4 format for "c16" and "plus4", the PET/VIC-20/C64
/// format otherwise), named after the file.
pub fn open_tape(path: &Path, machine_id: &str) -> Result<Media, MediaError> {
    let is_prg = path
        .extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("prg"));
    if !is_prg {
        let media = open(path)?;
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        return Ok(media);
    }
    let prg = std::fs::read(path).map_err(|e| MediaError::Backend(e.to_string()))?;
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    tape_from_bytes(&name, prg, machine_id)
}

/// [`open_tape`] for bytes that did not come from a file (the browser): a
/// file `name` ending in `.prg` is recorded as the machine saves it, other
/// names are taken as tape images.
pub fn tape_from_bytes(name: &str, data: Vec<u8>, machine_id: &str) -> Result<Media, MediaError> {
    use media::tap::{encode_cbm, encode_ted, TapMachine, Video};
    let path = Path::new(name);
    let is_prg = path
        .extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("prg"));
    if !is_prg {
        return Ok(from_bytes(name, data, MediaKind::Tape));
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    let tape_name: Vec<u8> = stem
        .bytes()
        .filter(|c| c.is_ascii_graphic() || *c == b' ')
        .take(16)
        .collect();
    let tap = match machine_id {
        "c16" | "plus4" => encode_ted(&data, &tape_name, true, Video::Pal)?,
        id => {
            let machine = match id {
                "vic20" => TapMachine::Vic20,
                "pet" => TapMachine::Pet,
                _ => TapMachine::C64,
            };
            encode_cbm(&data, &tape_name, true, machine, Video::Pal)?
        }
    };
    Ok(Media {
        name: format!("{stem}.tap"),
        kind: MediaKind::Tape,
        backend: Box::new(MemoryBackend {
            data: tap.to_bytes()?,
        }),
    })
}

/// Media from bytes in memory (uploaded in the browser) as `kind`, behind
/// a copy-on-write overlay like files: writes stay in the overlay.
pub fn from_bytes(name: &str, data: Vec<u8>, kind: MediaKind) -> Media {
    Media {
        name: name.rsplit(['/', '\\']).next().unwrap_or(name).to_string(),
        kind,
        backend: Box::new(CowOverlay::new(Box::new(MemoryBackend { data }))),
    }
}

/// A cartridge image behind a copy-on-write overlay: a CRT file, or a raw
/// dump in whatever form the machine's expansion port takes (VIC-20 ROM
/// with load address, C16/Plus/4 16/32 KB ROM), which its slot checks.
pub fn open_cart(path: &Path) -> Result<Media, MediaError> {
    let file = FileBackend::open(path)?;
    Ok(Media {
        name: path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        kind: MediaKind::Cartridge,
        backend: Box::new(CowOverlay::new(Box::new(file))),
    })
}

/// Open an image file as media behind a copy-on-write overlay; a folder
/// opens as a disk of its PRG files (see [`crate::host_dir`]).
pub fn open(path: &Path) -> Result<Media, MediaError> {
    if path.is_dir() {
        return crate::host_dir::open(path);
    }
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

    #[test]
    fn tapes_from_tap_and_prg_files() {
        let dir = std::env::temp_dir().join(format!("mimic-tape-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prg = dir.join("hello.prg");
        std::fs::write(&prg, [0x01, 0x08, 0x60]).unwrap();
        let mut m = open_tape(&prg, "c64").unwrap();
        assert_eq!((m.kind, m.name.as_str()), (MediaKind::Tape, "HELLO.tap"));
        let mut bytes = vec![0; m.backend.len() as usize];
        m.backend.read(0, &mut bytes).unwrap();
        let tap = media::tap::Tap::parse(&bytes).unwrap();
        assert_eq!(tap.machine, media::tap::TapMachine::C64);
        let ted = open_tape(&prg, "plus4").unwrap();
        assert!(!ted.backend.is_empty());
        let tap_file = dir.join("t.tap");
        std::fs::write(&tap_file, &bytes).unwrap();
        assert_eq!(open_tape(&tap_file, "c64").unwrap().kind, MediaKind::Tape);
        let d64 = dir.join("x.d64");
        std::fs::write(&d64, media::D64::blank(b"X", *b"XX").to_bytes()).unwrap();
        assert!(matches!(
            open_tape(&d64, "c64"),
            Err(MediaError::Unsupported(MediaKind::Disk))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
        let dir = std::env::temp_dir().join(format!("mimic-media-{}", std::process::id()));
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

    #[test]
    fn the_overlay_grows_and_shrinks() {
        let mut cow = CowOverlay::new(mem(300));
        cow.set_len(700).unwrap();
        assert_eq!(cow.len(), 700);
        cow.write(650, &[7; 50]).unwrap();
        let mut buf = vec![1; 400];
        cow.read(300, &mut buf).unwrap();
        assert!(buf[..350].iter().all(|&b| b == 0), "grown bytes read 0");
        assert!(buf[350..].iter().all(|&b| b == 7));
        // Cut into the base, grow again: the cut bytes read 0.
        cow.set_len(100).unwrap();
        cow.set_len(300).unwrap();
        let mut buf = vec![1; 200];
        cow.read(100, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 0));
        cow.set_len(500).unwrap();
        cow.write(400, &[9; 100]).unwrap();
        cow.commit().unwrap();
        assert_eq!(cow.len(), 500);
        assert!(!cow.is_dirty());
        let mut all = vec![0; 500];
        cow.read(0, &mut all).unwrap();
        assert!(all[100..400].iter().all(|&b| b == 0));
        assert!(all[400..].iter().all(|&b| b == 9));
    }

    #[test]
    fn memory_and_file_backends_change_size() {
        let mut m = MemoryBackend { data: vec![5; 10] };
        m.set_len(20).unwrap();
        assert_eq!(m.data[10..], [0; 10]);
        let dir = std::env::temp_dir().join(format!("mimic-setlen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("d.deck");
        std::fs::write(&path, [1u8; 10]).unwrap();
        let mut f = FileBackend::open(&path).unwrap();
        f.set_len(4).unwrap();
        f.set_len(6).unwrap();
        f.write(4, &[2, 3]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), [1, 1, 1, 1, 2, 3]);
        assert_eq!(kind_for(&path), Some(MediaKind::Cards));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
