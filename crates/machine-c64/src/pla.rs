//! PLA 906114-01: what the CPU sees in each 4 KB page.
//!
//! Inputs are the 6510 port lines LORAM, HIRAM, CHAREN (`$01` bits 0–2) and
//! the cartridge lines EXROM and GAME (active low, pulled high without a
//! cartridge). The 32 combinations are numbered like the "mode" column of
//! the C64 bank-switching table: `EXROM GAME CHAREN HIRAM LORAM` as a 5-bit
//! number, so mode 31 is the power-on default without a cartridge.
//!
//! Sources: Commodore 64 Programmer's Reference Guide, chapter 5 (memory
//! map); C64-Wiki "Bank Switching" table. The table below is derived from
//! those documents, not from another emulator.
//!
//! This is the CPU view only. The VIC-II has its own view (16 KB banks
//! selected by CIA2, character ROM at `$1000`/`$9000`); that arrives with
//! the VIC-II.

/// What a 4 KB page of the CPU address space maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bank {
    Ram,
    Basic,
    Kernal,
    /// Character ROM at `$D000`.
    Char,
    /// I/O at `$D000`: VIC-II, SID, colour RAM, CIA1, CIA2, I/O1, I/O2.
    Io,
    /// Cartridge ROML (`$8000`, or `$8000`/`$A000` window per mode).
    RomL,
    /// Cartridge ROMH (`$A000` in 16 KB mode, `$E000` in Ultimax mode).
    RomH,
    /// Nothing drives the bus (Ultimax mode): reads see open bus, writes vanish.
    Open,
}

/// The PLA inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lines {
    pub loram: bool,
    pub hiram: bool,
    pub charen: bool,
    /// Cartridge line level (true = high = no cartridge pulling it).
    pub game: bool,
    pub exrom: bool,
}

impl Lines {
    /// From the 6510 port pin levels and the cartridge lines.
    pub fn new(port_pins: u8, game: bool, exrom: bool) -> Self {
        Self {
            loram: port_pins & 1 != 0,
            hiram: port_pins & 2 != 0,
            charen: port_pins & 4 != 0,
            game,
            exrom,
        }
    }

    /// Mode number 0–31 (`EXROM GAME CHAREN HIRAM LORAM`).
    pub fn mode(self) -> u8 {
        u8::from(self.exrom) << 4
            | u8::from(self.game) << 3
            | u8::from(self.charen) << 2
            | u8::from(self.hiram) << 1
            | u8::from(self.loram)
    }
}

/// The 16 page mappings of one mode.
pub type Map = [Bank; 16];

const fn mode_map(mode: u8) -> Map {
    let loram = mode & 1 != 0;
    let hiram = mode & 2 != 0;
    let charen = mode & 4 != 0;
    let game = mode & 8 != 0;
    let exrom = mode & 16 != 0;
    let mut map = [Bank::Ram; 16];

    // Ultimax: EXROM high, GAME low. Only the first 4 KB of RAM remain.
    if exrom && !game {
        let mut page = 1;
        while page < 16 {
            map[page] = Bank::Open;
            page += 1;
        }
        map[0x8] = Bank::RomL;
        map[0x9] = Bank::RomL;
        map[0xd] = Bank::Io;
        map[0xe] = Bank::RomH;
        map[0xf] = Bank::RomH;
        return map;
    }

    // 16 KB cartridge with HIRAM low hides everything but RAM, except I/O
    // when CHAREN is high and LORAM is high (mode 5).
    let cart16 = !exrom && !game;
    let any_rom = loram || hiram;

    // $8000–$9FFF: ROML with a cartridge (8 KB or 16 KB) when LORAM and HIRAM.
    if !exrom && loram && hiram {
        map[0x8] = Bank::RomL;
        map[0x9] = Bank::RomL;
    }

    // $A000–$BFFF
    if cart16 {
        if hiram {
            map[0xa] = Bank::RomH;
            map[0xb] = Bank::RomH;
        }
    } else if loram && hiram {
        map[0xa] = Bank::Basic;
        map[0xb] = Bank::Basic;
    }

    // $D000–$DFFF
    if any_rom {
        map[0xd] = if charen {
            Bank::Io
        } else if cart16 && !hiram {
            Bank::Ram
        } else {
            Bank::Char
        };
    }

    // $E000–$FFFF
    if hiram {
        map[0xe] = Bank::Kernal;
        map[0xf] = Bank::Kernal;
    }
    map
}

const fn build() -> [Map; 32] {
    let mut table = [[Bank::Ram; 16]; 32];
    let mut mode = 0;
    while mode < 32 {
        table[mode as usize] = mode_map(mode);
        mode += 1;
    }
    table
}

/// All 32 modes, computed at compile time.
pub static TABLE: [Map; 32] = build();

#[cfg(test)]
mod tests {
    use super::*;
    use Bank::*;

    /// Banks for $8000, $A000, $D000, $E000 per mode, transcribed from the
    /// C64-Wiki bank-switching table (modes 31 down to 0).
    #[rustfmt::skip]
    const DOC: [(u8, [Bank; 4]); 32] = [
        (31, [Ram,  Basic, Io,   Kernal]),
        (30, [Ram,  Ram,   Io,   Kernal]),
        (29, [Ram,  Ram,   Io,   Ram]),
        (28, [Ram,  Ram,   Ram,  Ram]),
        (27, [Ram,  Basic, Char, Kernal]),
        (26, [Ram,  Ram,   Char, Kernal]),
        (25, [Ram,  Ram,   Char, Ram]),
        (24, [Ram,  Ram,   Ram,  Ram]),
        (23, [RomL, Open,  Io,   RomH]),
        (22, [RomL, Open,  Io,   RomH]),
        (21, [RomL, Open,  Io,   RomH]),
        (20, [RomL, Open,  Io,   RomH]),
        (19, [RomL, Open,  Io,   RomH]),
        (18, [RomL, Open,  Io,   RomH]),
        (17, [RomL, Open,  Io,   RomH]),
        (16, [RomL, Open,  Io,   RomH]),
        (15, [RomL, Basic, Io,   Kernal]),
        (14, [Ram,  Ram,   Io,   Kernal]),
        (13, [Ram,  Ram,   Io,   Ram]),
        (12, [Ram,  Ram,   Ram,  Ram]),
        (11, [RomL, Basic, Char, Kernal]),
        (10, [Ram,  Ram,   Char, Kernal]),
        (9,  [Ram,  Ram,   Char, Ram]),
        (8,  [Ram,  Ram,   Ram,  Ram]),
        (7,  [RomL, RomH,  Io,   Kernal]),
        (6,  [Ram,  RomH,  Io,   Kernal]),
        (5,  [Ram,  Ram,   Io,   Ram]),
        (4,  [Ram,  Ram,   Ram,  Ram]),
        (3,  [RomL, RomH,  Char, Kernal]),
        (2,  [Ram,  RomH,  Char, Kernal]),
        (1,  [Ram,  Ram,   Ram,  Ram]),
        (0,  [Ram,  Ram,   Ram,  Ram]),
    ];

    #[test]
    fn matches_documented_table() {
        for (mode, [b8, ba, bd, be]) in DOC {
            let m = &TABLE[mode as usize];
            assert_eq!(
                [m[0x8], m[0xa], m[0xd], m[0xe]],
                [b8, ba, bd, be],
                "mode {mode}"
            );
            assert_eq!(m[0x9], m[0x8], "mode {mode}: $9000 follows $8000");
            assert_eq!(m[0xb], m[0xa], "mode {mode}: $B000 follows $A000");
            assert_eq!(m[0xf], m[0xe], "mode {mode}: $F000 follows $E000");
            assert_eq!(m[0x0], Ram, "mode {mode}: zero page is always RAM");
            let ultimax = (16..24).contains(&mode);
            for page in [1, 2, 3, 4, 5, 6, 7, 0xc] {
                let expected = if ultimax { Open } else { Ram };
                assert_eq!(m[page], expected, "mode {mode} page {page:X}");
            }
        }
    }

    #[test]
    fn mode_numbering() {
        assert_eq!(Lines::new(0x37, true, true).mode(), 31);
        assert_eq!(Lines::new(0x30, true, true).mode(), 24);
        assert_eq!(Lines::new(0x07, false, true).mode(), 23);
        assert_eq!(Lines::new(0x07, false, false).mode(), 7);
    }
}
