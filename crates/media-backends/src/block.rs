//! Block media for a machine's storage slots (the Ultimate 64 Elite II's
//! SD card and USB sticks): an image file, or a host folder shown as a
//! FAT32 volume ([`crate::fat_dir`]). The frontends' `--sd` and `--usb`.

use std::path::Path;

use mimic_core::media::{Media, MediaError, MediaKind, SlotId};
use mimic_core::Machine;

use crate::image::{CowOverlay, FileBackend};

/// Open `path` as a block medium: a folder becomes a FAT32 volume of its
/// files (written back into the folder), a file an image behind a
/// copy-on-write overlay (written into the file when the slot flushes).
pub fn open(path: &Path) -> Result<Media, MediaError> {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    if path.is_dir() {
        let backend = crate::fat_dir::FatDirBackend::open(path, None)?;
        return Ok(Media {
            name: format!("{name}/"),
            kind: MediaKind::Disk,
            backend: Box::new(backend),
        });
    }
    let file = FileBackend::open(path)?;
    Ok(Media {
        name,
        kind: MediaKind::Disk,
        backend: Box::new(CowOverlay::new(Box::new(file))),
    })
}

/// The machine's slot named `name` ("sd card", "usb 1" …).
pub fn slot_named(m: &dyn Machine, name: &str) -> Option<SlotId> {
    m.media_slots()
        .into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .map(|s| s.id)
}

/// Put `path` into the slot `name`; an error names what went wrong.
pub fn insert(m: &mut dyn Machine, name: &str, path: &str) -> Result<(), String> {
    let slot = slot_named(m, name).ok_or_else(|| format!("this machine has no slot '{name}'"))?;
    let media = open(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    m.media_slot(slot)
        .ok_or_else(|| format!("no slot '{name}'"))?
        .insert(media)
        .map_err(|e| format!("{path}: {e}"))
}

/// The frontends' `--sd PATH` and `--usb PATH` (up to three, on the
/// ports in order).
pub fn insert_all(m: &mut dyn Machine, sd: Option<&str>, usb: &[String]) -> Result<(), String> {
    if let Some(p) = sd {
        insert(m, "sd card", p)?;
    }
    if usb.len() > 3 {
        return Err(format!("{} USB media for 3 ports", usb.len()));
    }
    for (i, p) in usb.iter().enumerate() {
        insert(m, &format!("usb {}", i + 1), p)?;
    }
    Ok(())
}
