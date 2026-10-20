//! Built-in KERNAL replacement for runs without Commodore ROMs
//! (`options["roms"] = "stub"`): tests and CI.
//!
//! Written for this project, not derived from the Commodore KERNAL. It
//! enables the retrace interrupt (PIA1 CB1) like the editor does, idles,
//! and acknowledges each interrupt by reading PIA1 port B.

use crate::KERNAL_SIZE;

/// Address of the idle loop after reset.
pub const IDLE: u16 = 0xf00b;

/// Address of the interrupt handler.
pub const IRQ: u16 = 0xf100;

const RESET: &[u8] = &[
    0x78, // F000 SEI
    0xa2, 0xff, // LDX #$FF
    0x9a, // TXS
    0xd8, // CLD
    0xa9, 0x3d, 0x8d, 0x13, 0xe8, // CRB: port B, CB1 IRQ on the falling edge
    0x58, // CLI
    0x4c, 0x0b, 0xf0, // F00B JMP $F00B
];

const IRQ_HANDLER: &[u8] = &[
    0x48, // F100 PHA
    0xad, 0x12, 0xe8, // LDA $E812 (acknowledge CB1)
    0x68, // PLA
    0x40, // RTI
];

pub fn kernal() -> Vec<u8> {
    let mut k = vec![0xff; KERNAL_SIZE];
    k[..RESET.len()].copy_from_slice(RESET);
    k[0x100..0x100 + IRQ_HANDLER.len()].copy_from_slice(IRQ_HANDLER);
    k[0x200] = 0x40; // NMI: RTI
    k[0xffa..].copy_from_slice(&[0x00, 0xf2, 0x00, 0xf0, 0x00, 0xf1]);
    k
}
