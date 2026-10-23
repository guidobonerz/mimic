//! Commodore Amiga 500, 1000, 2000 (OCS), 500+ (ECS) and 1200 (AGA).
//!
//! 68000 at 7.09379 MHz (PAL; NTSC 7.15909 MHz; the A1200's 68EC020 at
//! twice that), the OCS, ECS or AGA custom chips (`chips-amiga`: Agnus with
//! Copper and Blitter, Denise, Paula), two 8520
//! CIAs (`chips-cbm`, [`CiaModel::Mos8520`]), chip RAM, slow RAM, Zorro II
//! fast RAM, the Kickstart ROM (see [`memory`] for the map) and up to four
//! floppy drives ("DF0:"–"DF3:", ADF images; `drives_other::amiga_drive`).
//!
//! **Models** (`--model`, the board; "-ntsc" for the NTSC version of each):
//! - `500` (default): 512 KB chip RAM
//!   (option `chip=1m` for the 8372 "Fat Agnus" revision), 512 KB slow RAM
//!   in the trapdoor, Kickstart in ROM.
//! - `1000`: 256 KB chip RAM (`chip=512k` with the front expansion), no
//!   slow RAM, the Kickstart in the writable control store: preloaded by
//!   default, or loaded from a Kickstart disk by the bootstrap ROM (ROM
//!   role `a1000boot`, option `wcs=disk`).
//! - `2000`: 1 MB chip RAM, no slow RAM, Kickstart in ROM.
//! - `500plus`: the A500+ with the ECS chip set (Agnus 8375, Denise 8373),
//!   1 MB chip RAM (`chip=2m` with the trapdoor expansion), no slow RAM,
//!   Kickstart 2.0 by default.
//! - `1200`: 68EC020 at 14.18 MHz (four CPU clocks per colour clock; the
//!   68020's bus cycles take three of them, a long-word aligned long is
//!   one cycle on the 32-bit chip RAM), the AGA chip set (Alice, Lisa), 2 MB
//!   chip RAM, Gayle (its id register only; no IDE drive, no PCMCIA),
//!   Kickstart 3.1 by default (the A500/A600/A2000 image 40.063 boots the
//!   A1200 model; an A1200 image, 3.0 39.106 or 3.1 40.068, goes in as
//!   `kick30`/`kick31` too — the A4000's 3.0 does not fit: it scans for
//!   32-bit RAM at $08000000, which the 68EC020's 24-bit bus folds onto
//!   chip RAM). The E clock stays at the 7 MHz clock's tenth.
//!
//! **Disks.** Paula reads and writes the selected drive one bit cell at a
//! time: every 2 µs with ADKCON FAST, else every 4 µs. The index hole
//! pulses CIA-B /FLAG.
//!
//! **Keyboard.** [`keyboard::Keyboard`] sends raw key codes
//! ([`emu_core::input::MachineKey`] = the raw code, 0–$67) to CIA-A's
//! serial port; Ctrl + both Amiga keys reset the machine (RAM stays).
//!
//! **Audio.** Paula's channels 0+3 (left) and 1+2 (right) at the colour
//! clock, averaged down to 48 kHz stereo, then the A500's output filters:
//! a fixed first-order low-pass (about 4.9 kHz) and the "LED" filter, a
//! second-order Butterworth (Sallen-Key) low-pass at about 3.3 kHz, on
//! while CIA-A PA1 (/LED) is low (HRM chapter 5, "Audio filter"; the
//! corner frequencies as derived from the A500 schematics).
//!
//! Options:
//! - `kickstart`: "1.3" (default; A500+ "2.0", A1200 "3.1"), "1.2", "2.0",
//!   "3.0", "3.1" — which ROM role to use (`kick13` …, 256 or 512 KB
//!   images).
//! - `chip`: chip RAM, "256k", "512k", "1m" or "2m" (default per model;
//!   2 MB needs the ECS Agnus).
//! - `chipset`: "ocs", "ecs" or "aga" (default per model); `agnus=ocs|ecs` and
//!   `denise=ocs|ecs` pick the chips separately (an A500 rev 6A with the
//!   ECS Agnus 8372A and the OCS Denise is `agnus=ecs`). The ECS Agnus
//!   reports its id in VPOSR, follows BEAMCON0 (PAL/NTSC switch,
//!   programmable beam), DIWHIGH and BLTSIZV/BLTSIZH; the ECS Denise
//!   answers DENISEID and adds border blank, border sprites and
//!   super-hires (see `chips_amiga::custom` and `chips_amiga::denise`).
//! - `slow`: slow RAM, "512k" (A500 default) or "0".
//! - `fast`: Zorro II fast RAM on an autoconfig board, "0" (default),
//!   "512k", "1m", "2m", "4m" or "8m".
//! - `drives`: floppy drives fitted, "1" (default) to "4".
//! - `drivetype`: "dd" (default) or "hd" (1.76 MB drives; they spin at
//!   half speed for HD disks and report DD disks as a DD drive).
//! - `serial`: "none" (default) or "loopback" (TXD wired to RXD).
//! - `wcs` (A1000): "rom" (default, Kickstart preloaded) or "disk".
//! - `port0` / `port1`: game port devices, "mouse", "joystick" or "none"
//!   (defaults: the mouse in port 0, a joystick in port 1). The ports are
//!   sampled once a raster line; Denise counts mouse steps into JOYxDAT.
//! - `roms`: "stub" runs a tiny built-in program instead of Kickstart
//!   ([`stub`]; tests without ROMs).
//!
//! **CIAs.** CIA-A ($BFE001, odd bytes): port A 0 OVL (out), 1 LED (out),
//! 2 /CHNG, 3 /WPRO, 4 /TK0, 5 /RDY, 6 /FIR0, 7 /FIR1; port B the parallel
//! port; TOD counts VSYNC; /INT → INTREQ PORTS (level 2). CIA-B ($BFD000,
//! even bytes): port B the floppy control lines (see
//! `drives_other::amiga_drive`); TOD counts HSYNC; /INT → EXTER (level 6).
//! Both run on the E clock (CPU clock / 10); CPU accesses wait for it.
//!
//! **Timing.** The CPU runs whole instructions with
//! [`M68000::step_timed`]; before each bus cycle the bus brings the custom
//! chips and the CIAs up to that clock. Chip RAM, slow RAM and custom
//! registers take one free DMA slot (colour clock = two CPU clocks): the
//! CPU waits while DMA owns the bus. ROM accesses never wait.
//! [`Machine::step_cycle`] runs one instruction. The time base
//! ([`Machine::now`]) is CPU clocks.
//!
//! Sources: Amiga Hardware Reference Manual, 3rd edition (memory map,
//! appendix F "8520 CIA", chapter 7 "System control hardware"); M68000
//! User's Manual (VPA/E-clock cycles). The implementation is this
//! project's own.

#![forbid(unsafe_code)]

pub mod keyboard;
pub mod memory;
pub mod stub;

use chips_amiga::custom::{self, Chipset, Custom, Standard, FIRST_CCK, WIDTH};
use chips_cbm::cia::{Cia, CiaModel};
use cpu_m68k::Model as CpuModel;
use cpu_m68k::{Access, Step, TimedBus68, M68000};
use drives_other::amiga_drive::AmigaDrive;
use emu_core::audio::Decimator;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use keyboard::Keyboard;
use memory::{Memory, Region};

pub const CPU: DeviceId = DeviceId("cpu");
/// The 68000's 24-bit view.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// Chip RAM as the DMA channels see it.
pub const CHIP_SPACE: MemSpaceId = MemSpaceId(1);
/// Chip RAM as the Copper runs it: exec breakpoints here stop when the
/// Copper fetches an instruction at that address.
pub const COPPER_SPACE: MemSpaceId = MemSpaceId(2);

/// The Copper and the Blitter as debug targets (registers, chip RAM,
/// Copper disassembly).
pub const COPPER: DeviceId = DeviceId("copper");
pub const BLITTER: DeviceId = DeviceId("blitter");

/// Game port 0 (mouse) and 1 (joystick).
pub const PORT0: PortId = PortId(0);
pub const PORT1: PortId = PortId(1);

pub const CPU_HZ_PAL: u32 = 7_093_790;
pub const CPU_HZ_NTSC: u32 = 7_159_090;

/// The 68000's clock (colour clocks times two).
fn cpu_hz(standard: Standard) -> u32 {
    match standard {
        Standard::Pal => CPU_HZ_PAL,
        Standard::Ntsc => CPU_HZ_NTSC,
    }
}

/// Colour clocks per second.
fn cck_hz(standard: Standard) -> u32 {
    cpu_hz(standard) / 2
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "amiga",
        name: "Commodore Amiga 500/1000/2000/500+/1200",
        models: &[
            "500",
            "500-ntsc",
            "1000",
            "1000-ntsc",
            "2000",
            "2000-ntsc",
            "500plus",
            "500plus-ntsc",
            "1200",
            "1200-ntsc",
        ],
        create: |config| Ok(Box::new(Amiga::new(config)?)),
    }
}

/// The board a model names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    A500,
    A1000,
    A2000,
    A500Plus,
    A1200,
}

impl Model {
    /// CPU clocks per colour clock: two for the 68000 at 7 MHz, four for
    /// the A1200's 68EC020 at 14 MHz.
    pub fn clocks_per_cck(self) -> u64 {
        if self == Model::A1200 {
            4
        } else {
            2
        }
    }

    pub fn cpu(self) -> CpuModel {
        if self == Model::A1200 {
            CpuModel::Mc68020
        } else {
            CpuModel::Mc68000
        }
    }
}

/// `--machine amiga --model 500|1000|2000|500plus|1200`, "-ntsc" for the
/// NTSC version.
fn parse_model(model: Option<&str>) -> Result<(Model, Standard), ConfigError> {
    let Some(model) = model else {
        return Ok((Model::A500, Standard::Pal));
    };
    let (board, standard) = match model.strip_suffix("-ntsc") {
        Some(b) => (b, Standard::Ntsc),
        None => (model, Standard::Pal),
    };
    let board = match board {
        "500" => Model::A500,
        "1000" => Model::A1000,
        "2000" => Model::A2000,
        "500plus" => Model::A500Plus,
        "1200" => Model::A1200,
        _ => return Err(ConfigError::UnknownModel(model.into())),
    };
    Ok((board, standard))
}

/// `chipset`, `agnus` and `denise` options: the chips fitted.
fn chipset_option(config: &MachineConfig, default: Chipset) -> Result<Chipset, ConfigError> {
    let parse = |key: &str, default: bool| -> Result<bool, ConfigError> {
        let fallback = if default { "ecs" } else { "ocs" };
        match option(config, key, fallback) {
            "ocs" => Ok(false),
            "ecs" => Ok(true),
            other => Err(ConfigError::Invalid(format!(
                "{key} option '{other}' (ocs or ecs)"
            ))),
        }
    };
    let both = match config.options.get("chipset").map(String::as_str) {
        None => default,
        Some("aga") => Chipset::AGA,
        Some(_) => {
            let ecs = parse("chipset", false)?;
            Chipset {
                ecs_agnus: ecs,
                ecs_denise: ecs,
                aga: false,
            }
        }
    };
    if both.aga {
        if config.options.contains_key("agnus") || config.options.contains_key("denise") {
            return Err(ConfigError::Invalid(
                "agnus/denise options do not apply to the AGA chip set".into(),
            ));
        }
        return Ok(both);
    }
    Ok(Chipset {
        ecs_agnus: parse("agnus", both.ecs_agnus)?,
        ecs_denise: parse("denise", both.ecs_denise)?,
        aga: false,
    })
}

/// A size option: "0", "256k", "512k", "1m" … from `allowed`.
fn size_option(
    config: &MachineConfig,
    key: &str,
    default: &str,
    allowed: &[&str],
) -> Result<usize, ConfigError> {
    let v = option(config, key, default);
    if !allowed.contains(&v) {
        return Err(ConfigError::Invalid(format!(
            "{key} option '{v}' ({})",
            allowed.join(", ")
        )));
    }
    Ok(match v {
        "0" | "none" => 0,
        "256k" => 0x4_0000,
        "512k" => 0x8_0000,
        "1m" => 0x10_0000,
        "2m" => 0x20_0000,
        "4m" => 0x40_0000,
        "8m" => 0x80_0000,
        _ => 0,
    })
}

fn option<'a>(config: &'a MachineConfig, key: &str, default: &'a str) -> &'a str {
    config
        .options
        .get(key)
        .map(String::as_str)
        .unwrap_or(default)
}

fn load_rom(config: &MachineConfig, default_kick: &str) -> Result<Vec<u8>, ConfigError> {
    if let Some(v) = config.options.get("roms") {
        return match v.as_str() {
            "stub" => Ok(stub::rom()),
            other => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        };
    }
    let role = match option(config, "kickstart", default_kick) {
        "1.2" => "kick12",
        "1.3" => "kick13",
        "2.0" => "kick20",
        "3.0" => "kick30",
        "3.1" => "kick31",
        other => {
            return Err(ConfigError::Invalid(format!(
                "kickstart option '{other}' (1.2, 1.3, 2.0, 3.0 or 3.1)"
            )))
        }
    };
    let rom = config
        .roms
        .get(role)
        .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
    if rom.len() != 0x4_0000 && rom.len() != 0x8_0000 {
        return Err(ConfigError::Invalid(format!(
            "{role} ROM must be 256 or 512 KB, got {} bytes",
            rom.len()
        )));
    }
    Ok(rom.clone())
}

fn port_option(
    config: &MachineConfig,
    key: &str,
    default: &str,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match option(config, key, default) {
        "joystick" => Ok(Some(Box::new(devices_input::Joystick::new(
            PortKind::AmigaGameport,
        )))),
        // At most one step per 1000 clocks: sampled every line (454), no
        // phase is missed.
        "mouse" => Ok(Some(Box::new(devices_input::AmigaMouse::new(1000)))),
        "none" => Ok(None),
        other => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' (mouse, joystick or none)"
        ))),
    }
}

/// The two-bit counter phases (x, y) a port's lines give: right (pin 4)
/// is X1, right XOR down (pin 2) X0; left (pin 3) Y1, left XOR up (pin 1)
/// Y0 — a joystick's directions as the HRM reads them, and a mouse's
/// quadrature phases (HRM chapter 8, "Joysticks", "Mouse").
fn phases(lines: PortLines) -> (u8, u8) {
    use devices_input::joystick::pin;
    let low = |p: u32| u8::from(lines.pulled_low & p != 0);
    let (right, left) = (low(pin::RIGHT), low(pin::LEFT));
    let (down, up) = (low(pin::DOWN), low(pin::UP));
    (right << 1 | (right ^ down), left << 1 | (left ^ up))
}

/// Colour clocks between port samples (one raster line).
const PORT_SAMPLE_CCK: u64 = custom::PAL_LINE as u64;

/// Everything on the boards' buses except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    mem: Memory,
    custom: Custom,
    cia_a: Cia,
    cia_b: Cia,
    drives: Vec<AmigaDrive>,
    /// Colour clocks the chips have run.
    cck: u64,
    /// CPU clocks per colour clock (2 for the 68000, 4 for the A1200).
    clk: u64,
    /// Clocks of a CPU bus cycle (4 on the 68000, 3 on the 68020).
    bus_clocks: u64,
    /// The interrupt level Paula drives on IPL0–2, the level before its
    /// last change and the colour clock of that change: the CPU sees a
    /// change only [`IPL_DELAY`] later.
    ipl: u8,
    ipl_before: u8,
    ipl_changed: u64,
    /// A field ended since the last check.
    field_done: bool,
    /// /FIR0, /FIR1 pressed (CIA-A port A bits 6, 7).
    fire_low: u8,
    /// Colour clocks per second (the disk bit clock divides it).
    cck_hz: u32,
    disk_acc: u32,
    decimators: [Decimator; 2],
    /// Output filter states per side: the fixed stage (16.16) and the LED
    /// biquad's x[n-1], x[n-2], y[n-1], y[n-2] (16.16).
    filter: [[i32; 5]; 2],
    /// Address bits Agnus keeps in its pointers (512 KB: $7FFFE).
    ptr_mask: u32,
    /// Byte addresses in chip RAM (512 KB: $7FFFF).
    ptr_mask_byte: u32,
    /// Interleaved stereo samples since the last `audio` call.
    samples: Vec<i16>,
    keyboard: Keyboard,
}

/// Colour clocks from a change of Paula's interrupt request to the CPU
/// acting on it: Paula's IPL0–2 outputs follow INTREQ/INTENA about four
/// colour clocks later (logic-analyzer observation on an A500, vAmiga
/// issue #274, see `docs/sources.md`), and the 68000 decides whether to
/// take an interrupt at the end of an instruction from IPL0–2 as sampled
/// before its last bus cycle, through a synchroniser that wants the level
/// on two sampling edges (MC68000 User's Manual, interrupt exception
/// processing): about six clocks, three colour clocks. The sampling
/// point is taken as the start of the instruction's last bus cycle (a
/// rule of thumb from the same discussion); a bus cycle that then waits
/// for the chip bus does not move it. A request that arrives later is
/// taken after the next instruction. Desert Dream's main loop depends on
/// this: it polls VHPOSR for line 0 in a loop whose branch waits for the
/// Copper's burst at the line start, and reads the 0 only because the
/// vertical blank interrupt is taken one instruction later.
const IPL_DELAY: u64 = 7;

/// One-pole low-pass coefficient at 48 kHz (16-bit fraction):
/// 1 − e^(−2π·fc/48000) for 4.9 kHz.
const FILTER_FIXED: i32 = 31_025;
/// The LED filter: a second-order Butterworth low-pass at 3.3 kHz,
/// bilinear transform at 48 kHz; b0, b1, b2, a1, a2 with a 24-bit
/// fraction (DC gain exactly 1 within the rounding).
const LED_B: [i64; 3] = [594_544, 1_189_088, 594_544];
const LED_A: [i64; 2] = [-23_511_848, 9_112_808];
const LED_ONE: i64 = 1 << 24;

impl Board {
    /// One colour clock. Returns true if a waiting CPU got the slot.
    fn tick(&mut self, cpu_wants: bool) -> bool {
        let out = self.custom.tick(&mut self.mem.chip, cpu_wants);
        if self.cck.is_multiple_of(5) {
            self.keyboard.tick(&mut self.cia_a);
            self.cia_a.tick();
            self.cia_b.tick();
        }
        if out.hsync {
            self.cia_b.tod_tick();
        }
        if out.vsync {
            self.cia_a.tod_tick();
        }
        self.custom.paula.ports_line = self.cia_a.irq();
        self.custom.paula.exter_line = self.cia_b.irq();
        let ipl = self.custom.ipl();
        if ipl != self.ipl {
            self.ipl_before = self.ipl;
            self.ipl = ipl;
            self.ipl_changed = self.cck;
        }
        self.field_done |= out.field_done;
        self.audio(out.audio);
        let rate = if self.custom.paula.adkcon & chips_amiga::disk::FAST != 0 {
            500_000
        } else {
            250_000
        };
        self.disk_acc += rate;
        if self.disk_acc >= self.cck_hz {
            self.disk_acc -= self.cck_hz;
            self.disk_cell();
        }
        self.cck += 1;
        out.cpu_slot
    }

    /// The interrupt level the CPU saw at colour clock `cck` (see
    /// [`IPL_DELAY`]; at most one change since then is told apart).
    fn ipl_seen_at(&self, cck: u64) -> u8 {
        if cck < self.ipl_changed + IPL_DELAY {
            self.ipl_before
        } else {
            self.ipl
        }
    }

    /// One disk bit cell: Paula writes to or reads from the selected
    /// drives; every spinning disk turns on.
    fn disk_cell(&mut self) {
        let write = self.custom.disk_bit_out();
        let (mut bit, mut index) = (false, false);
        for d in &mut self.drives {
            if !d.motor {
                continue;
            }
            let selected = d.selected;
            let (b, i) = d.cell(if selected { write } else { None });
            if selected {
                bit |= b;
                index |= i;
            }
        }
        self.custom.disk_bit_in(bit);
        if index {
            self.cia_b.set_flag(false);
            self.cia_b.set_flag(true);
        }
    }

    fn audio(&mut self, levels: [i32; 2]) {
        let [l, r] = [
            self.decimators[0].push(levels[0] * 2),
            self.decimators[1].push(levels[1] * 2),
        ];
        let (Some(l), Some(r)) = (l, r) else {
            return;
        };
        let led = self.cia_a.port_a_out() & 2 == 0;
        for (side, x) in [l, r].into_iter().enumerate() {
            let f = &mut self.filter[side];
            let x = i32::from(x) << 16;
            f[0] += (((x - f[0]) as i64 * i64::from(FILTER_FIXED)) >> 16) as i32;
            let x = f[0];
            let y = if led {
                let [x1, x2, y1, y2] = [f[1], f[2], f[3], f[4]].map(i64::from);
                let acc = LED_B[0] * i64::from(x) + LED_B[1] * x1 + LED_B[2] * x2
                    - LED_A[0] * y1
                    - LED_A[1] * y2;
                (acc / LED_ONE).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
            } else {
                x
            };
            f[2] = f[1];
            f[1] = x;
            f[4] = f[3];
            f[3] = y;
            self.samples
                .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        }
    }

    /// Run the chips up to (not including) colour clock `to`.
    fn run_to(&mut self, to: u64) {
        while self.cck < to {
            self.tick(false);
        }
    }

    fn cia_a_ext(&self) -> u8 {
        let drives = self.drives.iter().fold(0, |v, d| v | d.pulled_low());
        !(drives | self.fire_low)
    }

    /// CIA outputs changed: overlay and floppy lines.
    fn cia_outputs(&mut self) {
        self.mem.overlay = self.cia_a.port_a_out() & 1 != 0;
        let prb = self.cia_b.port_b_out();
        for (i, d) in self.drives.iter_mut().enumerate() {
            d.write_prb(prb, 3 + i as u8);
        }
    }

    /// /RSTO (68000 RESET instruction) or power-on: the CIAs reset, the
    /// expansion board goes back to autoconfig.
    fn reset_devices(&mut self) {
        self.cia_a.reset();
        self.cia_b.reset();
        self.mem.reset_expansion();
        self.cia_outputs();
    }

    fn read_cia(&mut self, addr: u32, peek: bool) -> u8 {
        let reg = ((addr >> 8) & 15) as u8;
        let ext = self.cia_a_ext();
        if addr & 1 == 1 {
            if addr & 0x1000 != 0 {
                return 0xff;
            }
            if peek {
                self.cia_a.peek(reg, ext, 0xff)
            } else {
                self.cia_a.read(reg, ext, 0xff)
            }
        } else {
            if addr & 0x2000 != 0 {
                return 0xff;
            }
            if peek {
                self.cia_b.peek(reg, 0xff, 0xff)
            } else {
                self.cia_b.read(reg, 0xff, 0xff)
            }
        }
    }

    fn write_cia(&mut self, addr: u32, v: u8) {
        let reg = ((addr >> 8) & 15) as u8;
        if addr & 0x1000 == 0 {
            self.cia_a.write(reg, v);
        }
        if addr & 0x2000 == 0 {
            self.cia_b.write(reg, v);
        }
        self.cia_outputs();
    }

    fn word(&mut self, region: Region, addr: u32, peek: bool) -> u16 {
        match region {
            Region::Chip => self.mem.chip.word(addr),
            Region::Slow => self.mem.slow_word(addr),
            Region::Fast => self.mem.fast_word(addr),
            Region::Rom | Region::Wcs => self.mem.rom_word(addr),
            Region::Boot => self.mem.boot_word(addr),
            Region::Autoconfig => {
                let f = self.mem.fast.as_ref();
                u16::from(f.map_or(0xff, |f| f.read(addr & 0xffff))) << 8 | 0xff
            }
            Region::Custom => {
                if peek {
                    self.custom.peek(addr as u16)
                } else {
                    self.custom.read(addr as u16)
                }
            }
            Region::Cia => {
                u16::from(self.read_cia(addr & !1, peek)) << 8
                    | u16::from(self.read_cia(addr | 1, peek))
            }
            Region::Gayle => u16::from(self.read_gayle(addr & !1, peek)) << 8,
            Region::Open => 0,
        }
    }

    fn read_gayle(&mut self, addr: u32, peek: bool) -> u8 {
        match &mut self.mem.gayle {
            Some(g) if peek => g.peek(addr),
            Some(g) => g.read(addr),
            None => 0,
        }
    }

    fn read(&mut self, addr: u32, size: Size, peek: bool) -> u32 {
        let addr = addr & 0xff_ffff;
        let region = self.mem.region(addr, false);
        match size {
            Size::Byte => match region {
                Region::Chip => u32::from(self.mem.chip.byte(addr)),
                Region::Cia => u32::from(self.read_cia(addr, peek)),
                Region::Gayle => u32::from(self.read_gayle(addr, peek)),
                Region::Autoconfig => u32::from(
                    self.mem
                        .fast
                        .as_ref()
                        .map_or(0xff, |f| f.read(addr & 0xffff)),
                ),
                _ => {
                    let w = self.word(region, addr & !1, peek);
                    u32::from(if addr & 1 == 0 { w >> 8 } else { w & 0xff })
                }
            },
            Size::Word => u32::from(self.word(region, addr, peek)),
            Size::Long => {
                let hi = self.word(region, addr, peek);
                let lo_addr = addr.wrapping_add(2) & 0xff_ffff;
                let lo = self.word(self.mem.region(lo_addr, false), lo_addr, peek);
                u32::from(hi) << 16 | u32::from(lo)
            }
        }
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        let addr = addr & 0xff_ffff;
        match size {
            Size::Byte => self.write_byte(addr, v as u8),
            Size::Word => self.write_word(addr, v as u16),
            Size::Long => {
                self.write_word(addr, (v >> 16) as u16);
                self.write_word(addr.wrapping_add(2) & 0xff_ffff, v as u16);
            }
        }
    }

    fn write_word(&mut self, addr: u32, v: u16) {
        match self.mem.region(addr, true) {
            Region::Chip => self.mem.chip.set_word(addr, v),
            Region::Slow => self.mem.set_slow_word(addr, v),
            Region::Fast => self.mem.set_fast_word(addr, v),
            Region::Wcs => self.mem.set_wcs_word(addr, v),
            Region::Boot => self.mem.protect_wcs(),
            Region::Autoconfig => {
                if let Some(f) = &mut self.mem.fast {
                    f.write(addr & 0xffff, (v >> 8) as u8);
                }
            }
            Region::Custom => self.custom.write(addr as u16, v),
            Region::Cia => {
                self.write_cia(addr & !1, (v >> 8) as u8);
                self.write_cia(addr | 1, v as u8);
            }
            Region::Gayle => {
                if let Some(g) = &mut self.mem.gayle {
                    g.write(addr, (v >> 8) as u8);
                }
            }
            Region::Rom | Region::Open => {}
        }
    }

    /// The 68000 puts a byte on both halves of the data bus; the custom
    /// chips ignore UDS/LDS and take the whole word.
    fn write_byte(&mut self, addr: u32, v: u8) {
        match self.mem.region(addr, true) {
            Region::Chip => self.mem.chip.set_byte(addr, v),
            Region::Slow => self.mem.set_slow_byte(addr, v),
            Region::Fast => {
                let w = self.mem.fast_word(addr & !1);
                self.mem
                    .set_fast_word(addr & !1, memory::merge_byte(w, addr, v));
            }
            Region::Wcs => {
                let w = self.mem.rom_word(addr & !1);
                self.mem
                    .set_wcs_word(addr & !1, memory::merge_byte(w, addr, v));
            }
            Region::Boot => self.mem.protect_wcs(),
            Region::Autoconfig => {
                if let Some(f) = &mut self.mem.fast {
                    f.write(addr & 0xffff, v);
                }
            }
            Region::Custom => self
                .custom
                .write(addr as u16, u16::from(v) << 8 | u16::from(v)),
            Region::Cia => self.write_cia(addr, v),
            Region::Gayle => {
                if let Some(g) = &mut self.mem.gayle {
                    g.write(addr, v);
                }
            }
            Region::Rom | Region::Open => {}
        }
    }
}

/// Clocks a VPA (E-clock synchronous) cycle starting at CPU clock `at`
/// waits: it ends on an E-clock edge (every ten 7 MHz clocks, i.e. five
/// colour clocks) at least one E period after it began. `clk` is the
/// CPU's clocks per colour clock, `bus` its bus cycle length.
fn e_clock_waits(at: u64, clk: u64, bus: u64) -> u32 {
    let period = 5 * clk;
    let end = (at + period).div_ceil(period) * period;
    (end - at - bus) as u32
}

/// The 68000's bus for one instruction.
struct AmigaBus<'a> {
    b: &'a mut Board,
    /// CPU clock the instruction started on.
    start: u64,
    /// CPU clock the instruction's latest bus cycle started on: where
    /// the 68000 samples IPL0–2 for the interrupt decision at the end of
    /// the instruction (see [`IPL_DELAY`]).
    bus_start: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus for AmigaBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let mut v = self.b.read(addr, size, false);
        let addr = addr & 0xff_ffff;
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.read_armed(CPU_SPACE) {
                v = dbg.on_read(CPU_SPACE, addr, size, v);
                dbg.trace_read(CPU_SPACE, addr, size, v, self.beam);
            }
            // Chip RAM by its physical address (freezes and watchpoints
            // set on the "chip" space).
            if dbg.read_armed(CHIP_SPACE) && self.b.mem.region(addr, false) == Region::Chip {
                v = dbg.on_read(CHIP_SPACE, addr & self.b.ptr_mask_byte, size, v);
            }
        }
        v
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        peek(self.b, addr, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            let addr = addr & 0xff_ffff;
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, size, v, self.beam);
                if !dbg.on_write(CPU_SPACE, addr, size, v) {
                    return;
                }
            }
            if dbg.write_armed(CHIP_SPACE)
                && self.b.mem.region(addr, true) == Region::Chip
                && !dbg.on_write(CHIP_SPACE, addr & self.b.ptr_mask_byte, size, v)
            {
                return;
            }
        }
        self.b.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        24
    }
}

impl TimedBus68 for AmigaBus<'_> {
    fn bus_cycle(&mut self, access: Access, _fc: u8, addr: u32, _size: Size, t: u32) -> u32 {
        let at = self.start + u64::from(t);
        self.bus_start = at;
        let (clk, bus) = (self.b.clk, self.b.bus_clocks);
        if access == Access::IntAck {
            let w = e_clock_waits(at, clk, bus);
            self.b.run_to((at + bus + u64::from(w)) / clk);
            return w;
        }
        let region = self.b.mem.region(addr, access == Access::Write);
        if region.chip_bus() {
            // The CPU's data slot is the second colour clock of its cycle;
            // in the first it already asks for the bus (the Blitter's
            // starvation count sees it). On the A1200 the request comes
            // with the cycle's start, its data slot being the next colour
            // clock.
            let c0 = at / clk + 1;
            self.b.run_to(c0 - 1);
            if self.b.cck == c0 - 1 {
                self.b.custom.cpu_request();
                self.b.tick(false);
            }
            loop {
                let c = self.b.cck;
                if self.b.tick(true) {
                    return (clk * (c - c0)) as u32;
                }
            }
        }
        if region == Region::Cia {
            let w = e_clock_waits(at, clk, bus);
            self.b.run_to((at + bus + u64::from(w)) / clk - 1);
            return w;
        }
        0
    }

    fn reset_devices(&mut self) {
        self.b.reset_devices();
    }
}

fn peek(b: &Board, addr: u32, size: Size) -> u32 {
    let addr = addr & 0xff_ffff;
    match b.mem.region(addr, false) {
        Region::Cia => {
            let byte = |a: u32| -> u32 {
                let reg = ((a >> 8) & 15) as u8;
                if a & 1 == 1 {
                    if a & 0x1000 != 0 {
                        0xff
                    } else {
                        u32::from(b.cia_a.peek(reg, b.cia_a_ext(), 0xff))
                    }
                } else if a & 0x2000 != 0 {
                    0xff
                } else {
                    u32::from(b.cia_b.peek(reg, 0xff, 0xff))
                }
            };
            match size {
                Size::Byte => byte(addr),
                Size::Word => byte(addr & !1) << 8 | byte(addr | 1),
                Size::Long => {
                    (byte(addr) << 24)
                        | (byte(addr + 1) << 16)
                        | (byte(addr + 2) << 8)
                        | byte(addr + 3)
                }
            }
        }
        _ => peek_plain(b, addr, size),
    }
}

/// Peek without CIAs.
fn peek_plain(b: &Board, addr: u32, size: Size) -> u32 {
    let word = |a: u32| -> u16 {
        let a = a & 0xff_ffff;
        match b.mem.region(a, false) {
            Region::Chip => b.mem.chip.word(a),
            Region::Slow => b.mem.slow_word(a),
            Region::Fast => b.mem.fast_word(a),
            Region::Rom | Region::Wcs => b.mem.rom_word(a),
            Region::Boot => b.mem.boot_word(a),
            Region::Autoconfig => {
                let f = b.mem.fast.as_ref();
                u16::from(f.map_or(0xff, |f| f.read(a & 0xffff))) << 8 | 0xff
            }
            Region::Custom => b.custom.peek(a as u16),
            Region::Gayle => u16::from(b.mem.gayle.as_ref().map_or(0, |g| g.peek(a))) << 8,
            Region::Cia | Region::Open => 0,
        }
    };
    match size {
        Size::Byte => {
            let w = word(addr & !1);
            u32::from(if addr & 1 == 0 { w >> 8 } else { w & 0xff })
        }
        Size::Word => u32::from(word(addr)),
        Size::Long => u32::from(word(addr)) << 16 | u32::from(word(addr.wrapping_add(2))),
    }
}

/// Reads through the board without timing (reset vectors).
struct PlainBus<'a>(&'a mut Board);

impl Bus for PlainBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.0.read(addr, size, false)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        peek(self.0, addr, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        self.0.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        24
    }
}

impl TimedBus68 for PlainBus<'_> {
    fn bus_cycle(&mut self, _: Access, _: u8, _: u32, _: Size, _: u32) -> u32 {
        0
    }

    fn reset_devices(&mut self) {
        self.0.reset_devices();
    }
}

/// Peeks for the disassembler.
struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        peek(self.0, addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        peek(self.0, addr, size)
    }

    fn write(&mut self, _: u32, _: Size, _: u32) {}

    fn addr_bits(&self) -> u8 {
        24
    }
}

pub struct Amiga {
    model: Model,
    standard: Standard,
    cpu: M68000,
    board: Board,
    /// CPU clocks since power-on.
    sched: Scheduler,
    input: InputQueue,
    ports: [Option<Box<dyn PortDevice>>; 2],
    jam_reported: bool,
    /// CPU clock of the next game port sample.
    ports_due: u64,
    /// The device [`DebugTarget`] calls go to.
    debug_dev: DeviceId,
}

impl Amiga {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let (model, standard) = parse_model(config.model.as_deref())?;
        let (chip_default, slow_default, chipset_default, kick_default) = match model {
            Model::A500 => ("512k", "512k", Chipset::OCS, "1.3"),
            Model::A1000 => ("256k", "0", Chipset::OCS, "1.3"),
            Model::A2000 => ("1m", "0", Chipset::OCS, "1.3"),
            Model::A500Plus => ("1m", "0", Chipset::ECS, "2.0"),
            Model::A1200 => ("2m", "0", Chipset::AGA, "3.1"),
        };
        let chipset = chipset_option(config, chipset_default)?;
        let chip = size_option(config, "chip", chip_default, &["256k", "512k", "1m", "2m"])?;
        if chip > 0x10_0000 && !chipset.ecs_agnus {
            return Err(ConfigError::Invalid(
                "chip=2m needs the ECS Agnus (agnus=ecs or chipset=ecs)".into(),
            ));
        }
        let slow = size_option(config, "slow", slow_default, &["0", "none", "512k"])?;
        let fast = size_option(
            config,
            "fast",
            "0",
            &["0", "none", "512k", "1m", "2m", "4m", "8m"],
        )?;
        let loopback = match option(config, "serial", "none") {
            "none" => false,
            "loopback" => true,
            other => {
                return Err(ConfigError::Invalid(format!(
                    "serial option '{other}' (none or loopback)"
                )))
            }
        };
        let hd = match option(config, "drivetype", "dd") {
            "dd" => false,
            "hd" => true,
            other => {
                return Err(ConfigError::Invalid(format!(
                    "drivetype option '{other}' (dd or hd)"
                )))
            }
        };
        let boot_rom = match (model, option(config, "wcs", "rom")) {
            (Model::A1000, "disk") => Some(
                config
                    .roms
                    .get("a1000boot")
                    .ok_or_else(|| ConfigError::MissingRom("a1000boot".into()))?
                    .clone(),
            ),
            (_, "rom") | (_, "disk") => None,
            (_, other) => {
                return Err(ConfigError::Invalid(format!(
                    "wcs option '{other}' (rom or disk)"
                )))
            }
        };
        let drives = match option(config, "drives", "1") {
            n @ ("1" | "2" | "3" | "4") => n.parse::<usize>().unwrap_or(1),
            other => {
                return Err(ConfigError::Invalid(format!(
                    "drives option '{other}' (1–4)"
                )))
            }
        };
        let rom = if boot_rom.is_some() {
            Vec::new()
        } else {
            load_rom(config, kick_default)?
        };
        let mut mem = Memory::new(chip, slow, &rom).with_fast(fast);
        if model == Model::A1000 {
            mem = mem.with_a1000(boot_rom.as_deref());
        }
        if model == Model::A1200 {
            mem = mem.with_gayle();
        }
        let ptr_mask_byte = chip as u32 - 1;
        let mut custom = Custom::new(standard, ptr_mask_byte & !1, chipset);
        custom.paula.uart.loopback = loopback;
        let mut m = Self {
            model,
            standard,
            cpu: M68000::with_model(model.cpu()),
            board: Board {
                mem,
                custom,
                cia_a: Cia::new(CiaModel::Mos8520),
                cia_b: Cia::new(CiaModel::Mos8520),
                drives: vec![AmigaDrive::with_type(hd); drives],
                cck: 0,
                clk: model.clocks_per_cck(),
                bus_clocks: if model.cpu() == CpuModel::Mc68020 {
                    3
                } else {
                    4
                },
                ipl: 0,
                ipl_before: 0,
                ipl_changed: 0,
                field_done: false,
                fire_low: 0,
                cck_hz: cck_hz(standard),
                disk_acc: 0,
                decimators: [Decimator::new(cck_hz(standard), emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?;
                    2],
                filter: [[0; 5]; 2],
                ptr_mask: ptr_mask_byte & !1,
                ptr_mask_byte,
                samples: Vec::new(),
                keyboard: Keyboard::new(),
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            ports: [
                port_option(config, "port0", "mouse")?,
                port_option(config, "port1", "joystick")?,
            ],
            jam_reported: false,
            ports_due: 0,
            debug_dev: CPU,
        };
        m.power_on();
        Ok(m)
    }

    fn power_on(&mut self) {
        self.board.reset_devices();
        self.cpu.reset_timed(&mut PlainBus(&mut self.board));
    }

    pub fn standard(&self) -> Standard {
        self.standard
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let c = &self.board.custom;
        BeamPos {
            frame: c.fields,
            line: c.v,
            cycle: c.h,
            hpos: (4 * c.h).saturating_sub(4 * FIRST_CCK as u16),
        }
    }

    /// Read the game ports' lines: JOY0DAT/JOY1DAT, the fire buttons and
    /// the second buttons on the POT pins.
    fn refresh_ports(&mut self) {
        use devices_input::joystick::pin;
        let now = self.sched.now();
        let mut fire = 0;
        let mut pot = 0;
        for (i, dev) in self.ports.iter_mut().enumerate() {
            let lines = match dev {
                Some(d) => {
                    d.write_lines(PortLines::default(), now);
                    d.read_lines(now)
                }
                None => PortLines::default(),
            };
            let (x, y) = phases(lines);
            self.board.custom.port_phases(i, x, y);
            if lines.pulled_low & pin::FIRE != 0 {
                fire |= 0x40 << i;
            }
            // Second button on pin 9 (POTY), third on pin 5 (POTX).
            if lines.pulled_low & pin::BUTTON2 != 0 {
                pot |= 0x0400 << (4 * i);
            }
            if lines.pulled_low & pin::BUTTON3 != 0 {
                pot |= 0x0100 << (4 * i);
            }
        }
        self.board.fire_low = fire;
        self.board.custom.pot_low = pot;
    }

    /// One instruction (or interrupt, or STOP wait). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let now = self.sched.now().0;
        if now >= self.ports_due && self.ports.iter().any(Option::is_some) {
            self.ports_due = now + PORT_SAMPLE_CCK * self.board.clk;
            self.refresh_ports();
        }
        let start = self.sched.now().0;
        let beam = self.beam_pos();
        let (step, sampled) = {
            let mut bus = AmigaBus {
                b: &mut self.board,
                start,
                bus_start: start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            let step = self.cpu.step_timed(&mut bus);
            (step, bus.bus_start)
        };
        let clocks = match step {
            Step::Done { clocks } | Step::Stopped { clocks } => clocks,
            // A halted CPU (double bus fault): the chips keep running.
            Step::Halted => 4,
        };
        let end = start + u64::from(clocks);
        let clk = self.board.clk;
        self.board.run_to(end / clk);
        self.sched.advance(u64::from(clocks));
        // The interrupt decision at the end of this instruction uses the
        // level as it stood when its last bus cycle began (the next step
        // acts on it).
        self.cpu.set_ipl(self.board.ipl_seen_at(sampled / clk));
        if step == Step::Halted {
            if !self.jam_reported {
                self.jam_reported = true;
                return Some(StopReason::CpuJam {
                    device: CPU,
                    pc: self.cpu.pc(),
                });
            }
        } else {
            self.jam_reported = false;
        }
        let entered = self.cpu.take_interrupt();
        let new_field = std::mem::take(&mut self.board.field_done);
        let copper_pcs = self.board.custom.copper.take_fetched();
        if let Some(dbg) = dbg {
            for pc in copper_pcs {
                dbg.check_exec(COPPER_SPACE, pc);
            }
            self.board.custom.copper.watch = dbg.exec_armed(COPPER_SPACE);
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            let next_pc = self.cpu.pc();
            if dbg.trace_instr_armed() {
                dbg.trace_instr(CPU, next_pc, self.beam_pos());
            }
            if dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.exec_armed(CPU_SPACE) {
                dbg.check_exec(CPU_SPACE, next_pc);
            }
            if dbg.raster_armed() {
                let b = self.beam_pos();
                dbg.check_raster(b.line, b.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                self.board.field_done |= new_field;
                return Some(stop);
            }
        }
        new_field.then_some(StopReason::FrameDone)
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                if self.board.keyboard.key(machine_key.0 as u8, down) {
                    Machine::reset(self, ResetKind::Soft);
                }
            }
            (InputTarget::Port(p), event) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&event);
                }
                self.refresh_ports();
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) => match id.0 {
                0 => Machine::reset(self, ResetKind::Hard),
                1 => Machine::reset(self, ResetKind::Soft),
                _ => {}
            },
            _ => {}
        }
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            match f.space {
                CPU_SPACE => self.board.write(f.addr, f.size, f.value),
                CHIP_SPACE => {
                    for i in 0..f.size.bytes() {
                        let shift = 8 * (f.size.bytes() - 1 - i);
                        self.board
                            .mem
                            .chip
                            .set_byte(f.addr.wrapping_add(i), (f.value >> shift) as u8);
                    }
                }
                _ => {}
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        let c = &self.board.custom;
        // The pixel clock is the crystal's; the beam standard can differ
        // on the ECS Agnus (BEAMCON0).
        let clock = 4 * cck_hz(self.standard);
        let standard = match c.standard {
            Standard::Pal => VideoStandard::Pal,
            Standard::Ntsc => VideoStandard::Ntsc,
        };
        SignalInfo {
            pixel_clock_hz: clock,
            active_pixels: WIDTH,
            total_pixels: 4 * u32::from(custom::PAL_LINE),
            lines_per_field: u32::from(c.field_lines()),
            standard,
            output: OutputType::Rgb,
            field: if !c.interlaced() {
                FieldParity::Progressive
            } else if c.lof {
                FieldParity::Even
            } else {
                FieldParity::Odd
            },
        }
    }
}

/// Read memory as the CPU sees it now, without side effects (tests, tools).
pub fn peek_cpu(m: &Amiga, addr: u32, size: Size) -> u32 {
    peek(&m.board, addr, size)
}

fn chip_regs(name: &'static str, values: &[(&'static str, Option<u32>, u32, u8)]) -> ChipInfo {
    ChipInfo {
        name,
        regs: values
            .iter()
            .map(|&(name, addr, value, bits)| ChipReg {
                name,
                addr,
                value,
                bits,
            })
            .collect(),
    }
}

fn cia_info(name: &'static str, cia: &Cia, base: u32, step: u32) -> ChipInfo {
    const NAMES: [&str; 16] = [
        "PRA", "PRB", "DDRA", "DDRB", "TALO", "TAHI", "TBLO", "TBHI", "TODLO", "TODMID", "TODHI",
        "-", "SDR", "ICR", "CRA", "CRB",
    ];
    let mut regs: Vec<(&'static str, Option<u32>, u32, u8)> = NAMES
        .iter()
        .enumerate()
        .map(|(i, &n)| {
            (
                n,
                Some(base + step * i as u32),
                u32::from(cia.peek(i as u8, 0xff, 0xff)),
                8,
            )
        })
        .collect();
    regs.push(("IMR", None, u32::from(cia.mask()), 8));
    regs.push(("TALATCH", None, u32::from(cia.ta.latch), 16));
    regs.push(("TBLATCH", None, u32::from(cia.tb.latch), 16));
    chip_regs(name, &regs)
}

impl Machine for Amiga {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.board.field_done) {
            return StopReason::FrameDone;
        }
        let c = &self.board.custom;
        if c.v == 0 && c.h < 8 {
            self.apply_frame_freezes(dbg);
        }
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.instruction(None);
        StopReason::StepDone
    }

    /// Hard: power cycle (RAM cleared). Soft: the reset line (keyboard
    /// reset): CPU, CIAs and custom chips restart, RAM keeps its contents.
    fn reset(&mut self, kind: ResetKind) {
        self.board.custom.reset();
        self.board.keyboard = Keyboard::new();
        if kind == ResetKind::Hard {
            self.board.mem.clear();
            // The mechanics restart; a disk stays in its drive.
            for d in &mut self.board.drives {
                let mut n = AmigaDrive::new();
                n.take_disk(d);
                *d = n;
            }
        }
        self.input.clear();
        self.board.field_done = false;
        self.jam_reported = false;
        self.power_on();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: WIDTH,
            height: self.board.custom.standard.height(),
            pixels: PixelData::Rgb(self.board.custom.frame()),
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 2,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![
            MemSpace {
                id: CPU_SPACE,
                name: "cpu",
                device: CPU,
                addr_bits: 24,
                big_endian: true,
            },
            MemSpace {
                id: CHIP_SPACE,
                name: "chip",
                device: CPU,
                addr_bits: self.board.mem.chip_bits(),
                big_endian: true,
            },
            MemSpace {
                id: COPPER_SPACE,
                name: "copper",
                device: COPPER,
                addr_bits: self.board.mem.chip_bits(),
                big_endian: true,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let c = &self.board.custom;
        let d = &c.denise;
        let p = &c.paula;
        let b = &c.blitter;
        let r = |n, a: u16, v: u32, bits| (n, Some(0xdf_f000 + u32::from(a)), v, bits);
        let mut agnus = vec![
            r("DMACON", 0x096, u32::from(c.peek(0x002)), 16),
            r("VPOSR", 0x004, u32::from(c.peek(0x004)), 16),
            r("VHPOSR", 0x006, u32::from(c.peek(0x006)), 16),
            r("DIWSTRT", 0x08e, u32::from(c.diwstrt), 16),
            r("DIWSTOP", 0x090, u32::from(c.diwstop), 16),
            r("DDFSTRT", 0x092, u32::from(c.ddfstrt), 16),
            r("DDFSTOP", 0x094, u32::from(c.ddfstop), 16),
            r("BPL1MOD", 0x108, u32::from(c.bpl1mod), 16),
            r("BPL2MOD", 0x10a, u32::from(c.bpl2mod), 16),
        ];
        if c.chipset.aga {
            agnus.push(r("BPL7PT", 0x0f8, c.bplpt[6], 32));
            agnus.push(r("BPL8PT", 0x0fc, c.bplpt[7], 32));
            agnus.push(r("FMODE", 0x1fc, u32::from(c.fmode), 16));
        }
        if c.chipset.ecs_agnus {
            agnus.push(r("DIWHIGH", 0x1e4, u32::from(c.diwhigh), 16));
            for (i, n) in custom::BEAM_REG_NAMES.iter().enumerate() {
                agnus.push(r(n, 0x1c0 + 2 * i as u16, u32::from(c.beam_regs[i]), 16));
            }
        }
        const BPL: [&str; 6] = ["BPL1PT", "BPL2PT", "BPL3PT", "BPL4PT", "BPL5PT", "BPL6PT"];
        for (i, n) in BPL.iter().enumerate() {
            agnus.push(r(n, 0x0e0 + 4 * i as u16, c.bplpt[i], 32));
        }
        let copper = [
            r("COP1LC", 0x080, c.copper.cop1lc, 32),
            r("COP2LC", 0x084, c.copper.cop2lc, 32),
            ("COPPC", None, c.copper.pc, 32),
            r("COPCON", 0x02e, u32::from(c.copper.danger) << 1, 16),
        ];
        let blitter = [
            r("BLTCON0", 0x040, u32::from(b.con0), 16),
            r("BLTCON1", 0x042, u32::from(b.con1), 16),
            r("BLTAFWM", 0x044, u32::from(b.afwm), 16),
            r("BLTALWM", 0x046, u32::from(b.alwm), 16),
            r("BLTAPT", 0x050, b.pt[0], 32),
            r("BLTBPT", 0x04c, b.pt[1], 32),
            r("BLTCPT", 0x048, b.pt[2], 32),
            r("BLTDPT", 0x054, b.pt[3], 32),
            r("BLTAMOD", 0x064, u32::from(b.modulo[0]), 16),
            r("BLTBMOD", 0x062, u32::from(b.modulo[1]), 16),
            r("BLTCMOD", 0x060, u32::from(b.modulo[2]), 16),
            r("BLTDMOD", 0x066, u32::from(b.modulo[3]), 16),
            r("BLTSIZV", 0x05c, u32::from(b.sizv), 15),
            ("BUSY", None, u32::from(b.busy), 1),
            (
                "WIDTH",
                None,
                u32::from(b.job_size().map_or(0, |s| s.0)),
                11,
            ),
            (
                "HEIGHT",
                None,
                u32::from(b.job_size().map_or(0, |s| s.1)),
                16,
            ),
        ];
        let mut denise = vec![
            r("BPLCON0", 0x100, u32::from(d.bplcon0), 16),
            r("BPLCON1", 0x102, u32::from(d.bplcon1), 16),
            r("BPLCON2", 0x104, u32::from(d.bplcon2), 16),
            r("CLXCON", 0x098, u32::from(d.clxcon), 16),
            r("CLXDAT", 0x00e, u32::from(c.peek(0x00e)), 16),
        ];
        if d.aga {
            denise.push(r("BPLCON4", 0x10c, u32::from(d.bplcon4), 16));
            denise.push(r("CLXCON2", 0x10e, u32::from(d.clxcon2), 16));
        }
        if d.ecs {
            denise.push(r("BPLCON3", 0x106, u32::from(d.bplcon3), 16));
            denise.push(r("DIWHIGH", 0x1e4, u32::from(d.diwhigh), 16));
            denise.push(r("DENISEID", 0x07c, u32::from(c.peek(0x07c)), 16));
        }
        const COLORS: [&str; 32] = [
            "COLOR00", "COLOR01", "COLOR02", "COLOR03", "COLOR04", "COLOR05", "COLOR06", "COLOR07",
            "COLOR08", "COLOR09", "COLOR10", "COLOR11", "COLOR12", "COLOR13", "COLOR14", "COLOR15",
            "COLOR16", "COLOR17", "COLOR18", "COLOR19", "COLOR20", "COLOR21", "COLOR22", "COLOR23",
            "COLOR24", "COLOR25", "COLOR26", "COLOR27", "COLOR28", "COLOR29", "COLOR30", "COLOR31",
        ];
        for (i, n) in COLORS.iter().enumerate() {
            if d.aga {
                denise.push(r(n, 0x180 + 2 * i as u16, d.colors[i], 24));
            } else {
                denise.push(r(n, 0x180 + 2 * i as u16, u32::from(d.color12(i)), 12));
            }
        }
        let mut paula = vec![
            r("INTENA", 0x09a, u32::from(p.intena), 16),
            r("INTREQ", 0x09c, u32::from(p.requests()), 16),
            r("ADKCON", 0x09e, u32::from(p.adkcon), 16),
            r("POTGO", 0x034, u32::from(p.potgo), 16),
            ("IPL", None, u32::from(p.ipl()), 3),
            r("DSKPT", 0x020, p.disk.pt, 32),
            r("DSKLEN", 0x024, u32::from(p.dsklen), 16),
            r("DSKSYNC", 0x07e, u32::from(p.dsksync), 16),
            r("DSKBYTR", 0x01a, u32::from(p.disk.dskbytr()), 16),
            ("DSKLEFT", None, u32::from(p.disk.remaining), 16),
            r("SERPER", 0x032, u32::from(p.uart.serper), 16),
            r("SERDATR", 0x018, u32::from(p.uart.serdatr()), 16),
        ];
        const AUD: [[&str; 4]; 4] = [
            ["AUD0LC", "AUD0LEN", "AUD0PER", "AUD0VOL"],
            ["AUD1LC", "AUD1LEN", "AUD1PER", "AUD1VOL"],
            ["AUD2LC", "AUD2LEN", "AUD2PER", "AUD2VOL"],
            ["AUD3LC", "AUD3LEN", "AUD3PER", "AUD3VOL"],
        ];
        for (i, (n, a)) in AUD.iter().zip(&p.audio).enumerate() {
            let base = 0x0a0 + 0x10 * i as u16;
            paula.push(r(n[0], base, a.lc, 32));
            paula.push(r(n[1], base + 4, u32::from(a.len), 16));
            paula.push(r(n[2], base + 6, u32::from(a.per), 16));
            paula.push(r(n[3], base + 8, u32::from(a.vol), 7));
        }
        vec![
            chip_regs("agnus", &agnus),
            chip_regs("copper", &copper),
            chip_regs("blitter", &blitter),
            chip_regs("denise", &denise),
            chip_regs("paula", &paula),
            cia_info("cia_a", &self.board.cia_a, 0xbf_e001, 0x100),
            cia_info("cia_b", &self.board.cia_b, 0xbf_d000, 0x100),
            chip_regs(
                "keyboard",
                &[("PENDING", None, self.board.keyboard.pending() as u32, 8)],
            ),
            chip_regs(
                "floppy",
                &self
                    .board
                    .drives
                    .iter()
                    .flat_map(|d| {
                        [
                            ("CYL", None, u32::from(d.cylinder), 8),
                            ("HEAD", None, u32::from(d.upper), 1),
                            ("MOTOR", None, u32::from(d.motor), 1),
                            ("POS", None, d.pos, 32),
                        ]
                    })
                    .collect::<Vec<_>>(),
            ),
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU, COPPER, BLITTER]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if [CPU, COPPER, BLITTER].contains(&dev) {
            self.debug_dev = dev;
            Some(self as &mut dyn DebugTarget)
        } else {
            None
        }
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        use emu_core::ControlInfo;
        vec![
            ControlInfo::button(0, "POWER", false),
            ControlInfo::button(1, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        [
            (PORT0, "game port 0 (mouse)"),
            (PORT1, "game port 1 (joystick)"),
        ]
        .into_iter()
        .map(|(id, name)| PortInfo {
            id,
            kind: PortKind::AmigaGameport,
            name,
            connected: self.ports[usize::from(id.0)].is_some(),
        })
        .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::AmigaGameport {
            return Err(PortError::Incompatible {
                port: PortKind::AmigaGameport,
                device: dev.kind(),
            });
        }
        if slot.is_some() {
            return Err(PortError::Occupied(port));
        }
        *slot = Some(dev);
        self.refresh_ports();
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        let dev = self.ports.get_mut(usize::from(port.0))?.take();
        self.refresh_ports();
        dev
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        const NAMES: [&str; 4] = ["DF0:", "DF1:", "DF2:", "DF3:"];
        self.board
            .drives
            .iter()
            .enumerate()
            .map(|(i, d)| MediaSlotInfo {
                id: SlotId(i as u8),
                name: NAMES[i],
                kind: MediaKind::Disk,
                inserted: d.has_disk(),
            })
            .collect()
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        self.board
            .drives
            .get_mut(usize::from(slot.0))
            .map(|d| d as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        // Version 5: the LED filter is a biquad (five states per side);
        // version 6: Gayle's id shift (A1200).
        w.chunk(*b"A500", 6, |w| {
            w.u8(match self.standard {
                Standard::Pal => 0,
                Standard::Ntsc => 1,
            });
            w.u8(b.drives.len() as u8);
            w.u64(b.cck);
            w.bool(b.field_done);
            w.u8(b.fire_low);
            w.bool(self.jam_reported);
            w.u64(self.ports_due);
            w.u32(b.disk_acc);
            for d in &b.decimators {
                d.save(w);
            }
            for v in b.filter.iter().flatten() {
                w.u32(*v as u32);
            }
            w.u8(b.ipl);
            w.u8(b.ipl_before);
            w.u64(b.ipl_changed);
            w.bool(b.mem.gayle.is_some());
            if let Some(g) = &b.mem.gayle {
                g.save(w);
            }
        });
        self.cpu.save(&mut w);
        b.mem.save(&mut w);
        b.custom.save(&mut w);
        b.cia_a.save(&mut w);
        b.cia_b.save(&mut w);
        for d in &b.drives {
            d.save(&mut w);
        }
        b.keyboard.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"GPT0", *b"GPT1"].into_iter().enumerate() {
            w.chunk(tag, 1, |w| {
                w.bytes(
                    &self.ports[i]
                        .as_ref()
                        .map(|d| d.save_state())
                        .unwrap_or_default(),
                );
            });
        }
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (version, mut c) = r.chunk(*b"A500", 6)?;
        let standard = match c.u8()? {
            0 => Standard::Pal,
            1 => Standard::Ntsc,
            _ => return Err(StateError::Invalid("video standard")),
        };
        if standard != self.standard {
            return Err(StateError::Invalid("video standard differs"));
        }
        if usize::from(c.u8()?) != self.board.drives.len() {
            return Err(StateError::Invalid("drive count differs"));
        }
        let mut board = self.board.clone();
        board.cck = c.u64()?;
        board.field_done = c.bool()?;
        board.fire_low = c.u8()? & 0xc0;
        let jam_reported = c.bool()?;
        let ports_due = c.u64()?;
        board.disk_acc = c.u32()?;
        if board.disk_acc >= board.cck_hz {
            return Err(StateError::Invalid("disk clock"));
        }
        for d in &mut board.decimators {
            d.load(&mut c)?;
        }
        let states = if version >= 5 { 5 } else { 2 };
        for side in board.filter.iter_mut() {
            *side = [0; 5];
            for v in side.iter_mut().take(states) {
                *v = c.u32()? as i32;
            }
        }
        if version >= 4 {
            board.ipl = c.u8()? & 7;
            board.ipl_before = c.u8()? & 7;
            board.ipl_changed = c.u64()?;
        }
        if version >= 6 {
            let has_gayle = c.bool()?;
            if has_gayle != board.mem.gayle.is_some() {
                return Err(StateError::Invalid("Gayle differs"));
            }
            if has_gayle {
                board.mem.gayle = Some(memory::Gayle::load(&mut c)?);
            }
        }
        board.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        board.mem.load(&mut r)?;
        board.custom.load(&mut r)?;
        if version < 4 {
            // Older states: the CPU sees Paula's level at once.
            board.ipl = board.custom.ipl();
            board.ipl_before = board.ipl;
            board.ipl_changed = 0;
        }
        board.cia_a.load(&mut r)?;
        board.cia_b.load(&mut r)?;
        for d in &mut board.drives {
            d.load(&mut r)?;
        }
        board.keyboard.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"GPT0", *b"GPT1"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        let backups: Vec<Option<Vec<u8>>> = self
            .ports
            .iter()
            .map(|d| d.as_ref().map(|d| d.save_state()))
            .collect();
        for (i, state) in port_states.iter().enumerate() {
            if let (Some(dev), false) = (&mut self.ports[i], state.is_empty()) {
                if let Err(e) = dev.load_state(state) {
                    for (dev, b) in self.ports.iter_mut().zip(&backups) {
                        if let (Some(d), Some(b)) = (dev, b) {
                            let _ = d.load_state(b);
                        }
                    }
                    return Err(e);
                }
            }
        }
        // Disks are media, not state: they stay in their drives.
        for (new, old) in board.drives.iter_mut().zip(&mut self.board.drives) {
            new.take_disk(old);
        }
        self.cpu = cpu;
        self.board = board;
        self.sched = sched;
        self.input = input;
        self.jam_reported = jam_reported;
        self.ports_due = ports_due;
        Ok(())
    }
}

/// Register list from (name, value, bits).
fn register_set(values: &[(&'static str, u32, u8)]) -> RegisterSet {
    let mut set = RegisterSet::default();
    for &(name, value, bits) in values {
        set.0.push(emu_core::Register { name, value, bits });
    }
    set
}

impl Amiga {
    fn copper_regs(&self) -> RegisterSet {
        use chips_amiga::copper::State;
        let c = &self.board.custom.copper;
        let (state, ir1, ir2) = match c.state {
            State::Fetch1 => (0, 0, 0),
            State::Fetch2(a) => (1, a, 0),
            State::Wait(a, b) => (2, a, b),
            State::Stopped => (3, 0, 0),
        };
        register_set(&[
            ("PC", c.pc, 32),
            ("COP1LC", c.cop1lc, 32),
            ("COP2LC", c.cop2lc, 32),
            // 0 fetch, 1 second word, 2 waiting, 3 stopped (illegal MOVE)
            ("STATE", state, 8),
            ("IR1", u32::from(ir1), 16),
            ("IR2", u32::from(ir2), 16),
            ("DANGER", u32::from(c.danger), 1),
        ])
    }

    fn blitter_regs(&self) -> RegisterSet {
        let b = &self.board.custom.blitter;
        register_set(&[
            ("BLTCON0", u32::from(b.con0), 16),
            ("BLTCON1", u32::from(b.con1), 16),
            ("BLTAFWM", u32::from(b.afwm), 16),
            ("BLTALWM", u32::from(b.alwm), 16),
            ("BLTAPT", b.pt[0], 32),
            ("BLTBPT", b.pt[1], 32),
            ("BLTCPT", b.pt[2], 32),
            ("BLTDPT", b.pt[3], 32),
            ("BLTAMOD", u32::from(b.modulo[0]), 16),
            ("BLTBMOD", u32::from(b.modulo[1]), 16),
            ("BLTCMOD", u32::from(b.modulo[2]), 16),
            ("BLTDMOD", u32::from(b.modulo[3]), 16),
            ("BLTADAT", u32::from(b.adat), 16),
            ("BLTBDAT", u32::from(b.bdat), 16),
            ("BLTCDAT", u32::from(b.cdat), 16),
            ("BUSY", u32::from(b.busy), 1),
            ("ZERO", u32::from(b.zero), 1),
        ])
    }

    fn chip_peek(&self, addr: u32, size: Size) -> u32 {
        let chip = &self.board.mem.chip;
        (0..size.bytes()).fold(0, |v, i| {
            v << 8 | u32::from(chip.byte(addr.wrapping_add(i)))
        })
    }
}

impl DebugTarget for Amiga {
    fn regs(&self) -> RegisterSet {
        match self.debug_dev {
            COPPER => self.copper_regs(),
            BLITTER => self.blitter_regs(),
            _ => self.cpu.regs(),
        }
    }

    /// Copper: PC (restarts fetching there), COP1LC, COP2LC, DANGER.
    /// Blitter: its registers by name (written like the CPU would).
    fn set_reg(&mut self, name: &str, v: u32) {
        let upper = name.to_ascii_uppercase();
        let mask = self.board.ptr_mask;
        match self.debug_dev {
            COPPER => {
                let c = &mut self.board.custom.copper;
                match upper.as_str() {
                    "PC" => {
                        c.pc = v & mask;
                        c.state = chips_amiga::copper::State::Fetch1;
                    }
                    "COP1LC" => c.cop1lc = v & mask,
                    "COP2LC" => c.cop2lc = v & mask,
                    "DANGER" => c.danger = v != 0,
                    _ => {}
                }
            }
            BLITTER => {
                let custom = &mut self.board.custom;
                if let Some(ptr) = upper.strip_suffix("PT") {
                    // BLTxPT: high and low word.
                    if let Some(off) = chips_amiga::regnames::offset(&format!("{ptr}PTH")) {
                        custom.write(off, (v >> 16) as u16);
                        custom.write(off + 2, v as u16);
                    }
                } else if let Some(off) = chips_amiga::regnames::offset(&upper) {
                    if (0x040..0x080).contains(&off) && off != 0x058 {
                        custom.write(off, v as u16);
                    }
                }
            }
            _ => {
                self.cpu.set_reg(name, v);
                if name.eq_ignore_ascii_case("pc") {
                    self.cpu.refill_queue(&PeekBus(&self.board));
                }
            }
        }
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        match space {
            CPU_SPACE => Some(peek(&self.board, addr, size)),
            CHIP_SPACE | COPPER_SPACE => Some(self.chip_peek(addr, size)),
            _ => None,
        }
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        match space {
            CPU_SPACE => {
                if self.board.mem.region(addr, true) == Region::Rom {
                    return false;
                }
                self.board.write(addr, size, v);
                true
            }
            CHIP_SPACE | COPPER_SPACE => {
                let n = size.bytes();
                for i in 0..n {
                    self.board
                        .mem
                        .chip
                        .set_byte(addr.wrapping_add(i), (v >> (8 * (n - 1 - i))) as u8);
                }
                true
            }
            _ => false,
        }
    }

    /// CPU: 68000 code in its address space; Copper: a Copper instruction
    /// in chip RAM; Blitter: the chip RAM word (the Blitter has no code).
    fn disassemble(&self, addr: u32) -> Instr {
        match self.debug_dev {
            COPPER => {
                let addr = addr & self.board.ptr_mask;
                let (ir1, ir2) = (
                    self.chip_peek(addr, Size::Word) as u16,
                    self.chip_peek(addr + 2, Size::Word) as u16,
                );
                let (mnemonic, operand) = chips_amiga::copper::disassemble(ir1, ir2);
                Instr {
                    addr,
                    bytes: [ir1.to_be_bytes(), ir2.to_be_bytes()].concat(),
                    mnemonic,
                    operand,
                    target: None,
                    flow: emu_core::Flow::Normal,
                }
            }
            BLITTER => {
                let w = self.chip_peek(addr, Size::Word) as u16;
                Instr {
                    addr,
                    bytes: w.to_be_bytes().to_vec(),
                    mnemonic: "DC.W".into(),
                    operand: format!("${w:04X}"),
                    target: None,
                    flow: emu_core::Flow::Normal,
                }
            }
            _ => self.cpu.disassemble(&PeekBus(&self.board), addr),
        }
    }
}

#[cfg(test)]
mod tests;
