//! Beam position and raster trace.
//!
//! Every machine with a video beam reports where the beam is
//! ([`crate::Machine::beam`]). The debugger uses it for raster breakpoints,
//! partial frames and overlays; the raster trace stamps instructions and
//! memory accesses with it, so a trace shows *when* in the frame a `$D020`
//! write or a `$D019` poll happened.

use std::collections::VecDeque;

use crate::bus::Size;
use crate::machine::{DeviceId, MemSpaceId};

/// Position of the video beam, in the units of the machine's video chip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BeamPos {
    /// Field counter since power-on.
    pub frame: u64,
    /// Raster line, including vertical blank; 0 is the first line of a field.
    pub line: u16,
    /// CPU/DMA cycle within the line (C64: 0–62 PAL, Amiga: colour clocks).
    pub cycle: u16,
    /// Horizontal pixel position, matching the pixel columns of [`crate::FrameRef`].
    pub hpos: u16,
}

/// An address range to trace in one memory space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceRange {
    pub space: MemSpaceId,
    pub addr: u32,
    pub len: u32,
}

/// What the raster trace records. Empty (the default) means off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceConfig {
    /// Every executed instruction of every device.
    pub instructions: bool,
    pub reads: Vec<TraceRange>,
    pub writes: Vec<TraceRange>,
    /// Ring buffer size; the oldest entries are dropped when full.
    pub capacity: usize,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            instructions: false,
            reads: Vec::new(),
            writes: Vec::new(),
            capacity: 65_536,
        }
    }
}

impl TraceConfig {
    pub fn is_off(&self) -> bool {
        !self.instructions && self.reads.is_empty() && self.writes.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// Instruction starting at `pc`.
    Instr { device: DeviceId, pc: u32 },
    Read {
        space: MemSpaceId,
        addr: u32,
        size: Size,
        value: u32,
    },
    Write {
        space: MemSpaceId,
        addr: u32,
        size: Size,
        value: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceEntry {
    pub beam: BeamPos,
    pub event: TraceEvent,
}

/// Bounded trace buffer. Debug-only: never part of machine state.
#[derive(Clone, Debug, Default)]
pub struct TraceBuffer {
    entries: VecDeque<TraceEntry>,
    capacity: usize,
    dropped: u64,
}

impl TraceBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            dropped: 0,
        }
    }

    pub fn push(&mut self, entry: TraceEntry) {
        if self.capacity == 0 {
            self.dropped += 1;
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.dropped += 1;
        }
        self.entries.push_back(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries lost to the ring buffer since the last [`TraceBuffer::drain`].
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Remove and return all entries, oldest first, and reset the drop counter.
    pub fn drain(&mut self) -> Vec<TraceEntry> {
        self.dropped = 0;
        self.entries.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(line: u16) -> TraceEntry {
        TraceEntry {
            beam: BeamPos {
                line,
                ..BeamPos::default()
            },
            event: TraceEvent::Instr {
                device: DeviceId("cpu"),
                pc: 0,
            },
        }
    }

    #[test]
    fn ring_buffer_drops_oldest() {
        let mut t = TraceBuffer::new(2);
        for line in 0..5 {
            t.push(entry(line));
        }
        assert_eq!(t.dropped(), 3);
        let lines: Vec<u16> = t.drain().iter().map(|e| e.beam.line).collect();
        assert_eq!(lines, [3, 4]);
        assert_eq!(t.dropped(), 0);
        assert!(t.is_empty());
    }

    #[test]
    fn beam_positions_order_by_frame_then_line_then_cycle() {
        let a = BeamPos {
            frame: 1,
            line: 0,
            cycle: 0,
            hpos: 0,
        };
        let b = BeamPos {
            frame: 0,
            line: 300,
            cycle: 62,
            hpos: 400,
        };
        assert!(b < a);
    }

    #[test]
    fn default_config_is_off() {
        assert!(TraceConfig::default().is_off());
    }
}
