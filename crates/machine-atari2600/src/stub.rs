//! Built-in test cartridge for `roms=stub`: a complete NTSC frame loop
//! (3 lines VSYNC, 37 VBLANK, 192 picture, 30 overscan = 262 lines) that
//! sets the background colour of each picture line from a down counter,
//! 192 on the first picture line. Hand-assembled; 4 KB at `$F000`.

const PROGRAM: [u8; 0x37] = [
    0x78, //             F000 SEI
    0xd8, //             F001 CLD
    0xa2, 0xff, //       F002 LDX #$FF
    0x9a, //             F004 TXS
    0xa9, 0x02, //       F005 frame: LDA #2
    0x85, 0x01, //       F007 STA VBLANK
    0x85, 0x00, //       F009 STA VSYNC
    0x85, 0x02, //       F00B STA WSYNC
    0x85, 0x02, //       F00D STA WSYNC
    0x85, 0x02, //       F00F STA WSYNC
    0xa9, 0x00, //       F011 LDA #0
    0x85, 0x00, //       F013 STA VSYNC
    0xa2, 0x25, //       F015 LDX #37
    0x85, 0x02, //       F017 vblank: STA WSYNC
    0xca, //             F019 DEX
    0xd0, 0xfb, //       F01A BNE vblank
    0xa9, 0x00, //       F01C LDA #0
    0x85, 0x01, //       F01E STA VBLANK
    0xa2, 0xc0, //       F020 LDX #192
    0x86, 0x09, //       F022 picture: STX COLUBK
    0x85, 0x02, //       F024 STA WSYNC
    0xca, //             F026 DEX
    0xd0, 0xf9, //       F027 BNE picture
    0xa9, 0x02, //       F029 LDA #2
    0x85, 0x01, //       F02B STA VBLANK
    0xa2, 0x1e, //       F02D LDX #30
    0x85, 0x02, //       F02F overscan: STA WSYNC
    0xca, //             F031 DEX
    0xd0, 0xfb, //       F032 BNE overscan
    0x4c, 0x05, 0xf0, // F034 JMP frame
];

/// Picture lines of a field and the line of the first one (after VSYNC).
pub const PICTURE_LINES: u16 = 192;
pub const FIRST_PICTURE_LINE: u16 = 37;

/// The 4 KB cartridge image.
pub fn cartridge() -> Vec<u8> {
    let mut rom = vec![0u8; 0x1000];
    rom[..PROGRAM.len()].copy_from_slice(&PROGRAM);
    // RESET and IRQ/BRK vectors: $F000.
    rom[0xffc..].copy_from_slice(&[0x00, 0xf0, 0x00, 0xf0]);
    rom
}
