//! The interrupt core of the ITU: the sources' lines, the latched edges,
//! the enables and the one line to the processor.
//!
//! Low byte, eight sources: bits 0 (periodic timer), 2 (USB) and 7 (C64
//! reset) are edges: a rise of the source sets a flag that stays until the
//! firmware clears it, whether the source is enabled or not (enabling a
//! source with an old flag interrupts at once, which is why the firmware
//! clears before it enables). The others are levels. High byte, eight
//! sources: levels with an enable each and no clear register.
//!
//! The line to the processor: global enable, and (an active low source
//! that is enabled, or an active high source that is enabled). After
//! power-on the global enable is set and all source enables are clear.
//!
//! Which low sources are edges is fixed in the FPGA; the firmware never
//! writes the register for it. The value `$85` is the one of every open
//! top level of the 1541 Ultimate project; the one of the Ultimate 64
//! Elite II (closed) is assumed to be the same, which agrees with how the
//! firmware treats bits 2 and 7.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/00-memory-map.md` (section
//! "Interrupts", contradiction C2) and `02-itu-uart.md`; 1541 Ultimate
//! `fpga/io/itu/vhdl_source/itu.vhd` (flag set on a rise independent of
//! the enables, reset values).

use mimic_core::{StateError, StateReader, StateWriter};

/// Low sources that are edges.
pub const EDGE_MASK: u8 = 0x85;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Irq {
    /// Global enable (ITU register 0, bit 0); set after power-on.
    pub global: bool,
    /// Enables of the low sources.
    pub mask: u8,
    /// Latched edges of the low sources (only bits of [`EDGE_MASK`]).
    pub flags: u8,
    /// Present levels of the low sources.
    pub level: u8,
    /// Enables of the high sources.
    pub mask_high: u8,
    /// Present levels of the high sources.
    pub high: u8,
}

impl Default for Irq {
    fn default() -> Self {
        Self {
            global: true,
            mask: 0,
            flags: 0,
            level: 0,
            mask_high: 0,
            high: 0,
        }
    }
}

impl Irq {
    /// A short pulse on low source `bit`: an edge source latches it, a
    /// level source does not show it.
    pub fn pulse(&mut self, bit: u8) {
        self.flags |= (1 << (bit & 7)) & EDGE_MASK;
    }

    /// The level of low source `bit`. On an edge source the change from
    /// low to high latches the flag.
    pub fn set_level(&mut self, bit: u8, on: bool) {
        let m = 1 << (bit & 7);
        if on {
            if self.level & m == 0 {
                self.flags |= m & EDGE_MASK;
            }
            self.level |= m;
        } else {
            self.level &= !m;
        }
    }

    /// The level of high source `bit`.
    pub fn set_high(&mut self, bit: u8, on: bool) {
        let m = 1 << (bit & 7);
        if on {
            self.high |= m;
        } else {
            self.high &= !m;
        }
    }

    /// The low sources that are active and enabled (register ACTIVE).
    pub fn active(&self) -> u8 {
        (self.flags | (self.level & !EDGE_MASK)) & self.mask
    }

    /// The high sources that are active and enabled.
    pub fn high_active(&self) -> u8 {
        self.high & self.mask_high
    }

    /// Clear latched edges (register CLEAR).
    pub fn clear(&mut self, bits: u8) {
        self.flags &= !bits;
    }

    /// The line to the processor.
    pub fn line(&self) -> bool {
        self.global && (self.active() != 0 || self.high_active() != 0)
    }

    /// Power-on state: global enable set, everything else clear. The
    /// sources' lines are taken as low, as all blocks are reset together.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.bool(self.global);
        for v in [self.mask, self.flags, self.level, self.mask_high, self.high] {
            w.u8(v);
        }
    }

    /// Corrupt data gives an error and leaves the state as it was.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let global = r.bool()?;
        let mask = r.u8()?;
        let flags = r.u8()?;
        let level = r.u8()?;
        let mask_high = r.u8()?;
        let high = r.u8()?;
        if flags & !EDGE_MASK != 0 {
            return Err(StateError::Invalid("ITU edge flag on a level source"));
        }
        *self = Self {
            global,
            mask,
            flags,
            level,
            mask_high,
            high,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_latch_and_levels_follow() {
        let mut i = Irq {
            mask: 0xff,
            ..Irq::default()
        };
        i.pulse(0);
        assert!(i.line());
        i.clear(1);
        assert!(!i.line());
        // Bit 4 is a level: a pulse does not latch, the level shows.
        i.pulse(4);
        assert!(!i.line());
        i.set_level(4, true);
        assert_eq!(i.active(), 0x10);
        i.set_level(4, false);
        assert!(!i.line());
        i.set_high(3, true);
        assert!(!i.line());
        i.mask_high = 8;
        assert!(i.line());
        i.global = false;
        assert!(!i.line());
    }

    #[test]
    fn power_on_has_the_global_enable_set() {
        let mut i = Irq::default();
        assert!(i.global);
        assert_eq!(i.mask, 0);
        assert!(!i.line());
        i.global = false;
        i.mask = 0x55;
        i.flags = 0x81;
        i.reset();
        assert_eq!(i, Irq::default());
    }

    #[test]
    fn an_edge_latches_while_its_enable_is_clear() {
        let mut i = Irq::default();
        i.pulse(2);
        assert_eq!(i.active(), 0);
        assert!(!i.line());
        // Enabling with the old flag interrupts at once.
        i.mask |= 4;
        assert_eq!(i.active(), 4);
        assert!(i.line());
    }

    #[test]
    fn a_rising_level_on_an_edge_source_latches_once() {
        let mut i = Irq {
            mask: 0xff,
            ..Irq::default()
        };
        i.set_level(7, true);
        assert_eq!(i.active(), 0x80);
        // The line stays high: no second edge after the clear.
        i.clear(0x80);
        i.set_level(7, true);
        assert_eq!(i.active(), 0);
        i.set_level(7, false);
        assert_eq!(i.active(), 0);
        i.set_level(7, true);
        assert_eq!(i.active(), 0x80);
    }

    #[test]
    fn savestate_round_trip() {
        let i = Irq {
            global: false,
            mask: 0x91,
            flags: 0x84,
            level: 0x30,
            mask_high: 0x68,
            high: 0x08,
        };
        let mut w = StateWriter::new();
        i.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = Irq::default();
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(i, d);
        for cut in 0..bytes.len() {
            assert!(d.load(&mut StateReader::new(&bytes[..cut])).is_err());
        }
        // A flag on a level source cannot exist.
        let mut bad = bytes.clone();
        bad[2] = 0x10;
        assert!(d.load(&mut StateReader::new(&bad)).is_err());
        assert_eq!(i, d);
    }
}
