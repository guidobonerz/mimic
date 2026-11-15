//! A small built-in program in place of CAOS (`roms=stub`), for tests
//! without the user's ROMs: it sets the PIO up, shows one white-on-blue
//! pixel byte in the picture's top left corner, starts CTC channel 2 with
//! an interrupt that counts in RAM at `$0000`, and waits.

use crate::Model;

/// The first cell's pixel byte and colour byte.
pub const PIXELS: u8 = 0xa5;
pub const COLOUR: u8 = 7 << 3 | 1;
/// The interrupt routine counts here.
pub const COUNTER: u16 = 0x0000;

/// The 8 KB ROM at `$E000`.
pub fn rom(model: Model) -> Vec<u8> {
    let mut code: Vec<u8> = vec![
        0xf3, // DI
        0x31, 0xc0, 0x01, // LD SP,$01C0
        // The output registers first: the lines switch the memory the
        // moment the ports become outputs.
        0x3e, 0x0f, 0xd3, 0x88, // CAOS ROM, RAM0, IRM on, RAM0 writable
        0x3e, 0x0f, 0xd3, 0x8a, // PIO A: output
        0x3e, 0x00, 0xd3, 0x89, // full volume, no blinking
        0x3e, 0x0f, 0xd3, 0x8b, // PIO B: output
    ];
    if model.four() {
        code.extend([
            0x3e, 0x0a, 0xd3, 0x84, // colour plane of image 0
            0x21, 0x00, 0x80, 0x36, COLOUR, // LD HL,$8000 / LD (HL),colour
            0x3e, 0x08, 0xd3, 0x84, // pixel plane
        ]);
    } else {
        code.extend([0x21, 0x00, 0xa8, 0x36, COLOUR]);
    }
    code.extend([
        0x21, 0x00, 0x80, 0x36, PIXELS, // LD HL,$8000 / LD (HL),pixels
        0xaf, 0x32, 0x00, 0x00, // XOR A / LD ($0000),A
        0x3e, 0xe0, 0xed, 0x47, // LD A,$E0 / LD I,A
        0xed, 0x5e, // IM 2
        0x3e, 0xf8, 0xd3, 0x8c, // CTC vectors from $F8: table at $E0F8
        0x3e, 0xa7, 0xd3, 0x8e, // channel 2: timer, prescaler 256, interrupt
        0x3e, 0x40, 0xd3, 0x8e, // time constant 64
        0xfb, // EI
        0x76, // HALT
        0x18, 0xfd, // JR to the HALT
    ]);
    let mut rom = vec![0xff; 0x2000];
    rom[..code.len()].copy_from_slice(&code);
    // The interrupt routine at $E200, its vector (channel 2) at $E0FC.
    rom[0xfc..0xfe].copy_from_slice(&[0x00, 0xe2]);
    rom[0x200..0x208].copy_from_slice(&[
        0xf5, // PUSH AF
        0x21, 0x00, 0x00, // LD HL,$0000
        0x34, // INC (HL)
        0xf1, // POP AF
        0xfb, // EI
        0xed, // RETI ...
    ]);
    rom[0x208] = 0x4d;
    // Power-on enters at $F000.
    rom[0x1000..0x1003].copy_from_slice(&[0xc3, 0x00, 0xe0]);
    rom
}
