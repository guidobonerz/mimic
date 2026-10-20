//! Built-in ROM replacement for runs without Commodore ROMs
//! (`options["roms"] = "stub"`): tests and CI.
//!
//! Written for this project, not derived from Commodore's ROMs. Like the
//! real boot, the Z80 starts at `$0000` (its BIOS window, KERNAL ROM
//! `$D000`) and hands over to the 8502 through MMU `$D505`; the 8502 then
//! starts at its reset vector, sets up a 60 Hz CIA1 timer interrupt and
//! idles.

use crate::memory::ROM_16K;

/// The 8502's idle loop.
pub const IDLE: u16 = 0xe01f;

/// The 8502's interrupt handler.
pub const IRQ: u16 = 0xe100;

/// Z80 at `$0000`: LD BC,$D505; LD A,$01; OUT (C),A (8502 on); JR $.
const Z80_BOOT: &[u8] = &[0x01, 0x05, 0xd5, 0x3e, 0x01, 0xed, 0x79, 0x18, 0xfe];

const RESET: &[u8] = &[
    0x78, // E000 SEI
    0xa2, 0xff, // LDX #$FF
    0x9a, // TXS
    0xd8, // CLD
    0xa9, 0x7f, 0x8d, 0x0d, 0xdc, // CIA1 interrupts off
    0xa9, 0x25, 0x8d, 0x04, 0xdc, // timer A = $4025
    0xa9, 0x40, 0x8d, 0x05, 0xdc, //
    0xa9, 0x81, 0x8d, 0x0d, 0xdc, // timer A interrupt on
    0xa9, 0x11, 0x8d, 0x0e, 0xdc, // start, force load
    0x58, // CLI
    0x4c, 0x1f, 0xe0, // E01F JMP $E01F
];

const IRQ_HANDLER: &[u8] = &[
    0x48, // PHA
    0xad, 0x0d, 0xdc, // LDA $DC0D (acknowledge)
    0x68, // PLA
    0x40, // RTI
];

pub fn kernal() -> Box<[u8; ROM_16K]> {
    let mut k = Box::new([0xff; ROM_16K]);
    k[0x1000..0x1000 + Z80_BOOT.len()].copy_from_slice(Z80_BOOT);
    k[0x2000..0x2000 + RESET.len()].copy_from_slice(RESET);
    k[0x2100..0x2100 + IRQ_HANDLER.len()].copy_from_slice(IRQ_HANDLER);
    k[0x2200] = 0x40; // NMI: RTI
    k[0x3ffa..].copy_from_slice(&[0x00, 0xe2, 0x00, 0xe0, 0x00, 0xe1]);
    k
}
