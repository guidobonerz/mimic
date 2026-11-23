//! A small test cartridge built in code, for tests without any ROM file
//! (golden rule 6): it draws a tile, sends "OK" through the serial port,
//! writes its battery RAM, counts vertical blanks by interrupt and keeps
//! the buttons' state in RAM.

/// Work RAM the program writes: the buttons A, B, Select, Start (bits
/// 0–3, set when pressed), the directions Right, Left, Up, Down, and the
/// count of vertical blanks.
pub const BUTTONS: u16 = 0xc000;
pub const DIRECTIONS: u16 = 0xc001;
pub const BLANKS: u16 = 0xc002;
/// The byte it leaves at the start of the cartridge's RAM.
pub const SAVED: u8 = 0x5a;

/// A little assembler: bytes, and relative jumps to marks.
struct Asm {
    rom: Vec<u8>,
    at: usize,
}

impl Asm {
    fn put(&mut self, bytes: &[u8]) {
        self.rom[self.at..self.at + bytes.len()].copy_from_slice(bytes);
        self.at += bytes.len();
    }

    /// A two-byte relative jump with opcode `op` back to `mark`.
    fn back(&mut self, op: u8, mark: usize) {
        let offset = mark as i32 - (self.at as i32 + 2);
        self.put(&[op, offset as i8 as u8]);
    }
}

/// The 32 KB image: MBC1 with 8 KB of battery RAM.
pub fn rom() -> Vec<u8> {
    let mut a = Asm {
        rom: vec![0xff; 0x8000],
        at: 0x40,
    };
    // The vertical blank's interrupt: count it.
    a.put(&[0xf5, 0xfa, 0x02, 0xc0, 0x3c, 0xea, 0x02, 0xc0, 0xf1, 0xd9]);
    a.at = 0x100;
    a.put(&[0x00, 0xc3, 0x50, 0x01]);
    a.rom[0x134..0x13e].copy_from_slice(b"MIMIC DEMO");
    a.rom[0x13e..0x150].fill(0);
    a.rom[0x147] = 0x03;
    a.rom[0x148] = 0x00;
    a.rom[0x149] = 0x02;
    let sum = a.rom[0x134..0x14d]
        .iter()
        .fold(0u8, |s, b| s.wrapping_sub(*b).wrapping_sub(1));
    a.rom[0x14d] = sum;

    a.at = 0x150;
    // DI : LD SP,$FFFE
    a.put(&[0xf3, 0x31, 0xfe, 0xff]);
    // Wait for line 144, then the display off.
    let wait = a.at;
    a.put(&[0xf0, 0x44, 0xfe, 0x90]);
    a.back(0x20, wait);
    a.put(&[0xaf, 0xe0, 0x40]);
    // Tile 1: colour 3 everywhere.
    a.put(&[0x21, 0x10, 0x80, 0x06, 0x10, 0x3e, 0xff]);
    let fill = a.at;
    a.put(&[0x22, 0x05]);
    a.back(0x20, fill);
    // The map's first entry, the palette, the display on.
    a.put(&[0x3e, 0x01, 0xea, 0x00, 0x98]);
    a.put(&[0x3e, 0xe4, 0xe0, 0x47]);
    a.put(&[0x3e, 0x91, 0xe0, 0x40]);
    // The cartridge's RAM on, a byte into it.
    a.put(&[0x3e, 0x0a, 0xea, 0x00, 0x00]);
    a.put(&[0x3e, SAVED, 0xea, 0x00, 0xa0]);
    // "OK" through the serial port.
    for c in *b"OK" {
        a.put(&[0x3e, c, 0xe0, 0x01, 0x3e, 0x81, 0xe0, 0x02]);
        let sending = a.at;
        a.put(&[0xf0, 0x02, 0xcb, 0x7f]);
        a.back(0x20, sending);
    }
    // The vertical blank's interrupt on.
    a.put(&[0xaf, 0xe0, 0x0f, 0x3e, 0x01, 0xe0, 0xff, 0xfb]);
    // Each frame: the buttons, then the directions, into RAM.
    let main = a.at;
    a.put(&[0x3e, 0x10, 0xe0, 0x00, 0xf0, 0x00, 0x2f, 0xe6, 0x0f]);
    a.put(&[0xea, 0x00, 0xc0]);
    a.put(&[0x3e, 0x20, 0xe0, 0x00, 0xf0, 0x00, 0x2f, 0xe6, 0x0f]);
    a.put(&[0xea, 0x01, 0xc0]);
    a.put(&[0x76]);
    a.back(0x18, main);
    a.rom
}
