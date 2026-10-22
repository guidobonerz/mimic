//! A tiny built-in ROM for tests without the Sinclair ROM (`--option
//! roms=stub`): it builds a display file at `$4000` of 7 blank rows, 24
//! rows of boxes and 7 blank rows (each row 24 characters and a HALT),
//! points I at a pattern table at `$1E00` (a box for every code but 0) and
//! drives the picture the ZX81 way: the NMI generator runs on every line
//! and the NMI handler jumps into the current row at its `$C000`-based
//! address, eight lines per row (the HALT at the row's end waits for the
//! next NMI); after the last row it makes a VSYNC with `IN ($FE)` /
//! `OUT ($FE)` and counts fields at `$4500`. 304 + 16 lines per field.

use crate::memory::ROM_SIZE;

/// The code at $0000 (reset; it runs through $0038, interrupts stay
/// disabled) and $0066 (NMI).
/// Hand-assembled; opcodes from the Zilog Z80 manual. Both paths of the
/// NMI handler (a line within a row, and the row change) take 56
/// T-states, so with the ULA's 16-T-state release and the 11 of the
/// acknowledge every row starts at T-state 83 of its line and its 24
/// characters (100 T-states) end well inside the 207.
const CODE: &[(u16, &[u8])] = &[
    (
        0x0000,
        &[
            0xf3, // DI
            0x31, 0xff, 0x7f, // LD SP,$7FFF
            0x3e, 0x1e, // LD A,$1E
            0xed, 0x47, // LD I,A
            0x21, 0x00, 0x40, // LD HL,$4000
            0x36, 0x76, // LD (HL),$76       first HALT
            0x23, // INC HL
            0x06, 0x26, // LD B,38          rows: 7 blank, 24 boxes, 7 blank
            0x0e, 0x18, // LD C,24          ROW:
            0x78, // LD A,B
            0xfe, 0x20, // CP 32
            0x30, 0x08, // JR NC,BLANK
            0xfe, 0x08, // CP 8
            0x38, 0x04, // JR C,BLANK
            0xc6, 0x1f, // ADD A,$1F        code $27-$3E
            0x18, 0x01, // JR COL
            0xaf, // XOR A             BLANK:
            0x77, // LD (HL),A         COL:
            0x23, // INC HL
            0x0d, // DEC C
            0x20, 0xfb, // JR NZ,COL
            0x36, 0x76, // LD (HL),$76
            0x23, // INC HL
            0x10, 0xe6, // DJNZ ROW
            0x21, 0x01, 0xc0, // LD HL,$C001      the first row, at +$8000
            0x0e, 0x26, // LD C,38          rows left
            0x06, 0x08, // LD B,8           lines left in the row
            0x11, 0x19, 0x00, // LD DE,25         a row: 24 characters and the HALT
            0xaf, // XOR A
            0x32, 0x00, 0x45, // LD (FRAMES),A
            0xd3, 0xfe, // OUT ($FE),A       NMI on (runs over $0038: INT stays off)
            0x76, // HALT              MAIN:
            0x18, 0xfd, // JR MAIN
        ],
    ),
    (
        0x0066,
        &[
            0x33, // INC SP            drop the return address
            0x33, // INC SP
            0x05, // DEC B
            0x20, 0x07, // JR NZ,FAST
            0x06, 0x08, // LD B,8
            0x19, // ADD HL,DE         next row
            0x0d, // DEC C
            0x28, 0x08, // JR Z,VSYNC
            0xe9, // JP (HL)           the row, ending in HALT
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // FAST: six NOPs, as long as the row change
            0xe9, // JP (HL)
            0xd3, 0xfd, // OUT ($FD),A       VSYNC: NMI off
            0xdb, 0xfe, // IN A,($FE)        VSYNC on
            0x06, 0x00, // LD B,0
            0x10, 0xfe, // DJNZ $            about 16 lines
            0x21, 0x01, 0xc0, // LD HL,$C001
            0x0e, 0x26, // LD C,38
            0x06, 0x08, // LD B,8
            0x3a, 0x00, 0x45, // LD A,(FRAMES)
            0x3c, // INC A
            0x32, 0x00, 0x45, // LD (FRAMES),A
            0xd3, 0xfe, // OUT ($FE),A       VSYNC off, NMI on
            0x76, // HALT
            0x18, 0xfd, // JR $-1
        ],
    ),
];

/// Where the stub counts fields (needs the 16 KB RAM pack).
pub const FRAMES: u16 = 0x4500;
/// Characters per row and rows of boxes in the stub's display file.
pub const COLUMNS: u32 = 24;
pub const ROWS: u32 = 24;
/// The pattern table (I = $1E).
pub const PATTERNS: u16 = 0x1e00;

/// The 8 KB image.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0u8; ROM_SIZE];
    for (at, code) in CODE {
        let at = usize::from(*at);
        rom[at..at + code.len()].copy_from_slice(code);
    }
    // Patterns: code 0 blank, every other code a box.
    for code in 1..64usize {
        for row in 0..8usize {
            let byte = if row == 0 || row == 7 { 0xff } else { 0x81 };
            rom[usize::from(PATTERNS) + code * 8 + row] = byte;
        }
    }
    rom
}
