//! Sega's 8-bit machines before the Mark III: the SG-1000 and SG-1000 II
//! consoles and the SC-3000 / SC-3000H computers, as models of one
//! machine (Phase 36; the Master System and Game Gear come later).
//!
//! Z80A at 3.579545 MHz (the 10.738635 MHz main clock divided by 3, also on
//! the PAL machines), the TMS9918A (PAL: TMS9929A) with 16 KB VRAM, the
//! SN76489A, two joystick ports, the cartridge slot; the SC-3000 adds a
//! 64-key keyboard and the cassette through an 8255 PPI.
//!
//! **Memory.** $0000–$BFFF the cartridge ([`devices_cartridge::sega`]): ROM
//! from $0000 (repeated below $8000), cartridge RAM at $8000–$BFFF when the
//! cartridge has some (option `cartram`); $C000–$FFFF the work RAM (1 KB on
//! the SG-1000s, 2 KB on the SC-3000s), repeated; with 32 KB cartridge RAM
//! (BASIC Level III B) the cartridge disables the work RAM and its RAM
//! runs on to $FFFF. Unused areas read as the high byte of the address.
//!
//! **I/O.** Address bits 7–5 select the chips: A7 = 0 the SN76489 (write
//! only), A6 = 0 the VDP (A0 = 0 data, 1 control/status), A5 = 0 the PPI
//! (A1–A0: port A, B, C, control); a write goes to every chip selected,
//! a read where the PPI is selected returns the PPI (the officially used
//! ports: $7F PSG, $BE/$BF VDP, $DC–$DF PPI). A read where no chip answers
//! returns $FF here; the real bus holds the byte of the Z80's last refresh
//! cycle there (not emulated).
//!
//! **PPI** (SC-3000). Port C bits 2–0 select a row of the key matrix
//! (74LS145), bit 3 drives the cassette output; port A bits 7–0 and port B
//! bits 3–0 read the row's 12 columns, active low; port B bit 4 /CONT (the
//! cartridge's pin B11, high), bits 6 and 5 the printer's BUSY and FAULT
//! (high), bit 7 the cassette input. Row 7 is the two joystick ports: port
//! A bits 0–5 pad 1 up, down, left, right, TL, TR, bits 6–7 pad 2 up and
//! down, port B bits 0–3 pad 2 left, right, TL, TR. The control register
//! takes a mode byte (bit 7 set: the ports' directions) or a port C bit
//! set/reset; it reads as $FF, port C as its output latch. The SG-1000s
//! have no PPI: their ports read the pads (row 7) whatever was written.
//! Ghost keys of the matrix are not emulated.
//!
//! **Interrupts.** The VDP's /INT drives the Z80's /INT (mode 1 at $0038);
//! PAUSE (SG-1000) or the RESET key (SC-3000) the /NMI.
//!
//! **Timing.** As the ColecoVision: whole instructions with
//! [`Z80::step_timed`], the chips brought up to each bus access; the VDP
//! runs three pixel clocks per two T-states, the SN76489 one input clock a
//! T-state; a field is 262 (PAL 313) lines of 228 T-states.
//!
//! Models (`--model`): "sg1000" (default), "sg1000ii", "sc3000",
//! "sc3000h". Video formats: "ntsc" (default), "pal".
//! Options:
//! - `port1`, `port2`: "joystick" (default) or "none".
//! - `cartram`: "auto" (default: 16 KB at $8000 on an SC-3000 with a
//!   cartridge of at most 32 KB, as the BASIC Level III A; none on the
//!   SG-1000s), "none", "2k", "8k", "16k", "32k".
//! - `cart`: "demo" inserts the built-in test cartridge ([`demo`]).
//!
//! Media slot "cartridge": a ROM image up to 48 KB. Inserting or removing
//! one powers the machine up again.
//!
//! **Keyboard** (SC-3000; machine keys): row × 12 + column of the matrix
//! below (MacDonald's table), the RESET key 84.
//!
//! Sources: Charles MacDonald, "Sega SC-3000H hardware notes" (memory and
//! port map, PPI, key matrix, joystick wiring, interrupts, clocks);
//! Texas Instruments TMS9918A and SN76489AN data (`chips-ti`). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

pub mod demo;
pub mod sms;

use chips_ti::{vdp, Sn76489, Tms9918, VdpModel};
use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use devices_cartridge::sega::SegaCart;
use devices_cartridge::CartSlot;
use devices_input::joystick::pin;
use devices_input::Joystick;
use drives_other::apple_tape::AppleTape;
use mimic_core::audio::Decimator;
use mimic_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use mimic_core::gfx::{GfxOptions, GfxView, GfxViewInfo};
use mimic_core::input::{InputEvent, InputQueue, InputTarget, MachineKey, TimedInput};
use mimic_core::machine::ChipReg;
use mimic_core::media::{Media, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use mimic_core::overlay::{Overlay, OverlayInfo};
use mimic_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use mimic_core::registry::{ConfigError, MachineConfig, MachineEntry};
use mimic_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The VDP's 16 KB of video RAM.
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);
/// The SC-3000's cassette recorder.
pub const SLOT_CASSETTE: SlotId = SlotId(1);
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const CPU_HZ: u32 = 3_579_545;
/// T-states per raster line (342 pixel clocks × 2/3).
pub const LINE_T: u64 = 228;

/// Machine keys of the SC-3000 keyboard.
pub mod keys {
    /// Columns a row of the matrix has.
    pub const COLUMNS: u16 = 12;
    /// Key at `row` (0–6) and `column` (0–11).
    pub const fn at(row: u16, column: u16) -> u16 {
        row * COLUMNS + column
    }
    /// The RESET key (to the Z80's /NMI).
    pub const RESET: u16 = 84;
    /// The Master System's PAUSE (the Game Gear's START) and RESET buttons.
    pub const PAUSE: u16 = 85;
    pub const RESET_BUTTON: u16 = 86;
    pub const SHIFT: u16 = at(6, 11);
    pub const CR: u16 = at(5, 6);

    /// The keys' characters, unshifted, by matrix position (MacDonald's
    /// table; `\0` for keys without one).
    const PLAIN: [[char; 9]; 7] = [
        ['1', 'Q', 'A', 'Z', '\0', ',', 'K', 'I', '8'],
        ['2', 'W', 'S', 'X', ' ', '.', 'L', 'O', '9'],
        ['3', 'E', 'D', 'C', '\0', '/', ';', 'P', '0'],
        ['4', 'R', 'F', 'V', '\0', '\0', ':', '@', '-'],
        ['5', 'T', 'G', 'B', '\0', '\0', ']', '[', '^'],
        ['6', 'Y', 'H', 'N', '\0', '\0', '\0', '\0', '\0'],
        ['7', 'U', 'J', 'M', '\0', '\0', '\0', '\0', '\0'],
    ];

    /// With SHIFT. Not in the hardware notes: this project assumes the
    /// Japanese (JIS) layout's shifted legends, unchecked.
    const SHIFTED: [(char, char); 17] = [
        ('!', '1'),
        ('"', '2'),
        ('#', '3'),
        ('$', '4'),
        ('%', '5'),
        ('&', '6'),
        ('\'', '7'),
        ('(', '8'),
        (')', '9'),
        ('=', '-'),
        ('~', '^'),
        ('+', ';'),
        ('*', ':'),
        ('<', ','),
        ('>', '.'),
        ('?', '/'),
        ('_', '0'),
    ];

    fn plain_key(c: char) -> Option<u16> {
        PLAIN.iter().enumerate().find_map(|(row, cols)| {
            cols.iter()
                .position(|&k| k == c)
                .map(|col| at(row as u16, col as u16))
        })
    }

    /// The keys that type `c`, SHIFT first; letters type as the capitals
    /// the keys show.
    pub fn for_char(c: char) -> Option<Vec<u16>> {
        if c == '\n' || c == '\r' {
            return Some(vec![CR]);
        }
        if c == '\0' {
            return None;
        }
        if let Some(k) = plain_key(c.to_ascii_uppercase()) {
            return Some(vec![k]);
        }
        let (_, base) = SHIFTED.iter().find(|(s, _)| *s == c)?;
        Some(vec![SHIFT, plain_key(*base)?])
    }
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "sega8",
        name: "Sega SG-1000 / SC-3000 / Master System / Game Gear",
        models: &[
            "sg1000", "sg1000ii", "sc3000", "sc3000h", "mark3", "sms", "sms2", "gg",
        ],
        video_formats: mimic_core::registry::PAL_NTSC,
        create: |config| match config.model.as_deref().and_then(sms::SmsModel::from_name) {
            Some(model) => Ok(Box::new(sms::Sms::new(model, config)?)),
            None => Ok(Box::new(Sega8::new(config)?)),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Sg1000,
    Sg1000Ii,
    Sc3000,
    Sc3000H,
}

impl Model {
    fn from_name(name: Option<&str>) -> Result<Self, ConfigError> {
        Ok(match name {
            None | Some("sg1000") => Model::Sg1000,
            Some("sg1000ii") => Model::Sg1000Ii,
            Some("sc3000") => Model::Sc3000,
            Some("sc3000h") => Model::Sc3000H,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        })
    }

    /// An SC-3000: keyboard, PPI, cassette, 2 KB work RAM.
    pub fn computer(self) -> bool {
        matches!(self, Model::Sc3000 | Model::Sc3000H)
    }

    fn work_ram(self) -> usize {
        if self.computer() {
            0x800
        } else {
            0x400
        }
    }
}

/// Cartridge RAM at $8000 (option `cartram`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CartRam {
    Auto,
    Size(usize),
}

impl CartRam {
    fn from_option(v: Option<&str>) -> Result<Self, ConfigError> {
        Ok(match v {
            None | Some("auto") => CartRam::Auto,
            Some("none") => CartRam::Size(0),
            Some("2k") => CartRam::Size(0x800),
            Some("8k") => CartRam::Size(0x2000),
            Some("16k") => CartRam::Size(0x4000),
            Some("32k") => CartRam::Size(0x8000),
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "cartram option '{other}' (auto, none, 2k, 8k, 16k or 32k)"
                )))
            }
        })
    }

    /// The RAM a cartridge of `rom` bytes gets on `model`.
    fn size(self, model: Model, rom: usize) -> usize {
        match self {
            CartRam::Size(n) => n,
            CartRam::Auto if model.computer() && rom <= 0x8000 => 0x4000,
            CartRam::Auto => 0,
        }
    }
}

/// The 8255 PPI (mode 0 only, as the machines use it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ppi {
    /// The control register's last mode byte.
    mode: u8,
    /// Port C's output latch.
    c: u8,
}

impl Default for Ppi {
    fn default() -> Self {
        // After reset all ports are inputs (8255 data sheet); row 0
        // selected through the pull-downs.
        Ppi { mode: 0x9b, c: 0 }
    }
}

impl Ppi {
    fn write_control(&mut self, v: u8) {
        if v & 0x80 != 0 {
            self.mode = v;
            // A mode write clears the output latches.
            self.c = 0;
        } else {
            let bit = (v >> 1) & 7;
            if v & 1 != 0 {
                self.c |= 1 << bit;
            } else {
                self.c &= !(1 << bit);
            }
        }
    }

    /// Port C's lower half is an output (the row select, the cassette).
    fn row(&self) -> u8 {
        if self.mode & 0x01 == 0 {
            self.c & 7
        } else {
            // An input: the 74LS145's inputs float high, row 7.
            7
        }
    }
}

/// Everything on the bus except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    model: Model,
    ram: Vec<u8>,
    /// The working copy of the inserted cartridge and its RAM.
    cart: Option<SegaCart>,
    cart_ram: Vec<u8>,
    vdp: Tms9918,
    psg: Sn76489,
    ppi: Ppi,
    /// Keys down: bit n of row r is column n.
    matrix: [u16; 7],
    /// The pads' row (row 7), as the joystick ports give it, active high.
    pads: u16,
    /// The cassette input level (PPI port B bit 7).
    tape_in: bool,
    /// T-states the chips have run.
    chips_t: u64,
    /// A field ended since the last check.
    field_done: bool,
    audio: Decimator,
    /// DC blocker: last input and output (16.16).
    dc: [i32; 2],
    samples: Vec<i16>,
}

impl Board {
    fn tick(&mut self) {
        let level = self.psg.tick();
        if let Some(s) = self.audio.push(level - 16_382) {
            let x = i32::from(s) << 16;
            let y = x - self.dc[0] + ((i64::from(self.dc[1]) * 65_208) >> 16) as i32;
            self.dc = [x, y];
            self.samples
                .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        }
        let pixels = 1 + (self.chips_t & 1);
        for _ in 0..pixels {
            self.field_done |= self.vdp.tick();
        }
        self.chips_t += 1;
    }

    fn run_to(&mut self, to: u64) {
        while self.chips_t < to {
            self.tick();
        }
    }

    /// The work RAM is switched off: 32 KB of cartridge RAM take $C000.
    fn cart_ram_high(&self) -> bool {
        self.cart_ram.len() > 0x4000
    }

    fn mem_peek(&self, addr: u16) -> u8 {
        let open = (addr >> 8) as u8;
        match addr {
            0x0000..=0x7fff => self
                .cart
                .as_ref()
                .and_then(|c| c.read(addr))
                .unwrap_or(open),
            0x8000..=0xbfff => {
                if !self.cart_ram.is_empty() {
                    self.cart_ram[usize::from(addr - 0x8000) % self.cart_ram.len()]
                } else {
                    self.cart
                        .as_ref()
                        .and_then(|c| c.read(addr))
                        .unwrap_or(open)
                }
            }
            _ if self.cart_ram_high() => self.cart_ram[usize::from(addr - 0x8000)],
            _ => self.ram[usize::from(addr) % self.ram.len()],
        }
    }

    fn mem_write(&mut self, addr: u16, v: u8) {
        match addr {
            0x8000..=0xbfff if !self.cart_ram.is_empty() => {
                let n = self.cart_ram.len();
                self.cart_ram[usize::from(addr - 0x8000) % n] = v;
            }
            0xc000..=0xffff if self.cart_ram_high() => {
                self.cart_ram[usize::from(addr - 0x8000)] = v;
            }
            0xc000..=0xffff => {
                let n = self.ram.len();
                self.ram[usize::from(addr) % n] = v;
            }
            _ => {}
        }
    }

    /// The 12 columns of the row selected, active low (bit n column n).
    fn columns(&self) -> u16 {
        let row = if self.model.computer() {
            self.ppi.row()
        } else {
            7
        };
        let down = match row {
            7 => self.pads,
            r => self.matrix[usize::from(r)],
        };
        !down & 0x0fff
    }

    fn ppi_peek(&self, reg: u8) -> u8 {
        let cols = self.columns();
        match reg & 3 {
            0 => cols as u8,
            1 => {
                // /CONT, FAULT and BUSY pulled up; the cassette input.
                let high = 0x70 | if self.tape_in { 0x80 } else { 0 };
                high | (cols >> 8) as u8 & 0x0f
            }
            2 if self.model.computer() => self.ppi.c,
            _ => 0xff,
        }
    }

    fn io_peek(&self, port: u8) -> u8 {
        if port & 0x20 == 0 {
            return self.ppi_peek(port);
        }
        if port & 0x40 == 0 {
            let (data, status) = self.vdp.peek();
            return if port & 1 == 0 { data } else { status };
        }
        0xff
    }
}

/// The pad row's bits from the two ports' lines (see the crate doc).
pub(crate) fn pad_row(ports: &[Option<Box<dyn PortDevice>>; 2], now: Cycle) -> u16 {
    let lines = |n: usize| {
        ports[n]
            .as_ref()
            .map_or(0, |d| d.read_lines(now).pulled_low)
    };
    let bits = |low: u32| {
        [
            pin::UP,
            pin::DOWN,
            pin::LEFT,
            pin::RIGHT,
            pin::FIRE,
            pin::BUTTON2,
        ]
        .iter()
        .enumerate()
        .fold(0u16, |acc, (i, &p)| acc | u16::from(low & p != 0) << i)
    };
    bits(lines(0)) | bits(lines(1)) << 6
}

/// The Z80's bus for one instruction.
struct SegaBus<'a> {
    b: &'a mut Board,
    ports: &'a mut [Option<Box<dyn PortDevice>>; 2],
    tape: &'a mut AppleTape,
    /// T-state the instruction started on.
    start: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl SegaBus<'_> {
    fn io_read(&mut self, port: u8) -> u8 {
        if port & 0x20 == 0 {
            self.b.pads = pad_row(self.ports, Cycle(self.b.chips_t));
            if self.b.model.computer() && port & 3 == 1 {
                self.b.tape_in = self.tape.read(self.b.chips_t);
            }
            return self.b.ppi_peek(port);
        }
        if port & 0x40 == 0 {
            return if port & 1 == 0 {
                self.b.vdp.read_data()
            } else {
                self.b.vdp.read_status()
            };
        }
        0xff
    }

    fn io_write(&mut self, port: u8, v: u8) {
        if port & 0x80 == 0 {
            self.b.psg.write(v);
        }
        if port & 0x40 == 0 {
            if port & 1 == 0 {
                self.b.vdp.write_data(v);
            } else {
                self.b.vdp.write_control(v);
            }
        }
        if port & 0x20 == 0 && self.b.model.computer() {
            match port & 3 {
                2 => self.b.ppi.c = v,
                3 => self.b.ppi.write_control(v),
                _ => {}
            }
            // Port C bit 3: the cassette output.
            self.tape
                .set_output(self.b.ppi.c & 0x08 != 0, self.b.chips_t);
        }
    }
}

impl Bus for SegaBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = if addr >= IO_BASE {
            u32::from(self.io_read(addr as u8))
        } else {
            u32::from(self.b.mem_peek(addr as u16))
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
        u32::from(peek(self.b, addr))
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, size, v & 0xff, self.beam);
                if !dbg.on_write(CPU_SPACE, addr, size, v & 0xff) {
                    return;
                }
            }
        }
        if addr >= IO_BASE {
            self.io_write(addr as u8, v as u8);
        } else {
            self.b.mem_write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for SegaBus<'_> {
    fn wait_states(&mut self, _access: Access, t: u32) -> u32 {
        self.b.run_to(self.start + u64::from(t));
        0
    }
}

/// Side-effect-free CPU view.
fn peek(b: &Board, addr: u32) -> u8 {
    if addr >= IO_BASE {
        b.io_peek(addr as u8)
    } else {
        b.mem_peek(addr as u16)
    }
}

struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.0, addr))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

fn port_option(
    config: &MachineConfig,
    key: &str,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match config.options.get(key).map(String::as_str) {
        None | Some("joystick") => Ok(Some(Box::new(Joystick::new(PortKind::SegaJoy)))),
        Some("none") => Ok(None),
        Some(other) => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' (joystick or none)"
        ))),
    }
}

/// Panel controls.
const CONTROL_POWER: u16 = 0;
/// PAUSE (SG-1000) or RESET (SC-3000): the /NMI.
const CONTROL_NMI: u16 = 1;

pub struct Sega8 {
    model: Model,
    standard: VideoStandard,
    cart_ram: CartRam,
    cpu: Z80,
    board: Board,
    ports: [Option<Box<dyn PortDevice>>; 2],
    cart: CartSlot<SegaCart>,
    /// The SC-3000's cassette recorder (no motor control: the tape moves
    /// while the program reads it, [`AppleTape`]).
    tape: AppleTape,
    /// PAUSE / the RESET key held (the /NMI line low).
    nmi_held: bool,
    /// T-states since power-on.
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
    /// Debug overlays switched on (kept over a power cycle) and whether
    /// the beam is at the start of a field, where `overlay` shows the last
    /// complete one. Not machine state.
    overlay_ids: Vec<&'static str>,
    overlay_fresh: bool,
}

impl Sega8 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = Model::from_name(config.model.as_deref())?;
        let standard = config.video_format.unwrap_or(VideoStandard::Ntsc);
        let cart_ram = CartRam::from_option(config.options.get("cartram").map(String::as_str))?;
        let ports = [port_option(config, "port1")?, port_option(config, "port2")?];
        let vdp_model = match standard {
            VideoStandard::Pal => VdpModel::Pal,
            _ => VdpModel::Ntsc,
        };
        let mut m = Self {
            model,
            standard,
            cart_ram,
            cpu: Z80::new(),
            board: Board {
                model,
                ram: vec![0; model.work_ram()],
                cart: None,
                cart_ram: Vec::new(),
                vdp: Tms9918::new(vdp_model),
                psg: Sn76489::new(),
                ppi: Ppi::default(),
                matrix: [0; 7],
                pads: 0,
                tape_in: false,
                chips_t: 0,
                field_done: false,
                audio: Decimator::new(CPU_HZ, mimic_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                dc: [0; 2],
                samples: Vec::new(),
            },
            ports,
            cart: devices_cartridge::sega::slot(),
            tape: AppleTape::new(CPU_HZ),
            nmi_held: false,
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
            overlay_ids: Vec::new(),
            overlay_fresh: false,
        };
        match config.options.get("cart").map(String::as_str) {
            None => {}
            Some("demo") => {
                let media = Media {
                    name: "demo".into(),
                    kind: MediaKind::Cartridge,
                    backend: Box::new(MemoryImage(demo::rom())),
                };
                m.cart
                    .insert(media)
                    .map_err(|e| ConfigError::Invalid(format!("demo cartridge: {e}")))?;
            }
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "cart option '{other}' (demo)"
                )))
            }
        }
        if m.cart.take_changed() {
            m.reset(ResetKind::Hard);
        }
        m.cpu.reset();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.vdp;
        BeamPos {
            frame: v.fields,
            line: v.line,
            cycle: v.dot * 2 / 3,
            hpos: v.dot.min(vdp::WIDTH as u16 - 1),
        }
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let start = self.sched.now().0;
        let beam = self.beam_pos();
        let t = {
            let mut bus = SegaBus {
                b: &mut self.board,
                ports: &mut self.ports,
                tape: &mut self.tape,
                start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step_timed(&mut bus)
        };
        let end = start + u64::from(t);
        if self.tape.has_tape() {
            for c in start..end {
                self.tape.tick(c);
            }
        }
        self.board.run_to(end);
        self.sched.advance(u64::from(t));
        self.cpu.set_nmi(self.nmi_held);
        self.cpu.set_int(self.board.vdp.int());
        let entered = self.cpu.take_interrupt();
        let field = std::mem::take(&mut self.board.field_done);
        if field && self.board.vdp.overlays_on() {
            self.board.vdp.overlay_end_frame();
            self.overlay_fresh = true;
        }
        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
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
                    self.key(machine_key, down);
                }
            }
            InputTarget::Panel => {
                if let InputEvent::Control { id, value } = ev.event {
                    match id.0 {
                        CONTROL_POWER if value == 1 => self.reset(ResetKind::Hard),
                        CONTROL_NMI => self.nmi_held = value != 0,
                        _ => {}
                    }
                }
            }
        }
    }

    /// A key of the SC-3000's keyboard (the SG-1000s have none).
    fn key(&mut self, key: MachineKey, down: bool) {
        if !self.model.computer() {
            return;
        }
        if key.0 == keys::RESET {
            self.nmi_held = down;
            return;
        }
        let (row, col) = (key.0 / keys::COLUMNS, key.0 % keys::COLUMNS);
        let Some(r) = self.board.matrix.get_mut(usize::from(row)) else {
            return;
        };
        if down {
            *r |= 1 << col;
        } else {
            *r &= !(1 << col);
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: CPU_HZ * 3 / 2,
            active_pixels: vdp::WIDTH,
            total_pixels: u32::from(vdp::LINE),
            lines_per_field: u32::from(self.board.vdp.model.lines()),
            standard: self.standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }

    /// The cartridge from the slot, with the RAM the model and option give.
    fn insert_board_cart(&mut self) {
        self.board.cart = self.cart.cart().cloned();
        let n = self
            .board
            .cart
            .as_ref()
            .map_or(0, |c| self.cart_ram.size(self.model, c.rom().len()));
        self.board.cart_ram = vec![0; n];
    }
}

/// A ROM image in memory (the demo cartridge).
pub(crate) struct MemoryImage(pub(crate) Vec<u8>);

impl mimic_core::media::MediaBackend for MemoryImage {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), mimic_core::media::MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }

    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), mimic_core::media::MediaError> {
        Err(mimic_core::media::MediaError::WriteProtected)
    }

    fn commit(&mut self) -> Result<(), mimic_core::media::MediaError> {
        Ok(())
    }
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Sega8, addr: u16) -> u8 {
    m.board.mem_peek(addr)
}

/// The VDP (tests and tools).
pub fn vdp(m: &Sega8) -> &Tms9918 {
    &m.board.vdp
}

impl Machine for Sega8 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        self.overlay_fresh = false;
        if self.board.vdp.line == 0 && self.board.vdp.dot < 3 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE && f.addr < IO_BASE {
                    for i in 0..f.size.bytes() {
                        self.board
                            .mem_write((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
                    }
                }
            }
        }
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

    /// Hard: power cycle (RAM and VRAM cleared, the cartridge in again).
    /// Soft: the Z80 and the VDP restart (the consoles have no RESET; the
    /// SC-3000's RESET key is the /NMI).
    fn reset(&mut self, kind: ResetKind) {
        self.cpu = Z80::new();
        self.cpu.reset();
        self.board.vdp.reset();
        if kind == ResetKind::Hard {
            self.board.ram.fill(0);
            self.board.vdp = Tms9918::new(self.board.vdp.model);
            self.board.vdp.set_overlays(&self.overlay_ids);
            self.board.psg = Sn76489::new();
            self.board.ppi = Ppi::default();
            self.insert_board_cart();
        }
        self.nmi_held = false;
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

    fn audio(&mut self, out: &mut Vec<i16>) -> mimic_core::AudioFormat {
        out.append(&mut self.board.samples);
        mimic_core::AudioFormat {
            sample_rate: mimic_core::SAMPLE_RATE,
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
                addr_bits: 17,
                big_endian: false,
            },
            MemSpace {
                id: VRAM_SPACE,
                name: "vram",
                device: CPU,
                addr_bits: 14,
                big_endian: false,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let v = &self.board.vdp;
        let p = &self.board.psg;
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
        let psg_regs = vec![
            reg("TONE0", u32::from(p.period[0]), 10),
            reg("TONE1", u32::from(p.period[1]), 10),
            reg("TONE2", u32::from(p.period[2]), 10),
            reg("NOISE", u32::from(p.noise), 3),
            reg("ATT0", u32::from(p.atten[0]), 4),
            reg("ATT1", u32::from(p.atten[1]), 4),
            reg("ATT2", u32::from(p.atten[2]), 4),
            reg("ATT3", u32::from(p.atten[3]), 4),
        ];
        let b = &self.board;
        let mut chips = vec![
            ChipInfo {
                name: "vdp",
                regs: vdp_regs,
            },
            ChipInfo {
                name: "psg",
                regs: psg_regs,
            },
        ];
        if self.model.computer() {
            chips.push(ChipInfo {
                name: "ppi",
                regs: vec![
                    reg("PORTA", u32::from(b.ppi_peek(0)), 8),
                    reg("PORTB", u32::from(b.ppi_peek(1)), 8),
                    reg("PORTC", u32::from(b.ppi.c), 8),
                    reg("MODE", u32::from(b.ppi.mode), 8),
                    reg("ROW", u32::from(b.ppi.row()), 3),
                ],
            });
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
        self.sched.now()
    }

    /// The SC-3000's keyboard types text: a key held 60 ms, 40 ms between
    /// two (the programs scan the matrix in their frame interrupt); the
    /// consoles have no keyboard.
    fn typed_keys(&self, text: &str, start: Cycle) -> Option<(Vec<TimedInput>, Cycle)> {
        if !self.model.computer() {
            return None;
        }
        let ms = u64::from(CPU_HZ) / 1000;
        let timing = mimic_core::input::TypeTiming {
            hold: 60 * ms,
            gap: 40 * ms,
            lead: 5 * ms,
        };
        Some(mimic_core::input::typed_lines(
            text,
            start,
            u64::from(CPU_HZ),
            |line, from| mimic_core::input::typed_chars(line, from, timing, keys::for_char),
        ))
    }

    fn controls(&self) -> Vec<mimic_core::ControlInfo> {
        let nmi = if self.model.computer() {
            "RESET"
        } else {
            "PAUSE"
        };
        vec![
            mimic_core::ControlInfo::button(CONTROL_POWER, "POWER", false),
            mimic_core::ControlInfo::button(CONTROL_NMI, nmi, self.nmi_held),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        [(PORT1, "joystick 1"), (PORT2, "joystick 2")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::SegaJoy,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::SegaJoy {
            return Err(PortError::Incompatible {
                port: PortKind::SegaJoy,
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
        let mut slots = vec![MediaSlotInfo {
            id: SLOT_CARTRIDGE,
            name: "cartridge",
            kind: MediaKind::Cartridge,
            inserted: self.cart.is_inserted(),
        }];
        if self.model.computer() {
            slots.push(MediaSlotInfo {
                id: SLOT_CASSETTE,
                name: "cassette",
                kind: MediaKind::Tape,
                inserted: self.tape.has_tape(),
            });
        }
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_CARTRIDGE => Some(&mut self.cart as &mut dyn MediaSlot),
            SLOT_CASSETTE if self.model.computer() => Some(&mut self.tape as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"SG8B", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.field_done_pending);
            w.bool(self.nmi_held);
            w.bytes(&b.ram);
            w.bytes(&b.cart_ram);
            w.bool(b.cart.is_some());
            w.u8(b.ppi.mode);
            w.u8(b.ppi.c);
            for r in b.matrix {
                w.u16(r);
            }
            w.u16(b.pads);
            w.bool(b.tape_in);
            w.u64(b.chips_t);
            w.bool(b.field_done);
            b.audio.save(w);
            w.u32(b.dc[0] as u32);
            w.u32(b.dc[1] as u32);
        });
        self.cpu.save(&mut w);
        b.vdp.save(&mut w);
        b.psg.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"SGP1", *b"SGP2"].into_iter().enumerate() {
            w.chunk(tag, 1, |w| {
                w.bytes(
                    &self.ports[i]
                        .as_ref()
                        .map(|d| d.save_state())
                        .unwrap_or_default(),
                );
            });
        }
        self.tape.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"SG8B", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let field_done_pending = c.bool()?;
        let nmi_held = c.bool()?;
        let mut board = self.board.clone();
        let ram = c.bytes()?;
        if ram.len() != board.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        board.ram = ram.to_vec();
        let cart_ram = c.bytes()?;
        if cart_ram.len() != board.cart_ram.len() {
            return Err(StateError::Invalid("cartridge RAM differs"));
        }
        board.cart_ram = cart_ram.to_vec();
        if c.bool()? != board.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        board.ppi = Ppi {
            mode: c.u8()?,
            c: c.u8()?,
        };
        for row in &mut board.matrix {
            *row = c.u16()? & 0x0fff;
        }
        board.pads = c.u16()? & 0x0fff;
        board.tape_in = c.bool()?;
        board.chips_t = c.u64()?;
        board.field_done = c.bool()?;
        board.audio.load(&mut c)?;
        board.dc = [c.u32()? as i32, c.u32()? as i32];
        board.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        board.vdp.load(&mut r)?;
        board.psg.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"SGP1", *b"SGP2"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        for (dev, st) in self.ports.iter().zip(&port_states) {
            if dev.is_none() != st.is_empty() {
                return Err(StateError::Invalid("joysticks differ"));
            }
        }
        let mut ports: Vec<Option<Vec<u8>>> = Vec::new();
        for (dev, st) in self.ports.iter().zip(&port_states) {
            ports.push(dev.as_ref().map(|_| st.clone()));
        }
        // The recorder's chunk is checked on a copy first.
        AppleTape::new(CPU_HZ).load(&mut StateReader::new(s))?;
        // The port devices and the recorder last: their own loads are
        // atomic too.
        for (dev, st) in self.ports.iter_mut().zip(ports) {
            if let (Some(d), Some(st)) = (dev, st) {
                d.load_state(&st)?;
            }
        }
        self.cpu = cpu;
        self.board = board;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        self.nmi_held = nmi_held;
        self.tape.load(&mut StateReader::new(s))?;
        Ok(())
    }
}

impl DebugTarget for Sega8 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE => Some(u32::from(peek(&self.board, a))),
                VRAM_SPACE => Some(u32::from(self.board.vdp.vram[(a & 0x3fff) as usize])),
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            match space {
                CPU_SPACE
                    if a < 0x1_0000
                        && (a >= 0xc000 || !self.board.cart_ram.is_empty() && a >= 0x8000) =>
                {
                    self.board.mem_write(a as u16, byte);
                }
                VRAM_SPACE => self.board.vdp.vram[(a & 0x3fff) as usize] = byte,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.board), addr)
    }
}

#[cfg(test)]
mod tests;
