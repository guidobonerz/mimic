//! Getting programs into the C64 without a drive: the "program" media
//! slot (PRG or T64, loaded straight into RAM like `LOAD"x",8,1`) and
//! autostart (type `LOAD"*",8,1` / `RUN` once BASIC is up).
//!
//! Everything happens at frame boundaries after [`BOOT_FRAMES`], when the
//! KERNAL has finished its cold start, and goes through the input queue, so
//! runs stay deterministic.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use media::{t64, Prg};

/// Frames after power-on before a program is injected or typed.
pub const BOOT_FRAMES: u64 = 150;

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

/// Disk autostart progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Autostart {
    #[default]
    Off,
    /// Waiting for the boot to finish, then type `LOAD"*",8,1`.
    Armed,
    /// LOAD typed; type RUN when BASIC reports READY after LOADING.
    Loading,
    Done,
}

impl Autostart {
    pub fn to_u8(self) -> u8 {
        match self {
            Autostart::Off => 0,
            Autostart::Armed => 1,
            Autostart::Loading => 2,
            Autostart::Done => 3,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Autostart::Off,
            1 => Autostart::Armed,
            2 => Autostart::Loading,
            3 => Autostart::Done,
            _ => return None,
        })
    }
}

/// Screen codes of "LOADING" and "READY.".
const LOADING: [u8; 7] = [0x0c, 0x0f, 0x01, 0x04, 0x09, 0x0e, 0x07];
const READY: [u8; 6] = [0x12, 0x05, 0x01, 0x04, 0x19, 0x2e];

/// True if the screen shows READY. below a LOADING line.
pub fn loaded(screen: &[u8]) -> bool {
    let find = |pat: &[u8]| screen.windows(pat.len()).rposition(|w| w == pat);
    match (find(&LOADING), find(&READY)) {
        (Some(l), Some(r)) => r > l,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ready_after_loading() {
        let mut screen = vec![0x20u8; 1000];
        screen[40..47].copy_from_slice(&LOADING);
        assert!(!loaded(&screen));
        screen[120..126].copy_from_slice(&READY);
        assert!(loaded(&screen));
        screen[10..16].copy_from_slice(&READY);
        screen[120..126].fill(0x20);
        assert!(!loaded(&screen), "READY above LOADING does not count");
    }
}
