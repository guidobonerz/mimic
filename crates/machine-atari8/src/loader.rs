//! Program slot: XEX (DOS binary load) files, loaded the way DOS does.
//!
//! Inserting a program powers the machine up with OPTION held (no BASIC)
//! and lets D1: serve a one-sector boot disk ([`boot_sector`]). The OS
//! boots it and calls the boot continuation at [`BOOT_TRAP`]; there the
//! machine takes over: it writes the segments into memory through the
//! CPU's bus (banking applies), calls each INITAD routine a segment sets
//! (returning to [`INIT_TRAP`]) and finally jumps to RUNAD, or to the
//! first segment without one.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::xex::{Segment, Xex};

/// Where the boot sector's continuation (load address + 6) is called.
pub const BOOT_TRAP: u16 = 0x0706;
/// Return address of INITAD calls: an unused I/O address, never executed.
pub const INIT_TRAP: u16 = 0xd5ff;

/// The boot sector: flags 0, one sector, loaded at $0700, DOSINI $0706;
/// the continuation `CLC; RTS` only runs if a RESET calls DOSINI later.
pub fn boot_sector() -> Vec<u8> {
    let mut s = vec![0x00, 0x01, 0x00, 0x07, 0x06, 0x07, 0x18, 0x60];
    s.resize(128, 0);
    s
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the OS to call the boot continuation.
    Boot,
    /// An INITAD routine runs.
    Init,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loader {
    pub segments: Vec<Segment>,
    pub next: usize,
    pub phase: Phase,
}

impl Loader {
    pub fn new(xex: Xex) -> Self {
        Self {
            segments: xex.segments,
            next: 0,
            phase: Phase::Boot,
        }
    }

    /// The PC at which the machine continues loading.
    pub fn trap(&self) -> u16 {
        match self.phase {
            Phase::Boot => BOOT_TRAP,
            Phase::Init => INIT_TRAP,
        }
    }

    pub fn save(this: &Option<Self>, w: &mut StateWriter) {
        w.chunk(*b"XEXL", 1, |w| {
            w.bool(this.is_some());
            if let Some(l) = this {
                w.u8(u8::from(l.phase == Phase::Init));
                w.u32(l.next as u32);
                w.u32(l.segments.len() as u32);
                for s in &l.segments {
                    w.u16(s.start);
                    w.bytes(&s.data);
                }
            }
        });
    }

    pub fn load(r: &mut StateReader<'_>) -> Result<Option<Self>, StateError> {
        let (_, mut c) = r.chunk(*b"XEXL", 1)?;
        if !c.bool()? {
            return Ok(None);
        }
        let phase = if c.u8()? != 0 {
            Phase::Init
        } else {
            Phase::Boot
        };
        let next = c.u32()? as usize;
        let n = c.u32()? as usize;
        if n > 0x10000 || next > n {
            return Err(StateError::Invalid("XEX loader"));
        }
        let mut segments = Vec::with_capacity(n);
        for _ in 0..n {
            let start = c.u16()?;
            segments.push(Segment {
                start,
                data: c.bytes()?.to_vec(),
            });
        }
        Ok(Some(Self {
            segments,
            next,
            phase,
        }))
    }
}

/// The "program" slot: holds an XEX until the machine starts loading it.
#[derive(Default)]
pub struct ProgramSlot {
    pub pending: Option<Xex>,
    name: Option<String>,
    changed: bool,
}

impl ProgramSlot {
    pub fn inserted(&self) -> bool {
        self.name.is_some()
    }

    /// Whether a program went in since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

impl MediaSlot for ProgramSlot {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Program {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let xex = Xex::parse(&bytes)?;
        self.pending = Some(xex);
        self.name = Some(media.name);
        self.changed = true;
        Ok(())
    }

    /// The program stays in memory; the slot only forgets it.
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
