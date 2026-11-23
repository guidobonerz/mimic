//! The configuration registers of the C64 core, `$180000`-`$1800FF`.
//!
//! All are byte latches that read back what was written, except
//! C64_CORE_VERSION (a constant) and the envelope levels at `$180080`
//! (read only). Offsets from `$180000`:
//!
//! | Off | Name | Meaning |
//! |---|---|---|
//! | `$00` | C64_SCANLINES | 0/1 |
//! | `$01` | C64_VIDEOFORMAT | bit 0 NTSC colour encoding, bit 1 60 Hz, bit 2 RGB, bit 3 NTSC clock, bits 5:4 cycles per line (0 = 63, 1 = 64, 2 = 65), bit 6 burst reset |
//! | `$02` | C64_TURBOREGS_EN | bit 0 turbo registers, bit 2 turbo enable bit, bit 1 SuperCPU detection |
//! | `$03` | C64_DMA_MEMONLY | bit 0: DMA reaches RAM only |
//! | `$05` | C64_PHASE_INCR | colour phase step |
//! | `$08`-`$0B` | SID1, SID2, EMUSID1, EMUSID2 base | C64 address bits 11:4 |
//! | `$0C`-`$0F` | the same four, mask | compare mask for those bits |
//! | `$10` | C64_CORE_VERSION | R |
//! | `$11`, `$12` | C64_SID1_EN, SID2_EN | socket enabled |
//! | `$13` | C64_PADDLE_EN | |
//! | `$14` | C64_STEREO_ADDRSEL | 0-5: which address bit splits a stereo SID |
//! | `$1A` | C64_PADDLE_SWAP | |
//! | `$20`-`$23` | EMUSID1/2 waves, EMUSID1/2 resonance | |
//! | `$27`, `$28` | EMUSID1/2 digi level | 0-3 |
//! | `$29` | C64_EMUSID_SPLIT | 0-7 |
//! | `$2A` | C64_BUS_BRIDGE | bit 0 mirrors writes to the external port |
//! | `$2B`, `$2C` | C64_BUS_INTERNAL, _EXTERNAL | bit 0 IO1, bit 1 IO2, bit 2 ROM, bit 3 IRQ served by that side |
//! | `$2D` | C64_SPEED_PREFER | bits 3:0 speed step, bit 7 keep bad-line timing |
//! | `$2E` | C64_SPEED_UPDATE | W 1: take `$02` and `$2D` over |
//! | `$2F` | C64_VIC_SPLIT | writes to `$D200`-`$D2FF` drive the LED strip |
//! | `$30`, `$31` | C64_JOY1/2_SWOUT | injected joystick lines, low = pressed, bits 4:0 |
//! | `$32`-`$35` | paddle 1 X/Y, 2 X/Y | |
//! | `$36`, `$37` | C64_MOUSE_EN_1/2 | |
//! | `$80`-`$87` | C64_VOICE_ADSR | R: envelope level, index SID * 4 + voice; reads 0 here |
//!
//! Only three of them act at once: the video format (the port tells the
//! C64 whether it is an NTSC machine), "DMA memory only" (the port's DMA
//! window) and the speed update strobe, which copies the two turbo latches
//! into the applied pair the machine reads with [`CoreConfig::cpu_speed`],
//! [`CoreConfig::badline_timing`] and [`CoreConfig::turbo_control`].
//! Everything else is kept for the machine to read.
//!
//! Source: UE2-C64U-Emulator `docs/hw/10-c64-machine.md` (table "C64 core
//! I/O", hazard H16, T0/T1 lists); names, bit values of the video format
//! and the speed table from the firmware's `software/system/u64.h` and
//! `software/u64/u64_config.cc`.

use mimic_core::{StateError, StateReader, StateWriter};

pub const SCANLINES: usize = 0x00;
pub const VIDEOFORMAT: usize = 0x01;
pub const TURBOREGS_EN: usize = 0x02;
pub const DMA_MEMONLY: usize = 0x03;
pub const PHASE_INCR: usize = 0x05;
/// Base registers of socket 1, socket 2, UltiSID 1, UltiSID 2.
pub const SID_BASE: usize = 0x08;
/// Their masks.
pub const SID_MASK: usize = 0x0c;
pub const CORE_VERSION_REG: usize = 0x10;
pub const SID1_EN: usize = 0x11;
pub const SID2_EN: usize = 0x12;
pub const PADDLE_EN: usize = 0x13;
pub const STEREO_ADDRSEL: usize = 0x14;
pub const PADDLE_SWAP: usize = 0x1a;
pub const EMUSID1_WAVES: usize = 0x20;
pub const EMUSID1_RES: usize = 0x22;
pub const EMUSID1_DIGI: usize = 0x27;
pub const EMUSID_SPLIT: usize = 0x29;
pub const BUS_BRIDGE: usize = 0x2a;
pub const BUS_INTERNAL: usize = 0x2b;
pub const BUS_EXTERNAL: usize = 0x2c;
pub const SPEED_PREFER: usize = 0x2d;
pub const SPEED_UPDATE: usize = 0x2e;
pub const VIC_SPLIT: usize = 0x2f;
pub const JOY1_SWOUT: usize = 0x30;
pub const JOY2_SWOUT: usize = 0x31;
pub const PADDLE_1_X: usize = 0x32;
pub const MOUSE_EN_1: usize = 0x36;
pub const VOICE_ADSR: usize = 0x80;

/// What C64_CORE_VERSION reads. The firmware only shows it ("1.xx"); the
/// value of a real core is not documented, this one is ours.
pub const CORE_VERSION: u8 = 0x47;

/// Video format: 65 cycles per line in bits 5:4.
const CYCLES_MASK: u8 = 0x30;
const CYCLES_65: u8 = 0x20;

/// The clock rates in MHz that the speed steps 0-15 stand for.
const SPEED_MHZ: [u8; 16] = [1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 20, 24, 32, 40, 48, 64];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    regs: [u8; 0x80],
    /// C64_TURBOREGS_EN as the last update strobe took it.
    applied_turbo: u8,
    /// C64_SPEED_PREFER as the last update strobe took it.
    applied_speed: u8,
    /// Counts the update strobes.
    speed_updates: u32,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreConfig {
    /// Power-on: everything 0, but the injected joystick lines released
    /// (a 0 there would hold all directions and fire) and the speed at
    /// 1 MHz with bad lines.
    pub fn new() -> Self {
        let mut regs = [0; 0x80];
        regs[JOY1_SWOUT] = 0xff;
        regs[JOY2_SWOUT] = 0xff;
        regs[SPEED_PREFER] = 0x80;
        Self {
            regs,
            applied_turbo: 0,
            applied_speed: 0x80,
            speed_updates: 0,
        }
    }

    pub fn read(&self, off: u32) -> u8 {
        match (off & 0xff) as usize {
            CORE_VERSION_REG => CORE_VERSION,
            o if o < VOICE_ADSR => self.regs[o],
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        let o = (off & 0xff) as usize;
        if o >= VOICE_ADSR || o == CORE_VERSION_REG {
            return;
        }
        self.regs[o] = v;
        if o == SPEED_UPDATE && v & 1 != 0 {
            self.applied_turbo = self.regs[TURBOREGS_EN];
            self.applied_speed = self.regs[SPEED_PREFER];
            self.speed_updates = self.speed_updates.wrapping_add(1);
        }
    }

    /// A latch by its offset (`$00`-`$7F`), for what has no getter.
    pub fn reg(&self, off: usize) -> u8 {
        self.regs.get(off).copied().unwrap_or(0)
    }

    pub fn video_format(&self) -> u8 {
        self.regs[VIDEOFORMAT]
    }

    /// The video format asks for an NTSC machine: 65 cycles per line.
    pub fn ntsc(&self) -> bool {
        self.regs[VIDEOFORMAT] & CYCLES_MASK == CYCLES_65
    }

    /// The colour encoding is NTSC (independent of the timing).
    pub fn ntsc_colour(&self) -> bool {
        self.regs[VIDEOFORMAT] & 1 != 0
    }

    /// DMA reaches RAM only.
    pub fn dma_memonly(&self) -> bool {
        self.regs[DMA_MEMONLY] & 1 != 0
    }

    /// The applied speed step, 0-15.
    pub fn cpu_speed(&self) -> u8 {
        self.applied_speed & 0x0f
    }

    /// The applied speed in MHz.
    pub fn cpu_speed_mhz(&self) -> u8 {
        SPEED_MHZ[usize::from(self.cpu_speed())]
    }

    /// The applied setting keeps the VIC's bad-line stalls.
    pub fn badline_timing(&self) -> bool {
        self.applied_speed & 0x80 != 0
    }

    /// The applied C64_TURBOREGS_EN: bit 0 the C64 may set the speed
    /// through the turbo registers, bit 2 through the turbo enable bit,
    /// bit 1 it answers the SuperCPU detection.
    pub fn turbo_control(&self) -> u8 {
        self.applied_turbo
    }

    /// Counts the update strobes, so the machine sees a new setting.
    pub fn speed_updates(&self) -> u32 {
        self.speed_updates
    }

    /// Base and mask of a SID decode: 0 socket 1, 1 socket 2, 2 UltiSID 1,
    /// 3 UltiSID 2. The chip answers where `(address bits 11:4) & mask ==
    /// base` (taken from the firmware's tables, not confirmed in the core).
    pub fn sid_decode(&self, which: usize) -> (u8, u8) {
        let i = which & 3;
        (self.regs[SID_BASE + i], self.regs[SID_MASK + i])
    }

    /// A SID socket (0 or 1) is switched on.
    pub fn sid_socket_enabled(&self, socket: usize) -> bool {
        self.regs[SID1_EN + (socket & 1)] & 1 != 0
    }

    /// The injected joystick lines of port 1 or 2 (index 0 or 1), low =
    /// pressed, bits 4:0.
    pub fn joy_swout(&self, port: usize) -> u8 {
        self.regs[JOY1_SWOUT + (port & 1)]
    }

    /// The injected paddle values: 0 port 1 X, 1 port 1 Y, 2 port 2 X,
    /// 3 port 2 Y.
    pub fn paddle(&self, which: usize) -> u8 {
        self.regs[PADDLE_1_X + (which & 3)]
    }

    pub fn paddle_enabled(&self) -> bool {
        self.regs[PADDLE_EN] & 1 != 0
    }

    pub fn paddle_swap(&self) -> bool {
        self.regs[PADDLE_SWAP] & 1 != 0
    }

    /// A mouse drives the paddle inputs of port 1 or 2 (index 0 or 1).
    pub fn mouse_enabled(&self, port: usize) -> bool {
        self.regs[MOUSE_EN_1 + (port & 1)] & 1 != 0
    }

    /// Bus sharing: (bridge, internal, external).
    pub fn bus_sharing(&self) -> (u8, u8, u8) {
        (
            self.regs[BUS_BRIDGE],
            self.regs[BUS_INTERNAL],
            self.regs[BUS_EXTERNAL],
        )
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.regs);
        w.u8(self.applied_turbo);
        w.u8(self.applied_speed);
        w.u32(self.speed_updates);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        r.raw_into(&mut self.regs)?;
        self.applied_turbo = r.u8()?;
        self.applied_speed = r.u8()?;
        self.speed_updates = r.u32()?;
        Ok(())
    }
}
