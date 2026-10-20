//! VIC (6560/6561) colours.
//!
//! Names and order from the VIC-20 Programmer's Reference Guide; colours
//! 0–7 are the character and border colours, 8–15 their light variants
//! (background and auxiliary colours only). The RGB values are this
//! project's approximation of the hues on a monitor, not measurements; the
//! composite signal stage in `video-crt` refines the look.

use emu_core::frame::Rgb8;

const COLORS: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00), // 0 black
    (0xff, 0xff, 0xff), // 1 white
    (0xa8, 0x2b, 0x24), // 2 red
    (0x7a, 0xdc, 0xe4), // 3 cyan
    (0xa8, 0x38, 0xb0), // 4 purple
    (0x58, 0xb8, 0x40), // 5 green
    (0x30, 0x2c, 0xb4), // 6 blue
    (0xd8, 0xd8, 0x60), // 7 yellow
    (0xb0, 0x68, 0x20), // 8 orange
    (0xe8, 0xb0, 0x78), // 9 light orange
    (0xe8, 0x90, 0x90), // 10 pink
    (0xb0, 0xf0, 0xf8), // 11 light cyan
    (0xe0, 0x98, 0xe8), // 12 light purple
    (0xa0, 0xe8, 0x90), // 13 light green
    (0x98, 0x90, 0xf0), // 14 light blue
    (0xf0, 0xf0, 0xa8), // 15 light yellow
];

pub fn palette() -> [Rgb8; 16] {
    COLORS.map(|(r, g, b)| Rgb8 { r, g, b })
}
