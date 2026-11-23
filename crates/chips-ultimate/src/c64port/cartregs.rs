//! The cartridge and machine control registers, `$040000`-`$04000F`
//! (decoded on the low four address bits).
//!
//! | Addr | Name | Bits |
//! |---|---|---|
//! | `$040000` | C64_MODE | W: bit 2 set pulls the C64's reset; else bit 3 set releases it; else bit 1 = ULTIMAX, bit 4 = NMI level. R: bit 1, bit 2 (reset held), bit 4 |
//! | `$040001` | C64_STOP | W bit 0 stop request. R bit 0 request, bit 1 the 6510 stands |
//! | `$040002` | C64_STOP_MODE | bits 1:0: 0 in a bad line, 1 after write-then-read, 2 at once |
//! | `$040003` | C64_CLOCK_DETECT | R: bit 0 PHI2 present, bit 4 reset line sensed |
//! | `$040005` | C64_CARTRIDGE_TYPE | bits 4:0 type, bits 7:5 variant |
//! | `$040006` | C64_CARTRIDGE_KILL / _ACTIVE | W bit 0 kill, bit 1 force; R bit 0 cartridge mapped |
//! | `$040007` | C64_KERNAL_ENABLE | bit 0 |
//! | `$040008` | C64_REU_ENABLE | bit 0 |
//! | `$040009` | C64_REU_SIZE | bits 2:0, 128 KB shifted left by the value |
//! | `$04000A` | C64_SWAP_CART_BUTTONS | W bit 0 |
//! | `$04000B` | C64_TIMING_ADDR_VALID | W: two 4-bit sample points |
//! | `$04000C` | C64_PHI2_EDGE_RECOVER | bit 0 recover, bit 2 serve the VIC (both read back); bits 1 and 3 measurement, write only |
//! | `$04000D` | C64_SERVE_CONTROL | bit 0: the cartridge answers DMA while the C64 is stopped |
//! | `$04000E` | C64_SAMPLER_ENABLE | bit 0 |
//!
//! This struct holds the latches only; what a write does to the C64 (reset,
//! stop, memory map, NMI) is done by [`super::C64Port`], which owns the
//! lines.
//!
//! What a cartridge model will need from here later: the type and variant
//! (written before the ROM image is copied to board memory `$03C00000`),
//! the kill and force strobes, the REU latches (memory at board memory
//! `$01000000`), the sampler enable and "serve while stopped".
//!
//! Source: UE2-C64U-Emulator `docs/hw/10-c64-machine.md` (table "Cartridge
//! / machine control", hazards H1-H4, H8, H16); the bit rules were checked
//! in the firmware's `fpga/cart_slot/vhdl_source/cart_slot_registers.vhd`
//! and `software/io/c64/c64.h`.

use mimic_core::{StateError, StateReader, StateWriter};

pub const MODE: u32 = 0x0;
pub const STOP: u32 = 0x1;
pub const STOP_MODE: u32 = 0x2;
pub const CLOCK_DETECT: u32 = 0x3;
pub const CARTRIDGE_TYPE: u32 = 0x5;
pub const CARTRIDGE_KILL: u32 = 0x6;
pub const KERNAL_ENABLE: u32 = 0x7;
pub const REU_ENABLE: u32 = 0x8;
pub const REU_SIZE: u32 = 0x9;
pub const SWAP_CART_BUTTONS: u32 = 0xa;
pub const TIMING_ADDR_VALID: u32 = 0xb;
pub const PHI2_EDGE_RECOVER: u32 = 0xc;
pub const SERVE_CONTROL: u32 = 0xd;
pub const SAMPLER_ENABLE: u32 = 0xe;

/// C64_MODE bits.
pub const MODE_ULTIMAX: u8 = 0x02;
pub const MODE_RESET: u8 = 0x04;
pub const MODE_UNRESET: u8 = 0x08;
pub const MODE_NMI: u8 = 0x10;

/// Stop at once (C64_STOP_MODE).
pub const STOP_FORCE: u8 = 2;

/// C64_CLOCK_DETECT: PHI2 is there.
pub const CD_PHI2: u8 = 0x01;
/// C64_CLOCK_DETECT: the reset line is low.
pub const CD_RESET: u8 = 0x10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CartRegs {
    /// C64_MODE bit 1.
    pub ultimax: bool,
    /// The board holds the C64's reset line.
    pub reset: bool,
    /// C64_MODE bit 4.
    pub nmi: bool,
    /// Stop request.
    pub stop: bool,
    pub stop_mode: u8,
    /// Type in bits 4:0, variant in bits 7:5.
    pub cartridge_type: u8,
    /// The last write to C64_CARTRIDGE_KILL (bit 0 kill, bit 1 force).
    pub kill: u8,
    /// How often the force strobe (bit 1) was written.
    pub force_strobes: u32,
    pub kernal_enable: bool,
    pub reu_enable: bool,
    pub reu_size: u8,
    pub swap_buttons: bool,
    pub timing: u8,
    /// Bits 0 and 2 of C64_PHI2_EDGE_RECOVER.
    pub phi2_recover: u8,
    pub serve_while_stopped: bool,
    pub sampler_enable: bool,
}

impl Default for CartRegs {
    fn default() -> Self {
        Self::new()
    }
}

impl CartRegs {
    /// Power-on: the C64 is held in reset until the firmware releases it
    /// (the real state is a build option of the FPGA that is not known;
    /// the firmware sets reset and stop itself and assumes neither).
    pub fn new() -> Self {
        Self {
            ultimax: false,
            reset: true,
            nmi: false,
            stop: false,
            stop_mode: 0,
            cartridge_type: 0,
            kill: 0,
            force_strobes: 0,
            kernal_enable: false,
            reu_enable: false,
            reu_size: 0,
            swap_buttons: false,
            timing: 0,
            phi2_recover: 0,
            serve_while_stopped: false,
            sampler_enable: false,
        }
    }

    /// The latch part of a write. C64_MODE and C64_STOP are written by the
    /// port itself, which follows the lines.
    pub fn write(&mut self, reg: u32, v: u8) {
        match reg & 0xf {
            STOP_MODE => self.stop_mode = v & 3,
            CARTRIDGE_TYPE => self.cartridge_type = v,
            CARTRIDGE_KILL => {
                self.kill = v & 3;
                if v & 2 != 0 {
                    self.force_strobes = self.force_strobes.wrapping_add(1);
                }
            }
            KERNAL_ENABLE => self.kernal_enable = v & 1 != 0,
            REU_ENABLE => self.reu_enable = v & 1 != 0,
            REU_SIZE => self.reu_size = v & 7,
            SWAP_CART_BUTTONS => self.swap_buttons = v & 1 != 0,
            TIMING_ADDR_VALID => self.timing = v,
            PHI2_EDGE_RECOVER => self.phi2_recover = v & 5,
            SERVE_CONTROL => self.serve_while_stopped = v & 1 != 0,
            SAMPLER_ENABLE => self.sampler_enable = v & 1 != 0,
            _ => {}
        }
    }

    /// What C64_MODE reads.
    pub fn mode(&self) -> u8 {
        (if self.ultimax { MODE_ULTIMAX } else { 0 })
            | (if self.reset { MODE_RESET } else { 0 })
            | (if self.nmi { MODE_NMI } else { 0 })
    }

    /// What a register reads; `stopped` is whether the 6510 stands and
    /// `active` whether the cartridge is mapped.
    pub fn read(&self, reg: u32, stopped: bool, active: bool) -> u8 {
        match reg & 0xf {
            MODE => self.mode(),
            STOP => u8::from(self.stop) | (u8::from(stopped) << 1),
            STOP_MODE => self.stop_mode,
            CLOCK_DETECT => CD_PHI2 | if self.reset { CD_RESET } else { 0 },
            CARTRIDGE_TYPE => self.cartridge_type,
            CARTRIDGE_KILL => u8::from(active),
            KERNAL_ENABLE => u8::from(self.kernal_enable),
            REU_ENABLE => u8::from(self.reu_enable),
            REU_SIZE => self.reu_size,
            PHI2_EDGE_RECOVER => self.phi2_recover,
            SERVE_CONTROL => u8::from(self.serve_while_stopped),
            SAMPLER_ENABLE => u8::from(self.sampler_enable),
            _ => 0,
        }
    }

    /// The REU's size in bytes as the size latch selects it.
    pub fn reu_bytes(&self) -> u32 {
        0x2_0000 << self.reu_size
    }

    pub fn save(&self, w: &mut StateWriter) {
        for b in [
            self.ultimax,
            self.reset,
            self.nmi,
            self.stop,
            self.kernal_enable,
            self.reu_enable,
            self.swap_buttons,
            self.serve_while_stopped,
            self.sampler_enable,
        ] {
            w.bool(b);
        }
        for v in [
            self.stop_mode,
            self.cartridge_type,
            self.kill,
            self.reu_size,
            self.timing,
            self.phi2_recover,
        ] {
            w.u8(v);
        }
        w.u32(self.force_strobes);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.ultimax = r.bool()?;
        self.reset = r.bool()?;
        self.nmi = r.bool()?;
        self.stop = r.bool()?;
        self.kernal_enable = r.bool()?;
        self.reu_enable = r.bool()?;
        self.swap_buttons = r.bool()?;
        self.serve_while_stopped = r.bool()?;
        self.sampler_enable = r.bool()?;
        self.stop_mode = r.u8()? & 3;
        self.cartridge_type = r.u8()?;
        self.kill = r.u8()? & 3;
        self.reu_size = r.u8()? & 7;
        self.timing = r.u8()?;
        self.phi2_recover = r.u8()? & 5;
        self.force_strobes = r.u32()?;
        Ok(())
    }
}
