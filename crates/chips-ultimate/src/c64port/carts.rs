//! The board's own cartridge logic: the cartridge the FPGA plays on the
//! C64's expansion port, its ROM and RAM in board memory.
//!
//! The firmware selects a logic with C64_CARTRIDGE_TYPE (`$040005`: bits
//! 4:0 the logic, bits 7:5 its variant), copies the ROM image to board
//! memory [`ROM_BASE`] and lets the C64 out of reset. What the register
//! holds is taken over (latched) only
//! - while the C64's reset is held (the board's, C64_MODE bit 2, or the
//!   C64's own reset line): the cartridge is then switched on;
//! - with the force strobe (C64_CARTRIDGE_KILL bit 1): switched off, a
//!   freezer switches itself on later.
//!
//! The kill strobe (bit 0) switches the cartridge off. C64_CARTRIDGE_ACTIVE
//! (the same address, read) is whether it is on. The cartridge's /GAME and
//! /EXROM only reach the port while it is on.
//!
//! Built so far, the logics the firmware's own programs need:
//!
//! | Logic | Name | Here |
//! |---|---|---|
//! | 0 | none | switches the cartridge off |
//! | 1 | normal | variant bit 0 = /EXROM high, bit 1 = /GAME high, bit 2 the VIC-II reads it in ULTIMAX; ROM at ROML and ROMH (16 KB window, ROMH the upper 8 KB); a write to `$DFFF` with bits 7:6 = `01` switches it off for good (until the next latch) |
//!
//! Every other logic is taken as "not built": it latches, but the
//! cartridge stays off, so the C64 starts without it.
//!
//! The firmware's boot cartridge ("Run Disk", "Run" of a program, the DMA
//! loader) is an 8 KB one: logic 1, variant 2 (`$41`). It runs from ROML,
//! talks to the firmware through C64 RAM (`$02`, `$0150`, `$0172`-`$0185`)
//! and switches itself off with `$40` to `$DFFF` before it hands over.
//!
//! Address of a ROM byte in board memory: [`ROM_BASE`] plus the CPU
//! address bits 12:0, plus bit 13 in the 16 KB window (ROMH), plus the
//! logic's bank bits from bit 13 (or 14) up, 4 MB at most. The cartridge's
//! RAM (used by later logics) is 64 KB at [`RAM_BASE`].
//!
//! Sources: the firmware's `fpga/cart_slot/vhdl_source/all_carts_v5.vhd`
//! (logic numbers, the latch rules on reset and force, the normal logic's
//! line bits and its `$DFFF` switch, the ROM address), `slot_server_v4.vhd`
//! (the lines reach the port only while the cartridge is on; ULTIMAX of
//! C64_MODE wins), `software/io/c64/c64.h` (type and variant numbers),
//! `software/io/c64/c64.cc` (`start_cartridge`, `set_cartridge`,
//! `init_cartridge`), `software/io/c64/c64_subsys.cc` (`dma_load`,
//! `restoreCart`), `software/6502/bootcrt.tas` (what the boot cartridge
//! does on the C64), `target/u64ii/riscv/ultimate/linker.x`
//! (`__cart_rom_start` `$03C00000`, `__cart_ram_start` `$00EF0000`), all
//! read for facts.

use mimic_core::{StateError, StateReader, StateWriter};

/// Board memory of the cartridge ROM (4 MB).
pub const ROM_BASE: u32 = 0x03c0_0000;
/// The most ROM a logic can address.
pub const ROM_SIZE: u32 = 0x40_0000;
/// Board memory of the cartridge RAM (64 KB).
pub const RAM_BASE: u32 = 0x00ef_0000;

/// Logic numbers (C64_CARTRIDGE_TYPE bits 4:0).
pub const LOGIC_NONE: u8 = 0x00;
pub const LOGIC_NORMAL: u8 = 0x01;

/// Where on the C64's bus an access lands, as the C64's PLA decides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CartWindow {
    /// `$8000`-`$9FFF` with /ROML.
    RomL,
    /// `$A000`-`$BFFF` or `$E000`-`$FFFF` with /ROMH.
    RomH,
    /// `$DE00`-`$DFFF`, /IO1 and /IO2.
    Io,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CartLogic {
    /// The latched logic (bits 4:0 of the type register).
    logic: u8,
    /// The latched variant (bits 7:5).
    variant: u8,
    /// The cartridge is on (C64_CARTRIDGE_ACTIVE).
    on: bool,
}

impl CartLogic {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take over the type register's value; `on`: the reset is held (the
    /// cartridge starts on), else the force strobe (it starts off).
    pub fn latch(&mut self, type_reg: u8, on: bool) {
        self.logic = type_reg & 0x1f;
        self.variant = type_reg >> 5;
        self.on = on && self.built();
    }

    /// The kill strobe.
    pub fn kill(&mut self) {
        self.on = false;
    }

    /// The latched logic and variant as the type register holds them.
    pub fn latched(&self) -> u8 {
        self.logic | self.variant << 5
    }

    /// C64_CARTRIDGE_ACTIVE.
    pub fn active(&self) -> bool {
        self.on
    }

    /// The logic is built here (others latch but stay off).
    fn built(&self) -> bool {
        self.logic == LOGIC_NORMAL
    }

    /// The cartridge's own lines on the port, `true` = pulled low:
    /// (/GAME, /EXROM).
    pub fn lines(&self) -> (bool, bool) {
        if !self.on {
            return (false, false);
        }
        match self.logic {
            LOGIC_NORMAL => (self.variant & 2 == 0, self.variant & 1 == 0),
            _ => (false, false),
        }
    }

    /// The VIC-II reads the cartridge's ROMH in ULTIMAX.
    pub fn serves_vic(&self) -> bool {
        self.on && self.logic == LOGIC_NORMAL && self.variant & 4 != 0
    }

    /// Board memory address of the ROM byte at CPU address `addr`.
    fn rom_addr(&self, addr: u16) -> u32 {
        // 16 KB window, no banking (the normal logic).
        ROM_BASE + u32::from(addr & 0x3fff)
    }

    /// What the cartridge drives for a read in `window`, without side
    /// effects; `None`: it keeps quiet. `ddr` is board memory.
    pub fn peek(&self, window: CartWindow, addr: u16, ddr: &[u8]) -> Option<u8> {
        if !self.on {
            return None;
        }
        match (self.logic, window) {
            (LOGIC_NORMAL, CartWindow::RomL | CartWindow::RomH) => {
                ddr.get(self.rom_addr(addr) as usize).copied()
            }
            _ => None,
        }
    }

    /// A read with its side effects (none for the logics built so far).
    pub fn read(&mut self, window: CartWindow, addr: u16, ddr: &[u8]) -> Option<u8> {
        self.peek(window, addr, ddr)
    }

    /// A write in `window`. `true`: the cartridge's lines may have
    /// changed.
    pub fn write(&mut self, window: CartWindow, addr: u16, v: u8, _ddr: &mut [u8]) -> bool {
        if !self.on || window != CartWindow::Io {
            return false;
        }
        match self.logic {
            LOGIC_NORMAL if addr & 0x1ff == 0x1ff && v & 0xc0 == 0x40 => {
                // $DFFF: off for good.
                self.on = false;
                true
            }
            _ => false,
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.latched());
        w.bool(self.on);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let t = r.u8()?;
        let on = r.bool()?;
        self.latch(t, on);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ddr() -> Vec<u8> {
        let mut m = vec![0u8; 0x400_0000];
        for i in 0..0x4000u32 {
            m[(ROM_BASE + i) as usize] = (i ^ (i >> 8)) as u8;
        }
        m
    }

    #[test]
    fn boot_cartridge_is_8k_at_roml() {
        let mut m = ddr();
        let mut c = CartLogic::new();
        assert_eq!(c.lines(), (false, false));
        // Written while the reset is held: on, /EXROM low.
        c.latch(0x41, true);
        assert!(c.active());
        assert_eq!(c.lines(), (false, true));
        assert_eq!(c.peek(CartWindow::RomL, 0x8004, &m), Some(0x04));
        assert_eq!(c.peek(CartWindow::RomL, 0x9123, &m), Some(0x23 ^ 0x11));
        assert_eq!(c.peek(CartWindow::Io, 0xdfff, &m), None);
        // $0D to $DFFF (the sync) leaves it on, $40 switches it off.
        assert!(!c.write(CartWindow::Io, 0xdfff, 0x0d, &mut m));
        assert!(c.active());
        assert!(!c.write(CartWindow::Io, 0xdffe, 0x40, &mut m));
        assert!(c.write(CartWindow::Io, 0xdfff, 0x40, &mut m));
        assert!(!c.active());
        assert_eq!(c.lines(), (false, false));
        assert_eq!(c.peek(CartWindow::RomL, 0x8004, &m), None);
    }

    #[test]
    fn variants_and_romh() {
        let m = ddr();
        let mut c = CartLogic::new();
        c.latch(0x01, true); // 16 KB
        assert_eq!(c.lines(), (true, true));
        assert_eq!(c.peek(CartWindow::RomH, 0xa000, &m), Some(0x20));
        c.latch(0x21, true); // ULTIMAX: ROMH at $E000 is the upper 8 KB
        assert_eq!(c.lines(), (true, false));
        assert_eq!(c.peek(CartWindow::RomH, 0xe000, &m), Some(0x20));
        assert!(!c.serves_vic());
        c.latch(0xa1, true);
        assert!(c.serves_vic());
        c.latch(0x61, true); // "off": on, but no lines
        assert!(c.active());
        assert_eq!(c.lines(), (false, false));
    }

    #[test]
    fn force_none_and_unbuilt_logics() {
        let mut c = CartLogic::new();
        c.latch(0x41, false); // the force strobe: off
        assert!(!c.active());
        c.latch(0x00, true);
        assert!(!c.active());
        c.latch(0x19, true); // a freezer, not built
        assert!(!c.active());
        assert_eq!(c.latched(), 0x19);
        c.latch(0x41, true);
        c.kill();
        assert!(!c.active());
    }

    #[test]
    fn state_round_trip() {
        let mut c = CartLogic::new();
        c.latch(0x41, true);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = CartLogic::new();
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, d);
    }
}
