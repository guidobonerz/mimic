//! The board's cartridge on the C64's expansion port.
//!
//! The FPGA plays the cartridge itself: its logic is
//! `chips_ultimate::c64port::CartLogic` (type, on/off, the lines), its ROM
//! and RAM are board memory, read live. Here it is put on the C64's
//! expansion port as an [`ExpansionCart`], so the C64's PLA decides what
//! is mapped as for any cartridge. The 6510 and the VIC-II reach it
//! through these views; the board's own DMA reaches it inside
//! `chips-ultimate` (`C64Side::cart_window`).
//!
//! While the board is there it owns the expansion port: `ultimate64` has
//! no cartridge slot of its own (a cartridge of the user's would be one
//! the firmware loads, a later step).

use chips_ultimate::c64port::{CartLogic, CartWindow};
use devices_cartridge::expansion::{CartLines, ExpansionCart};
use mimic_core::{StateError, StateReader, StateWriter};

/// The cartridge for the 6510: reads with their side effects, writes.
pub(crate) struct BoardCart<'a> {
    pub logic: &'a mut CartLogic,
    pub ddr: &'a mut [u8],
}

/// The cartridge for peeks and the VIC-II.
pub(crate) struct BoardCartPeek<'a> {
    pub logic: &'a CartLogic,
    pub ddr: &'a [u8],
}

fn lines(logic: &CartLogic) -> CartLines {
    let (game, exrom) = logic.lines();
    CartLines {
        game,
        exrom,
        nmi: false,
        irq: false,
    }
}

/// The board saves the logic with its own state.
const NAME: &str = "Ultimate board cartridge";

impl ExpansionCart for BoardCart<'_> {
    fn name(&self) -> &'static str {
        NAME
    }

    fn lines(&self) -> CartLines {
        lines(self.logic)
    }

    fn peek_roml(&self, addr: u16) -> Option<u8> {
        self.logic.peek(CartWindow::RomL, addr, self.ddr)
    }

    fn peek_romh(&self, addr: u16) -> Option<u8> {
        self.logic.peek(CartWindow::RomH, addr, self.ddr)
    }

    fn read_roml(&mut self, addr: u16) -> Option<u8> {
        self.logic.read(CartWindow::RomL, addr, self.ddr)
    }

    fn read_romh(&mut self, addr: u16) -> Option<u8> {
        self.logic.read(CartWindow::RomH, addr, self.ddr)
    }

    fn write_roml(&mut self, addr: u16, v: u8) {
        self.logic.write(CartWindow::RomL, addr, v, self.ddr);
    }

    fn write_romh(&mut self, addr: u16, v: u8) {
        self.logic.write(CartWindow::RomH, addr, v, self.ddr);
    }

    fn vic_sees_ultimax(&self) -> bool {
        true
    }

    fn peek_io(&self, addr: u16) -> Option<u8> {
        self.logic.peek(CartWindow::Io, addr, self.ddr)
    }

    fn read_io(&mut self, addr: u16) -> Option<u8> {
        self.logic.read(CartWindow::Io, addr, self.ddr)
    }

    fn write_io(&mut self, addr: u16, v: u8) {
        // The lines that change reach the PLA after the access
        // (`C64::cartridge_cycle`).
        self.logic.write(CartWindow::Io, addr, v, self.ddr);
    }

    fn reset(&mut self) {}

    fn save(&self, _w: &mut StateWriter) {}

    fn load(&mut self, _r: &mut StateReader<'_>) -> Result<(), StateError> {
        Ok(())
    }
}

impl ExpansionCart for BoardCartPeek<'_> {
    fn name(&self) -> &'static str {
        NAME
    }

    fn lines(&self) -> CartLines {
        lines(self.logic)
    }

    fn peek_roml(&self, addr: u16) -> Option<u8> {
        self.logic.peek(CartWindow::RomL, addr, self.ddr)
    }

    fn peek_romh(&self, addr: u16) -> Option<u8> {
        self.logic.peek(CartWindow::RomH, addr, self.ddr)
    }

    fn peek_io(&self, addr: u16) -> Option<u8> {
        self.logic.peek(CartWindow::Io, addr, self.ddr)
    }

    fn reset(&mut self) {}

    fn save(&self, _w: &mut StateWriter) {}

    fn load(&mut self, _r: &mut StateReader<'_>) -> Result<(), StateError> {
        Ok(())
    }
}

impl super::glue::BoardPart {
    pub(crate) fn cart(&mut self) -> BoardCart<'_> {
        let (logic, ddr) = self.board.cart_parts();
        BoardCart { logic, ddr }
    }

    pub(crate) fn cart_peek(&self) -> BoardCartPeek<'_> {
        BoardCartPeek {
            logic: &self.board.c64.carts,
            ddr: self.board.ram(),
        }
    }

    /// The lines the board drives into the port (`true` = low): /GAME,
    /// /EXROM.
    pub(crate) fn cart_lines(&self) -> (bool, bool) {
        self.board.c64.cart_lines()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chips_ultimate::c64port::carts::ROM_BASE;

    #[test]
    fn views_read_board_memory() {
        let mut ddr = vec![0u8; 0x400_0000];
        ddr[ROM_BASE as usize + 4] = 0xc3;
        ddr[ROM_BASE as usize + 0x2000] = 0x55;
        let mut logic = CartLogic::new();
        logic.latch(0x41, true);
        {
            let c = BoardCartPeek {
                logic: &logic,
                ddr: &ddr,
            };
            assert_eq!(c.peek_roml(0x8004), Some(0xc3));
            assert!(c.lines().exrom && !c.lines().game);
        }
        let mut c = BoardCart {
            logic: &mut logic,
            ddr: &mut ddr,
        };
        assert_eq!(c.read_roml(0x8004), Some(0xc3));
        assert_eq!(c.read_io(0xdfff), None);
        c.write_io(0xdfff, 0x40);
        assert_eq!(c.read_roml(0x8004), None);
        assert!(!c.lines().exrom);
    }
}
