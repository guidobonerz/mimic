//! The NEC DSP boards (DSP-1 to DSP-4): the µPD77C25 (`cpu-upd7725`)
//! with the user's program ROM, its clock and its debug target.
//!
//! **Clock:** 7.6 MHz, one instruction per clock; the DSP runs behind the
//! master clock and is caught up whenever the CPU reaches its registers
//! and at every line.
//!
//! **Program:** ROM roles `dsp1`, `dsp1b`, `dsp2`, `dsp3`, `dsp4` (8 KB
//! each: program then data ROM). DSP-1 games take the DSP-1B (the later
//! revision with fixed rounding) when it is there, Pilotwings the DSP-1
//! (its demo flight was recorded with the first one); either stands in
//! for the other.
//!
//! Sources: Martin Korth, fullsnes, "SNES Cart DSP-n/ST010/ST011 - NEC
//! uPD77C25 / uPD96050" (clock, revisions; read 2026-11-09).

use std::collections::BTreeMap;

use cpu_upd7725::Upd7725;
use devices_cartridge::snes::{Dsp, SnesCart};
use emu_core::{
    DebugTarget, DeviceId, Flow, Instr, MemSpaceId, RegisterSet, Size, StateError, StateReader,
    StateWriter,
};

pub const DSP: DeviceId = DeviceId("dsp");
/// The DSP's data RAM, 256 words as 512 bytes (little-endian).
pub const DSP_RAM_SPACE: MemSpaceId = MemSpaceId(5);
pub const DSP_HZ: u64 = 7_600_000;

/// The ROM roles to try for a cartridge's DSP, best first.
pub fn roles(cart: &SnesCart) -> &'static [&'static str] {
    match cart.dsp {
        None => &[],
        Some(Dsp::Dsp1) if cart.title.to_ascii_uppercase().starts_with("PILOTWINGS") => {
            &["dsp1", "dsp1b"]
        }
        Some(Dsp::Dsp1) => &["dsp1b", "dsp1"],
        Some(Dsp::Dsp2) => &["dsp2"],
        Some(Dsp::Dsp3) => &["dsp3"],
        Some(Dsp::Dsp4) => &["dsp4"],
    }
}

/// The program for `cart`'s DSP from `roms`.
pub fn program<'a>(cart: &SnesCart, roms: &'a BTreeMap<String, Vec<u8>>) -> Option<&'a [u8]> {
    roles(cart)
        .iter()
        .find_map(|r| roms.get(*r))
        .map(Vec::as_slice)
}

/// The processor on the board.
#[derive(Clone)]
pub struct DspUnit {
    pub chip: Upd7725,
    /// Master clocks × DSP_HZ not yet run.
    pub acc: u64,
}

impl DspUnit {
    pub fn new(rom: &[u8]) -> Option<Self> {
        Upd7725::new(rom).ok().map(|chip| Self { chip, acc: 0 })
    }

    /// Run what `master_hz` has left owed.
    pub fn sync(&mut self, master_hz: u64) {
        let n = self.acc / master_hz;
        self.acc %= master_hz;
        for _ in 0..n {
            self.chip.step();
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"SNDS", 1, |w| w.u64(self.acc));
        self.chip.save(w);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"SNDS", 1)?;
        let acc = c.u64()?;
        self.chip.load(r)?;
        self.acc = acc;
        Ok(())
    }
}

impl DebugTarget for DspUnit {
    fn regs(&self) -> RegisterSet {
        self.chip.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.chip.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != DSP_RAM_SPACE {
            return None;
        }
        let byte = |a: u32| {
            let w = self.chip.ram[((a >> 1) & 0xff) as usize];
            u32::from(if a & 1 == 0 { w as u8 } else { (w >> 8) as u8 })
        };
        Some((0..size.bytes()).fold(0, |v, i| v | byte(addr.wrapping_add(i)) << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != DSP_RAM_SPACE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let b = u16::from((v >> (8 * i)) as u8);
            let w = &mut self.chip.ram[((a >> 1) & 0xff) as usize];
            *w = if a & 1 == 0 {
                (*w & 0xff00) | b
            } else {
                (*w & 0x00ff) | b << 8
            };
        }
        true
    }

    /// `addr` is an instruction number (the program ROM has its own
    /// address space of 24-bit words).
    fn disassemble(&self, addr: u32) -> Instr {
        let op = self.chip.program_word(addr as u16);
        let (mnemonic, operand, target) = cpu_upd7725::disassemble(op);
        let flow = match mnemonic.as_str() {
            "JMP" => Flow::Jump,
            "CALL" => Flow::Call,
            "RT" => Flow::Return,
            m if m.starts_with('J') => Flow::Branch,
            _ => Flow::Normal,
        };
        Instr {
            addr: addr & 0x7ff,
            bytes: op.to_le_bytes()[..3].to_vec(),
            mnemonic,
            operand,
            target: target.map(u32::from),
            flow,
        }
    }
}
