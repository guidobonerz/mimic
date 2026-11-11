//! A 128 KB test ROM of this project's own (`--option roms=stub`): the C64
//! KERNAL's reset vector at `$2FFFC` points to code at `$E000` that knocks
//! the MEGA65 I/O personality open, sets the border to red, writes
//! "MIMIC" in screen codes to `$0800` and stops in a loop.

/// The program at `$E000` (file offset `$E000`).
const CODE: &[u8] = &[
    0x78, // SEI
    0xa9, 0x47, 0x8d, 0x2f, 0xd0, // LDA #$47, STA $D02F
    0xa9, 0x53, 0x8d, 0x2f, 0xd0, // LDA #$53, STA $D02F
    0xa9, 0x02, 0x8d, 0x20, 0xd0, // LDA #$02, STA $D020
    0xa2, 0x00, // LDX #$00
    0xbd, 0x20, 0xe0, // loop: LDA text,X
    0xf0, 0x06, // BEQ done
    0x9d, 0x00, 0x08, // STA $0800,X
    0xe8, // INX
    0xd0, 0xf5, // BNE loop
    0x4c, 0x1d, 0xe0, // done: JMP done
    0x0d, 0x09, 0x0d, 0x09, 0x03, 0x00, // text: "MIMIC"
];

/// The address of the final loop.
pub const DONE: u16 = 0xe01d;

pub fn rom() -> Vec<u8> {
    let mut rom = vec![0xff; 0x2_0000];
    rom[0xe000..0xe000 + CODE.len()].copy_from_slice(CODE);
    // NMI, RESET, IRQ at $FFFA (C64 KERNAL half, file offset $FFFA).
    rom[0xfffa..0x1_0000].copy_from_slice(&[0x1d, 0xe0, 0x00, 0xe0, 0x1d, 0xe0]);
    rom
}
