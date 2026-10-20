//! Getting programs into the VIC-20 without a drive: the "program" media
//! slot (PRG or T64, put into RAM at frame boundaries once BASIC is up)
//! and disk autostart (type `LOAD"*",8` / `RUN`).
//!
//! VIC-20 BASIC starts at `$0401`, `$1001` or `$1201` depending on the RAM
//! expansion. A BASIC program saved for another configuration is moved to
//! the current start and its line links are rebuilt, as `LOAD"x",8` does;
//! anything else loads at its own address like `LOAD"x",8,1`.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use media::{t64, Prg};

/// Frames after power-on before a program is injected or typed.
pub const BOOT_FRAMES: u64 = 150;

/// Load addresses of BASIC programs for the three memory layouts.
pub const BASIC_STARTS: [u16; 3] = [0x0401, 0x1001, 0x1201];

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
    /// Waiting for the boot to finish, then type `LOAD"*",8`.
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

/// Rebuild the line links of a BASIC program placed at `start` in `ram`
/// (each line: link, line number, text, 0; the program ends with a zero
/// link). Returns the address after the end marker, or `None` if the
/// program is malformed.
pub fn relink(ram: &mut [u8], start: u16) -> Option<u16> {
    let mut line = usize::from(start);
    loop {
        if line + 1 >= ram.len() {
            return None;
        }
        if ram[line] == 0 && ram[line + 1] == 0 {
            return Some(line as u16 + 2);
        }
        // Text starts after the link and the line number.
        let text = line + 4;
        let end = text + ram.get(text..)?.iter().position(|&b| b == 0)?;
        let next = end + 1;
        if next > 0xffff {
            return None;
        }
        ram[line..line + 2].copy_from_slice(&(next as u16).to_le_bytes());
        line = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ready_after_loading() {
        let mut screen = vec![0x20u8; 506];
        screen[22..29].copy_from_slice(&LOADING);
        assert!(!loaded(&screen));
        screen[66..72].copy_from_slice(&READY);
        assert!(loaded(&screen));
    }

    #[test]
    fn relinks_a_moved_program() {
        // 10 PRINT 1 / 20 END, saved for $1001, placed at $1201.
        let program = [
            0x0b, 0x10, 0x0a, 0x00, 0x99, 0x20, 0x31, 0x00, // 10 PRINT 1
            0x11, 0x10, 0x14, 0x00, 0x80, 0x00, // 20 END
            0x00, 0x00,
        ];
        let mut ram = vec![0u8; 0x10000];
        ram[0x1201..0x1201 + program.len()].copy_from_slice(&program);
        assert_eq!(relink(&mut ram, 0x1201), Some(0x1201 + 16));
        assert_eq!(&ram[0x1201..0x1203], &[0x09, 0x12]);
        assert_eq!(&ram[0x1209..0x120b], &[0x0f, 0x12]);
        assert_eq!(relink(&mut [0u8; 8][..], 7), None);
    }
}
