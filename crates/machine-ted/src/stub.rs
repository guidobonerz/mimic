//! Built-in KERNAL replacement for runs without Commodore ROMs
//! (`options["roms"] = "stub"`): tests and CI.
//!
//! Written for this project, not derived from the Commodore KERNAL. It
//! switches on the KERNAL's text screen, enables a raster interrupt at line
//! 204, idles, and acknowledges each interrupt.

use crate::memory::ROM_SIZE;

/// Address of the idle loop after reset.
pub const IDLE: u16 = 0xc01a;

/// Address of the interrupt handler.
pub const IRQ: u16 = 0xc100;

const RESET: &[u8] = &[
    0x78, // C000 SEI
    0xa2, 0xff, // LDX #$FF
    0x9a, // TXS
    0xd8, // CLD
    0xa9, 0x1b, 0x8d, 0x06, 0xff, // 25 rows, display on, YSCROLL 3
    0xa9, 0x08, 0x8d, 0x07, 0xff, // 40 columns
    0xa9, 0xcc, 0x8d, 0x0b, 0xff, // raster compare line 204
    0xa9, 0x02, 0x8d, 0x0a, 0xff, // raster IRQ on
    0x58, // CLI
    0x4c, 0x1a, 0xc0, // C01A JMP $C01A
];

const IRQ_HANDLER: &[u8] = &[
    0x48, // C100 PHA
    0xa9, 0x02, 0x8d, 0x09, 0xff, // acknowledge the raster IRQ
    0x68, // PLA
    0x40, // RTI
];

pub fn kernal() -> Box<[u8; ROM_SIZE]> {
    let mut k = Box::new([0xff; ROM_SIZE]);
    k[..RESET.len()].copy_from_slice(RESET);
    k[0x100..0x100 + IRQ_HANDLER.len()].copy_from_slice(IRQ_HANDLER);
    k[0x200] = 0x40; // NMI: RTI
    k[0x3ffa..].copy_from_slice(&[0x00, 0xc2, 0x00, 0xc0, 0x00, 0xc1]);
    k
}
