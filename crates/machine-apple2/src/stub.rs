//! Built-in test ROM for `roms=stub`: clears the text page, prints a
//! line on row 0 and echoes key presses into row 2 (40 columns, then from
//! the left again). Hand-assembled at $F000; IRQ and NMI return at once.

/// The program at $F000.
const PROGRAM: [u8; 0x43] = [
    0xd8, //             F000 CLD
    0xa2, 0xff, //       F001 LDX #$FF
    0x9a, //             F003 TXS
    0xad, 0x51, 0xc0, // F004 LDA $C051   TEXT
    0xad, 0x54, 0xc0, // F007 LDA $C054   PAGE1
    0xa9, 0xa0, //       F00A LDA #$A0    space
    0xa2, 0x00, //       F00C LDX #0
    0x9d, 0x00, 0x04, // F00E clear: STA $0400,X
    0x9d, 0x00, 0x05, // F011 STA $0500,X
    0x9d, 0x00, 0x06, // F014 STA $0600,X
    0x9d, 0x00, 0x07, // F017 STA $0700,X
    0xe8, //             F01A INX
    0xd0, 0xf1, //       F01B BNE clear
    0xa2, 0x00, //       F01D LDX #0
    0xbd, 0x43, 0xf0, // F01F msg: LDA text,X
    0xf0, 0x06, //       F022 BEQ done
    0x9d, 0x00, 0x04, // F024 STA $0400,X
    0xe8, //             F027 INX
    0xd0, 0xf5, //       F028 BNE msg
    0xa0, 0x00, //       F02A done: LDY #0
    0xad, 0x00, 0xc0, // F02C key: LDA $C000
    0x10, 0xfb, //       F02F BPL key
    0x8d, 0x10, 0xc0, // F031 STA $C010
    0x09, 0x80, //       F034 ORA #$80
    0x99, 0x00, 0x05, // F036 STA $0500,Y   row 2
    0xc8, //             F039 INY
    0xc0, 0x28, //       F03A CPY #40
    0x90, 0xee, //       F03C BCC key
    0xa0, 0x00, //       F03E LDY #0
    0xf0, 0xea, //       F040 BEQ key
    0x40, //             F042 RTI
];

/// The text on row 0.
pub const TEXT: &[u8] = b"MIMIC APPLE II STUB ROM";

/// A ROM image of `len` bytes ending at $FFFF (12 KB for the II/II+,
/// 16 KB for the IIe).
pub fn rom(len: usize) -> Vec<u8> {
    let mut rom = vec![0u8; len];
    let at = |addr: usize| addr - (0x10000 - len);
    rom[at(0xf000)..at(0xf000) + PROGRAM.len()].copy_from_slice(&PROGRAM);
    let text = at(0xf043);
    for (i, &c) in TEXT.iter().enumerate() {
        rom[text + i] = c | 0x80;
    }
    rom[text + TEXT.len()] = 0;
    // NMI $F042, RESET $F000, IRQ $F042.
    rom[at(0xfffa)..].copy_from_slice(&[0x42, 0xf0, 0x00, 0xf0, 0x42, 0xf0]);
    rom
}
