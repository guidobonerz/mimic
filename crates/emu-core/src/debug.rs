//! Debug hooks: breakpoints, watchpoints and memory freezes.
//!
//! Hot-path contract (golden rule 5): machines ask the cheap `*_armed` queries
//! first and only call the `check_*` / `on_*` methods when something is armed.
//! With nothing armed the cost is one length check per query.
//!
//! All lookup structures are rebuilt from the breakpoint and freeze lists
//! whenever those change. That keeps overlapping breakpoints correct without
//! reference counting; configuration changes are rare compared to lookups.

use std::collections::BTreeMap;

use crate::bus::Size;
use crate::freeze::{Freeze, FreezeMode};
use crate::machine::{MemSpaceId, StopReason};
use crate::savestate::{StateError, StateReader, StateWriter};

const PAGE_BITS: u32 = 12;
const PAGE_ADDRS: usize = 1 << PAGE_BITS;
const PAGE_WORDS: usize = PAGE_ADDRS / 64;

/// Largest watchpoint range; protects against runaway debugger requests.
pub const MAX_RANGE: u32 = 1 << 24;

type Page = [u64; PAGE_WORDS];

/// Sparse one-bit-per-address bitmap. Pages of 4096 addresses are allocated
/// on first use, so a 24-bit Amiga space costs nothing until a bit is set.
#[derive(Clone, Debug, Default)]
pub struct AddrBitmap {
    pages: Vec<Option<Box<Page>>>,
    count: usize,
}

impl AddrBitmap {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Number of set addresses.
    pub fn len(&self) -> usize {
        self.count
    }

    #[inline]
    pub fn test(&self, addr: u32) -> bool {
        match self.pages.get((addr >> PAGE_BITS) as usize) {
            Some(Some(page)) => {
                let bit = addr as usize & (PAGE_ADDRS - 1);
                page[bit / 64] & (1u64 << (bit % 64)) != 0
            }
            _ => false,
        }
    }

    /// True if any address in `addr..addr + len` is set.
    #[inline]
    pub fn test_range(&self, addr: u32, len: u32) -> bool {
        (0..len).any(|i| self.test(addr.wrapping_add(i)))
    }

    /// Set `addr`; returns true if it was not set before.
    pub fn set(&mut self, addr: u32) -> bool {
        let index = (addr >> PAGE_BITS) as usize;
        if self.pages.len() <= index {
            self.pages.resize_with(index + 1, || None);
        }
        let page = self.pages[index].get_or_insert_with(|| Box::new([0; PAGE_WORDS]));
        let bit = addr as usize & (PAGE_ADDRS - 1);
        let mask = 1u64 << (bit % 64);
        let word = &mut page[bit / 64];
        if *word & mask != 0 {
            return false;
        }
        *word |= mask;
        self.count += 1;
        true
    }

    /// Clear `addr`; returns true if it was set before.
    pub fn clear(&mut self, addr: u32) -> bool {
        let index = (addr >> PAGE_BITS) as usize;
        let Some(Some(page)) = self.pages.get_mut(index) else {
            return false;
        };
        let bit = addr as usize & (PAGE_ADDRS - 1);
        let mask = 1u64 << (bit % 64);
        if page[bit / 64] & mask == 0 {
            return false;
        }
        page[bit / 64] &= !mask;
        self.count -= 1;
        if page.iter().all(|&w| w == 0) {
            self.pages[index] = None;
        }
        true
    }

    pub fn clear_all(&mut self) {
        self.pages.clear();
        self.count = 0;
    }
}

/// Breakpoint handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BpId(pub u32);

/// Freeze handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FreezeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptKind {
    Irq,
    Nmi,
}

/// What a breakpoint triggers on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakpointKind {
    /// Instruction fetch at `addr`.
    Exec { space: MemSpaceId, addr: u32 },
    /// Read watchpoint over `addr..addr + len` (also used for chip-register I/O).
    Read { space: MemSpaceId, addr: u32, len: u32 },
    /// Write watchpoint over `addr..addr + len`.
    Write { space: MemSpaceId, addr: u32, len: u32 },
    /// Raster position; `cycle: None` matches the start of the line.
    Raster { line: u16, cycle: Option<u16> },
    /// Interrupt taken by the CPU.
    Interrupt(InterruptKind),
}

/// A breakpoint with its hit bookkeeping. Conditions arrive with the debug
/// server's expression evaluator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub kind: BreakpointKind,
    pub enabled: bool,
    /// Hits to ignore before stopping.
    pub ignore_count: u32,
    pub hits: u32,
    /// Removed after it first stops (used by `run_to`).
    pub temporary: bool,
}

impl Breakpoint {
    pub fn new(kind: BreakpointKind) -> Self {
        Self { kind, enabled: true, ignore_count: 0, hits: 0, temporary: false }
    }
}

/// A guest write suppressed by a [`FreezeMode::BlockWrites`] freeze.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockedWrite {
    pub space: MemSpaceId,
    pub addr: u32,
    pub size: Size,
    pub value: u32,
}

#[derive(Clone, Debug, Default)]
struct SpaceHooks {
    big_endian: bool,
    exec: AddrBitmap,
    read_watch: AddrBitmap,
    write_watch: AddrBitmap,
    /// Reads return the frozen value (BlockWrites and ReapplyOnRead).
    freeze_read: AddrBitmap,
    /// Writes are suppressed (BlockWrites).
    freeze_write: AddrBitmap,
    /// `read_watch | freeze_read`: one test on the read path.
    read_any: AddrBitmap,
    /// `write_watch | freeze_write`: one test on the write path.
    write_any: AddrBitmap,
}

impl SpaceHooks {
    fn clear_bitmaps(&mut self) {
        *self = SpaceHooks { big_endian: self.big_endian, ..SpaceHooks::default() };
    }
}

fn space_mut(spaces: &mut Vec<SpaceHooks>, space: MemSpaceId) -> &mut SpaceHooks {
    let index = space.0 as usize;
    if spaces.len() <= index {
        spaces.resize_with(index + 1, SpaceHooks::default);
    }
    &mut spaces[index]
}

fn overlaps(a: u32, a_len: u32, b: u32, b_len: u32) -> bool {
    let (a0, a1) = (u64::from(a), u64::from(a) + u64::from(a_len));
    let (b0, b1) = (u64::from(b), u64::from(b) + u64::from(b_len));
    a0 < b1 && b0 < a1
}

/// Byte `i` of a value of `n` bytes in the space's byte order.
fn byte_shift(i: u32, n: u32, big_endian: bool) -> u32 {
    if big_endian {
        (n - 1 - i) * 8
    } else {
        i * 8
    }
}

const FREEZE_TAG: [u8; 4] = *b"FRZS";
const FREEZE_VERSION: u16 = 1;

/// Debug state passed to [`crate::Machine::run_frame`].
#[derive(Clone, Debug, Default)]
pub struct DebugHooks {
    spaces: Vec<SpaceHooks>,
    breakpoints: BTreeMap<BpId, Breakpoint>,
    next_bp: u32,
    freezes: BTreeMap<FreezeId, Freeze>,
    next_freeze: u32,
    /// Byte-level frozen values for read/write interception.
    frozen_bytes: BTreeMap<(MemSpaceId, u32), u8>,
    raster_armed: bool,
    irq_armed: bool,
    nmi_armed: bool,
    pending: Option<StopReason>,
    blocked_write: Option<BlockedWrite>,
}

impl DebugHooks {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- configuration -------------------------------------------------

    /// Declare the byte order of a memory space (default little-endian).
    /// Needed so word/long freezes map to the right bytes.
    pub fn set_big_endian(&mut self, space: MemSpaceId, big_endian: bool) {
        space_mut(&mut self.spaces, space).big_endian = big_endian;
        self.rebuild();
    }

    pub fn add_breakpoint(&mut self, kind: BreakpointKind) -> BpId {
        self.insert_breakpoint(Breakpoint::new(kind))
    }

    pub fn insert_breakpoint(&mut self, bp: Breakpoint) -> BpId {
        let id = BpId(self.next_bp);
        self.next_bp = self.next_bp.wrapping_add(1);
        self.breakpoints.insert(id, bp);
        self.rebuild();
        id
    }

    pub fn remove_breakpoint(&mut self, id: BpId) -> Option<Breakpoint> {
        let bp = self.breakpoints.remove(&id);
        if bp.is_some() {
            self.rebuild();
        }
        bp
    }

    /// Returns false if no breakpoint has this id.
    pub fn set_breakpoint_enabled(&mut self, id: BpId, enabled: bool) -> bool {
        let Some(bp) = self.breakpoints.get_mut(&id) else {
            return false;
        };
        bp.enabled = enabled;
        self.rebuild();
        true
    }

    pub fn breakpoint(&self, id: BpId) -> Option<&Breakpoint> {
        self.breakpoints.get(&id)
    }

    pub fn breakpoints(&self) -> impl Iterator<Item = (BpId, &Breakpoint)> {
        self.breakpoints.iter().map(|(id, bp)| (*id, bp))
    }

    pub fn clear_breakpoints(&mut self) {
        self.breakpoints.clear();
        self.rebuild();
    }

    pub fn add_freeze(&mut self, freeze: Freeze) -> FreezeId {
        let id = FreezeId(self.next_freeze);
        self.next_freeze = self.next_freeze.wrapping_add(1);
        self.freezes.insert(id, freeze);
        self.rebuild();
        id
    }

    pub fn remove_freeze(&mut self, id: FreezeId) -> Option<Freeze> {
        let freeze = self.freezes.remove(&id);
        if freeze.is_some() {
            self.rebuild();
        }
        freeze
    }

    /// Returns false if no freeze has this id.
    pub fn set_freeze_enabled(&mut self, id: FreezeId, enabled: bool) -> bool {
        let Some(freeze) = self.freezes.get_mut(&id) else {
            return false;
        };
        freeze.enabled = enabled;
        self.rebuild();
        true
    }

    pub fn freezes(&self) -> impl Iterator<Item = (FreezeId, &Freeze)> {
        self.freezes.iter().map(|(id, f)| (*id, f))
    }

    /// Enabled [`FreezeMode::ReapplyPerFrame`] freezes; the machine writes
    /// them at the start of each frame.
    pub fn per_frame_freezes(&self) -> impl Iterator<Item = &Freeze> {
        self.freezes
            .values()
            .filter(|f| f.enabled && f.mode == FreezeMode::ReapplyPerFrame)
    }

    fn rebuild(&mut self) {
        for space in &mut self.spaces {
            space.clear_bitmaps();
        }
        self.frozen_bytes.clear();
        self.raster_armed = false;
        self.irq_armed = false;
        self.nmi_armed = false;

        for bp in self.breakpoints.values().filter(|bp| bp.enabled) {
            match bp.kind {
                BreakpointKind::Exec { space, addr } => {
                    space_mut(&mut self.spaces, space).exec.set(addr);
                }
                BreakpointKind::Read { space, addr, len } => {
                    let hooks = space_mut(&mut self.spaces, space);
                    for i in 0..len.min(MAX_RANGE) {
                        hooks.read_watch.set(addr.wrapping_add(i));
                        hooks.read_any.set(addr.wrapping_add(i));
                    }
                }
                BreakpointKind::Write { space, addr, len } => {
                    let hooks = space_mut(&mut self.spaces, space);
                    for i in 0..len.min(MAX_RANGE) {
                        hooks.write_watch.set(addr.wrapping_add(i));
                        hooks.write_any.set(addr.wrapping_add(i));
                    }
                }
                BreakpointKind::Raster { .. } => self.raster_armed = true,
                BreakpointKind::Interrupt(InterruptKind::Irq) => self.irq_armed = true,
                BreakpointKind::Interrupt(InterruptKind::Nmi) => self.nmi_armed = true,
            }
        }

        // Later freezes win where freezes overlap.
        for f in self.freezes.values().filter(|f| f.enabled) {
            if f.mode == FreezeMode::ReapplyPerFrame {
                continue;
            }
            let hooks = space_mut(&mut self.spaces, f.space);
            let n = f.size.bytes();
            for i in 0..n {
                let addr = f.addr.wrapping_add(i);
                let byte = (f.value >> byte_shift(i, n, hooks.big_endian)) as u8;
                self.frozen_bytes.insert((f.space, addr), byte);
                hooks.freeze_read.set(addr);
                hooks.read_any.set(addr);
                if f.mode == FreezeMode::BlockWrites {
                    hooks.freeze_write.set(addr);
                    hooks.write_any.set(addr);
                }
            }
        }
    }

    // ---- hot-path queries ----------------------------------------------

    #[inline]
    fn space(&self, space: MemSpaceId) -> Option<&SpaceHooks> {
        self.spaces.get(space.0 as usize)
    }

    #[inline]
    pub fn exec_armed(&self, space: MemSpaceId) -> bool {
        self.space(space).is_some_and(|h| !h.exec.is_empty())
    }

    #[inline]
    pub fn read_armed(&self, space: MemSpaceId) -> bool {
        self.space(space).is_some_and(|h| !h.read_any.is_empty())
    }

    #[inline]
    pub fn write_armed(&self, space: MemSpaceId) -> bool {
        self.space(space).is_some_and(|h| !h.write_any.is_empty())
    }

    #[inline]
    pub fn raster_armed(&self) -> bool {
        self.raster_armed
    }

    #[inline]
    pub fn interrupt_armed(&self, kind: InterruptKind) -> bool {
        match kind {
            InterruptKind::Irq => self.irq_armed,
            InterruptKind::Nmi => self.nmi_armed,
        }
    }

    // ---- checks (call only when armed) ---------------------------------

    /// Count a hit on every enabled breakpoint matching `matches`; returns the
    /// first one whose ignore count is exhausted.
    fn register_hit(&mut self, matches: impl Fn(&BreakpointKind) -> bool) -> Option<BpId> {
        let mut stop = None;
        for (id, bp) in &mut self.breakpoints {
            if !bp.enabled || !matches(&bp.kind) {
                continue;
            }
            bp.hits = bp.hits.saturating_add(1);
            if bp.hits > bp.ignore_count && stop.is_none() {
                stop = Some(*id);
            }
        }
        if let Some(id) = stop {
            if self.breakpoints.get(&id).is_some_and(|bp| bp.temporary) {
                self.remove_breakpoint(id);
            }
        }
        stop
    }

    fn set_pending(&mut self, reason: StopReason) {
        if self.pending.is_none() {
            self.pending = Some(reason);
        }
    }

    /// Check an instruction fetch at `pc`. Returns true if emulation must stop.
    pub fn check_exec(&mut self, space: MemSpaceId, pc: u32) -> bool {
        if !self.space(space).is_some_and(|h| h.exec.test(pc)) {
            return false;
        }
        let hit = self.register_hit(|kind| {
            matches!(*kind, BreakpointKind::Exec { space: s, addr } if s == space && addr == pc)
        });
        match hit {
            Some(id) => {
                self.set_pending(StopReason::Breakpoint(id));
                true
            }
            None => false,
        }
    }

    /// Guest read of `value` from `addr`. Returns the value the guest sees
    /// (frozen bytes substituted) and records watchpoint hits.
    pub fn on_read(&mut self, space: MemSpaceId, addr: u32, size: Size, value: u32) -> u32 {
        let Some(hooks) = self.space(space) else {
            return value;
        };
        let n = size.bytes();
        if !hooks.read_any.test_range(addr, n) {
            return value;
        }
        let watched = hooks.read_watch.test_range(addr, n);
        let frozen = hooks.freeze_read.test_range(addr, n);
        let big_endian = hooks.big_endian;

        let mut v = value;
        if frozen {
            for i in 0..n {
                if let Some(&b) = self.frozen_bytes.get(&(space, addr.wrapping_add(i))) {
                    let shift = byte_shift(i, n, big_endian);
                    v = (v & !(0xffu32 << shift)) | (u32::from(b) << shift);
                }
            }
        }
        if watched {
            let hit = self.register_hit(|kind| {
                matches!(*kind, BreakpointKind::Read { space: s, addr: a, len }
                    if s == space && overlaps(a, len, addr, n))
            });
            if let Some(id) = hit {
                self.set_pending(StopReason::Watchpoint { id, space, addr, write: false });
            }
        }
        v
    }

    /// Guest write. Returns false if a freeze blocks it. A write touching any
    /// frozen byte is blocked as a whole.
    pub fn on_write(&mut self, space: MemSpaceId, addr: u32, size: Size, value: u32) -> bool {
        let Some(hooks) = self.space(space) else {
            return true;
        };
        let n = size.bytes();
        if !hooks.write_any.test_range(addr, n) {
            return true;
        }
        let watched = hooks.write_watch.test_range(addr, n);
        let blocked = hooks.freeze_write.test_range(addr, n);

        if watched {
            let hit = self.register_hit(|kind| {
                matches!(*kind, BreakpointKind::Write { space: s, addr: a, len }
                    if s == space && overlaps(a, len, addr, n))
            });
            if let Some(id) = hit {
                self.set_pending(StopReason::Watchpoint { id, space, addr, write: true });
            }
        }
        if blocked {
            self.blocked_write = Some(BlockedWrite { space, addr, size, value });
        }
        !blocked
    }

    /// Check the beam position. Returns true if emulation must stop.
    pub fn check_raster(&mut self, line: u16, cycle: u16) -> bool {
        let hit = self.register_hit(|kind| match *kind {
            BreakpointKind::Raster { line: l, cycle: c } => l == line && c.unwrap_or(0) == cycle,
            _ => false,
        });
        match hit {
            Some(id) => {
                self.set_pending(StopReason::Breakpoint(id));
                true
            }
            None => false,
        }
    }

    /// Check an interrupt being taken. Returns true if emulation must stop.
    pub fn check_interrupt(&mut self, irq: InterruptKind) -> bool {
        let hit = self.register_hit(|kind| *kind == BreakpointKind::Interrupt(irq));
        match hit {
            Some(id) => {
                self.set_pending(StopReason::Breakpoint(id));
                true
            }
            None => false,
        }
    }

    /// The stop reason recorded by the first hit since the last call.
    pub fn take_stop(&mut self) -> Option<StopReason> {
        self.pending.take()
    }

    /// The most recent write blocked by a freeze (for `freeze_blocked_write`).
    pub fn take_blocked_write(&mut self) -> Option<BlockedWrite> {
        self.blocked_write.take()
    }

    // ---- savestate -----------------------------------------------------

    /// Save the freeze list; freezes are part of savestates and replays.
    pub fn save_freezes(&self, w: &mut StateWriter) {
        w.chunk(FREEZE_TAG, FREEZE_VERSION, |w| {
            w.u32(self.next_freeze);
            w.u32(self.freezes.len() as u32);
            for (id, f) in &self.freezes {
                w.u32(id.0);
                f.write(w);
            }
        });
    }

    /// Replace the freeze list. On error the current freezes are kept.
    pub fn load_freezes(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(FREEZE_TAG, FREEZE_VERSION)?;
        let next = c.u32()?;
        let count = c.u32()?;
        let mut freezes = BTreeMap::new();
        for _ in 0..count {
            let id = FreezeId(c.u32()?);
            freezes.insert(id, Freeze::read(&mut c)?);
        }
        self.next_freeze = next;
        self.freezes = freezes;
        self.rebuild();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPU: MemSpaceId = MemSpaceId(0);
    const CHIP: MemSpaceId = MemSpaceId(1);

    #[test]
    fn bitmap_set_clear_across_pages() {
        let mut b = AddrBitmap::new();
        assert!(b.is_empty());
        for addr in [0, 63, 64, 4095, 4096, 0xff_ffff] {
            assert!(b.set(addr));
            assert!(!b.set(addr), "second set reports no change");
            assert!(b.test(addr));
        }
        assert_eq!(b.len(), 6);
        assert!(!b.test(1));
        assert!(!b.test(0x1_0000_0));
        assert!(b.test_range(4094, 2));
        assert!(!b.test_range(1, 62));
        assert!(b.clear(4096));
        assert!(!b.clear(4096));
        assert!(!b.test(4096));
        assert!(!b.clear(0xffff_ffff), "clearing an unallocated page is a no-op");
        b.clear_all();
        assert!(b.is_empty() && !b.test(0));
    }

    #[test]
    fn nothing_armed_by_default() {
        let hooks = DebugHooks::new();
        assert!(!hooks.exec_armed(CPU));
        assert!(!hooks.read_armed(CPU));
        assert!(!hooks.write_armed(MemSpaceId(99)));
        assert!(!hooks.raster_armed());
    }

    #[test]
    fn exec_breakpoint_with_ignore_count() {
        let mut hooks = DebugHooks::new();
        let mut bp = Breakpoint::new(BreakpointKind::Exec { space: CPU, addr: 0xc000 });
        bp.ignore_count = 2;
        let id = hooks.insert_breakpoint(bp);
        assert!(hooks.exec_armed(CPU));
        assert!(!hooks.check_exec(CPU, 0xc001));
        assert!(!hooks.check_exec(CPU, 0xc000));
        assert!(!hooks.check_exec(CPU, 0xc000));
        assert!(hooks.check_exec(CPU, 0xc000));
        assert_eq!(hooks.take_stop(), Some(StopReason::Breakpoint(id)));
        assert_eq!(hooks.take_stop(), None);
        assert_eq!(hooks.breakpoint(id).unwrap().hits, 3);
    }

    #[test]
    fn disabled_and_temporary_breakpoints() {
        let mut hooks = DebugHooks::new();
        let id = hooks.add_breakpoint(BreakpointKind::Exec { space: CPU, addr: 0x1000 });
        hooks.set_breakpoint_enabled(id, false);
        assert!(!hooks.exec_armed(CPU));

        let mut bp = Breakpoint::new(BreakpointKind::Exec { space: CPU, addr: 0x2000 });
        bp.temporary = true;
        let tmp = hooks.insert_breakpoint(bp);
        assert!(hooks.check_exec(CPU, 0x2000));
        assert!(hooks.breakpoint(tmp).is_none(), "temporary breakpoint removed after stop");
        assert!(!hooks.exec_armed(CPU));
    }

    #[test]
    fn removing_one_of_two_overlapping_breakpoints_keeps_the_other() {
        let mut hooks = DebugHooks::new();
        let a = hooks.add_breakpoint(BreakpointKind::Exec { space: CPU, addr: 0x0400 });
        let b = hooks.add_breakpoint(BreakpointKind::Exec { space: CPU, addr: 0x0400 });
        hooks.remove_breakpoint(a);
        assert!(hooks.check_exec(CPU, 0x0400));
        assert_eq!(hooks.take_stop(), Some(StopReason::Breakpoint(b)));
    }

    #[test]
    fn read_and_write_watchpoints_over_ranges() {
        let mut hooks = DebugHooks::new();
        let r = hooks.add_breakpoint(BreakpointKind::Read { space: CPU, addr: 0xd000, len: 0x40 });
        let w = hooks.add_breakpoint(BreakpointKind::Write { space: CPU, addr: 0xd020, len: 1 });
        assert!(hooks.read_armed(CPU) && hooks.write_armed(CPU));
        assert!(!hooks.read_armed(CHIP));

        assert_eq!(hooks.on_read(CPU, 0xcfff, Size::Byte, 7), 7);
        assert_eq!(hooks.take_stop(), None);
        assert_eq!(hooks.on_read(CPU, 0xd03f, Size::Byte, 7), 7);
        assert_eq!(
            hooks.take_stop(),
            Some(StopReason::Watchpoint { id: r, space: CPU, addr: 0xd03f, write: false })
        );
        assert!(hooks.on_write(CPU, 0xd020, Size::Byte, 1));
        assert_eq!(
            hooks.take_stop(),
            Some(StopReason::Watchpoint { id: w, space: CPU, addr: 0xd020, write: true })
        );
    }

    #[test]
    fn block_writes_freeze() {
        let mut hooks = DebugHooks::new();
        let id = hooks.add_freeze(Freeze::new(CPU, 0x00c0, Size::Byte, 3));
        assert!(!hooks.on_write(CPU, 0x00c0, Size::Byte, 2), "write is blocked");
        assert_eq!(
            hooks.take_blocked_write(),
            Some(BlockedWrite { space: CPU, addr: 0x00c0, size: Size::Byte, value: 2 })
        );
        assert!(hooks.on_write(CPU, 0x00c1, Size::Byte, 2));
        assert_eq!(hooks.on_read(CPU, 0x00c0, Size::Byte, 0x55), 3);
        assert_eq!(hooks.take_stop(), None, "freezes never stop emulation");

        hooks.set_freeze_enabled(id, false);
        assert!(hooks.on_write(CPU, 0x00c0, Size::Byte, 2));
        assert!(!hooks.read_armed(CPU));
    }

    #[test]
    fn reapply_on_read_freeze_does_not_block_writes() {
        let mut hooks = DebugHooks::new();
        let mut f = Freeze::new(CPU, 0xdc00, Size::Byte, 0x7f);
        f.mode = FreezeMode::ReapplyOnRead;
        hooks.add_freeze(f);
        assert!(!hooks.write_armed(CPU));
        assert!(hooks.on_write(CPU, 0xdc00, Size::Byte, 0));
        assert_eq!(hooks.on_read(CPU, 0xdc00, Size::Byte, 0), 0x7f);
    }

    #[test]
    fn per_frame_freezes_use_no_bitmaps() {
        let mut hooks = DebugHooks::new();
        let mut f = Freeze::new(CPU, 0x0800, Size::Byte, 9);
        f.mode = FreezeMode::ReapplyPerFrame;
        hooks.add_freeze(f.clone());
        assert!(!hooks.read_armed(CPU) && !hooks.write_armed(CPU));
        assert_eq!(hooks.per_frame_freezes().collect::<Vec<_>>(), vec![&f]);
    }

    #[test]
    fn word_freeze_respects_byte_order() {
        let mut hooks = DebugHooks::new();
        hooks.add_freeze(Freeze::new(CPU, 0x10, Size::Word, 0x1234));
        assert_eq!(hooks.on_read(CPU, 0x10, Size::Byte, 0), 0x34, "little-endian low byte");
        assert_eq!(hooks.on_read(CPU, 0x11, Size::Byte, 0), 0x12);

        hooks.set_big_endian(CHIP, true);
        hooks.add_freeze(Freeze::new(CHIP, 0x100, Size::Word, 0xabcd));
        assert_eq!(hooks.on_read(CHIP, 0x100, Size::Byte, 0), 0xab, "big-endian high byte first");
        assert_eq!(hooks.on_read(CHIP, 0x100, Size::Word, 0), 0xabcd);
        assert_eq!(hooks.on_read(CHIP, 0x0ff, Size::Long, 0x1111_1111), 0x11ab_cd11);
    }

    #[test]
    fn raster_and_interrupt_breakpoints() {
        let mut hooks = DebugHooks::new();
        hooks.add_breakpoint(BreakpointKind::Raster { line: 0x33, cycle: None });
        hooks.add_breakpoint(BreakpointKind::Interrupt(InterruptKind::Nmi));
        assert!(hooks.raster_armed());
        assert!(hooks.interrupt_armed(InterruptKind::Nmi));
        assert!(!hooks.interrupt_armed(InterruptKind::Irq));
        assert!(!hooks.check_raster(0x33, 5));
        assert!(hooks.check_raster(0x33, 0));
        assert!(!hooks.check_interrupt(InterruptKind::Irq));
        assert!(hooks.check_interrupt(InterruptKind::Nmi));
    }

    #[test]
    fn freezes_round_trip_through_savestate() {
        let mut hooks = DebugHooks::new();
        let mut f = Freeze::new(CHIP, 0x1234, Size::Long, 0xdead_beef);
        f.label = Some("Lives".into());
        hooks.add_freeze(f);
        hooks.add_freeze(Freeze::new(CPU, 0x02, Size::Byte, 1));

        let mut w = StateWriter::new();
        hooks.save_freezes(&mut w);
        let data = w.into_bytes();

        let mut restored = DebugHooks::new();
        restored.load_freezes(&mut StateReader::new(&data)).unwrap();
        assert_eq!(
            restored.freezes().collect::<Vec<_>>(),
            hooks.freezes().collect::<Vec<_>>()
        );
        assert_eq!(restored.on_read(CPU, 0x02, Size::Byte, 0), 1, "bitmaps rebuilt on load");
        assert_eq!(
            restored.add_freeze(Freeze::new(CPU, 0, Size::Byte, 0)),
            FreezeId(2),
            "id counter restored"
        );
    }

    #[test]
    fn failed_freeze_load_keeps_current_freezes() {
        let mut hooks = DebugHooks::new();
        hooks.add_freeze(Freeze::new(CPU, 0x02, Size::Byte, 1));
        let mut w = StateWriter::new();
        hooks.save_freezes(&mut w);
        let data = w.into_bytes();

        let mut other = DebugHooks::new();
        other.add_freeze(Freeze::new(CPU, 0x99, Size::Byte, 5));
        let truncated = &data[..data.len() - 1];
        assert!(other.load_freezes(&mut StateReader::new(truncated)).is_err());
        assert_eq!(other.freezes().count(), 1);
        assert_eq!(other.on_read(CPU, 0x99, Size::Byte, 0), 5);
    }
}
