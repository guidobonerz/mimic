//! [`HookedBus`]: routes guest accesses of one memory space through
//! [`DebugHooks`] (watchpoints, freezes, raster trace). Machines wrap their
//! CPU bus in it for each cycle; with nothing armed it costs one length
//! check per access (golden rule 5).

use crate::beam::BeamPos;
use crate::bus::{Bus, Size};
use crate::debug::DebugHooks;
use crate::machine::MemSpaceId;

pub struct HookedBus<'a> {
    pub inner: &'a mut dyn Bus,
    pub dbg: Option<&'a mut DebugHooks>,
    pub space: MemSpaceId,
    /// Beam position of the current cycle, for the raster trace.
    pub beam: BeamPos,
}

impl Bus for HookedBus<'_> {
    #[inline]
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = self.inner.read(addr, size);
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(self.space) => {
                let seen = dbg.on_read(self.space, addr, size, v);
                dbg.trace_read(self.space, addr, size, seen, self.beam);
                seen
            }
            _ => v,
        }
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.inner.peek(addr, size)
    }

    #[inline]
    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(self.space) {
                dbg.trace_write(self.space, addr, size, v & size.mask(), self.beam);
                if !dbg.on_write(self.space, addr, size, v & size.mask()) {
                    return;
                }
            }
        }
        self.inner.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        self.inner.addr_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beam::{TraceConfig, TraceRange};
    use crate::freeze::Freeze;
    use crate::testing::FlatBus;

    const S: MemSpaceId = MemSpaceId(0);

    #[test]
    fn passes_through_without_hooks() {
        let mut mem = FlatBus::new(16);
        let mut bus = HookedBus {
            inner: &mut mem,
            dbg: None,
            space: S,
            beam: BeamPos::default(),
        };
        bus.write(0x10, Size::Byte, 0x42);
        assert_eq!(bus.read(0x10, Size::Byte), 0x42);
    }

    #[test]
    fn freezes_and_trace_apply() {
        let mut mem = FlatBus::new(16);
        let mut dbg = DebugHooks::new();
        dbg.add_freeze(Freeze::new(S, 0x20, Size::Byte, 7));
        dbg.set_trace(TraceConfig {
            writes: vec![TraceRange {
                space: S,
                addr: 0x20,
                len: 1,
            }],
            ..TraceConfig::default()
        });
        {
            let mut bus = HookedBus {
                inner: &mut mem,
                dbg: Some(&mut dbg),
                space: S,
                beam: BeamPos::default(),
            };
            bus.write(0x20, Size::Byte, 1);
            assert_eq!(bus.read(0x20, Size::Byte), 7, "frozen value");
        }
        assert_eq!(mem.mem[0x20], 0, "write blocked");
        assert_eq!(dbg.drain_trace().len(), 1, "blocked write still traced");
    }
}
