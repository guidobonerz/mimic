//! Software memory freezer: values pinned at a memory-space address.
//!
//! Freezes are managed by [`crate::debug::DebugHooks`], which shares the
//! per-space bitmap mechanism with watchpoints.

use crate::bus::Size;
use crate::machine::MemSpaceId;
use crate::savestate::{StateError, StateReader, StateWriter};

/// How a freeze keeps its value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FreezeMode {
    /// Guest writes are suppressed at bus level; reads return the frozen value.
    #[default]
    BlockWrites,
    /// The value is written at the start of every frame (classic trainer POKE).
    ReapplyPerFrame,
    /// The value is forced only when read (I/O-mapped or bank-switched addresses).
    ReapplyOnRead,
}

/// A frozen value, bound to a memory space + address (not a CPU address), so
/// it respects banking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Freeze {
    pub space: MemSpaceId,
    pub addr: u32,
    pub size: Size,
    pub value: u32,
    pub mode: FreezeMode,
    pub enabled: bool,
    /// User label, e.g. "Lives".
    pub label: Option<String>,
}

impl Freeze {
    /// An enabled, unlabeled [`FreezeMode::BlockWrites`] freeze.
    pub fn new(space: MemSpaceId, addr: u32, size: Size, value: u32) -> Self {
        Self {
            space,
            addr,
            size,
            value: value & size.mask(),
            mode: FreezeMode::default(),
            enabled: true,
            label: None,
        }
    }

    pub fn write(&self, w: &mut StateWriter) {
        w.u16(self.space.0);
        w.u32(self.addr);
        w.u8(size_to_u8(self.size));
        w.u32(self.value);
        w.u8(match self.mode {
            FreezeMode::BlockWrites => 0,
            FreezeMode::ReapplyPerFrame => 1,
            FreezeMode::ReapplyOnRead => 2,
        });
        w.bool(self.enabled);
        w.bool(self.label.is_some());
        if let Some(label) = &self.label {
            w.str(label);
        }
    }

    pub fn read(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let space = MemSpaceId(r.u16()?);
        let addr = r.u32()?;
        let size = size_from_u8(r.u8()?)?;
        let value = r.u32()?;
        let mode = match r.u8()? {
            0 => FreezeMode::BlockWrites,
            1 => FreezeMode::ReapplyPerFrame,
            2 => FreezeMode::ReapplyOnRead,
            _ => return Err(StateError::Invalid("freeze mode")),
        };
        let enabled = r.bool()?;
        let label = if r.bool()? { Some(r.string()?) } else { None };
        Ok(Self { space, addr, size, value, mode, enabled, label })
    }
}

pub(crate) fn size_to_u8(size: Size) -> u8 {
    match size {
        Size::Byte => 1,
        Size::Word => 2,
        Size::Long => 4,
    }
}

pub(crate) fn size_from_u8(v: u8) -> Result<Size, StateError> {
    match v {
        1 => Ok(Size::Byte),
        2 => Ok(Size::Word),
        4 => Ok(Size::Long),
        _ => Err(StateError::Invalid("access size")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_round_trip() {
        let mut f = Freeze::new(MemSpaceId(2), 0x0801, Size::Word, 0x1_2345);
        f.mode = FreezeMode::ReapplyOnRead;
        f.enabled = false;
        f.label = Some("Energy".into());
        assert_eq!(f.value, 0x2345, "value is masked to the access size");

        let mut w = StateWriter::new();
        f.write(&mut w);
        let data = w.into_bytes();
        let back = Freeze::read(&mut StateReader::new(&data)).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn bad_mode_is_rejected() {
        let mut w = StateWriter::new();
        Freeze::new(MemSpaceId(0), 0, Size::Byte, 0).write(&mut w);
        let mut data = w.into_bytes();
        data[11] = 9; // mode byte: u16 space + u32 addr + u8 size + u32 value
        assert_eq!(
            Freeze::read(&mut StateReader::new(&data)),
            Err(StateError::Invalid("freeze mode"))
        );
    }
}
