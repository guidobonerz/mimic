//! Memory: RAM, ROM, the language card and the IIe's memory switches.
//!
//! **II/II+.** RAM from $0000 up (4–48 KB), the 12 KB ROM at $D000–$FFFF
//! (the II's Integer BASIC set fills $E000–$FFFF only). A 16 KB language
//! card (slot 0, standard on the IIe) puts RAM over the ROM: 8 KB at
//! $E000–$FFFF and two 4 KB banks at $D000–$DFFF. Its switches at
//! $C080–$C08F (Apple, "Language Card Installation and Operation
//! Manual"; Sather, "Understanding the Apple IIe", chapter 5): bit 3 picks
//! bank 1 (set) or bank 2; bits 1–0 = 00 or 11 read RAM, 01 or 10 read
//! ROM; writing to RAM is enabled by two successive reads of odd
//! addresses (bit 0 set). Any even address, or a write, resets the
//! pre-write flip-flop; an even address also write-protects the RAM.
//!
//! The banks live in the 64 KB arrays where the addresses would be: bank 2
//! at $D000–$DFFF, bank 1 at $C000–$CFFF (under the I/O space), the rest
//! at $E000–$FFFF.
//!
//! **IIe** (Apple IIe Technical Reference Manual, chapter 4, "Memory
//! Organization"): a second 64 KB bank (the Extended 80-Column Text Card;
//! the 80-Column Text Card has only 1 KB, the text page) switched by
//! RAMRD/RAMWRT ($0200–$BFFF), ALTZP ($0000–$01FF and the language card
//! area) and 80STORE (with it PAGE2 selects the bank for the text page
//! $0400–$07FF and, with HIRES, for $2000–$3FFF). The 16 KB ROM covers
//! $C000–$FFFF; its $C100–$CFFF part replaces the slot ROMs with
//! INTCXROM, its $C300 page (the 80-column firmware) with SLOTC3ROM off,
//! which also switches the internal ROM into $C800–$CFFF until an access
//! to $CFFF.

use crate::video::Modes;
use emu_core::{StateError, StateReader, StateWriter};

/// Auxiliary memory of a IIe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aux {
    None,
    /// 80-Column Text Card: 1 KB, the text page only.
    Text1k,
    /// Extended 80-Column Text Card: 64 KB.
    Ext64k,
}

/// Language card state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LangCard {
    pub bank1: bool,
    pub read_ram: bool,
    pub write_ram: bool,
    prewrite: bool,
}

impl LangCard {
    /// Power-on and RESET: read ROM, write RAM, bank 2.
    pub fn reset() -> Self {
        Self {
            write_ram: true,
            ..Self::default()
        }
    }

    /// An access to $C080–$C08F (`n` = address bits 3–0).
    pub fn access(&mut self, n: u8, write: bool) {
        self.bank1 = n & 8 != 0;
        self.read_ram = matches!(n & 3, 0 | 3);
        if n & 1 == 0 {
            self.prewrite = false;
            self.write_ram = false;
        } else if write {
            self.prewrite = false;
        } else {
            if self.prewrite {
                self.write_ram = true;
            }
            self.prewrite = true;
        }
    }

    fn bits(&self) -> u8 {
        u8::from(self.bank1)
            | u8::from(self.read_ram) << 1
            | u8::from(self.write_ram) << 2
            | u8::from(self.prewrite) << 3
    }

    fn from_bits(v: u8) -> Self {
        Self {
            bank1: v & 1 != 0,
            read_ram: v & 2 != 0,
            write_ram: v & 4 != 0,
            prewrite: v & 8 != 0,
        }
    }
}

/// IIe memory switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IIeSwitches {
    pub ramrd: bool,
    pub ramwrt: bool,
    pub intcxrom: bool,
    pub altzp: bool,
    pub slotc3rom: bool,
    /// The internal ROM shows at $C800–$CFFF (set by $C3xx).
    pub intc8rom: bool,
}

impl IIeSwitches {
    fn bits(&self) -> u8 {
        [
            self.ramrd,
            self.ramwrt,
            self.intcxrom,
            self.altzp,
            self.slotc3rom,
            self.intc8rom,
        ]
        .iter()
        .enumerate()
        .fold(0, |v, (i, &b)| v | (u8::from(b) << i))
    }

    fn from_bits(v: u8) -> Self {
        let b = |i: u8| v & (1 << i) != 0;
        Self {
            ramrd: b(0),
            ramwrt: b(1),
            intcxrom: b(2),
            altzp: b(3),
            slotc3rom: b(4),
            intc8rom: b(5),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mem {
    pub iie: bool,
    /// Bytes of RAM from $0000 (II/II+; 48 KB on the IIe).
    pub ram_size: usize,
    pub lc_present: bool,
    pub aux_kind: Aux,
    pub main: Vec<u8>,
    pub aux: Vec<u8>,
    /// II/II+: 12 KB for $D000–$FFFF; IIe: 16 KB for $C000–$FFFF.
    pub rom: Vec<u8>,
    pub lc: LangCard,
    pub sw: IIeSwitches,
}

/// Where a CPU address lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Main(usize),
    Aux(usize),
    Rom(usize),
    /// Nothing answers: the floating bus.
    Open,
}

impl Mem {
    pub fn new(iie: bool, ram_size: usize, lc_present: bool, aux_kind: Aux, rom: Vec<u8>) -> Self {
        Self {
            iie,
            ram_size,
            lc_present,
            aux_kind,
            main: vec![0; 0x10000],
            aux: match aux_kind {
                Aux::None => Vec::new(),
                Aux::Text1k | Aux::Ext64k => vec![0; 0x10000],
            },
            rom,
            lc: LangCard::default(),
            sw: IIeSwitches::default(),
        }
    }

    /// Power-on: RAM keeps a pattern like real chips (alternating pages of
    /// $00 and $FF), switches off.
    pub fn power_on(&mut self) {
        for (i, b) in self.main.iter_mut().enumerate() {
            *b = if i & 0x80 == 0 { 0x00 } else { 0xff };
        }
        self.aux.fill(0);
        self.lc = LangCard::reset();
        self.sw = IIeSwitches::default();
    }

    /// The auxiliary bank exists at `addr`.
    fn aux_at(&self, addr: u16) -> bool {
        match self.aux_kind {
            Aux::None => false,
            Aux::Text1k => (0x400..0x800).contains(&addr),
            Aux::Ext64k => true,
        }
    }

    /// RAM below $C000 for a read (`write` false) or a write.
    fn low_ram(&self, addr: u16, write: bool, m: &Modes) -> Target {
        let a = usize::from(addr);
        if !self.iie {
            return if a < self.ram_size {
                Target::Main(a)
            } else {
                Target::Open
            };
        }
        let alt = if addr < 0x200 {
            self.sw.altzp
        } else if m.store80
            && ((0x400..0x800).contains(&addr) || (m.hires && (0x2000..0x4000).contains(&addr)))
        {
            m.page2
        } else if write {
            self.sw.ramwrt
        } else {
            self.sw.ramrd
        };
        if alt {
            if self.aux_at(addr) {
                Target::Aux(a)
            } else {
                Target::Open
            }
        } else {
            Target::Main(a)
        }
    }

    /// The language card RAM for $D000–$FFFF.
    fn lc_ram(&self, addr: u16) -> Target {
        let a = if addr < 0xe000 && self.lc.bank1 {
            usize::from(addr) - 0x1000
        } else {
            usize::from(addr)
        };
        if self.iie && self.sw.altzp {
            if self.aux_at(addr) {
                Target::Aux(a)
            } else {
                Target::Open
            }
        } else {
            Target::Main(a)
        }
    }

    /// Where a read of `addr` (not $C000–$CFFF) goes.
    pub fn read_target(&self, addr: u16, m: &Modes) -> Target {
        match addr {
            0x0000..=0xbfff => self.low_ram(addr, false, m),
            0xd000..=0xffff if self.lc_present && self.lc.read_ram => self.lc_ram(addr),
            0xd000..=0xffff => self.rom_target(addr),
            _ => Target::Open,
        }
    }

    /// Where a write to `addr` (not $C000–$CFFF) goes.
    pub fn write_target(&self, addr: u16, m: &Modes) -> Target {
        match addr {
            0x0000..=0xbfff => self.low_ram(addr, true, m),
            0xd000..=0xffff if self.lc_present && self.lc.write_ram => self.lc_ram(addr),
            _ => Target::Open,
        }
    }

    /// ROM at `addr` ($C000–$FFFF on the IIe, $D000–$FFFF otherwise).
    pub fn rom_target(&self, addr: u16) -> Target {
        let base = if self.iie { 0xc000 } else { 0xd000 };
        let rom_start = 0x10000 - self.rom.len();
        match usize::from(addr) {
            a if a >= rom_start.max(base) => Target::Rom(a - (0x10000 - self.rom.len())),
            _ => Target::Open,
        }
    }

    pub fn get(&self, t: Target) -> Option<u8> {
        match t {
            Target::Main(a) => self.main.get(a).copied(),
            Target::Aux(a) => self.aux.get(a).copied(),
            Target::Rom(a) => self.rom.get(a).copied(),
            Target::Open => None,
        }
    }

    pub fn set(&mut self, t: Target, v: u8) {
        match t {
            Target::Main(a) => {
                if let Some(b) = self.main.get_mut(a) {
                    *b = v;
                }
            }
            Target::Aux(a) => {
                if let Some(b) = self.aux.get_mut(a) {
                    *b = v;
                }
            }
            Target::Rom(_) | Target::Open => {}
        }
    }

    /// The bytes the 80-column video modes read from the auxiliary bank.
    pub fn video_aux(&self) -> Option<&[u8]> {
        (self.aux_kind != Aux::None).then_some(self.aux.as_slice())
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"A2MM", 1, |w| {
            w.bytes(&self.main);
            w.bytes(&self.aux);
            w.u8(self.lc.bits());
            w.u8(self.sw.bits());
        });
    }

    /// Atomic: on error the memory is unchanged.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"A2MM", 1)?;
        let main = c.bytes()?;
        let aux = c.bytes()?;
        if main.len() != self.main.len() || aux.len() != self.aux.len() {
            return Err(StateError::Invalid("apple2 memory size"));
        }
        let lc = LangCard::from_bits(c.u8()?);
        let sw = IIeSwitches::from_bits(c.u8()?);
        self.main.copy_from_slice(main);
        self.aux.copy_from_slice(aux);
        self.lc = lc;
        self.sw = sw;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iiplus() -> Mem {
        let mut m = Mem::new(false, 0xc000, true, Aux::None, vec![0xee; 0x3000]);
        m.power_on();
        m
    }

    #[test]
    fn language_card_needs_two_odd_reads_to_write() {
        let mut m = iiplus();
        let modes = Modes::default();
        m.lc.access(0x0, false); // read RAM, protected
        assert_eq!(m.write_target(0xd000, &modes), Target::Open);
        m.lc.access(0xb, false);
        assert_eq!(m.write_target(0xd000, &modes), Target::Open, "one read");
        m.lc.access(0xb, false);
        assert_eq!(
            m.write_target(0xd000, &modes),
            Target::Main(0xc000),
            "bank 1"
        );
        assert_eq!(m.read_target(0xd000, &modes), Target::Main(0xc000));
        m.lc.access(0x3, true);
        assert_eq!(
            m.read_target(0xd000, &modes),
            Target::Main(0xd000),
            "bank 2"
        );
        // A write resets the pre-write flip-flop but keeps writing on.
        assert_eq!(m.write_target(0xe000, &modes), Target::Main(0xe000));
        m.lc.access(0x2, false);
        assert_eq!(m.read_target(0xe000, &modes), Target::Rom(0x1000));
        assert_eq!(m.write_target(0xe000, &modes), Target::Open);
    }

    #[test]
    fn ram_size_limits_the_ii() {
        let mut m = Mem::new(false, 0x4000, false, Aux::None, vec![0; 0x3000]);
        m.power_on();
        let modes = Modes::default();
        assert_eq!(m.read_target(0x3fff, &modes), Target::Main(0x3fff));
        assert_eq!(m.read_target(0x4000, &modes), Target::Open);
    }

    #[test]
    fn iie_aux_switches() {
        let mut m = Mem::new(true, 0xc000, true, Aux::Ext64k, vec![0; 0x4000]);
        m.power_on();
        let mut modes = Modes::default();
        m.sw.ramrd = true;
        assert_eq!(m.read_target(0x1000, &modes), Target::Aux(0x1000));
        assert_eq!(m.write_target(0x1000, &modes), Target::Main(0x1000));
        assert_eq!(m.read_target(0x0100, &modes), Target::Main(0x100), "ALTZP");
        modes.store80 = true;
        modes.page2 = true;
        m.sw.ramrd = false;
        assert_eq!(m.write_target(0x0400, &modes), Target::Aux(0x400));
        assert_eq!(
            m.read_target(0x2000, &modes),
            Target::Main(0x2000),
            "no HIRES"
        );
        modes.hires = true;
        assert_eq!(m.read_target(0x2000, &modes), Target::Aux(0x2000));
        m.sw.altzp = true;
        m.lc.access(0x8, false);
        assert_eq!(m.read_target(0xd000, &modes), Target::Aux(0xc000));
        assert_eq!(m.rom_target(0xc300), Target::Rom(0x300));
    }
}
