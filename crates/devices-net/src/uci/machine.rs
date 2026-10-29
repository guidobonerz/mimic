//! What the UCI reaches of the machine around it. On the Ultimate the
//! interface sits in the FPGA next to the cartridge logic, the drives and
//! the REU, so its commands DMA into the C64's memory, mount disks, switch
//! drives and change the palette. The machine lends these to a running
//! command through [`UciMachine`]; the UCI never sees a concrete machine.

use emu_core::host::{lock, FsError, OpenMode, SharedFiles};
use emu_core::media::{MediaBackend, MediaError};

/// A drive as GET_DRVINFO reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UciDrive {
    /// [`drive_kind`] value.
    pub kind: u8,
    /// IEC device number.
    pub address: u8,
    pub powered: bool,
}

/// Drive types of GET_DRVINFO (Ultimate documentation, "Control Target").
pub mod drive_kind {
    pub const C1541: u8 = 0x00;
    pub const C1571: u8 = 0x01;
    pub const C1581: u8 = 0x02;
    pub const UNDECIDED: u8 = 0x03;
    pub const SOFTWARE_IEC: u8 = 0x0f;
    pub const PRINTER: u8 = 0x50;
}

/// A SID as GET_HWINFO 01 reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UciSid {
    /// Primary base address.
    pub base: u16,
    /// Secondary base address (0: none).
    pub second: u16,
    /// Type byte; its meaning is undocumented (see the control target).
    pub kind: u8,
}

/// The machine's side. Every method has a default for machines (and tests)
/// without that part: no memory, no REU, no drives.
pub trait UciMachine {
    /// DMA read through the memory map the CPU sees now.
    fn dma_read(&mut self, _addr: u16) -> u8 {
        0xff
    }

    /// DMA write through the memory map the CPU sees now.
    fn dma_write(&mut self, _addr: u16, _v: u8) {}

    /// The 64 KB of RAM, whatever is banked in (U64_SAVEMEM).
    fn ram(&self) -> Option<&[u8]> {
        None
    }

    /// The REU's memory, if one is enabled.
    fn reu(&mut self) -> Option<&mut [u8]> {
        None
    }

    /// Drive A, drive B … in the Ultimate's order.
    fn drives(&self) -> Vec<UciDrive> {
        Vec::new()
    }

    /// Switch drive `index` (0 = A) on or off; `false` if there is none.
    fn set_drive_power(&mut self, _index: usize, _on: bool) -> bool {
        false
    }

    /// Insert `media` into the drive with IEC number `address`.
    fn mount(&mut self, _address: u8, _media: emu_core::media::Media) -> Result<(), MediaError> {
        Err(MediaError::NoMedia)
    }

    /// Eject the disk of the drive with IEC number `address`.
    fn unmount(&mut self, _address: u8) -> bool {
        false
    }

    /// REBOOT: reset the C64 (and with it the UCI) after this command.
    fn reboot(&mut self) {}

    /// EASYFLASH_ERASE: set the 64 KB sector holding 16 KB bank `bank` of
    /// ROML (`high = false`) or ROMH to `$FF`; `false` without an EasyFlash.
    fn erase_flash(&mut self, _bank: u8, _high: bool) -> bool {
        false
    }

    fn sids(&self) -> Vec<UciSid> {
        Vec::new()
    }

    /// The 16 colours as RGB triples, if the machine has a palette.
    fn palette(&self) -> Option<[u8; 48]> {
        None
    }

    fn set_palette(&mut self, _rgb: &[u8; 48]) -> bool {
        false
    }

    /// Back to the machine's own palette.
    fn reset_palette(&mut self) -> bool {
        false
    }
}

/// No machine around the interface (unit tests, [`super::Uci::tick`]).
pub struct NoMachine;

impl UciMachine for NoMachine {}

/// A disk image from the UCI's folder: kept in memory while mounted,
/// written back through the folder's [`emu_core::host::FileHost`] on
/// commit (the drive's flush), as the Ultimate writes to its card.
pub struct FolderImage {
    files: SharedFiles,
    path: String,
    data: Vec<u8>,
    dirty: bool,
}

impl FolderImage {
    /// Read the whole file at `path`.
    pub fn open(files: &SharedFiles, path: &str) -> Result<Self, FsError> {
        let data = read_file(&mut *lock(files), path)?;
        Ok(Self {
            files: files.clone(),
            path: path.into(),
            data,
            dirty: false,
        })
    }
}

/// A whole file of a folder.
pub fn read_file(f: &mut dyn emu_core::host::FileHost, path: &str) -> Result<Vec<u8>, FsError> {
    let h = f.open(
        path,
        OpenMode {
            read: true,
            ..Default::default()
        },
    )?;
    let mut data = Vec::new();
    let mut buf = vec![0; 16384];
    let r = loop {
        match f.read(h, &mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(e) => break Err(e),
        }
    };
    f.close(h);
    r.map(|()| data)
}

/// Create or replace the file at `path` with `data`.
pub fn write_file(
    f: &mut dyn emu_core::host::FileHost,
    path: &str,
    data: &[u8],
) -> Result<(), FsError> {
    let h = f.open(
        path,
        OpenMode {
            write: true,
            create: true,
            truncate: true,
            ..Default::default()
        },
    )?;
    let r = f.write(h, data).and_then(|n| {
        if n == data.len() {
            Ok(())
        } else {
            Err(FsError::Io("short write".into()))
        }
    });
    f.close(h);
    r
}

impl MediaBackend for FolderImage {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let start = usize::try_from(offset).map_err(|_| MediaError::OutOfRange)?;
        let src = start
            .checked_add(buf.len())
            .and_then(|end| self.data.get(start..end))
            .ok_or(MediaError::OutOfRange)?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let start = usize::try_from(offset).map_err(|_| MediaError::OutOfRange)?;
        let end = start
            .checked_add(data.len())
            .ok_or(MediaError::OutOfRange)?;
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(data);
        self.dirty = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        if !self.dirty {
            return Ok(());
        }
        write_file(&mut *lock(&self.files), &self.path, &self.data)
            .map_err(|e| MediaError::Backend(e.to_string()))?;
        self.dirty = false;
        Ok(())
    }
}
