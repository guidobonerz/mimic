//! Getting programs into the PET without a drive: the "program" media
//! slot (PRG or T64), put into RAM at a frame boundary once BASIC is up.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use media::{t64, Prg};

/// Cycles after power-on before a program is injected: the editor
/// reprograms the CRTC (frame length) during boot, so time is counted in
/// cycles, not frames.
pub const BOOT_CYCLES: u64 = 3_000_000;

/// BASIC programs start at `$0401` on every PET.
pub const BASIC_START: u16 = 0x0401;

/// The "program" slot: holds a program until the machine injects it.
#[derive(Default)]
pub struct ProgramSlot {
    pub pending: Option<Prg>,
    name: Option<String>,
}

impl ProgramSlot {
    pub fn inserted(&self) -> bool {
        self.name.is_some()
    }
}

impl MediaSlot for ProgramSlot {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Program {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let prg = if bytes.starts_with(b"C64") {
            t64::parse(&bytes)?
                .into_iter()
                .next()
                .ok_or_else(|| MediaError::Format("T64 without files".into()))?
                .prg
        } else {
            Prg::parse(&bytes)?
        };
        self.pending = Some(prg);
        self.name = Some(media.name);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.pending = None;
        self.name = None;
        None
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}
