//! MEMC (VL86C110): the Archimedes' memory controller.
//!
//! Decodes the 26-bit address space: logically mapped RAM $0000000–$1FFFFFF
//! through 128 page table entries, physical RAM $2000000–$2FFFFFF, I/O
//! $3000000–$33FFFFF, ROM $3400000–$3FFFFFF on reads; writes there reach
//! VIDC ($3400000), the DMA address generators and the control register
//! ($3600000) and the address translator ($3800000), all with the data in
//! the address lines.
//!
//! After reset ROM answers everywhere until an access with A25 = A24 = 0 is
//! followed by one with A25 set.
//!
//! Source: VLSI Technology, "Acorn RISC Machine (ARM) Family Data Manual"
//! (1990), VL86C110 chapter: memory map, protection levels (table 3),
//! control register (figure 6), translator programming (figure 7), address
//! generators (figure 8), sound buffer swap.

use emu_core::{StateError, StateReader, StateWriter};

/// Where an address leads, after protection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A byte address in RAM.
    Ram(u32),
    /// A byte address in the system ROM's address range ($3800000 up).
    Rom(u32),
    /// The low ROM area $3400000–$37FFFFF.
    LowRom(u32),
    /// IOC and what hangs on it.
    Io(u32),
    Vidc,
    /// MEMC's own registers and the translator: no bus data.
    Memc,
    /// The access aborts.
    Abort,
}

const INVALID: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct Memc {
    pub control: u32,
    /// log2 of the page size.
    pub page_shift: u32,
    /// The address each physical page's entry was programmed with.
    entries: [u32; 128],
    /// Logical page → physical page | PPL << 8.
    map: Vec<u32>,
    pub vinit: u32,
    pub vstart: u32,
    pub vend: u32,
    pub cinit: u32,
    pub sstart: u32,
    pub sendn: u32,
    pub sendc: u32,
    pub sptr: u32,
    pub vptr: u32,
    pub cptr: u32,
    next_valid: bool,
    /// The sound interrupt line.
    pub sirq: bool,
    /// ROM answers everywhere (after reset).
    rom_overlay: bool,
    overlay_armed: bool,
}

impl Default for Memc {
    fn default() -> Self {
        Self::new()
    }
}

impl Memc {
    pub fn new() -> Self {
        Self {
            control: 0,
            page_shift: 12,
            entries: [u32::MAX; 128],
            map: vec![INVALID; 8192],
            vinit: 0,
            vstart: 0,
            vend: 0,
            cinit: 0,
            sstart: 0,
            sendn: 0,
            sendc: 0,
            sptr: 0,
            vptr: 0,
            cptr: 0,
            next_valid: false,
            sirq: true,
            rom_overlay: true,
            overlay_armed: false,
        }
    }

    /// /RES: page size 4K, ROM speed, OS mode and sound DMA go; the
    /// translator, video enable and refresh stay.
    pub fn reset(&mut self) {
        self.control &= 0x0700;
        self.set_page_shift(12);
        self.next_valid = false;
        self.sirq = true;
        self.rom_overlay = true;
        self.overlay_armed = false;
    }

    pub fn sound_enabled(&self) -> bool {
        self.control & 0x800 != 0
    }

    pub fn video_enabled(&self) -> bool {
        self.control & 0x400 != 0
    }

    fn os_mode(&self) -> bool {
        self.control & 0x1000 != 0
    }

    /// Clocks of the 8 MHz bus for a ROM access in the high or low area.
    pub fn rom_clocks(&self, high: bool) -> u64 {
        let code = self.control >> if high { 6 } else { 4 } & 3;
        match code {
            2 => 2,
            1 => 3,
            _ => 4,
        }
    }

    /// Logical and physical page of a translator write, by page size
    /// (figure 7).
    fn decode(addr: u32, shift: u32) -> (usize, usize, u32) {
        let a = |n: u32| addr >> n & 1;
        let ppn = match shift {
            12 => addr & 0x7f,
            13 => a(0) << 6 | (addr >> 1 & 0x3f),
            14 => a(1) << 6 | a(0) << 5 | (addr >> 2 & 0x1f),
            _ => a(1) << 6 | a(2) << 5 | a(0) << 4 | (addr >> 3 & 0x0f),
        };
        let high = addr >> 10 & 3;
        let bits = 25 - shift - 2;
        let low = addr >> shift & ((1 << (23 - shift)) - 1);
        let lpn = high << (bits) | low;
        (ppn as usize, lpn as usize, addr >> 8 & 3)
    }

    fn rebuild(&mut self) {
        self.map.fill(INVALID);
        for e in self.entries {
            if e != u32::MAX {
                let (ppn, lpn, ppl) = Self::decode(e, self.page_shift);
                if let Some(m) = self.map.get_mut(lpn) {
                    *m = ppn as u32 | ppl << 8;
                }
            }
        }
    }

    fn set_page_shift(&mut self, shift: u32) {
        if shift != self.page_shift {
            self.page_shift = shift;
            // The entries stay as written and now mean something else.
            let entries = self.entries;
            self.entries = [u32::MAX; 128];
            for e in entries {
                if e != u32::MAX {
                    let (ppn, _, _) = Self::decode(e, shift);
                    self.entries[ppn] = e;
                }
            }
            self.rebuild();
        }
    }

    /// A supervisor write to $3600000–$3FFFFFF.
    pub fn write(&mut self, addr: u32) {
        if addr >= 0x380_0000 {
            // A7 picks the slave MEMC of a two-MEMC system; none is fitted.
            if addr & 0x80 != 0 {
                return;
            }
            let (ppn, lpn, ppl) = Self::decode(addr, self.page_shift);
            // The old mapping of this physical page goes.
            let old = self.entries[ppn];
            if old != u32::MAX {
                let (_, old_lpn, _) = Self::decode(old, self.page_shift);
                if let Some(m) = self.map.get_mut(old_lpn) {
                    if *m != INVALID && *m & 0xff == ppn as u32 {
                        *m = INVALID;
                    }
                }
            }
            self.entries[ppn] = addr;
            if let Some(m) = self.map.get_mut(lpn) {
                *m = ppn as u32 | ppl << 8;
            }
            return;
        }
        // The value in A16–A2 is a physical address / 16.
        let value = (addr >> 2 & 0x7fff) << 4;
        match addr >> 17 & 7 {
            0 => self.vinit = value,
            1 => self.vstart = value,
            2 => self.vend = value,
            3 => self.cinit = value,
            4 => {
                self.sstart = value;
                self.next_valid = true;
                self.sirq = false;
            }
            5 => self.sendn = value,
            6 => {
                self.sptr = self.sstart;
                self.sendc = self.sendn;
                self.next_valid = false;
                self.sirq = false;
            }
            _ => {
                self.control = addr & 0x3ffc;
                self.set_page_shift(12 + (addr >> 2 & 3));
            }
        }
    }

    /// The address's destination for a read or write, with protection.
    /// `supervisor` is the processor's /TRANS (a non-user mode).
    pub fn decode_access(&mut self, addr: u32, write: bool, supervisor: bool) -> Target {
        let addr = addr & 0x3ff_ffff;
        if self.rom_overlay {
            if addr < 0x100_0000 {
                self.overlay_armed = true;
            } else if self.overlay_armed && addr >= 0x200_0000 {
                self.rom_overlay = false;
            }
            if self.rom_overlay && addr < 0x200_0000 && !write {
                return Target::Rom(addr);
            }
        }
        match addr >> 24 {
            0 | 1 => {
                let shift = self.page_shift;
                let entry = self.map[(addr >> shift) as usize & (self.map.len() - 1)];
                if entry == INVALID {
                    return Target::Abort;
                }
                let ppl = entry >> 8;
                let allowed = if supervisor {
                    true
                } else if self.os_mode() {
                    !write || ppl < 2
                } else {
                    match ppl {
                        0 => true,
                        1 => !write,
                        _ => false,
                    }
                };
                if !allowed {
                    return Target::Abort;
                }
                Target::Ram((entry & 0x7f) << shift | (addr & ((1 << shift) - 1)))
            }
            2 => {
                if supervisor {
                    Target::Ram(addr & ((128 << self.page_shift) - 1))
                } else {
                    Target::Abort
                }
            }
            _ => {
                if addr < 0x340_0000 {
                    if supervisor {
                        Target::Io(addr)
                    } else {
                        Target::Abort
                    }
                } else if !write {
                    if addr >= 0x380_0000 {
                        Target::Rom(addr)
                    } else {
                        Target::LowRom(addr)
                    }
                } else if !supervisor {
                    Target::Abort
                } else if addr < 0x360_0000 {
                    Target::Vidc
                } else {
                    Target::Memc
                }
            }
        }
    }

    /// The translator's entry for a logical page (debugger).
    pub fn page_entry(&self, lpn: usize) -> u32 {
        self.map.get(lpn).copied().unwrap_or(INVALID)
    }

    /// The same without its effects, for the debugger: as a supervisor
    /// sees it.
    pub fn peek_target(&self, addr: u32) -> Target {
        self.clone().decode_access(addr, false, true)
    }

    /// A new video frame starts.
    pub fn frame_start(&mut self) {
        self.vptr = self.vinit;
    }

    /// The cursor pointer reloads in flyback.
    pub fn flyback(&mut self) {
        self.cptr = self.cinit;
    }

    /// Video DMA address for the next byte; wraps at Vend (a 16-byte block).
    pub fn next_video_byte(&mut self) -> u32 {
        let a = self.vptr;
        self.vptr += 1;
        if self.vptr & 15 == 0 && self.vptr - 16 >= self.vend {
            self.vptr = self.vstart;
        }
        a
    }

    pub fn next_cursor_byte(&mut self) -> u32 {
        let a = self.cptr;
        self.cptr += 1;
        a
    }

    /// Address of the next 16-byte block of sound; the pointer moves on and
    /// at the end of the buffer swaps to the next one.
    pub fn next_sound_block(&mut self) -> u32 {
        let a = self.sptr;
        self.sptr += 16;
        if self.sptr > self.sendc {
            if self.next_valid {
                self.sptr = self.sstart;
                std::mem::swap(&mut self.sendc, &mut self.sendn);
                self.next_valid = false;
                self.sirq = true;
            } else {
                self.sptr = self.sstart;
            }
        }
        a
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"MEMC", 1, |w| {
            w.u32(self.control);
            w.u32(self.page_shift);
            for e in self.entries {
                w.u32(e);
            }
            for v in [
                self.vinit,
                self.vstart,
                self.vend,
                self.cinit,
                self.sstart,
                self.sendn,
                self.sendc,
                self.sptr,
                self.vptr,
                self.cptr,
            ] {
                w.u32(v);
            }
            w.bool(self.next_valid);
            w.bool(self.sirq);
            w.bool(self.rom_overlay);
            w.bool(self.overlay_armed);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"MEMC", 1)?;
        let control = c.u32()?;
        let shift = c.u32()?;
        if !(12..=15).contains(&shift) {
            return Err(StateError::Invalid("MEMC page size"));
        }
        let mut entries = [0; 128];
        for e in &mut entries {
            *e = c.u32()?;
        }
        let mut v = [0u32; 10];
        for x in &mut v {
            *x = c.u32()?;
        }
        self.control = control;
        self.page_shift = shift;
        self.entries = entries;
        [
            self.vinit,
            self.vstart,
            self.vend,
            self.cinit,
            self.sstart,
            self.sendn,
            self.sendc,
            self.sptr,
            self.vptr,
            self.cptr,
        ] = v;
        self.next_valid = c.bool()?;
        self.sirq = c.bool()?;
        self.rom_overlay = c.bool()?;
        self.overlay_armed = c.bool()?;
        self.rebuild();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_answers_until_a25_follows_a_low_access() {
        let mut m = Memc::new();
        assert_eq!(m.decode_access(0, false, true), Target::Rom(0));
        assert_eq!(m.decode_access(0x10, false, true), Target::Rom(0x10));
        assert_eq!(
            m.decode_access(0x380_0000, false, true),
            Target::Rom(0x380_0000)
        );
        assert_eq!(m.decode_access(0, false, true), Target::Abort);
    }

    fn no_overlay() -> Memc {
        let mut m = Memc::new();
        m.decode_access(0, false, true);
        m.decode_access(0x380_0000, false, true);
        m
    }

    #[test]
    fn translator_8k_page() {
        let mut m = no_overlay();
        m.write(0x360_0000 | 7 << 17 | 1 << 2); // control: 8K
        assert_eq!(m.page_shift, 13);
        // Logical page 3, physical page 5, PPL 1: PPN6 in A0, PPN5-0 A6-A1.
        m.write(0x380_0000 | 3 << 13 | 1 << 8 | 5 << 1);
        assert_eq!(
            m.decode_access(0x6010, false, false),
            Target::Ram(5 << 13 | 0x10)
        );
        assert_eq!(m.decode_access(0x6010, true, false), Target::Abort, "PPL 1");
        assert_eq!(
            m.decode_access(0x6010, true, true),
            Target::Ram(5 << 13 | 0x10)
        );
        // Moving the physical page drops the old mapping.
        m.write(0x380_0000 | 9 << 13 | 5 << 1);
        assert_eq!(m.decode_access(0x6000, false, true), Target::Abort);
    }

    #[test]
    fn protection_levels_of_table_3() {
        let mut m = no_overlay();
        m.write(0x380_0000 | 1 << 12 | 2 << 8 | 1); // 4K: log 1, phys 1, PPL 2
        assert_eq!(m.decode_access(0x1000, false, false), Target::Abort);
        m.write(0x360_0000 | 7 << 17 | 1 << 12); // OS mode
        assert_eq!(m.decode_access(0x1000, false, false), Target::Ram(0x1000));
        assert_eq!(m.decode_access(0x1000, true, false), Target::Abort);
    }

    #[test]
    fn privileged_areas() {
        let mut m = no_overlay();
        assert_eq!(m.decode_access(0x200_0100, false, false), Target::Abort);
        assert_eq!(m.decode_access(0x320_0000, false, false), Target::Abort);
        assert_eq!(
            m.decode_access(0x320_0000, false, true),
            Target::Io(0x320_0000)
        );
        assert_eq!(m.decode_access(0x340_0000, true, true), Target::Vidc);
        assert_eq!(
            m.decode_access(0x340_0000, false, false),
            Target::LowRom(0x340_0000)
        );
        assert_eq!(m.decode_access(0x380_0000, true, false), Target::Abort);
    }

    #[test]
    fn dma_registers_and_sound_swap() {
        let mut m = no_overlay();
        m.write(0x360_0000 | 4 << 17 | (0x100 / 16) << 2);
        m.write(0x360_0000 | 5 << 17 | (0x110 / 16) << 2);
        assert!(!m.sirq);
        m.write(0x360_0000 | 6 << 17);
        assert_eq!((m.sptr, m.sendc), (0x100, 0x110));
        m.write(0x360_0000 | 4 << 17 | (0x200 / 16) << 2);
        assert_eq!(m.next_sound_block(), 0x100);
        assert_eq!(m.next_sound_block(), 0x110);
        assert!(m.sirq, "the swap interrupts");
        assert_eq!(m.sptr, 0x200);
    }

    #[test]
    fn video_pointer_wraps_after_the_last_block() {
        let mut m = no_overlay();
        m.vstart = 0x1000;
        m.vend = 0x1010;
        m.vinit = 0x1010;
        m.frame_start();
        for _ in 0..16 {
            m.next_video_byte();
        }
        assert_eq!(m.vptr, 0x1000);
    }

    #[test]
    fn savestate_round_trip() {
        let mut m = no_overlay();
        m.write(0x380_0000 | 3 << 12 | 7);
        let mut w = StateWriter::new();
        m.save(&mut w);
        let bytes = w.into_bytes();
        let mut n = Memc::new();
        n.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(n.decode_access(0x3000, false, true), Target::Ram(0x7000));
    }
}
