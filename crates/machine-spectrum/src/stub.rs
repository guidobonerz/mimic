//! A tiny built-in ROM for tests without the Sinclair ROMs (`--option
//! roms=stub`): it fills the bitmap with `$55` and the attributes with
//! black ink on white paper, then halts with interrupts on (IM 1); the
//! interrupt handler counts frames at `$5B00` and sets the border to the
//! count's low three bits.

use crate::memory::ROM_SIZE;

/// Address of the frame counter.
pub const FRAMES: u16 = 0x5b00;

/// Hand-assembled; opcodes from the Zilog Z80 manual.
const CODE: &[(u16, &[u8])] = &[
    (
        0x0000,
        &[
            0xf3, // DI
            0x31, 0xf0, 0xff, // LD SP,$FFF0
            0x21, 0x00, 0x40, // LD HL,$4000
            0x11, 0x01, 0x40, // LD DE,$4001
            0x01, 0xff, 0x17, // LD BC,$17FF
            0x36, 0x55, // LD (HL),$55
            0xed, 0xb0, // LDIR
            0x21, 0x00, 0x58, // LD HL,$5800
            0x11, 0x01, 0x58, // LD DE,$5801
            0x01, 0xff, 0x02, // LD BC,$02FF
            0x36, 0x38, // LD (HL),$38      white paper, black ink
            0xed, 0xb0, // LDIR
            0xaf, // XOR A
            0x32, 0x00, 0x5b, // LD (FRAMES),A
            0xed, 0x56, // IM 1
            0xfb, // EI
            0x76, // HALT              MAIN:
            0x18, 0xfd, // JR MAIN
        ],
    ),
    (
        0x0038,
        &[
            0xf5, // PUSH AF
            0x3a, 0x00, 0x5b, // LD A,(FRAMES)
            0x3c, // INC A
            0x32, 0x00, 0x5b, // LD (FRAMES),A
            0xe6, 0x07, // AND 7
            0xd3, 0xfe, // OUT ($FE),A
            0xf1, // POP AF
            0xfb, // EI
            0xc9, // RET
        ],
    ),
];

/// The stub as `count` 16 KB ROMs (the same code in each).
pub fn roms(count: usize) -> Vec<Vec<u8>> {
    let mut rom = vec![0u8; ROM_SIZE];
    for (at, code) in CODE {
        let at = usize::from(*at);
        rom[at..at + code.len()].copy_from_slice(code);
    }
    vec![rom; count]
}
