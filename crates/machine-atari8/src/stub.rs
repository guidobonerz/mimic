//! Built-in OS replacement for runs without Atari ROMs
//! (`options["roms"] = "stub"`): tests and CI.
//!
//! Written for this project, not derived from the Atari OS. It points
//! ANTIC at a display list in the ROM (24 blank lines, 24 mode 2 lines of
//! screen memory at `$4000`), enables the VBI, counts fields in `$14`
//! (like the OS's RTCLOK) and idles.

use crate::memory::OS_SIZE;

/// Address of the idle loop after reset.
pub const IDLE: u16 = 0xf024;
/// Screen memory of the stub's display list.
pub const SCREEN: u16 = 0x4000;
const DL: u16 = 0xf100;

const RESET: &[u8] = &[
    0x78, // F000 SEI
    0xd8, // CLD
    0xa2, 0xff, // LDX #$FF
    0x9a, // TXS
    0xa9, 0x00, 0x8d, 0x02, 0xd4, // DLISTL = <DL
    0xa9, 0xf1, 0x8d, 0x03, 0xd4, // DLISTH = >DL
    0xa9, 0xe0, 0x8d, 0x09, 0xd4, // CHBASE = $E0
    0xa9, 0x22, 0x8d, 0x00, 0xd4, // DMACTL: normal playfield + DL DMA
    0xa9, 0x40, 0x8d, 0x0e, 0xd4, // NMIEN: VBI
    0xa9, 0x03, 0x8d, 0x0f, 0xd2, // SKCTL: keyboard scan
    0x58, // CLI
    0x4c, 0x24, 0xf0, // F024 JMP F024
];

const NMI: &[u8] = &[
    0x48, // F030 PHA
    0xe6, 0x14, // INC $14
    0x8d, 0x0f, 0xd4, // STA NMIRES
    0x68, // PLA
    0x40, // RTI
];

const IRQ: &[u8] = &[0x40];

fn display_list() -> Vec<u8> {
    let mut dl = vec![0x70, 0x70, 0x70, 0x42, SCREEN as u8, (SCREEN >> 8) as u8];
    dl.extend([0x02; 23]);
    dl.extend([0x41, DL as u8, (DL >> 8) as u8]);
    dl
}

pub fn os() -> Box<[u8; OS_SIZE]> {
    let mut rom = Box::new([0xff; OS_SIZE]);
    let mut put = |addr: u16, code: &[u8]| {
        let at = usize::from(addr - 0xc000);
        rom[at..at + code.len()].copy_from_slice(code);
    };
    put(0xf000, RESET);
    put(0xf030, NMI);
    put(0xf040, IRQ);
    put(DL, &display_list());
    // Blank character set at $E000.
    put(0xe000, &[0; 0x400]);
    put(0xfffa, &[0x30, 0xf0, 0x00, 0xf0, 0x40, 0xf0]);
    rom
}
