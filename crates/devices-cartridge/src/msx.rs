//! MSX cartridges: plain ROMs up to 64 KB and the common "MegaROM" bank
//! switching schemes (Konami, Konami SCC with its sound chip, ASCII 8 KB,
//! ASCII 16 KB), three of them also with battery RAM.
//!
//! A cartridge sees the whole 64 KB of its slot. A ROM starts with the
//! header "AB", then the pointers INIT (+2), STATEMENT (+4), DEVICE (+6)
//! and TEXT (+8) (MSX Technical Data Book, "Cartridge header").
//!
//! **Plain.** Where the image lies follows from its size and its header:
//!
//! | Image       | Placed at                                                   |
//! | ----------- | ----------------------------------------------------------- |
//! | up to 16 KB | $8000 if its first non-zero pointer lies in $8000–$BFFF, else $4000 (8 KB repeats within the 16 KB) |
//! | up to 32 KB | $4000–$BFFF; $0000–$7FFF if "AB" is only at offset $4000    |
//! | up to 48 KB | $0000–$BFFF if "AB" is at offset $4000, $4000–$FFFF if only at 0 |
//! | up to 64 KB | $0000–$FFFF                                                 |
//!
//! Everything else in the slot reads $FF.
//!
//! **MegaROMs** (images above 64 KB, and small ones as described under the
//! guess below; 8 KB banks in $4000–$BFFF; a write to
//! the listed addresses selects the bank of a window):
//!
//! | Scheme        | $4000        | $6000        | $8000        | $A000        | At power-on |
//! | ------------- | ------------ | ------------ | ------------ | ------------ | ----------- |
//! | Konami        | fixed bank 0 | $6000–$7FFF  | $8000–$9FFF  | $A000–$BFFF  | 0 1 2 3     |
//! | Konami SCC    | $5000–$57FF  | $7000–$77FF  | $9000–$97FF  | $B000–$B7FF  | 0 1 2 3     |
//! | ASCII 8       | $6000–$67FF  | $6800–$6FFF  | $7000–$77FF  | $7800–$7FFF  | 0 0 0 0     |
//! | ASCII 16      | $6000–$67FF (16 KB at $4000) | | $7000–$77FF (16 KB at $8000) | | 0 0 |
//! | Game Master 2 | fixed bank 0 | $6000–$6FFF  | $8000–$8FFF  | $A000–$AFFF  | 0 1 2 3     |
//!
//! (The wiki gives the Konami scheme's last two windows a random bank at
//! power-on; here they start as 2 and 3.)
//!
//! The windows repeat in the other half of the slot for reading:
//! $8000–$BFFF also shows at $0000–$3FFF and $4000–$7FFF at $C000–$FFFF
//! (the cartridge does not decode A15). Not every real cartridge does
//! this, but programs that put the cartridge into page 0 rely on it
//! (Championship Lode Runner runs its code there). The bank registers,
//! the SCC and RAM writes answer only in $4000–$BFFF.
//!
//! **Konami SCC.** The bank numbers have six bits. While the window at
//! $8000 holds bank $3F, $9800–$9FFF is the sound chip ([`Scc`]) instead of
//! ROM.
//!
//! **Battery RAM** ([`MsxCart::sram`]; the machine keeps it in a file):
//!
//! | Scheme         | RAM                   | Selected by                         | Writable at |
//! | -------------- | --------------------- | ----------------------------------- | ----------- |
//! | ASCII 8 SRAM   | 8 KB                  | a bank number with bit 7 set, or with the bit of the image's bank count (bit 5 for Xanadu's 256 KB) | $8000–$BFFF |
//! | ASCII 16 SRAM  | 2 KB, repeating       | bit 4 of the bank number (Hydlide 2) | $8000–$BFFF |
//! | Game Master 2  | 8 KB as two 4 KB halves, each repeating in its 8 KB window | bit 4 of the bank number, bit 5 the half | $B000–$BFFF |
//!
//! A window that shows RAM reads it wherever it lies; writes outside the
//! writable range are ignored.
//!
//! An image does not say which scheme it uses. [`guess`] counts the
//! `LD (nn),A` instructions (opcode $32) in the image whose address is one
//! of the schemes' typical register addresses and takes the scheme with
//! the most (on a tie ASCII 16, then ASCII 8, Konami SCC, Konami: a
//! program that writes only $6000 and $7000 has two registers; with no
//! such write at all ASCII 8). An image of more than 32 KB and at most
//! 64 KB is plain, unless its only "AB" is at offset 0 — which would put
//! it at $4000–$FFFF, over the RAM in page 3 — and it has such writes:
//! then it is a small MegaROM (Championship Lode Runner, 48 KB, ASCII
//! 16). The machine's `mapper1`/`mapper2` options override the guess, and the
//! schemes with battery RAM are only ever chosen that way. The counting is
//! this project's own.
//!
//! Sources: MSX Resource Center wiki, "MegaROM Mappers" (register
//! addresses, power-on banks, the SCC's window, the battery RAM schemes),
//! read 2026-11-12; see `docs/sources.md`.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};

use crate::scc::Scc;
use crate::slot::CartSlot;

const PAGE: usize = 0x4000;
const BANK: usize = 0x2000;
/// The largest image: 256 banks of 8 KB.
const MAX: usize = 0x20_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapper {
    Plain,
    Konami,
    KonamiScc,
    Ascii8,
    Ascii16,
    Ascii8Sram,
    Ascii16Sram,
    GameMaster2,
}

impl Mapper {
    /// The names [`Mapper::parse`] knows, for error messages.
    pub const NAMES: &'static str =
        "plain, konami, konamiscc, ascii8, ascii16, ascii8sram, ascii16sram or gamemaster2";

    /// The scheme an option value names.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "plain" => Mapper::Plain,
            "konami" => Mapper::Konami,
            "konamiscc" | "scc" => Mapper::KonamiScc,
            "ascii8" => Mapper::Ascii8,
            "ascii16" => Mapper::Ascii16,
            "ascii8sram" => Mapper::Ascii8Sram,
            "ascii16sram" => Mapper::Ascii16Sram,
            "gamemaster2" => Mapper::GameMaster2,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Mapper::Plain => "plain",
            Mapper::Konami => "konami",
            Mapper::KonamiScc => "konamiscc",
            Mapper::Ascii8 => "ascii8",
            Mapper::Ascii16 => "ascii16",
            Mapper::Ascii8Sram => "ascii8sram",
            Mapper::Ascii16Sram => "ascii16sram",
            Mapper::GameMaster2 => "gamemaster2",
        }
    }

    fn code(self) -> u8 {
        match self {
            Mapper::Plain => 0,
            Mapper::Konami => 1,
            Mapper::KonamiScc => 2,
            Mapper::Ascii8 => 3,
            Mapper::Ascii16 => 4,
            Mapper::Ascii8Sram => 5,
            Mapper::Ascii16Sram => 6,
            Mapper::GameMaster2 => 7,
        }
    }

    fn power_on(self) -> [u8; 4] {
        match self {
            Mapper::Plain | Mapper::Konami | Mapper::KonamiScc | Mapper::GameMaster2 => {
                [0, 1, 2, 3]
            }
            Mapper::Ascii8 | Mapper::Ascii8Sram => [0; 4],
            Mapper::Ascii16 | Mapper::Ascii16Sram => [0, 1, 0, 1],
        }
    }

    /// Bytes of battery RAM.
    pub fn sram_len(self) -> usize {
        match self {
            Mapper::Ascii8Sram | Mapper::GameMaster2 => 0x2000,
            Mapper::Ascii16Sram => 0x800,
            _ => 0,
        }
    }
}

/// The scheme of a MegaROM image, by its bank register writes (see the
/// module doc); `None` for an image without any such write.
pub fn guess(rom: &[u8]) -> Option<Mapper> {
    // Konami, Konami SCC, ASCII 8, ASCII 16.
    let mut votes = [0u32; 4];
    for w in rom.windows(3) {
        if w[0] != 0x32 {
            continue;
        }
        match u16::from_le_bytes([w[1], w[2]]) {
            0x4000 | 0x8000 | 0xa000 => votes[0] += 1,
            0x5000 | 0x9000 | 0xb000 => votes[1] += 1,
            0x6800 | 0x7800 => votes[2] += 1,
            0x77ff => votes[3] += 1,
            0x6000 => {
                votes[0] += 1;
                votes[2] += 1;
                votes[3] += 1;
            }
            0x7000 => {
                votes[1] += 1;
                votes[2] += 1;
                votes[3] += 1;
            }
            _ => {}
        }
    }
    // On a tie the earlier of ASCII 16, ASCII 8, Konami SCC, Konami wins.
    let order = [
        (Mapper::Ascii16, votes[3]),
        (Mapper::Ascii8, votes[2]),
        (Mapper::KonamiScc, votes[1]),
        (Mapper::Konami, votes[0]),
    ];
    let mut best = order[0];
    for o in order {
        if o.1 > best.1 {
            best = o;
        }
    }
    (best.1 > 0).then_some(best.0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MsxCart {
    /// The image, filled up to a power of two (at least 16 KB).
    rom: Vec<u8>,
    /// The image's own length.
    len: usize,
    mapper: Mapper,
    /// Plain: the 16 KB piece of the image in each page of the slot.
    pages: [Option<u8>; 4],
    /// MegaROM: the 8 KB bank in each window of $4000–$BFFF.
    pub banks: [u8; 4],
    /// The battery RAM (empty without one).
    sram: Vec<u8>,
    /// Windows that show the battery RAM: 0 ROM, else 1 + the RAM's part
    /// (Game Master 2's halves; 1 elsewhere).
    sram_win: [u8; 4],
    /// The Konami SCC's sound chip.
    pub scc: Option<Scc>,
}

impl MsxCart {
    /// A cartridge from a raw ROM image, the scheme guessed.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.is_empty() {
            return Err(MediaError::Format("empty cartridge image".into()));
        }
        if bytes.len() > MAX {
            return Err(MediaError::Format(format!(
                "cartridge image of {} bytes (up to 2 MB)",
                bytes.len()
            )));
        }
        let mut rom = bytes.to_vec();
        if rom.len() <= BANK {
            // An 8 KB chip answers in both halves of its page.
            rom.resize(BANK, 0xff);
            rom.extend_from_within(..);
        }
        let size = rom.len().next_power_of_two().max(PAGE);
        rom.resize(size, 0xff);
        let small_mega =
            bytes.len() > 2 * PAGE && bytes.starts_with(b"AB") && bytes[PAGE..PAGE + 2] != *b"AB";
        let mapper = if bytes.len() > 4 * PAGE {
            guess(bytes).unwrap_or(Mapper::Ascii8)
        } else if small_mega {
            guess(bytes).unwrap_or(Mapper::Plain)
        } else {
            Mapper::Plain
        };
        let mut c = Self {
            rom,
            len: bytes.len(),
            mapper,
            pages: [None; 4],
            banks: [0; 4],
            sram: Vec::new(),
            sram_win: [0; 4],
            scc: None,
        };
        c.set_mapper(mapper);
        Ok(c)
    }

    pub fn mapper(&self) -> Mapper {
        self.mapper
    }

    /// Use `mapper` instead of the guessed scheme (power-on state, the
    /// battery RAM empty).
    pub fn set_mapper(&mut self, mapper: Mapper) {
        self.mapper = mapper;
        self.pages = if mapper == Mapper::Plain {
            self.plain_pages()
        } else {
            [None; 4]
        };
        self.sram = vec![0; mapper.sram_len()];
        self.reset();
    }

    fn has_header(&self, at: usize) -> bool {
        self.len >= at + 2 && self.rom[at..at + 2] == *b"AB"
    }

    /// Where a plain image lies in the slot (see the module doc).
    fn plain_pages(&self) -> [Option<u8>; 4] {
        if self.len <= PAGE {
            // INIT, STATEMENT, DEVICE, TEXT: the first one set.
            let pointer = (1..5)
                .map(|i| u16::from_le_bytes([self.rom[2 * i], self.rom[2 * i + 1]]))
                .find(|&p| p != 0)
                .unwrap_or(0);
            let high = self.has_header(0) && (0x8000..0xc000).contains(&pointer);
            if high {
                [None, None, Some(0), None]
            } else {
                [None, Some(0), None, None]
            }
        } else if self.len <= 2 * PAGE {
            if !self.has_header(0) && self.has_header(PAGE) {
                [Some(0), Some(1), None, None]
            } else {
                [None, Some(0), Some(1), None]
            }
        } else if self.len <= 3 * PAGE {
            if self.has_header(0) && !self.has_header(PAGE) {
                [None, Some(0), Some(1), Some(2)]
            } else {
                [Some(0), Some(1), Some(2), None]
            }
        } else {
            [Some(0), Some(1), Some(2), Some(3)]
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The battery RAM (empty without one).
    pub fn sram(&self) -> &[u8] {
        &self.sram
    }

    pub fn sram_mut(&mut self) -> &mut [u8] {
        &mut self.sram
    }

    /// Power-on and RESET state; the battery RAM keeps its contents.
    pub fn reset(&mut self) {
        self.banks = self.mapper.power_on();
        self.sram_win = [0; 4];
        self.scc = (self.mapper == Mapper::KonamiScc).then(Scc::new);
    }

    /// The Konami SCC's registers show at $9800–$9FFF.
    fn scc_selected(&self) -> bool {
        self.scc.is_some() && self.banks[2] & 0x3f == 0x3f
    }

    /// The battery RAM cell at `addr` in a window that shows the RAM's
    /// part `part` (see `sram_win`).
    fn sram_index(&self, addr: u16, part: u8) -> usize {
        let a = usize::from(addr);
        match self.mapper {
            Mapper::GameMaster2 => usize::from(part - 1) << 12 | a & 0xfff,
            _ => a & (self.sram.len() - 1),
        }
    }

    /// Side-effect-free read of the slot's address `addr`.
    pub fn peek(&self, addr: u16) -> u8 {
        let a = usize::from(addr);
        if self.mapper == Mapper::Plain {
            return match self.pages[a >> 14] {
                Some(p) => {
                    self.rom[(usize::from(p) * PAGE + (a & (PAGE - 1))) & (self.rom.len() - 1)]
                }
                None => 0xff,
            };
        }
        // The other half of the slot repeats the windows.
        let mirrored = !(0x4000..0xc000).contains(&addr);
        let a = if mirrored { a ^ 0x8000 } else { a };
        let window = (a - 0x4000) >> 13;
        if self.sram_win[window] != 0 {
            return self.sram[self.sram_index(addr, self.sram_win[window])];
        }
        if !mirrored && addr & 0xf800 == 0x9800 && self.scc_selected() {
            if let Some(scc) = &self.scc {
                return scc.read(addr as u8);
            }
        }
        let bank = usize::from(self.banks[window]);
        self.rom[(bank * BANK + (a & (BANK - 1))) & (self.rom.len() - 1)]
    }

    /// Put bank number `v` into `window`, or the battery RAM if the number
    /// selects it (ASCII 8 SRAM).
    fn select_ascii8(&mut self, window: usize, v: u8) {
        let banks = self.rom.len() / BANK;
        let select = 0x80 | if banks < 0x80 { banks as u8 } else { 0 };
        if self.mapper == Mapper::Ascii8Sram && v & select != 0 {
            self.sram_win[window] = 1;
        } else {
            self.sram_win[window] = 0;
            self.banks[window] = v;
        }
    }

    /// A write to the slot's address `addr`: the bank registers, the SCC,
    /// the battery RAM.
    pub fn write(&mut self, addr: u16, v: u8) {
        if !(0x4000..0xc000).contains(&addr) {
            return;
        }
        let window = usize::from(addr - 0x4000) >> 13;
        match self.mapper {
            Mapper::Plain => {}
            Mapper::Konami => {
                if addr >= 0x6000 {
                    self.banks[window] = v;
                }
            }
            Mapper::KonamiScc => {
                if addr & 0x1800 == 0x1000 {
                    self.banks[window] = v & 0x3f;
                } else if addr & 0xf800 == 0x9800 && self.scc_selected() {
                    if let Some(scc) = &mut self.scc {
                        scc.write(addr as u8, v);
                    }
                }
            }
            Mapper::Ascii8 | Mapper::Ascii8Sram => {
                if addr < 0x8000 {
                    if addr >= 0x6000 {
                        self.select_ascii8(usize::from(addr >> 11 & 3), v);
                    }
                } else if self.sram_win[window] != 0 {
                    let i = self.sram_index(addr, 1);
                    self.sram[i] = v;
                }
            }
            Mapper::Ascii16 | Mapper::Ascii16Sram => {
                let first = match addr & 0xf800 {
                    0x6000 => 0,
                    0x7000 => 2,
                    _ => {
                        if addr >= 0x8000 && self.sram_win[window] != 0 {
                            let i = self.sram_index(addr, 1);
                            self.sram[i] = v;
                        }
                        return;
                    }
                };
                let ram = self.mapper == Mapper::Ascii16Sram && v & 0x10 != 0;
                for half in 0..2 {
                    self.sram_win[first + half] = u8::from(ram);
                    if !ram {
                        self.banks[first + half] = v.wrapping_mul(2) | half as u8;
                    }
                }
            }
            Mapper::GameMaster2 => {
                if addr >= 0x6000 && addr & 0x1000 == 0 {
                    if v & 0x10 != 0 {
                        self.sram_win[window] = 1 + (v >> 5 & 1);
                    } else {
                        self.sram_win[window] = 0;
                        self.banks[window] = v & 0x0f;
                    }
                } else if addr >= 0xb000 && self.sram_win[3] != 0 {
                    let i = self.sram_index(addr, self.sram_win[3]);
                    self.sram[i] = v;
                }
            }
        }
    }

    /// `n` clocks of the bus clock for the cartridge's sound chip.
    pub fn clock(&mut self, n: u16) {
        if let Some(scc) = &mut self.scc {
            scc.clock(n);
        }
    }

    /// The sound chip's output (0 without one; the SCC's 11 bits).
    pub fn audio(&self) -> i16 {
        self.scc.as_ref().map_or(0, Scc::output)
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"MSXC", 1, |w| {
            w.u8(self.mapper.code());
            w.raw(&self.banks);
            w.raw(&self.sram_win);
            w.bytes(&self.sram);
        });
        if let Some(scc) = &self.scc {
            scc.save(w);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"MSXC", 1)?;
        if c.u8()? != self.mapper.code() {
            return Err(StateError::Invalid("cartridge mapper differs"));
        }
        let mut banks = [0; 4];
        c.raw_into(&mut banks)?;
        let mut sram_win = [0; 4];
        c.raw_into(&mut sram_win)?;
        let sram = c.bytes()?;
        let parts = if self.mapper == Mapper::GameMaster2 {
            2
        } else {
            1
        };
        if sram.len() != self.sram.len()
            || sram_win.iter().any(|&p| p > parts)
            || (self.sram.is_empty() && sram_win != [0; 4])
        {
            return Err(StateError::Invalid("cartridge battery RAM"));
        }
        let mut scc = self.scc.clone();
        if let Some(scc) = &mut scc {
            scc.load(r)?;
        }
        self.banks = banks;
        self.sram_win = sram_win;
        self.sram = sram.to_vec();
        self.scc = scc;
        Ok(())
    }
}

/// One of the MSX's cartridge slots.
pub fn slot() -> CartSlot<MsxCart> {
    CartSlot::new(MsxCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image of `len` bytes, each 8 KB bank starting with its number,
    /// with a header at `header` pointing INIT to `init`.
    fn image(len: usize, header: Option<usize>, init: u16) -> Vec<u8> {
        let mut rom = vec![0u8; len];
        for (b, chunk) in rom.chunks_mut(BANK).enumerate() {
            chunk[0x10] = b as u8;
        }
        if let Some(h) = header {
            rom[h..h + 2].copy_from_slice(b"AB");
            rom[h + 2..h + 4].copy_from_slice(&init.to_le_bytes());
        }
        rom
    }

    #[test]
    fn plain_images_lie_where_their_header_says() {
        let c = MsxCart::parse(&image(0x2000, Some(0), 0x4010)).unwrap();
        assert_eq!(c.mapper(), Mapper::Plain);
        assert_eq!(c.peek(0x4000), b'A');
        assert_eq!(c.peek(0x6001), b'B', "8 KB repeats in its page");
        assert_eq!(c.peek(0x8000), 0xff);
        assert_eq!(c.peek(0x0000), 0xff);

        let c = MsxCart::parse(&image(0x4000, Some(0), 0x8010)).unwrap();
        assert_eq!(c.peek(0x8000), b'A', "INIT in page 2");
        assert_eq!(c.peek(0x4000), 0xff);
        // A BASIC program: no INIT, TEXT at $8010.
        let mut basic = image(0x4000, Some(0), 0);
        basic[8..10].copy_from_slice(&0x8010u16.to_le_bytes());
        assert_eq!(MsxCart::parse(&basic).unwrap().peek(0x8001), b'B');

        let c = MsxCart::parse(&image(0x8000, Some(0), 0x4010)).unwrap();
        assert_eq!((c.peek(0x4000), c.peek(0xa010)), (b'A', 3));
        assert_eq!((c.peek(0x0000), c.peek(0xc000)), (0xff, 0xff));
        let c = MsxCart::parse(&image(0x8000, Some(0x4000), 0x4010)).unwrap();
        assert_eq!((c.peek(0x4000), c.peek(0x0010)), (b'A', 0));
        assert_eq!(c.peek(0x8000), 0xff);

        let c = MsxCart::parse(&image(0xc000, Some(0x4000), 0x4010)).unwrap();
        assert_eq!((c.peek(0x4000), c.peek(0xa010)), (b'A', 5));
        assert_eq!(c.peek(0xc000), 0xff);
        let c = MsxCart::parse(&image(0x1_0000, Some(0x4000), 0x4010)).unwrap();
        assert_eq!((c.peek(0x4000), c.peek(0xe010)), (b'A', 7));

        assert!(MsxCart::parse(&[]).is_err());
        assert!(MsxCart::parse(&vec![0; MAX + 1]).is_err());
    }

    fn mega(mapper: Mapper) -> MsxCart {
        let mut c = MsxCart::parse(&image(0x2_0000, Some(0), 0x4010)).unwrap();
        c.set_mapper(mapper);
        c
    }

    fn windows(c: &MsxCart) -> [u8; 4] {
        [0x4010, 0x6010, 0x8010, 0xa010].map(|a| c.peek(a))
    }

    #[test]
    fn konami_schemes_switch_8_kb_banks() {
        let mut c = mega(Mapper::Konami);
        assert_eq!(windows(&c), [0, 1, 2, 3]);
        c.write(0x4000, 9);
        c.write(0x6000, 5);
        c.write(0x9fff, 6);
        c.write(0xa000, 15);
        assert_eq!(windows(&c), [0, 5, 6, 15], "the first window is fixed");
        c.write(0xa000, 16 + 4);
        assert_eq!(windows(&c)[3], 4, "16 banks: the number wraps");
        assert_eq!(
            [0x0010, 0x2010, 0xc010, 0xe010].map(|a| c.peek(a)),
            [6, 4, 0, 5],
            "the windows repeat in the other half"
        );
        c.write(0x2000, 9);
        c.write(0xe000, 9);
        assert_eq!(windows(&c), [0, 5, 6, 4], "no registers there");

        let mut c = mega(Mapper::KonamiScc);
        c.write(0x5000, 7);
        c.write(0x77ff, 8);
        c.write(0x9000, 9);
        c.write(0xb000, 10);
        assert_eq!(windows(&c), [7, 8, 9, 10]);
        c.write(0x5800, 1);
        c.write(0x6000, 1);
        assert_eq!(
            windows(&c),
            [7, 8, 9, 10],
            "only $x000–$x7FF of each odd 4 KB"
        );
    }

    #[test]
    fn ascii_schemes_switch_through_6000_to_7fff() {
        let mut c = mega(Mapper::Ascii8);
        assert_eq!(windows(&c), [0, 0, 0, 0]);
        c.write(0x6000, 3);
        c.write(0x6800, 4);
        c.write(0x7000, 5);
        c.write(0x7800, 6);
        assert_eq!(windows(&c), [3, 4, 5, 6]);
        c.write(0x8000, 1);
        assert_eq!(windows(&c), [3, 4, 5, 6]);

        let mut c = mega(Mapper::Ascii16);
        assert_eq!(windows(&c), [0, 1, 0, 1]);
        c.write(0x6000, 2);
        c.write(0x7000, 7);
        assert_eq!(windows(&c), [4, 5, 14, 15]);
        c.write(0x6800, 1);
        assert_eq!(windows(&c)[0], 4);
        c.reset();
        assert_eq!(windows(&c), [0, 1, 0, 1]);
    }

    #[test]
    fn the_scheme_is_guessed_from_the_register_writes() {
        let with = |addrs: &[u16]| {
            let mut rom = image(0x2_0000, Some(0), 0x4010);
            for (i, a) in addrs.iter().enumerate() {
                let at = 0x100 + 4 * i;
                rom[at] = 0x32;
                rom[at + 1..at + 3].copy_from_slice(&a.to_le_bytes());
            }
            MsxCart::parse(&rom).unwrap().mapper()
        };
        assert_eq!(with(&[0x6000, 0x8000, 0xa000]), Mapper::Konami);
        assert_eq!(with(&[0x5000, 0x7000, 0x9000, 0xb000]), Mapper::KonamiScc);
        assert_eq!(with(&[0x6000, 0x6800, 0x7000, 0x7800]), Mapper::Ascii8);
        assert_eq!(with(&[0x6000, 0x7000, 0x77ff]), Mapper::Ascii16);
        assert_eq!(with(&[]), Mapper::Ascii8);
        assert_eq!(with(&[0x6000, 0x7000]), Mapper::Ascii16, "two registers");
        assert_eq!(with(&[0x7000]), Mapper::Ascii16);
        // A 48 KB image with its header at 0 and a register write is a
        // small MegaROM; without the write, or with the header at $4000,
        // it is plain.
        let mut rom = image(0xc000, Some(0), 0x4010);
        assert_eq!(MsxCart::parse(&rom).unwrap().mapper(), Mapper::Plain);
        rom[0x100..0x103].copy_from_slice(&[0x32, 0x00, 0x70]);
        let mut c = MsxCart::parse(&rom).unwrap();
        assert_eq!(c.mapper(), Mapper::Ascii16);
        c.write(0x7000, 2);
        assert_eq!(
            (c.peek(0x4010), c.peek(0x8010), c.peek(0xc000)),
            (0, 4, b'A'),
            "$C000 repeats $4000"
        );
        let mut rom = image(0xc000, Some(0x4000), 0x4010);
        rom[0x100..0x103].copy_from_slice(&[0x32, 0x00, 0x70]);
        assert_eq!(MsxCart::parse(&rom).unwrap().mapper(), Mapper::Plain);
        assert_eq!(Mapper::parse("scc"), Some(Mapper::KonamiScc));
        assert_eq!(Mapper::parse(Mapper::Ascii16.name()), Some(Mapper::Ascii16));
        assert_eq!(Mapper::parse("mbc5"), None);
        assert_eq!(Mapper::parse("gamemaster2"), Some(Mapper::GameMaster2));
    }

    #[test]
    fn savestate_keeps_the_banks_and_checks_the_scheme() {
        let mut c = mega(Mapper::Ascii8);
        c.write(0x7000, 11);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = mega(Mapper::Ascii8);
        d.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(d, c);
        let mut other = mega(Mapper::Konami);
        assert!(other.load(&mut StateReader::new(&data)).is_err());
        assert_eq!(other.banks, [0, 1, 2, 3]);
    }

    #[test]
    fn the_scc_shows_at_9800_while_bank_3f_is_at_8000() {
        let mut c = mega(Mapper::KonamiScc);
        assert!(c.scc.is_some());
        let rom_byte = c.peek(0x9800);
        c.write(0x9800, 0x12);
        assert_eq!(c.peek(0x9800), rom_byte, "not selected: ROM");
        c.write(0x9000, 0xff);
        assert_eq!(c.banks[2], 0x3f, "six bits");
        c.write(0x9800, 0x12);
        c.write(0x9f01, 0x34);
        assert_eq!((c.peek(0x9800), c.peek(0x9901)), (0x12, 0x34));
        assert_eq!(c.peek(0x9880), 0xff, "write-only registers");
        assert_eq!(c.peek(0x8010), 15, "below $9800 the ROM bank shows");
        // A tone: channel 1 at full volume steps through its waveform.
        c.write(0x9880, 20);
        c.write(0x988a, 15);
        c.write(0x988f, 1);
        assert_eq!(c.audio(), (0x12 * 15) >> 4);
        c.clock(21);
        assert_eq!(c.audio(), (0x34 * 15) >> 4);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = mega(Mapper::KonamiScc);
        d.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(d, c);
        c.write(0x9000, 2);
        assert_eq!(c.peek(0x9800), rom_byte);
        c.reset();
        assert_eq!(c.audio(), 0);
        assert!(mega(Mapper::Konami).scc.is_none());
    }

    #[test]
    fn ascii_8_battery_ram_is_selected_by_the_bank_number() {
        // 128 KB: 16 banks, so bit 4 or bit 7 selects the RAM.
        let mut c = mega(Mapper::Ascii8Sram);
        assert_eq!(c.sram().len(), 0x2000);
        c.write(0x7000, 0x80);
        c.write(0x8123, 0x5a);
        assert_eq!(c.peek(0x8123), 0x5a);
        c.write(0x7800, 0x10);
        assert_eq!(c.peek(0xa123), 0x5a, "the same 8 KB in another window");
        // In $4000–$7FFF the RAM reads but is not written.
        c.write(0x6000, 0x80);
        assert_eq!(c.peek(0x4123), 0x5a);
        c.write(0x4123, 0x11);
        assert_eq!(c.sram()[0x123], 0x5a);
        c.write(0x7000, 3);
        assert_eq!(c.peek(0x8010), 3, "ROM again");
        c.write(0x8123, 0x77);
        assert_eq!(c.sram()[0x123], 0x5a);
        // The plain ASCII 8 scheme has no RAM: the number is a bank.
        let mut c = mega(Mapper::Ascii8);
        c.write(0x7000, 0x83);
        assert_eq!(c.peek(0x8010), 3);
        assert!(c.sram().is_empty());
    }

    #[test]
    fn ascii_16_battery_ram_repeats_its_2_kb() {
        let mut c = mega(Mapper::Ascii16Sram);
        assert_eq!(c.sram().len(), 0x800);
        c.write(0x7000, 0x10);
        c.write(0x8001, 0xaa);
        assert_eq!((c.peek(0x8801), c.peek(0xb801)), (0xaa, 0xaa));
        c.write(0x7000, 1);
        assert_eq!((c.peek(0x8010), c.peek(0xa010)), (2, 3));
        c.reset();
        assert_eq!(c.sram()[1], 0xaa, "RESET keeps the RAM");
        c.set_mapper(Mapper::Ascii16Sram);
        assert_eq!(c.sram()[1], 0, "a new cartridge is empty");
    }

    #[test]
    fn game_master_2_has_two_ram_halves_writable_at_b000() {
        let mut c = mega(Mapper::GameMaster2);
        assert_eq!(windows(&c), [0, 1, 2, 3]);
        c.write(0x6000, 0x25);
        assert_eq!(windows(&c)[1], 5, "bits 3–0 are the bank");
        c.write(0x7000, 9);
        assert_eq!(windows(&c)[1], 5, "$7000 is no register");
        c.write(0xa000, 0x10);
        c.write(0xb005, 0x42);
        c.write(0xa005, 0x10);
        assert_eq!((c.peek(0xa005), c.peek(0xb005)), (0x42, 0x42));
        c.write(0xa000, 0x30);
        assert_eq!(c.peek(0xb005), 0, "the other half");
        c.write(0xb005, 0x43);
        assert_eq!((c.sram()[5], c.sram()[0x1005]), (0x42, 0x43));
        // The RAM in the $8000 window reads but is not written.
        c.write(0x8000, 0x10);
        c.write(0x9005, 0x99);
        assert_eq!(c.peek(0x9005), 0x42);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = mega(Mapper::GameMaster2);
        d.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(d, c);
        let mut other = mega(Mapper::Ascii8Sram);
        assert!(other.load(&mut StateReader::new(&data)).is_err());
    }
}
