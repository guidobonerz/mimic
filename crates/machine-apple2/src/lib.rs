//! Apple II, II+ and IIe (Apple Computer, 1977–1985).
//!
//! A 6502 (the enhanced IIe: 65C02) at 1.0205 MHz, RAM shared with the
//! video scanner, a ROM with the monitor and BASIC, a keyboard encoder, a
//! one-bit speaker, game I/O (paddles, push buttons, annunciators), a
//! cassette interface and seven expansion slots. This machine has the
//! language card (slot 0 on the II/II+, built in on the IIe), a Disk II
//! card with two drives in slot 6 and a Mockingboard in slot 4.
//!
//! **Timing.** The 14.318 MHz master clock gives the CPU cycle (14
//! periods, the 65th of a line 16) and the dot clock. Every CPU cycle the
//! video scanner reads a byte ([`video`]); the CPU runs in the other half
//! of the cycle. 65 cycles a line, 262 lines a frame (NTSC, 59.92 Hz).
//!
//! **Memory map** ([`mem`]): RAM $0000–$BFFF, I/O $C000–$C0FF, slot ROMs
//! $C100–$C7FF (slot n at $Cn00), the shared expansion ROM space
//! $C800–$CFFF (owned by the slot whose $Cn00 page was accessed last,
//! released by an access to $CFFF), ROM or language card RAM $D000–$FFFF.
//!
//! **I/O** (Apple II Reference Manual, "Input/Output"; Apple IIe Technical
//! Reference Manual, chapter 2 and appendix "Soft Switches"):
//!
//! | Address     | Function                                              |
//! | ----------- | ----------------------------------------------------- |
//! | $C000       | keyboard data (strobe in bit 7); IIe writes $C000–$C00F: 80STORE, RAMRD, RAMWRT, INTCXROM, ALTZP, SLOTC3ROM, 80COL, ALTCHARSET off/on |
//! | $C010       | clear the keyboard strobe (IIe read: bit 7 any key down) |
//! | $C011–$C01F | IIe status in bit 7: BSRBANK2, BSRREADRAM, RAMRD, RAMWRT, INTCXROM, ALTZP, SLOTC3ROM, 80STORE, /VBL, TEXT, MIXED, PAGE2, HIRES, ALTCHARSET, 80COL |
//! | $C020       | toggle the cassette output                            |
//! | $C030       | toggle the speaker                                    |
//! | $C040       | utility strobe                                        |
//! | $C050–$C057 | graphics/TEXT, full/MIXED, PAGE1/PAGE2, lo-res/HIRES  |
//! | $C058–$C05F | annunciators AN0–AN3 off/on (IIe: AN3 off = double hi-res) |
//! | $C060       | cassette input (bit 7)                                |
//! | $C061–$C063 | push buttons PB0–PB2 (IIe: the Apple keys too)        |
//! | $C064–$C067 | paddle timers PDL0–PDL3 (bit 7 high while running)    |
//! | $C070       | start the paddle timers                               |
//! | $C080–$C08F | language card                                         |
//! | $C090–$C0FF | slot devices (slot n at $C080 + n·16)                 |
//!
//! Reads of addresses nobody drives return the byte the video scanner
//! fetched in the same cycle (the soft switches put nothing on the bus),
//! except bit 7 where an input answers.
//!
//! **Speaker.** Each access to $C030 flips the speaker's coil; the output
//! goes through a high-pass filter like the speaker's coupling. The
//! paddle timers run 11 cycles per paddle count, 0–255 over the 150 kΩ
//! potentiometer ([`PADDLE_CYCLES`]: the monitor's PREAD loop counts once
//! every 11 cycles).
//!
//! **Models** (`--model`): `iie` (enhanced IIe, 65C02, default),
//! `iie-unenhanced`, `iiplus` (Applesoft and the autostart monitor), `ii`
//! (Integer BASIC and the original monitor). Options: `ram` = 4k … 48k
//! (II/II+, default 48k), `lc` = on/off (language card, II/II+, default
//! on), `aux` = 64k/1k/none (IIe auxiliary memory, default 64k), `slot4`
//! = mockingboard/none, `slot6` = disk2/none, `port` = joystick, paddles
//! or none, `roms` = stub ([`stub`]).
//!
//! ROMs (user-supplied, `media-backends::roms`): the system ROM (II: four
//! 2 KB chips 341-0001…0004 at $E000; II+: six 2 KB chips 341-0011…0015
//! and 341-0020 or one 12 KB image; IIe: 342-0135 (CD) and 342-0134 (EF),
//! enhanced 342-0304 and 342-0303), the character generator (341-0036;
//! IIe 342-0133, enhanced 342-0265; this project's font without one), the
//! Disk II boot PROM (P5, 341-0027) and the sequencer PROM (P6, 341-0028).
//!
//! Sources: Jim Sather, "Understanding the Apple II" (1983) and
//! "Understanding the Apple IIe" (1985); Apple II Reference Manual (1979);
//! Apple IIe Technical Reference Manual (1985). The implementation is this
//! project's own.

#![forbid(unsafe_code)]

pub mod font;
pub mod gfx;
pub mod keyboard;
pub mod mem;
pub mod mockingboard;
pub mod overlay;
pub mod stub;
pub mod video;

#[cfg(test)]
mod tests;

use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use drives_other::apple_tape::AppleTape;
use drives_other::disk2::Disk2;
use emu_core::audio::Decimator;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use keyboard::Keyboard;
use mem::{Aux, Mem, Target};
use mockingboard::Mockingboard;
use video::{Charset, Modes, Video};

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's view of memory.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// Main and auxiliary RAM as stored (64 KB each, language card banks at
/// $C000–$FFFF, see [`mem`]).
pub const MAIN_SPACE: MemSpaceId = MemSpaceId(1);
pub const AUX_SPACE: MemSpaceId = MemSpaceId(2);
pub const SLOT_DRIVE1: SlotId = SlotId(0);
pub const SLOT_DRIVE2: SlotId = SlotId(1);
pub const SLOT_CASSETTE: SlotId = SlotId(2);
pub const GAME_PORT: PortId = PortId(0);

/// CPU clock: 14.318 MHz × 65 / 912.
pub const CPU_HZ: u32 = 1_020_484;
/// CPU cycles per paddle count.
pub const PADDLE_CYCLES: u64 = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Ii,
    IiPlus,
    IIe,
    IIeEnhanced,
}

impl Model {
    pub fn iie(self) -> bool {
        matches!(self, Model::IIe | Model::IIeEnhanced)
    }
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "apple2",
        name: "Apple II",
        models: &["iie", "iie-unenhanced", "iiplus", "ii"],
        create: |config| Ok(Box::new(Apple2::new(config)?)),
    }
}

fn concat(config: &MachineConfig, roles: &[&str], size: usize) -> Result<Vec<u8>, ConfigError> {
    let mut out = Vec::new();
    for role in roles {
        let r = config
            .roms
            .get(*role)
            .ok_or_else(|| ConfigError::MissingRom((*role).into()))?;
        if r.len() != size {
            return Err(ConfigError::Invalid(format!(
                "ROM {role} must be {size} bytes, got {}",
                r.len()
            )));
        }
        out.extend_from_slice(r);
    }
    Ok(out)
}

fn system_rom(model: Model, config: &MachineConfig, stub: bool) -> Result<Vec<u8>, ConfigError> {
    if stub {
        return Ok(stub::rom(if model.iie() { 0x4000 } else { 0x3000 }));
    }
    match model {
        Model::Ii => concat(
            config,
            &["apple2-e0", "apple2-e8", "apple2-f0", "apple2-f8"],
            0x800,
        ),
        Model::IiPlus => match config.roms.get("apple2plus") {
            Some(r) if r.len() == 0x3000 => Ok(r.clone()),
            _ => concat(
                config,
                &[
                    "apple2plus-d0",
                    "apple2plus-d8",
                    "apple2plus-e0",
                    "apple2plus-e8",
                    "apple2plus-f0",
                    "apple2plus-f8",
                ],
                0x800,
            ),
        },
        Model::IIe => concat(config, &["apple2e-cd", "apple2e-ef"], 0x2000),
        Model::IIeEnhanced => concat(config, &["apple2ee-cd", "apple2ee-ef"], 0x2000),
    }
}

fn option<'a>(config: &'a MachineConfig, key: &str, default: &'a str) -> &'a str {
    config.options.get(key).map_or(default, String::as_str)
}

fn invalid(key: &str, value: &str, choices: &str) -> ConfigError {
    ConfigError::Invalid(format!("{key} option '{value}' ({choices})"))
}

/// Everything on the bus except the CPU and the game port device.
struct Board {
    model: Model,
    mem: Mem,
    video: Video,
    modes: Modes,
    /// Annunciators AN0–AN3.
    an: u8,
    kbd: Keyboard,
    disk: Option<Disk2>,
    /// The Disk II boot PROM (P5) for slot 6.
    disk_rom: Option<Vec<u8>>,
    mb: Option<Mockingboard>,
    /// The slot owning $C800–$CFFF (0 = none).
    c8_owner: u8,
    /// The byte the video scanner read this cycle.
    float: u8,
    cycle: u64,
    speaker: bool,
    cassette_out: bool,
    tape: AppleTape,
    paddle_trigger: u64,
    frame_done: bool,
    /// A RESET key press to act on after the cycle.
    reset_pressed: bool,
    audio: [Decimator; 2],
    /// High-pass filter state (input, output) per channel.
    dc: [[i32; 2]; 2],
    samples: Vec<i16>,
}

fn lines(port: &Option<Box<dyn PortDevice>>, now: u64) -> PortLines {
    port.as_ref().map_or(
        PortLines {
            pulled_low: 0,
            analog: [0xffff; 2],
        },
        |d| d.read_lines(Cycle(now)),
    )
}

impl Board {
    fn iie(&self) -> bool {
        self.model.iie()
    }

    fn charset(model: Model) -> Charset {
        match model {
            Model::Ii | Model::IiPlus => Charset::Ii,
            Model::IIe => Charset::IIe,
            Model::IIeEnhanced => Charset::IIeEnhanced,
        }
    }

    /// Bit 7 of the game inputs ($C060–$C067); the cassette input is
    /// [`AppleTape::read`] when the program reads it.
    fn game_input(&self, n: u8, port: &Option<Box<dyn PortDevice>>) -> bool {
        let l = lines(port, self.cycle);
        match n & 7 {
            0 => self.tape.peek(),
            1 => l.pulled_low & 1 != 0 || (self.iie() && self.kbd.open_apple),
            2 => l.pulled_low & 2 != 0 || (self.iie() && self.kbd.solid_apple),
            3 => false,
            p => {
                let value = match p {
                    4 | 5 if port.is_some() => u64::from(l.analog[usize::from(p - 4)] >> 8),
                    // Nothing connected: the timer runs for the longest time.
                    _ => 255,
                };
                self.cycle < self.paddle_trigger + 2 + value * PADDLE_CYCLES
            }
        }
    }

    fn set_mode_switch(&mut self, lo: u8) {
        let on = lo & 1 != 0;
        match lo & 0x0f {
            0x0 | 0x1 => self.modes.text = on,
            0x2 | 0x3 => self.modes.mixed = on,
            0x4 | 0x5 => self.modes.page2 = on,
            0x6 | 0x7 => self.modes.hires = on,
            n => {
                let bit = 1 << ((n - 8) >> 1);
                if on {
                    self.an |= bit;
                } else {
                    self.an &= !bit;
                }
                if n >> 1 == 7 && self.iie() {
                    self.modes.dhires = !on;
                }
            }
        }
    }

    /// IIe: writes to $C000–$C00F.
    fn iie_switch(&mut self, lo: u8) {
        let on = lo & 1 != 0;
        let sw = &mut self.mem.sw;
        match lo & 0x0e {
            0x00 => self.modes.store80 = on,
            0x02 => sw.ramrd = on,
            0x04 => sw.ramwrt = on,
            0x06 => sw.intcxrom = on,
            0x08 => sw.altzp = on,
            0x0a => sw.slotc3rom = on,
            0x0c => self.modes.col80 = on,
            _ => self.modes.altchar = on,
        }
    }

    /// IIe status bit 7 of $C011–$C01F.
    fn iie_status(&self, lo: u8) -> bool {
        let sw = &self.mem.sw;
        match lo {
            0x11 => !self.mem.lc.bank1,
            0x12 => self.mem.lc.read_ram,
            0x13 => sw.ramrd,
            0x14 => sw.ramwrt,
            0x15 => sw.intcxrom,
            0x16 => sw.altzp,
            0x17 => sw.slotc3rom,
            0x18 => self.modes.store80,
            0x19 => !self.video.vbl(),
            0x1a => self.modes.text,
            0x1b => self.modes.mixed,
            0x1c => self.modes.page2,
            0x1d => self.modes.hires,
            0x1e => self.modes.altchar,
            _ => self.modes.col80,
        }
    }

    /// An access to $C000–$C0FF. `write` is the value of a write cycle;
    /// `peek` reads without side effects.
    fn io(
        &mut self,
        addr: u16,
        write: Option<u8>,
        peek: bool,
        port: &Option<Box<dyn PortDevice>>,
    ) -> u8 {
        let lo = addr as u8;
        let float = self.float;
        let bit7 = |b: bool| u8::from(b) << 7 | (float & 0x7f);
        match lo {
            0x00..=0x0f => {
                if let (true, Some(_)) = (self.iie(), write) {
                    if !peek {
                        self.iie_switch(lo);
                    }
                    float
                } else {
                    self.kbd.latch
                }
            }
            0x10..=0x1f => {
                let v = if !self.iie() {
                    float
                } else if lo == 0x10 {
                    u8::from(self.kbd.any_down()) << 7 | (self.kbd.latch & 0x7f)
                } else {
                    u8::from(self.iie_status(lo)) << 7 | (self.kbd.latch & 0x7f)
                };
                if !peek && (lo == 0x10 || !self.iie()) {
                    self.kbd.clear_strobe();
                }
                v
            }
            0x20..=0x2f => {
                if !peek {
                    self.cassette_out = !self.cassette_out;
                    self.tape.toggle_output(self.cycle);
                }
                float
            }
            0x30..=0x3f => {
                if !peek {
                    self.speaker = !self.speaker;
                }
                float
            }
            0x40..=0x4f => float,
            0x50..=0x5f => {
                if !peek {
                    self.set_mode_switch(lo);
                }
                float
            }
            0x60..=0x6f => {
                if lo & 7 == 0 && !peek {
                    self.tape.read(self.cycle);
                }
                bit7(self.game_input(lo, port))
            }
            0x70..=0x7f => {
                if !peek {
                    self.paddle_trigger = self.cycle;
                }
                float
            }
            0x80..=0x8f => {
                if self.mem.lc_present && !peek {
                    self.mem.lc.access(lo & 0x0f, write.is_some());
                }
                float
            }
            _ => {
                let slot = (lo >> 4) - 8;
                match (slot, &mut self.disk) {
                    (6, Some(d)) if peek => {
                        if lo & 1 == 0 {
                            d.peek()
                        } else {
                            float
                        }
                    }
                    (6, Some(d)) => d.access(lo & 0x0f, write).unwrap_or(float),
                    _ => float,
                }
            }
        }
    }

    /// An access to $C100–$CFFF.
    fn slot_space(&mut self, addr: u16, write: Option<u8>, peek: bool) -> u8 {
        let float = self.float;
        let internal = |b: &Board| match b.mem.rom_target(addr) {
            t @ Target::Rom(_) => b.mem.get(t).unwrap_or(float),
            _ => float,
        };
        let page = (addr >> 8) as u8;
        let v = if self.iie() && self.mem.sw.intcxrom {
            internal(self)
        } else if self.iie() && page == 0xc3 && !self.mem.sw.slotc3rom {
            if !peek {
                self.mem.sw.intc8rom = true;
            }
            internal(self)
        } else if page >= 0xc8 {
            if self.iie() && self.mem.sw.intc8rom {
                internal(self)
            } else {
                // None of the cards here has an expansion ROM.
                float
            }
        } else {
            let slot = page & 7;
            if !peek {
                self.c8_owner = slot;
            }
            match slot {
                6 => match (&self.disk, &self.disk_rom) {
                    (Some(_), Some(rom)) => rom[usize::from(addr as u8)],
                    _ => float,
                },
                4 => match &mut self.mb {
                    Some(mb) if peek => mb.peek(addr as u8),
                    Some(mb) => match write {
                        Some(v) => {
                            mb.write(addr as u8, v);
                            float
                        }
                        None => mb.read(addr as u8),
                    },
                    None => float,
                },
                _ => float,
            }
        };
        if addr == 0xcfff && !peek {
            self.c8_owner = 0;
            self.mem.sw.intc8rom = false;
        }
        v
    }

    fn read(&mut self, addr: u16, port: &Option<Box<dyn PortDevice>>) -> u8 {
        match addr {
            0xc000..=0xc0ff => self.io(addr, None, false, port),
            0xc100..=0xcfff => self.slot_space(addr, None, false),
            _ => {
                let t = self.mem.read_target(addr, &self.modes);
                self.mem.get(t).unwrap_or(self.float)
            }
        }
    }

    fn write(&mut self, addr: u16, v: u8, port: &Option<Box<dyn PortDevice>>) {
        match addr {
            0xc000..=0xc0ff => {
                self.io(addr, Some(v), false, port);
            }
            0xc100..=0xcfff => {
                self.slot_space(addr, Some(v), false);
            }
            _ => {
                let t = self.mem.write_target(addr, &self.modes);
                self.mem.set(t, v);
            }
        }
    }

    /// Side-effect-free read (golden rule 2).
    fn peek(&self, addr: u16, port: &Option<Box<dyn PortDevice>>) -> u8 {
        match addr {
            0xc000..=0xc0ff => self.io_peek(addr, port),
            0xc100..=0xcfff => self.slot_peek(addr),
            _ => {
                let t = self.mem.read_target(addr, &self.modes);
                self.mem.get(t).unwrap_or(self.float)
            }
        }
    }

    /// What a read of $C000–$C0FF returns, without its side effects.
    fn io_peek(&self, addr: u16, port: &Option<Box<dyn PortDevice>>) -> u8 {
        let lo = addr as u8;
        let float = self.float;
        match lo {
            0x00..=0x0f => self.kbd.latch,
            0x10 if self.iie() => u8::from(self.kbd.any_down()) << 7 | (self.kbd.latch & 0x7f),
            0x11..=0x1f if self.iie() => {
                u8::from(self.iie_status(lo)) << 7 | (self.kbd.latch & 0x7f)
            }
            0x60..=0x6f => u8::from(self.game_input(lo, port)) << 7 | (float & 0x7f),
            0xe0..=0xef if lo & 1 == 0 => self.disk.as_ref().map_or(float, Disk2::peek),
            _ => float,
        }
    }

    /// What a read of $C100–$CFFF returns, without its side effects.
    fn slot_peek(&self, addr: u16) -> u8 {
        let float = self.float;
        let page = (addr >> 8) as u8;
        let internal = || match self.mem.rom_target(addr) {
            t @ Target::Rom(_) => self.mem.get(t).unwrap_or(float),
            _ => float,
        };
        if self.iie() && (self.mem.sw.intcxrom || (page == 0xc3 && !self.mem.sw.slotc3rom)) {
            return internal();
        }
        if page >= 0xc8 {
            return if self.iie() && self.mem.sw.intc8rom {
                internal()
            } else {
                float
            };
        }
        match page & 7 {
            6 => match (&self.disk, &self.disk_rom) {
                (Some(_), Some(rom)) => rom[usize::from(addr as u8)],
                _ => float,
            },
            4 => self.mb.as_ref().map_or(float, |mb| mb.peek(addr as u8)),
            _ => float,
        }
    }

    /// One cycle of everything but the CPU, after its bus access.
    fn tick(&mut self) {
        if let Some(d) = &mut self.disk {
            d.tick();
        }
        if let Some(mb) = &mut self.mb {
            mb.tick();
        }
        self.kbd.tick(self.cycle);
        self.tape.tick(self.cycle);
        self.sound();
        self.cycle += 1;
    }

    fn sound(&mut self) {
        let spk = if self.speaker { 6000 } else { -6000 };
        let (l, r) = match &self.mb {
            Some(mb) => {
                let [a, b] = mb.output();
                (spk + a / 3, spk + b / 3)
            }
            None => (spk, spk),
        };
        let channels = if self.mb.is_some() { 2 } else { 1 };
        for (ch, level) in [l, r].into_iter().enumerate().take(channels) {
            if let Some(s) = self.audio[ch].push(level) {
                // First-order high-pass (the speaker's coupling, ~20 Hz).
                let x = i32::from(s) << 16;
                let [px, py] = self.dc[ch];
                let y = x - px + ((i64::from(py) * 65_264) >> 16) as i32;
                self.dc[ch] = [x, y];
                self.samples
                    .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
            }
        }
    }
}

/// The CPU's bus for one cycle.
struct CpuBus<'a> {
    b: &'a mut Board,
    port: &'a Option<Box<dyn PortDevice>>,
}

impl Bus for CpuBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        u32::from(self.b.read(addr as u16, self.port))
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.b.peek(addr as u16, self.port))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        self.b.write(addr as u16, v as u8, self.port);
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

struct PeekBus<'a> {
    b: &'a Board,
    port: &'a Option<Box<dyn PortDevice>>,
}

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        u32::from(self.b.peek(addr as u16, self.port))
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.b.peek(addr as u16, self.port))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

pub struct Apple2 {
    model: Model,
    cpu: Cpu6502,
    board: Board,
    port: Option<Box<dyn PortDevice>>,
    input: InputQueue,
    frame_done_pending: bool,
    palette: [Rgb8; 2],
    /// Debug overlay record (not machine state, see [`overlay`]).
    modes_rec: overlay::ModesRecord,
}

impl Apple2 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("iie") | Some("iie-enhanced") => Model::IIeEnhanced,
            Some("iie-unenhanced") => Model::IIe,
            Some("iiplus") | Some("ii+") => Model::IiPlus,
            Some("ii") => Model::Ii,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let stub = match config.options.get("roms").map(String::as_str) {
            None => false,
            Some("stub") => true,
            Some(other) => return Err(invalid("roms", other, "stub")),
        };
        let rom = system_rom(model, config, stub)?;
        let ram_size = if model.iie() {
            0xc000
        } else {
            match option(config, "ram", "48k") {
                "4k" => 0x1000,
                "8k" => 0x2000,
                "12k" => 0x3000,
                "16k" => 0x4000,
                "20k" => 0x5000,
                "24k" => 0x6000,
                "32k" => 0x8000,
                "36k" => 0x9000,
                "48k" => 0xc000,
                other => {
                    return Err(invalid(
                        "ram",
                        other,
                        "4k, 8k, 12k, 16k, 20k, 24k, 32k, 36k or 48k",
                    ))
                }
            }
        };
        let lc = model.iie()
            || match option(config, "lc", "on") {
                "on" => true,
                "off" => false,
                other => return Err(invalid("lc", other, "on or off")),
            };
        let aux = if model.iie() {
            match option(config, "aux", "64k") {
                "64k" => Aux::Ext64k,
                "1k" => Aux::Text1k,
                "none" => Aux::None,
                other => return Err(invalid("aux", other, "64k, 1k or none")),
            }
        } else {
            Aux::None
        };
        let mb = match option(config, "slot4", "mockingboard") {
            "mockingboard" => Some(Mockingboard::new()),
            "none" => None,
            other => return Err(invalid("slot4", other, "mockingboard or none")),
        };
        let disk = match option(config, "slot6", "disk2") {
            "disk2" => Some(Disk2::new(config.roms.get("disk2-p6").map(Vec::as_slice))),
            "none" => None,
            other => return Err(invalid("slot6", other, "disk2 or none")),
        };
        let disk_rom = config.roms.get("disk2").filter(|r| r.len() == 256).cloned();
        let port: Option<Box<dyn PortDevice>> = match option(config, "port", "joystick") {
            "joystick" => Some(Box::new(devices_input::AppleJoystick::new(
                devices_input::AppleKind::Joystick,
            ))),
            "paddles" => Some(Box::new(devices_input::AppleJoystick::new(
                devices_input::AppleKind::Paddles,
            ))),
            "none" => None,
            other => return Err(invalid("port", other, "joystick, paddles or none")),
        };
        let char_role = match model {
            Model::Ii | Model::IiPlus => ("apple2-video", 0x800),
            Model::IIe => ("apple2e-video", 0x1000),
            Model::IIeEnhanced => ("apple2ee-video", 0x1000),
        };
        let mut video = Video::new(Board::charset(model));
        video.char_rom = config
            .roms
            .get(char_role.0)
            .filter(|r| r.len() == char_role.1 && !stub)
            .cloned();
        let variant = if model == Model::IIeEnhanced {
            Variant::Cmos65C02
        } else {
            Variant::Nmos6502
        };
        let audio = Decimator::new(CPU_HZ, emu_core::SAMPLE_RATE)
            .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?;
        let mut m = Self {
            model,
            cpu: Cpu6502::new(CpuConfig {
                variant,
                ..CpuConfig::default()
            }),
            board: Board {
                model,
                mem: Mem::new(model.iie(), ram_size, lc, aux, rom),
                video,
                modes: Modes::default(),
                an: 0,
                kbd: Keyboard::new(model.iie()),
                disk,
                disk_rom,
                mb,
                c8_owner: 0,
                float: 0,
                cycle: 0,
                speaker: false,
                cassette_out: false,
                tape: AppleTape::new(CPU_HZ),
                paddle_trigger: 0,
                frame_done: false,
                reset_pressed: false,
                audio: [audio; 2],
                dc: [[0; 2]; 2],
                samples: Vec::new(),
            },
            port,
            input: InputQueue::new(),
            frame_done_pending: false,
            modes_rec: overlay::ModesRecord::default(),
            palette: [
                Rgb8 { r: 0, g: 0, b: 0 },
                Rgb8 {
                    r: 255,
                    g: 255,
                    b: 255,
                },
            ],
        };
        m.power_on();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn power_on(&mut self) {
        let b = &mut self.board;
        b.mem.power_on();
        b.modes = Modes::default();
        b.an = 0;
        b.c8_owner = 0;
        b.kbd = Keyboard::new(b.iie());
        if let Some(d) = &mut b.disk {
            d.reset();
        }
        if let Some(mb) = &mut b.mb {
            mb.reset();
        }
        self.cpu.reset();
    }

    /// The RESET key (CONTROL-RESET on the IIe): the CPU restarts; the
    /// IIe also resets its memory and display switches, every model the
    /// language card and the cards in the slots.
    fn reset_key(&mut self) {
        let b = &mut self.board;
        b.mem.lc = mem::LangCard::reset();
        if b.iie() {
            b.mem.sw = mem::IIeSwitches::default();
            b.modes.store80 = false;
            b.modes.col80 = false;
            b.modes.altchar = false;
        }
        b.c8_owner = 0;
        if let Some(d) = &mut b.disk {
            d.reset();
        }
        if let Some(mb) = &mut b.mb {
            mb.reset();
        }
        self.cpu.reset();
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.video;
        BeamPos {
            frame: v.frame,
            line: v.line,
            cycle: v.h,
            // Frame column of the cycle's first dot; the horizontal
            // blanking before the picture maps to column 0.
            hpos: if v.h < video::FIRST_VISIBLE {
                0
            } else {
                (video::BORDER_X as usize
                    + video::PHASE_PX
                    + usize::from(v.h - video::FIRST_VISIBLE) * video::DOTS * video::PX_PER_DOT)
                    as u16
            },
        }
    }

    /// One CPU cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(Cycle(self.board.cycle)) {
            self.apply_input(ev);
        }
        if std::mem::take(&mut self.board.reset_pressed) {
            self.reset_key();
        }
        if self.modes_rec.on {
            let b = &self.board;
            let aux = b.mem.aux_kind != Aux::None;
            self.modes_rec.sample(&b.video, &b.modes, aux);
        }
        let b = &mut self.board;
        let (byte, frame) = b.video.cycle(&b.mem.main, b.mem.video_aux(), &b.modes);
        if frame && self.modes_rec.on {
            self.modes_rec.frame_done();
        }
        b.float = byte;
        b.frame_done |= frame;
        let beam = self.beam_pos();
        let result = {
            let mut bus = CpuBus {
                b: &mut self.board,
                port: &self.port,
            };
            let mut hooked = HookedBus {
                inner: &mut bus,
                dbg: dbg.as_deref_mut(),
                space: CPU_SPACE,
                beam,
            };
            self.cpu.tick(&mut hooked)
        };
        self.board.tick();
        let irq = self.board.mb.as_ref().is_some_and(Mockingboard::irq);
        self.cpu.set_irq(irq);
        let frame = std::mem::take(&mut self.board.frame_done);
        if let Some(dbg) = dbg {
            if let CycleResult::InstrDone { next_pc } = result {
                if let Some(kind) = self.cpu.take_interrupt() {
                    dbg.check_interrupt(kind);
                }
                if dbg.trace_instr_armed() {
                    dbg.trace_instr(CPU, next_pc, self.beam_pos());
                }
                if dbg.instr_step_armed() {
                    dbg.check_instr_step();
                }
                if dbg.exec_armed(CPU_SPACE) {
                    dbg.check_exec(CPU_SPACE, next_pc);
                }
            }
            if dbg.raster_armed() {
                let p = self.beam_pos();
                dbg.check_raster(p.line, p.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                self.frame_done_pending |= frame;
                return Some(stop);
            }
        }
        match result {
            CycleResult::Jammed => Some(StopReason::CpuJam {
                device: CPU,
                pc: u32::from(self.cpu.pc()),
            }),
            _ if frame => Some(StopReason::FrameDone),
            _ => None,
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match ev.target {
            InputTarget::Port(p) => {
                if p == GAME_PORT {
                    if let Some(dev) = &mut self.port {
                        dev.input(&ev.event);
                    }
                }
            }
            InputTarget::Keyboard => {
                if let InputEvent::Key { machine_key, down } = ev.event {
                    let now = self.board.cycle;
                    // The IIe needs CONTROL with RESET.
                    if self.board.kbd.key(machine_key.0, down, now)
                        && (!self.board.iie() || self.board.kbd.control())
                    {
                        self.board.reset_pressed = true;
                    }
                }
            }
            InputTarget::Panel => {
                if let InputEvent::Control { id, value: 1 } = ev.event {
                    match id.0 {
                        0 => self.reset(ResetKind::Hard),
                        1 => self.reset(ResetKind::Soft),
                        _ => {}
                    }
                }
            }
        }
    }

    /// A byte as the CPU sees it (tests and tools).
    pub fn peek_cpu(&self, addr: u16) -> u8 {
        self.board.peek(addr, &self.port)
    }

    /// Main RAM as stored (tests and tools).
    pub fn main_ram(&self) -> &[u8] {
        &self.board.mem.main
    }

    pub fn cpu(&self) -> &Cpu6502 {
        &self.cpu
    }

    pub fn modes(&self) -> Modes {
        self.board.modes
    }

    fn apply_frame_freezes(&mut self, dbg: &mut DebugHooks) {
        for f in dbg.per_frame_freezes() {
            if f.space != CPU_SPACE && f.space != MAIN_SPACE {
                continue;
            }
            for i in 0..f.size.bytes() {
                let a = (f.addr + i) as u16;
                let v = (f.value >> (8 * i)) as u8;
                if f.space == MAIN_SPACE {
                    self.board.mem.main[usize::from(a)] = v;
                } else if !(0xc000..0xd000).contains(&a) {
                    let t = self.board.mem.write_target(a, &self.board.modes);
                    self.board.mem.set(t, v);
                }
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: video::PIXEL_HZ,
            active_pixels: video::WIDTH,
            total_pixels: video::TOTAL_PIXELS,
            lines_per_field: u32::from(video::LINES),
            standard: VideoStandard::Ntsc,
            // Without the colour burst (text) monitors show black and white.
            output: if self.board.video.burst {
                OutputType::Composite
            } else {
                OutputType::Monochrome
            },
            field: FieldParity::Progressive,
        }
    }
}

impl Machine for Apple2 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.frame_done_pending) {
            return StopReason::FrameDone;
        }
        let p = self.beam_pos();
        if p.line == 0 && p.cycle == 0 {
            self.apply_frame_freezes(dbg);
        }
        loop {
            if let Some(stop) = self.cycle(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        match self.cycle(None) {
            Some(stop @ StopReason::CpuJam { .. }) => stop,
            _ => StopReason::StepDone,
        }
    }

    /// Hard: power cycle. Soft: the RESET key.
    fn reset(&mut self, kind: ResetKind) {
        if kind == ResetKind::Hard {
            self.power_on();
        } else {
            self.reset_key();
        }
        self.input.clear();
        self.frame_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: video::WIDTH,
            height: video::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.board.video.pixels(),
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn gfx_views(&self) -> Vec<emu_core::GfxViewInfo> {
        self.views()
    }

    /// See [`gfx`]: character generator, text page, lo-res, hi-res, colours.
    fn gfx_view(&self, id: &str, opts: &emu_core::GfxOptions) -> Option<emu_core::GfxView> {
        self.view(id, opts)
    }

    fn overlays(&self) -> Vec<emu_core::overlay::OverlayInfo> {
        vec![overlay::ModesRecord::info()]
    }

    fn set_overlays(&mut self, ids: &[&str]) {
        self.modes_rec.set(ids.contains(&overlay::MODES_ID));
    }

    /// See [`overlay`]: the video mode per line.
    fn overlay(&self, id: &str) -> Option<emu_core::overlay::Overlay> {
        match id {
            overlay::MODES_ID => self.modes_rec.overlay(self.board.video.line),
            _ => None,
        }
    }

    /// The 192 visible lines sit below the frame's top border.
    fn frame_row(&self, line: u16) -> Option<u32> {
        (line < video::ACTIVE_LINES).then(|| u32::from(line) + video::BORDER_Y)
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: if self.board.mb.is_some() { 2 } else { 1 },
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let mut v = vec![
            MemSpace {
                id: CPU_SPACE,
                name: "cpu",
                device: CPU,
                addr_bits: 16,
                big_endian: false,
            },
            MemSpace {
                id: MAIN_SPACE,
                name: "main",
                device: CPU,
                addr_bits: 16,
                big_endian: false,
            },
        ];
        if self.board.mem.aux_kind != Aux::None {
            v.push(MemSpace {
                id: AUX_SPACE,
                name: "aux",
                device: CPU,
                addr_bits: 16,
                big_endian: false,
            });
        }
        v
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let b = &self.board;
        let m = &b.modes;
        let sw = &b.mem.sw;
        let lc = &b.mem.lc;
        let mut chips = vec![
            ChipInfo {
                name: "video",
                regs: vec![
                    reg("LINE", u32::from(b.video.line), 9),
                    reg("CYCLE", u32::from(b.video.h), 7),
                    reg("TEXT", u32::from(m.text), 1),
                    reg("MIXED", u32::from(m.mixed), 1),
                    reg("PAGE2", u32::from(m.page2), 1),
                    reg("HIRES", u32::from(m.hires), 1),
                    reg("80COL", u32::from(m.col80), 1),
                    reg("80STORE", u32::from(m.store80), 1),
                    reg("ALTCHAR", u32::from(m.altchar), 1),
                    reg("DHIRES", u32::from(m.dhires), 1),
                    reg("AN", u32::from(b.an), 4),
                ],
            },
            ChipInfo {
                name: "memory",
                regs: vec![
                    reg("LCBANK1", u32::from(lc.bank1), 1),
                    reg("LCREAD", u32::from(lc.read_ram), 1),
                    reg("LCWRITE", u32::from(lc.write_ram), 1),
                    reg("RAMRD", u32::from(sw.ramrd), 1),
                    reg("RAMWRT", u32::from(sw.ramwrt), 1),
                    reg("ALTZP", u32::from(sw.altzp), 1),
                    reg("INTCXROM", u32::from(sw.intcxrom), 1),
                    reg("SLOTC3ROM", u32::from(sw.slotc3rom), 1),
                    reg("INTC8ROM", u32::from(sw.intc8rom), 1),
                    reg("C8SLOT", u32::from(b.c8_owner), 3),
                    reg("KBD", u32::from(b.kbd.latch), 8),
                ],
            },
        ];
        if let Some(d) = &b.disk {
            let drive = &d.drives[usize::from(d.drive2)];
            chips.push(ChipInfo {
                name: "disk2",
                regs: vec![
                    reg("PHASES", u32::from(d.phases), 4),
                    reg("MOTOR", u32::from(d.spinning()), 1),
                    reg("DRIVE", u32::from(d.drive2) + 1, 2),
                    reg("Q6", u32::from(d.q6), 1),
                    reg("Q7", u32::from(d.q7), 1),
                    reg("LATCH", u32::from(d.latch), 8),
                    reg("QTRACK", u32::from(drive.quarter), 8),
                    reg("BITPOS", drive.pos, 32),
                ],
            });
        }
        if let Some(mb) = &b.mb {
            for (n, name) in [(0usize, "mockingboard-ay1"), (1, "mockingboard-ay2")] {
                const AY: [&str; 16] = [
                    "AFINE", "ACOARSE", "BFINE", "BCOARSE", "CFINE", "CCOARSE", "NOISE", "MIXER",
                    "AVOL", "BVOL", "CVOL", "EFINE", "ECOARSE", "ESHAPE", "PORTA", "PORTB",
                ];
                chips.push(ChipInfo {
                    name,
                    regs: AY
                        .iter()
                        .zip(mb.ays[n].regs)
                        .map(|(r, v)| reg(*r, u32::from(v), 8))
                        .collect(),
                });
            }
            for (n, name, base) in [
                (0usize, "mockingboard-via1", 0xc400u32),
                (1, "mockingboard-via2", 0xc480),
            ] {
                const VIA: [&str; 16] = [
                    "ORB", "ORA", "DDRB", "DDRA", "T1CL", "T1CH", "T1LL", "T1LH", "T2CL", "T2CH",
                    "SR", "ACR", "PCR", "IFR", "IER", "ORA_NH",
                ];
                chips.push(ChipInfo {
                    name,
                    regs: VIA
                        .iter()
                        .enumerate()
                        .map(|(i, r)| ChipReg {
                            name: r,
                            addr: Some(base + i as u32),
                            value: u32::from(mb.vias[n].peek(i as u8, 0xff, 0xff)),
                            bits: 8,
                        })
                        .collect(),
                });
            }
        }
        chips
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == CPU).then_some(self as &mut dyn DebugTarget)
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        Cycle(self.board.cycle)
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        vec![
            emu_core::ControlInfo::button(0, "POWER", false),
            emu_core::ControlInfo::button(1, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        vec![PortInfo {
            id: GAME_PORT,
            kind: PortKind::AppleGame,
            name: "game I/O",
            connected: self.port.is_some(),
        }]
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        if port != GAME_PORT {
            return Err(PortError::NoSuchPort(port));
        }
        if dev.kind() != PortKind::AppleGame {
            return Err(PortError::Incompatible {
                port: PortKind::AppleGame,
                device: dev.kind(),
            });
        }
        if self.port.is_some() {
            return Err(PortError::Occupied(port));
        }
        self.port = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        if port == GAME_PORT {
            self.port.take()
        } else {
            None
        }
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut v = match &self.board.disk {
            Some(d) => vec![
                MediaSlotInfo {
                    id: SLOT_DRIVE1,
                    name: "S6D1",
                    kind: MediaKind::Disk,
                    inserted: d.drives[0].has_disk(),
                },
                MediaSlotInfo {
                    id: SLOT_DRIVE2,
                    name: "S6D2",
                    kind: MediaKind::Disk,
                    inserted: d.drives[1].has_disk(),
                },
            ],
            None => Vec::new(),
        };
        v.push(MediaSlotInfo {
            id: SLOT_CASSETTE,
            name: "cassette",
            kind: MediaKind::Tape,
            inserted: self.board.tape.has_tape(),
        });
        v
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        if slot == SLOT_CASSETTE {
            return Some(&mut self.board.tape as &mut dyn MediaSlot);
        }
        let d = self.board.disk.as_mut()?;
        match slot {
            SLOT_DRIVE1 => Some(&mut d.drives[0] as &mut dyn MediaSlot),
            SLOT_DRIVE2 => Some(&mut d.drives[1] as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"APL2", 1, |w| {
            w.u8(match self.model {
                Model::Ii => 0,
                Model::IiPlus => 1,
                Model::IIe => 2,
                Model::IIeEnhanced => 3,
            });
            w.bool(self.frame_done_pending);
            w.u8(b.modes.bits());
            w.u8(b.an);
            w.u8(b.c8_owner);
            w.u8(b.float);
            w.u64(b.cycle);
            w.bool(b.speaker);
            w.bool(b.cassette_out);
            w.u64(b.paddle_trigger);
            w.bool(b.frame_done);
            w.bool(b.reset_pressed);
            for a in &b.audio {
                a.save(w);
            }
            for d in b.dc.iter().flatten() {
                w.u32(*d as u32);
            }
            w.bool(b.disk.is_some());
            w.bool(b.mb.is_some());
        });
        self.cpu.save(&mut w);
        b.mem.save(&mut w);
        b.video.save(&mut w);
        b.kbd.save(&mut w);
        if let Some(d) = &b.disk {
            d.save(&mut w);
        }
        if let Some(mb) = &b.mb {
            mb.save(&mut w);
        }
        b.tape.save(&mut w);
        self.input.save(&mut w);
        w.chunk(*b"A2GP", 1, |w| {
            w.bytes(
                &self
                    .port
                    .as_ref()
                    .map(|d| d.save_state())
                    .unwrap_or_default(),
            );
        });
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"APL2", 1)?;
        let model = c.u8()?;
        let want = match self.model {
            Model::Ii => 0,
            Model::IiPlus => 1,
            Model::IIe => 2,
            Model::IIeEnhanced => 3,
        };
        if model != want {
            return Err(StateError::Invalid("apple2 model differs"));
        }
        let frame_done_pending = c.bool()?;
        let modes = Modes::from_bits(c.u8()?);
        let an = c.u8()? & 0x0f;
        let c8_owner = c.u8()? & 7;
        let float = c.u8()?;
        let cycle = c.u64()?;
        let speaker = c.bool()?;
        let cassette_out = c.bool()?;
        let paddle_trigger = c.u64()?;
        let frame_done = c.bool()?;
        let reset_pressed = c.bool()?;
        let mut audio = self.board.audio;
        for a in &mut audio {
            a.load(&mut c)?;
        }
        let mut dc = [[0i32; 2]; 2];
        for d in dc.iter_mut().flatten() {
            *d = c.u32()? as i32;
        }
        if c.bool()? != self.board.disk.is_some() || c.bool()? != self.board.mb.is_some() {
            return Err(StateError::Invalid("apple2 slot cards differ"));
        }
        let mut cpu = self.cpu.clone();
        cpu.load(&mut r)?;
        let mut mem = self.board.mem.clone();
        mem.load(&mut r)?;
        let mut video = self.board.video.clone();
        video.load(&mut r)?;
        let mut kbd = self.board.kbd.clone();
        kbd.load(&mut r)?;
        // The disk card's state goes in last (its disks stay), so check
        // it on a scratch card first.
        let disk_at = r.clone();
        if self.board.disk.is_some() {
            Disk2::new(None).load(&mut r)?;
        }
        let mut mb = self.board.mb.clone();
        if let Some(m) = &mut mb {
            m.load(&mut r)?;
        }
        let tape_at = r.clone();
        AppleTape::new(CPU_HZ).load(&mut r)?;
        let mut input = self.input.clone();
        input.load(&mut r)?;
        let (_, mut gp) = r.chunk(*b"A2GP", 1)?;
        let port_state = gp.bytes()?.to_vec();
        if self.port.is_none() != port_state.is_empty() {
            return Err(StateError::Invalid("game port device differs"));
        }
        if let Some(d) = &mut self.port {
            d.load_state(&port_state)?;
        }
        if let Some(d) = &mut self.board.disk {
            let mut dr = disk_at;
            d.load(&mut dr)?;
        }
        self.board.tape.load(&mut tape_at.clone())?;
        let b = &mut self.board;
        b.modes = modes;
        b.an = an;
        b.c8_owner = c8_owner;
        b.float = float;
        b.cycle = cycle;
        b.speaker = speaker;
        b.cassette_out = cassette_out;
        b.paddle_trigger = paddle_trigger;
        b.frame_done = frame_done;
        b.reset_pressed = reset_pressed;
        b.audio = audio;
        b.dc = dc;
        b.samples.clear();
        b.mem = mem;
        b.video = video;
        b.kbd = kbd;
        b.mb = mb;
        self.cpu = cpu;
        self.input = input;
        self.frame_done_pending = frame_done_pending;
        Ok(())
    }
}

impl DebugTarget for Apple2 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    /// Little-endian like the 6502.
    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u8> {
            let a16 = a as u16;
            match space {
                CPU_SPACE => Some(self.board.peek(a16, &self.port)),
                MAIN_SPACE => self.board.mem.main.get(usize::from(a16)).copied(),
                AUX_SPACE => self.board.mem.aux.get(usize::from(a16)).copied(),
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0u32, |v, i| {
            Some(v | u32::from(byte(addr.wrapping_add(i))?) << (8 * i))
        })
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i) as u16;
            let byte = (v >> (8 * i)) as u8;
            match space {
                CPU_SPACE if !(0xc000..0xd000).contains(&a) => {
                    let t = self.board.mem.write_target(a, &self.board.modes);
                    let t = match t {
                        Target::Open => self.board.mem.read_target(a, &self.board.modes),
                        t => t,
                    };
                    if matches!(t, Target::Rom(_) | Target::Open) {
                        return false;
                    }
                    self.board.mem.set(t, byte);
                }
                MAIN_SPACE => self.board.mem.main[usize::from(a)] = byte,
                AUX_SPACE if !self.board.mem.aux.is_empty() => {
                    self.board.mem.aux[usize::from(a)] = byte;
                }
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(
            &PeekBus {
                b: &self.board,
                port: &self.port,
            },
            addr,
        )
    }
}
