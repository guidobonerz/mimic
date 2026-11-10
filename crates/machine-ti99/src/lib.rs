//! Texas Instruments TI-99/4A (NTSC and PAL).
//!
//! TMS9900 at 3.0 MHz, the TMS9918A video display processor (PAL:
//! TMS9929A) with 16 KB VRAM, the SN76489 sound generator (the console's
//! SN94624 at the colour clock), the TMS9901 systems interface, the 8 KB
//! console ROM and GROM 0–2 (TI BASIC), 256 bytes of scratchpad RAM, the
//! cartridge port (ROM at `>6000`, GROMs from GROM address `>6000`) and the
//! 32 KB memory expansion of the Peripheral Expansion Box.
//!
//! **Memory** (the CPU addresses bytes, reads and writes words):
//! - `>0000`–`>1FFF` console ROM; `>8000`–`>83FF` scratchpad (256 bytes
//!   repeated four times). Both are 16 bits wide.
//! - `>2000`–`>3FFF`, `>A000`–`>FFFF` the 32 KB expansion (option `ram`);
//!   `>4000`–`>5FFF` the peripheral cards' ROMs (none yet); `>6000`–`>7FFF`
//!   the cartridge ROM (a write selects bank (address − `>6000`) / 2 of
//!   its 8 KB banks).
//! - The ports, on the data bus's high byte: `>8400` sound (write);
//!   `>8800` VDP data and `>8802` status (read), `>8C00` data and `>8C02`
//!   address/register (write); `>9000`/`>9400` speech (not fitted:
//!   reads 0); `>9800` GROM data and `>9802` address (read), `>9C00`
//!   data and `>9C02` address (write).
//! - Everything but the ROM and the scratchpad goes through the 8-bit
//!   multiplexer: 4 wait states per word access.
//!
//! **CRU**: the TMS9901 at bits `>000`–`>1FF` (repeating every 32). Its
//! INT2 is the VDP interrupt; INT3–INT10 are the keyboard rows, P2–P4 the
//! keyboard column, P5 the alpha lock line ([`keyboard`]). Its INTREQ is
//! the CPU's level-1 interrupt (the console wires IC0–IC3 to level 1).
//!
//! **Timing.** The CPU runs whole instructions; the chips catch up after
//! each one: the VDP's pixel clock (10.738635 MHz / 2) by a fraction of
//! the CPU clock, the sound generator at two thirds of it (3.579545 MHz),
//! the 9901 at the CPU clock. Interrupts are seen at instruction ends.
//!
//! Video formats (`--videoformat`): "ntsc" (default, TMS9918A, 262 lines)
//! and "pal" (TMS9929A, 313 lines). No models (the 99/4A only).
//!
//! Options:
//! - `roms`: "stub" runs the small built-in ROM ([`stub`]) without GROMs.
//! - `ram`: "32k" (default) or "none" — the memory expansion.
//! - `port1`, `port2`: "joystick" (default) or "none"; the two joysticks
//!   of the console's joystick connector.
//! - `disk`: "on" or "off" — the TI disk controller card ([`disk`]) with
//!   drives DSK1–DSK3 (media slots "DSK1"–"DSK3"); on by default when its
//!   DSR ROM (role "ti99disk") is there.
//! - `speech`: "on" or "off" — the Speech Synthesizer (TMS5220 with its
//!   TMS6100 speech ROM, role "ti99speech", 32 KB; `chips_ti::tms5220`),
//!   read at `>9000`, written at `>9400`; on by default when the ROM is
//!   there. Its 8 kHz samples are mixed into the sound.
//!
//! Media slot "cartridge": a dump `NAMEC.BIN` / `NAMED.BIN` / `NAMEG.BIN`
//! or the bundle of them ([`media::ticart`]). Inserting or removing one
//! powers the console up again.
//!
//! Media slot "cassette" (CS1): a WAV recording
//! ([`drives_other::apple_tape::AppleTape`] with motor control). The 9901's
//! P6 (CRU bit 22) switches the motor (on while driven high), P9 (bit 25)
//! is the output to the recorder, P11 (bit 27) the input from it. CS2
//! (P7) and the audio gate (P8) are not connected.
//!
//! ROMs: role "ti99rom" (8 KB console ROM) and "ti99grom" (24 KB, GROM 0–2
//! as three 8 KB windows), user-supplied, never in the repository.
//!
//! Sources: Texas Instruments, *TI-99/4A Console and Peripheral Expansion
//! System Technical Data* (1983): memory map, ports, CRU map; Thierry
//! Nouspikel, "The TI-99/4A Tech Pages" (architecture: the multiplexer's
//! wait states, the scratchpad mirror, the port decoding; keyboard; GROMs;
//! cartridge banking); the TMS9900, TMS9901, TMS9918A and SN76489 data
//! (see `cpu-tms9900` and `chips-ti`). The implementation is this
//! project's own.

#![forbid(unsafe_code)]

pub mod disk;
pub mod keyboard;
pub mod stub;

use chips_ti::{vdp, Grom, Sn76489, Tms5220, Tms9901, Tms9918, VdpModel};
use cpu_tms9900::{Tms9900, Tms9900Bus};
use devices_input::joystick::pin;
use disk::DiskCard;
use drives_other::apple_tape::AppleTape;
use emu_core::audio::Decimator;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::gfx::{GfxOptions, GfxView, GfxViewInfo};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::overlay::{Overlay, OverlayInfo};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use keyboard::{joy, Keyboard};
use media::ticart::TiCart;

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's view of memory.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The VDP's 16 KB of video RAM.
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(1);
/// The 64 KB GROM address space (console GROMs 0–2, cartridge 3–7).
pub const GROM_SPACE: MemSpaceId = MemSpaceId(2);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);
pub const SLOT_CASSETTE: SlotId = SlotId(1);
/// DSK1–DSK3 on the disk controller card.
pub const SLOT_DSK1: SlotId = SlotId(2);
const DSK_NAMES: [&str; disk::DRIVES] = ["DSK1", "DSK2", "DSK3"];
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const CPU_HZ: u32 = 3_000_000;
/// The VDP's crystal; its pixel clock is half of it.
const VDP_XTAL: u32 = 10_738_635;
/// The sound generator's input clock (two thirds of the pixel clock).
const PSG_HZ: u32 = 3_579_545;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "ti99",
        name: "TI-99/4A",
        models: &[],
        video_formats: emu_core::registry::PAL_NTSC,
        create: |config| Ok(Box::new(Ti99::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Ntsc,
    Pal,
}

/// The cartridge slot: the dumps by name, or a bundle.
#[derive(Default)]
struct CartSlotTi {
    cart: Option<TiCart>,
    media: Option<Media>,
    changed: bool,
}

impl MediaSlot for CartSlotTi {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Cartridge {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut data = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut data)?;
        self.cart = Some(TiCart::parse(&media.name, &data)?);
        self.media = Some(media);
        self.changed = true;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.cart = None;
        self.changed = true;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// The cartridge as the board uses it.
#[derive(Clone, Debug, Default)]
struct Cart {
    banks: Vec<Vec<u8>>,
    bank: usize,
}

impl Cart {
    fn word(&self, addr: u16) -> u16 {
        let b = &self.banks[self.bank];
        let o = usize::from(addr & 0x1ffe);
        match b.get(o..o + 2) {
            Some(w) => u16::from_be_bytes([w[0], w[1]]),
            None => 0,
        }
    }
}

/// Everything on the bus except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    rom: Vec<u8>,
    pad: Vec<u8>,
    /// The 32 KB expansion (`>2000`–`>3FFF` first, then `>A000`–`>FFFF`).
    exp: Option<Vec<u8>>,
    cart: Option<Cart>,
    grom: Grom,
    vdp: Tms9918,
    psg: Sn76489,
    pio: Tms9901,
    /// The Speech Synthesizer, its current sample and the cycles to the
    /// next one.
    speech: Option<Tms5220>,
    speech_level: i32,
    speech_t: u32,
    keyboard: Keyboard,
    /// The joysticks' rows (columns 6 and 7), from the port devices.
    joys: [u8; 2],
    /// CPU cycles the chips have run.
    chips_t: u64,
    /// Pixel clock phase (in VDP crystal ticks against 2 × CPU clock).
    pix_acc: u32,
    /// Pixel clocks: every third one the sound generator rests.
    pixels: u64,
    field_done: bool,
    audio: Decimator,
    dc: [i32; 2],
    samples: Vec<i16>,
}

fn word_of(mem: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([mem[o], mem[o + 1]])
}

fn set_word(mem: &mut [u8], o: usize, v: u16) {
    mem[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

/// The expansion offset of `addr`, if it is expansion RAM.
fn exp_offset(addr: u16) -> Option<usize> {
    match addr {
        0x2000..=0x3fff => Some(usize::from(addr - 0x2000)),
        0xa000..=0xffff => Some(usize::from(addr - 0xa000) + 0x2000),
        _ => None,
    }
}

impl Board {
    /// One CPU cycle of the chips.
    fn tick(&mut self) {
        self.pix_acc += VDP_XTAL;
        while self.pix_acc >= 2 * CPU_HZ {
            self.pix_acc -= 2 * CPU_HZ;
            self.field_done |= self.vdp.tick();
            if self.pixels % 3 != 2 {
                let level = self.psg.tick() + self.speech_level;
                if let Some(s) = self.audio.push(level - 16_382) {
                    let x = i32::from(s) << 16;
                    let y = x - self.dc[0] + ((i64::from(self.dc[1]) * 65_208) >> 16) as i32;
                    self.dc = [x, y];
                    self.samples
                        .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
                }
            }
            self.pixels += 1;
        }
        self.chips_t += 1;
        if let Some(sp) = &mut self.speech {
            self.speech_t += 1;
            if self.speech_t >= CPU_HZ / chips_ti::tms5220::SAMPLE_RATE {
                self.speech_t = 0;
                self.speech_level = i32::from(sp.sample()) / 2;
            }
        }
    }

    fn run_to(&mut self, to: u64) {
        let n = to.saturating_sub(self.chips_t);
        self.pio.tick(n as u32);
        while self.chips_t < to {
            self.tick();
        }
    }

    /// The keyboard rows of the selected column and the VDP interrupt as
    /// the 9901's interrupt inputs.
    fn update_pio(&mut self) {
        let col = u8::from(self.pio.pin(2))
            | u8::from(self.pio.pin(3)) << 1
            | u8::from(self.pio.pin(4)) << 2;
        let rows = self.keyboard.rows(col, self.joys, !self.pio.pin(5));
        let vdp = if self.vdp.int() { 1 << 2 } else { 0 };
        self.pio.set_interrupts(u16::from(rows) << 3 | vdp);
    }

    fn mem_read(&mut self, addr: u16) -> u16 {
        let a = addr & 0xfffe;
        match a {
            0x8800..=0x8bff => {
                u16::from(if a & 2 == 0 {
                    self.vdp.read_data()
                } else {
                    self.vdp.read_status()
                }) << 8
            }
            0x9800..=0x9bff => {
                u16::from(if a & 2 == 0 {
                    self.grom.read_data()
                } else {
                    self.grom.read_address()
                }) << 8
            }
            0x9000..=0x93ff => self.speech.as_mut().map_or(0, |s| u16::from(s.read()) << 8),
            _ => self.mem_peek(a),
        }
    }

    /// Side-effect-free read (the ports: what a read would see).
    fn mem_peek(&self, addr: u16) -> u16 {
        let a = addr & 0xfffe;
        match a {
            0x0000..=0x1fff => word_of(&self.rom, usize::from(a)),
            0x6000..=0x7fff => self.cart.as_ref().map_or(0, |c| c.word(a)),
            0x8000..=0x83ff => word_of(&self.pad, usize::from(a & 0xfe)),
            0x8800..=0x8bff => {
                let (data, status) = self.vdp.peek();
                u16::from(if a & 2 == 0 { data } else { status }) << 8
            }
            0x9800..=0x9bff => {
                u16::from(if a & 2 == 0 {
                    self.grom.peek_data()
                } else {
                    (self.grom.address() >> 8) as u8
                }) << 8
            }
            0x9000..=0x93ff => self.speech.as_ref().map_or(0, |s| u16::from(s.peek()) << 8),
            _ => match (exp_offset(a), &self.exp) {
                (Some(o), Some(m)) => word_of(m, o),
                _ => 0,
            },
        }
    }

    fn mem_write(&mut self, addr: u16, v: u16) {
        let a = addr & 0xfffe;
        let byte = (v >> 8) as u8;
        match a {
            0x6000..=0x7fff => {
                if let Some(c) = &mut self.cart {
                    c.bank = usize::from((a - 0x6000) >> 1) % c.banks.len();
                }
            }
            0x8000..=0x83ff => set_word(&mut self.pad, usize::from(a & 0xfe), v),
            0x8400..=0x87ff => self.psg.write(byte),
            0x9400..=0x97ff => {
                if let Some(s) = &mut self.speech {
                    s.write(byte);
                }
            }
            0x8c00..=0x8fff => {
                if a & 2 == 0 {
                    self.vdp.write_data(byte);
                } else {
                    self.vdp.write_control(byte);
                }
            }
            0x9c00..=0x9fff => {
                if a & 2 == 0 {
                    self.grom.write_data(byte);
                } else {
                    self.grom.write_address(byte);
                }
            }
            _ => {
                if let (Some(o), Some(m)) = (exp_offset(a), &mut self.exp) {
                    set_word(m, o, v);
                }
            }
        }
    }
}

/// The joystick rows of a port's lines.
fn joy_rows(dev: &Option<Box<dyn PortDevice>>, now: Cycle) -> u8 {
    let Some(d) = dev else {
        return 0;
    };
    let low = d.read_lines(now).pulled_low;
    let mut rows = 0;
    for (p, r) in [
        (pin::FIRE, joy::FIRE),
        (pin::LEFT, joy::LEFT),
        (pin::RIGHT, joy::RIGHT),
        (pin::DOWN, joy::DOWN),
        (pin::UP, joy::UP),
    ] {
        if low & p != 0 {
            rows |= r;
        }
    }
    rows
}

/// The CPU's bus for one instruction.
struct TiBus<'a> {
    b: &'a mut Board,
    ports: &'a [Option<Box<dyn PortDevice>>; 2],
    tape: &'a mut AppleTape,
    disk: &'a mut Option<DiskCard>,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl TiBus<'_> {
    /// The disk card, if it answers at `addr` now.
    fn card_at(&mut self, addr: u16) -> Option<&mut DiskCard> {
        self.disk
            .as_mut()
            .filter(|d| d.rom_on() && (0x4000..0x6000).contains(&addr))
    }
}

impl Bus for TiBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let a = addr as u16 & 0xfffe;
        let now = self.b.chips_t;
        let v = match self.card_at(a) {
            Some(card) => u32::from(card.read(a, now)),
            None => u32::from(self.b.mem_read(a)),
        };
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let seen = dbg.on_read(CPU_SPACE, addr, size, v);
                dbg.trace_read(CPU_SPACE, addr, size, seen, self.beam);
                seen
            }
            _ => v,
        }
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_word(self.b, self.disk.as_ref(), addr as u16))
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, size, v & 0xffff, self.beam);
                if !dbg.on_write(CPU_SPACE, addr, size, v & 0xffff) {
                    return;
                }
            }
        }
        let a = addr as u16 & 0xfffe;
        let now = self.b.chips_t;
        match self.card_at(a) {
            Some(card) => card.write(a, v as u16, now),
            None => self.b.mem_write(a, v as u16),
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl Tms9900Bus for TiBus<'_> {
    fn wait_states(&mut self, addr: u16) -> u32 {
        let now = self.b.chips_t;
        let hold = self.card_at(addr).map_or(0, |c| c.hold(addr, now));
        match addr {
            0x0000..=0x1fff | 0x8000..=0x83ff => 0,
            _ => 4 + hold,
        }
    }

    fn cru_read(&mut self, bit: u16) -> bool {
        if (disk::CRU_BASE..disk::CRU_BASE + 8).contains(&bit) {
            let now = self.b.chips_t;
            if let Some(d) = self.disk.as_ref() {
                return d.cru_read(bit, now);
            }
        }
        if bit >= 0x200 {
            return true;
        }
        let now = Cycle(self.b.chips_t);
        self.b.joys = [joy_rows(&self.ports[0], now), joy_rows(&self.ports[1], now)];
        self.b.update_pio();
        if bit & 31 == 27 {
            let level = self.tape.read(self.b.chips_t);
            self.b
                .pio
                .set_inputs(if level { 0xffff } else { !(1 << 11) });
        }
        self.b.pio.read_bit((bit & 31) as u8)
    }

    fn cru_write(&mut self, bit: u16, value: bool) {
        if (disk::CRU_BASE..disk::CRU_BASE + 8).contains(&bit) {
            let now = self.b.chips_t;
            if let Some(d) = self.disk.as_mut() {
                d.cru_write(bit, value, now);
            }
        }
        if bit < 0x200 {
            self.b.pio.write_bit((bit & 31) as u8, value);
            match bit & 31 {
                22 | 0 | 15 => self.tape.set_motor(cs1_motor(&self.b.pio)),
                25 => self.tape.set_output(self.b.pio.pin(9), self.b.chips_t),
                _ => {}
            }
        }
    }
}

/// The CS1 motor runs while the 9901 drives P6 high.
fn cs1_motor(pio: &Tms9901) -> bool {
    let (out, level) = pio.outputs();
    out & level & (1 << 6) != 0
}

/// A word as the CPU would read it, side-effect free, with the disk
/// card's ROM and registers.
fn peek_word(b: &Board, disk: Option<&DiskCard>, addr: u16) -> u16 {
    let a = addr & 0xfffe;
    match disk {
        Some(d) if d.rom_on() && (0x4000..0x6000).contains(&a) => d.peek(a, b.chips_t),
        _ => b.mem_peek(a),
    }
}

/// Side-effect-free CPU view.
struct PeekBus<'a>(&'a Board, Option<&'a DiskCard>);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }
    fn peek(&self, addr: u32, size: Size) -> u32 {
        let w = peek_word(self.0, self.1, addr as u16);
        match size {
            Size::Byte => u32::from(if addr & 1 == 0 { w >> 8 } else { w & 0xff }),
            _ => u32::from(w),
        }
    }
    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}
    fn addr_bits(&self) -> u8 {
        16
    }
}

pub struct Ti99 {
    model: Model,
    cpu: Tms9900,
    board: Board,
    ports: [Option<Box<dyn PortDevice>>; 2],
    cart: CartSlotTi,
    /// The cassette recorder CS1.
    tape: AppleTape,
    /// The disk controller card.
    disk: Option<DiskCard>,
    /// The console GROMs (0–2), kept for power cycles.
    console_grom: [Vec<u8>; 3],
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
    overlay_ids: Vec<&'static str>,
    overlay_fresh: bool,
}

fn port_option(
    config: &MachineConfig,
    key: &str,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match config.options.get(key).map(String::as_str) {
        None | Some("joystick") => Ok(Some(Box::new(devices_input::Joystick::new(
            PortKind::Ti99Joy,
        )))),
        Some("none") => Ok(None),
        Some(other) => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' (joystick or none)"
        ))),
    }
}

impl Ti99 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        if let Some(other) = config.model.as_deref() {
            return Err(ConfigError::UnknownModel(other.into()));
        }
        let model = match config.video_format {
            None | Some(VideoStandard::Ntsc) => Model::Ntsc,
            Some(VideoStandard::Pal) => Model::Pal,
        };
        let (rom, grom) = match config.options.get("roms").map(String::as_str) {
            Some("stub") => (stub::rom(), Vec::new()),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => {
                let get = |role: &str, len: usize| -> Result<Vec<u8>, ConfigError> {
                    let r = config
                        .roms
                        .get(role)
                        .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
                    if r.len() != len {
                        return Err(ConfigError::Invalid(format!(
                            "{role} must be {len} bytes, got {}",
                            r.len()
                        )));
                    }
                    Ok(r.clone())
                };
                (get("ti99rom", 0x2000)?, get("ti99grom", 0x6000)?)
            }
        };
        let exp = match config.options.get("ram").map(String::as_str) {
            None | Some("32k") => Some(vec![0; 0x8000]),
            Some("none") => None,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "ram option '{other}' (32k or none)"
                )))
            }
        };
        let disk = match config.options.get("disk").map(String::as_str) {
            Some("off") => None,
            Some("on") => Some(DiskCard::new(
                config
                    .roms
                    .get("ti99disk")
                    .cloned()
                    .ok_or_else(|| ConfigError::MissingRom("ti99disk".into()))?,
                CPU_HZ,
            )),
            None => config
                .roms
                .get("ti99disk")
                .filter(|_| !config.options.contains_key("roms"))
                .map(|r| DiskCard::new(r.clone(), CPU_HZ)),
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "disk option '{other}' (on or off)"
                )))
            }
        };
        let speech = match config.options.get("speech").map(String::as_str) {
            Some("off") => None,
            Some("on") => Some(Tms5220::new(
                config
                    .roms
                    .get("ti99speech")
                    .cloned()
                    .ok_or_else(|| ConfigError::MissingRom("ti99speech".into()))?,
            )),
            None => config
                .roms
                .get("ti99speech")
                .filter(|_| !config.options.contains_key("roms"))
                .map(|r| Tms5220::new(r.clone())),
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "speech option '{other}' (on or off)"
                )))
            }
        };
        let console_grom: [Vec<u8>; 3] = std::array::from_fn(|i| {
            grom.get(i * 0x2000..(i + 1) * 0x2000)
                .map_or_else(Vec::new, <[u8]>::to_vec)
        });
        let vdp_model = match model {
            Model::Ntsc => VdpModel::Ntsc,
            Model::Pal => VdpModel::Pal,
        };
        let mut g = Grom::new();
        for (i, c) in console_grom.iter().enumerate() {
            g.chips[i] = c.clone();
        }
        let mut m = Self {
            model,
            cpu: Tms9900::new(),
            board: Board {
                rom,
                pad: vec![0; 0x100],
                exp,
                cart: None,
                grom: g,
                vdp: Tms9918::new(vdp_model),
                psg: Sn76489::new(),
                pio: Tms9901::new(),
                speech,
                speech_level: 0,
                speech_t: 0,
                keyboard: Keyboard::default(),
                joys: [0; 2],
                chips_t: 0,
                pix_acc: 0,
                pixels: 0,
                field_done: false,
                audio: Decimator::new(PSG_HZ, emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                dc: [0; 2],
                samples: Vec::new(),
            },
            ports: [port_option(config, "port1")?, port_option(config, "port2")?],
            cart: CartSlotTi::default(),
            tape: AppleTape::with_motor_control(CPU_HZ),
            disk,
            console_grom,
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
            overlay_ids: Vec::new(),
            overlay_fresh: false,
        };
        m.power_on();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    /// Power-on state of the board with the inserted cartridge.
    fn power_on(&mut self) {
        let b = &mut self.board;
        b.pad.fill(0);
        if let Some(e) = &mut b.exp {
            e.fill(0);
        }
        b.vdp = Tms9918::new(b.vdp.model);
        b.vdp.set_overlays(&self.overlay_ids);
        b.psg = Sn76489::new();
        b.pio = Tms9901::new();
        if let Some(s) = &mut b.speech {
            s.reset();
        }
        b.speech_level = 0;
        b.speech_t = 0;
        let mut g = Grom::new();
        for (i, c) in self.console_grom.iter().enumerate() {
            g.chips[i] = c.clone();
        }
        b.cart = None;
        if let Some(c) = &self.cart.cart {
            for (i, chunk) in c.grom.chunks(0x2000).enumerate().take(5) {
                g.chips[3 + i] = chunk.to_vec();
            }
            b.cart = Some(Cart {
                banks: c.banks(),
                bank: 0,
            });
        }
        b.grom = g;
        self.cpu = Tms9900::new();
        self.tape.set_motor(false);
        if let Some(d) = &mut self.disk {
            d.reset();
        }
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.vdp;
        BeamPos {
            frame: v.fields,
            line: v.line,
            cycle: (u32::from(v.dot) * 2 * CPU_HZ / VDP_XTAL) as u16,
            hpos: v.dot.min(vdp::WIDTH as u16 - 1),
        }
    }

    /// One instruction (or context switch). `Some` when emulation stops.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if std::mem::take(&mut self.cart.changed) {
            self.power_on();
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let start = self.sched.now().0;
        let beam = self.beam_pos();
        let t = {
            let mut bus = TiBus {
                b: &mut self.board,
                ports: &self.ports,
                tape: &mut self.tape,
                disk: &mut self.disk,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step(&mut bus)
        };
        let end = start + u64::from(t);
        for c in start..end {
            self.tape.tick(c);
        }
        self.board.run_to(end);
        if let Some(d) = &mut self.disk {
            d.tick(end);
        }
        self.sched.advance(u64::from(t));
        self.board.update_pio();
        self.cpu.set_intreq(self.board.pio.intreq().then_some(1));
        let field = std::mem::take(&mut self.board.field_done);
        if field && self.board.vdp.overlays_on() {
            self.board.vdp.overlay_end_frame();
            self.overlay_fresh = true;
        }
        if let Some(dbg) = dbg {
            let next_pc = u32::from(self.cpu.pc);
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
                self.field_done_pending |= field;
                return Some(stop);
            }
        }
        field.then_some(StopReason::FrameDone)
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match ev.target {
            InputTarget::Port(p) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&ev.event);
                }
            }
            InputTarget::Keyboard => {
                if let InputEvent::Key { machine_key, down } = ev.event {
                    self.board.keyboard.key(machine_key.0, down);
                }
            }
            InputTarget::Panel => {
                if let InputEvent::Control { id, value: 1 } = ev.event {
                    if id.0 == 0 {
                        self.reset(ResetKind::Hard);
                    }
                }
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        let standard = match self.model {
            Model::Ntsc => VideoStandard::Ntsc,
            Model::Pal => VideoStandard::Pal,
        };
        SignalInfo {
            pixel_clock_hz: VDP_XTAL / 2,
            active_pixels: vdp::WIDTH,
            total_pixels: u32::from(vdp::LINE),
            lines_per_field: u32::from(self.board.vdp.model.lines()),
            standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

/// A word as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Ti99, addr: u16) -> u16 {
    peek_word(&m.board, m.disk.as_ref(), addr)
}

/// The VDP (tests and tools).
pub fn vdp(m: &Ti99) -> &Tms9918 {
    &m.board.vdp
}

impl Machine for Ti99 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        self.overlay_fresh = false;
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.overlay_fresh = false;
        self.instruction(None);
        StopReason::StepDone
    }

    /// Hard: power cycle. Soft: there is no reset button; the same.
    fn reset(&mut self, _kind: ResetKind) {
        self.power_on();
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: vdp::WIDTH,
            height: self.board.vdp.model.height(),
            pixels: PixelData::Indexed {
                pixels: self.board.vdp.frame(),
                palette: &vdp::PALETTE,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn gfx_views(&self) -> Vec<GfxViewInfo> {
        vdp::gfx::views(self.board.vdp.model.chip_name())
    }

    fn gfx_view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        self.board.vdp.gfx_view(id, VRAM_SPACE, opts)
    }

    fn overlays(&self) -> Vec<OverlayInfo> {
        vdp::overlay::infos(self.board.vdp.model.chip_name())
    }

    fn set_overlays(&mut self, ids: &[&str]) {
        self.overlay_ids = self
            .overlays()
            .into_iter()
            .map(|o| o.id)
            .filter(|id| ids.contains(id))
            .collect();
        self.board.vdp.set_overlays(ids);
    }

    fn overlay(&self, id: &str) -> Option<Overlay> {
        self.board.vdp.overlay(id, self.overlay_fresh)
    }

    fn frame_row(&self, line: u16) -> Option<u32> {
        self.board.vdp.line_row(line)
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
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
                addr_bits: 16,
                big_endian: true,
            },
            MemSpace {
                id: VRAM_SPACE,
                name: "vram",
                device: CPU,
                addr_bits: 14,
                big_endian: false,
            },
            MemSpace {
                id: GROM_SPACE,
                name: "grom",
                device: CPU,
                addr_bits: 16,
                big_endian: false,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let b = &self.board;
        let v = &b.vdp;
        let p = &b.psg;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        const R: [&str; 8] = ["R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7"];
        let mut vdp_regs: Vec<ChipReg> = R
            .iter()
            .zip(v.regs)
            .map(|(n, val)| reg(*n, u32::from(val), 8))
            .collect();
        vdp_regs.extend([
            reg("STATUS", u32::from(v.peek().1), 8),
            reg("ADDR", u32::from(v.addr), 14),
            reg("LINE", u32::from(v.line), 9),
            reg("DOT", u32::from(v.dot), 9),
        ]);
        let (dir, out) = b.pio.outputs();
        vec![
            ChipInfo {
                name: "vdp",
                regs: vdp_regs,
            },
            ChipInfo {
                name: "psg",
                regs: vec![
                    reg("TONE0", u32::from(p.period[0]), 10),
                    reg("TONE1", u32::from(p.period[1]), 10),
                    reg("TONE2", u32::from(p.period[2]), 10),
                    reg("NOISE", u32::from(p.noise), 3),
                    reg("ATT0", u32::from(p.atten[0]), 4),
                    reg("ATT1", u32::from(p.atten[1]), 4),
                    reg("ATT2", u32::from(p.atten[2]), 4),
                    reg("ATT3", u32::from(p.atten[3]), 4),
                ],
            },
            ChipInfo {
                name: "tms9901",
                regs: vec![
                    reg("OUTPUTS", u32::from(dir), 16),
                    reg("PINS", u32::from(out), 16),
                    reg("INTREQ", u32::from(b.pio.intreq()), 1),
                ],
            },
            ChipInfo {
                name: "speech",
                regs: vec![
                    reg(
                        "STATUS",
                        u32::from(b.speech.as_ref().map_or(0, |s| s.peek())),
                        8,
                    ),
                    reg(
                        "TALKING",
                        u32::from(b.speech.as_ref().is_some_and(|s| s.talking())),
                        1,
                    ),
                ],
            },
            ChipInfo {
                name: "grom",
                regs: vec![
                    reg("ADDR", u32::from(b.grom.address()), 16),
                    reg("BANK", b.cart.as_ref().map_or(0, |c| c.bank as u32), 8),
                ],
            },
        ]
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
        self.sched.now()
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        vec![emu_core::ControlInfo::button(0, "POWER", false)]
    }

    fn ports(&self) -> Vec<PortInfo> {
        [(PORT1, "joystick 1"), (PORT2, "joystick 2")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::Ti99Joy,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::Ti99Joy {
            return Err(PortError::Incompatible {
                port: PortKind::Ti99Joy,
                device: dev.kind(),
            });
        }
        if slot.is_some() {
            return Err(PortError::Occupied(port));
        }
        *slot = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        self.ports.get_mut(usize::from(port.0))?.take()
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut slots = vec![
            MediaSlotInfo {
                id: SLOT_CARTRIDGE,
                name: "cartridge",
                kind: MediaKind::Cartridge,
                inserted: self.cart.cart.is_some(),
            },
            MediaSlotInfo {
                id: SLOT_CASSETTE,
                name: "cassette",
                kind: MediaKind::Tape,
                inserted: self.tape.has_tape(),
            },
        ];
        if let Some(d) = &self.disk {
            for (i, name) in DSK_NAMES.iter().enumerate() {
                slots.push(MediaSlotInfo {
                    id: SlotId(SLOT_DSK1.0 + i as u8),
                    name,
                    kind: MediaKind::Disk,
                    inserted: d.drives[i].image.is_some(),
                });
            }
        }
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_CARTRIDGE => Some(&mut self.cart as &mut dyn MediaSlot),
            SLOT_CASSETTE => Some(&mut self.tape as &mut dyn MediaSlot),
            SlotId(n) if (SLOT_DSK1.0..SLOT_DSK1.0 + disk::DRIVES as u8).contains(&n) => self
                .disk
                .as_mut()
                .map(|d| &mut d.drives[usize::from(n - SLOT_DSK1.0)] as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"TI99", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.field_done_pending);
            w.bytes(&b.pad);
            w.bytes(b.exp.as_deref().unwrap_or_default());
            w.bool(b.cart.is_some());
            w.u32(b.cart.as_ref().map_or(0, |c| c.bank as u32));
            w.u64(b.chips_t);
            w.u32(b.pix_acc);
            w.u64(b.pixels);
            w.bool(b.field_done);
            b.audio.save(w);
            w.u32(b.dc[0] as u32);
            w.u32(b.dc[1] as u32);
            b.grom.save(w);
            b.pio.save(w);
            b.keyboard.save(w);
        });
        if let Some(sp) = &b.speech {
            w.chunk(*b"TISP", 1, |w| {
                sp.save(w);
                w.u32(b.speech_level as u32);
                w.u32(b.speech_t);
            });
        }
        self.cpu.save(&mut w);
        b.vdp.save(&mut w);
        b.psg.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        self.tape.save(&mut w);
        if let Some(d) = &self.disk {
            w.chunk(*b"TIDK", 1, |w| d.save(w));
        }
        for (i, tag) in [*b"TIJ1", *b"TIJ2"].into_iter().enumerate() {
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
        let (_, mut c) = r.chunk(*b"TI99", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let field_done_pending = c.bool()?;
        let mut board = self.board.clone();
        let pad = c.bytes()?;
        if pad.len() != board.pad.len() {
            return Err(StateError::Invalid("scratchpad size"));
        }
        board.pad = pad.to_vec();
        let exp = c.bytes()?;
        match &mut board.exp {
            Some(e) if exp.len() == e.len() => e.copy_from_slice(exp),
            None if exp.is_empty() => {}
            _ => return Err(StateError::Invalid("memory expansion differs")),
        }
        let has_cart = c.bool()?;
        if has_cart != board.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        let bank = c.u32()? as usize;
        if let Some(cart) = &mut board.cart {
            if bank >= cart.banks.len() {
                return Err(StateError::Invalid("cartridge bank"));
            }
            cart.bank = bank;
        }
        board.chips_t = c.u64()?;
        board.pix_acc = c.u32()? % (2 * CPU_HZ);
        board.pixels = c.u64()?;
        board.field_done = c.bool()?;
        board.audio.load(&mut c)?;
        board.dc = [c.u32()? as i32, c.u32()? as i32];
        board.grom.load(&mut c)?;
        board.pio.load(&mut c)?;
        board.keyboard.load(&mut c)?;
        board.samples.clear();
        if let Some(sp) = &mut board.speech {
            let (_, mut c) = StateReader::new(s).chunk(*b"TISP", 1)?;
            sp.load(&mut c)?;
            board.speech_level = c.u32()? as i32;
            board.speech_t = c.u32()? % (CPU_HZ / chips_ti::tms5220::SAMPLE_RATE);
        }
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        Savestate::load(&mut cpu, &mut r)?;
        board.vdp.load(&mut r)?;
        board.psg.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"TIJ1", *b"TIJ2"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        for (dev, st) in self.ports.iter().zip(&port_states) {
            if dev.is_none() != st.is_empty() {
                return Err(StateError::Invalid("joysticks differ"));
            }
        }
        // The disk card's registers: checked on a copy of the state first.
        if let Some(d) = &self.disk {
            let (_, mut c) = StateReader::new(s).chunk(*b"TIDK", 1)?;
            let mut probe = DiskCard::new(Vec::new(), CPU_HZ);
            probe.load(&mut c)?;
            let _ = d;
        }
        // Last, as they change the recorder and the card (atomically).
        self.tape.load(&mut StateReader::new(s))?;
        if let Some(d) = &mut self.disk {
            let (_, mut c) = StateReader::new(s).chunk(*b"TIDK", 1)?;
            d.load(&mut c)?;
        }
        for (dev, st) in self.ports.iter_mut().zip(&port_states) {
            if let Some(d) = dev {
                d.load_state(st)?;
            }
        }
        self.cpu = cpu;
        self.board = board;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for Ti99 {
    fn regs(&self) -> RegisterSet {
        let mut regs = self.cpu.regs();
        let pb = PeekBus(&self.board, self.disk.as_ref());
        const NAMES: [&str; 16] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14", "R15",
        ];
        for (i, name) in NAMES.iter().enumerate() {
            regs.0.push(emu_core::Register {
                name,
                value: u32::from(self.cpu.register(&pb, i as u16)),
                bits: 16,
            });
        }
        regs
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        let upper = name.to_ascii_uppercase();
        if let Some(n) = upper.strip_prefix('R').and_then(|n| n.parse::<u16>().ok()) {
            if n < 16 {
                let a = self.cpu.wp.wrapping_add(2 * n);
                self.board.mem_write(a, v as u16);
                return;
            }
        }
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE => {
                    Some(PeekBus(&self.board, self.disk.as_ref()).peek(a & 0xffff, Size::Byte))
                }
                VRAM_SPACE => Some(u32::from(self.board.vdp.vram[(a & 0x3fff) as usize])),
                GROM_SPACE => Some(u32::from(self.board.grom.byte(a as u16))),
                _ => None,
            }
        };
        // Big-endian in the CPU space, as the 9900 stores words.
        let n = size.bytes();
        (0..n).try_fold(0, |v, i| {
            let shift = if space == CPU_SPACE {
                8 * (n - 1 - i)
            } else {
                8 * i
            };
            Some(v | byte(addr.wrapping_add(i))? << shift)
        })
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        let n = size.bytes();
        for i in 0..n {
            let a = addr.wrapping_add(i);
            let shift = if space == CPU_SPACE {
                8 * (n - 1 - i)
            } else {
                8 * i
            };
            let byte = (v >> shift) as u8;
            match space {
                CPU_SPACE => {
                    let a = a as u16;
                    let w = self.board.mem_peek(a);
                    let w = if a & 1 == 0 {
                        (w & 0x00ff) | u16::from(byte) << 8
                    } else {
                        (w & 0xff00) | u16::from(byte)
                    };
                    match a {
                        0x8000..=0x83ff => set_word(&mut self.board.pad, usize::from(a & 0xfe), w),
                        _ => match (exp_offset(a & 0xfffe), &mut self.board.exp) {
                            (Some(o), Some(m)) => set_word(m, o, w),
                            _ => return false,
                        },
                    }
                }
                VRAM_SPACE => self.board.vdp.vram[(a & 0x3fff) as usize] = byte,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu
            .disassemble(&PeekBus(&self.board, self.disk.as_ref()), addr)
    }
}

#[cfg(test)]
mod tests;
