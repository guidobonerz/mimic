//! Atari 8-bit computers: 400, 800, 800XL, 65XE and 130XE.
//!
//! 6502C "SALLY", ANTIC (display lists, DMA), GTIA (colours, players and
//! missiles), POKEY (sound, timers, keyboard, serial I/O), a 6520 PIA
//! (joysticks; memory banking on the XL/XE) and RAM with the OS,
//! self-test and BASIC ROMs banked in by PIA port B (see [`memory`]).
//!
//! Models (`--model`): "800xl" (default), "65xe" (the same hardware),
//! "130xe" (128 KB, PORTB bank switching for CPU and ANTIC separately),
//! "800" and "400" (OS-A/OS-B ROM, 8–48 KB RAM, four joystick ports, no
//! built-in BASIC, RESET key as an NMI). Each as PAL (1 773 447 Hz, 312
//! lines) or with "-ntsc" (1 789 773 Hz, 262 lines); 114 cycles per line.
//!
//! Options:
//! - `port1`–`port4`: "joystick" (default port 1), "paddles", "lightpen"
//!   or "none" (default elsewhere). A light pen's pulse goes to the port's
//!   trigger line, ANTIC's light-pen input: PENH/PENV latch where the beam
//!   draws a bright pixel under its tip (`devices_input::lightpen`; its
//!   button reads as the stick's up line). With pens in several ports the
//!   first counts. Ports 3 and 4 exist on the 400/800 only (PIA port
//!   B, TRIG2/3, POT4–7). Paddles turn POKEY's POT0/1 (port 1) … POT6/7
//!   (port 4); their buttons are the stick's left/right lines.
//! - `basic`: "on" (default) or "off" — XL/XE: holds OPTION while the OS
//!   boots, like the real machine; 400/800: puts the BASIC ROM (role
//!   "basic") into the left cartridge slot, or leaves the slot empty.
//! - `ram` (400/800): "8k", "16k" (400 default), "32k", "48k" (800 default).
//! - `drives`: SIO disk drives D1:… connected, "0"–"4" (default "1");
//!   `drive`: their model, "1050" (default), "810" or "xf551".
//! - `roms`: "stub" uses a tiny built-in OS, for tests without ROMs.
//!
//! Media slots: "D1:"–"D4:" (ATR disks on the SIO drives), "program"
//! (XEX files, see [`loader`]), "tape" (CAS on the 410/1010 program
//! recorder, motor on PIA CA2) and "cartridge" (CAR or raw 8/16 KB ROM;
//! inserting or removing one powers the machine up again).
//!
//! ROMs come from `MachineConfig::roms`: "os" (16 KB XL/XE OS), "osb"
//! (10 KB OS-A/OS-B for the 400/800), "basic" (8 KB, optional).
//! User-supplied, never in the repository.
//!
//! Per cycle: ANTIC runs its DMA first (and halts the CPU for it), the CPU
//! runs unless halted, GTIA draws the cycle's two colour clocks, POKEY
//! clocks.

#![forbid(unsafe_code)]

pub mod keyboard;
pub mod loader;
pub mod memory;
pub mod overlay;
pub mod stub;

use chips_atari::antic::{self, Antic, AnticMemory};
use chips_atari::gtia::{self, Gtia};
use chips_atari::pokey::Pokey;
use chips_cbm::pia::Pia;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use emu_core::audio::Decimator;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::gfx::{GfxOptions, GfxView, GfxViewInfo};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::overlay::{Overlay, OverlayInfo};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::Cycle;
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

use devices_cartridge::atari::AtariCart;
use devices_cartridge::CartSlot;
use drives_other::recorder::Recorder;
use drives_other::sio::{DriveModel, SerialIn, Sio, SioDrive};
use keyboard::keys;
use loader::{Loader, Phase, ProgramSlot, INIT_TRAP};
use media::xex::{INITAD, RUNAD};
use memory::{Area, Kind, Memory, Roms, BASIC_SIZE, OS_AB_SIZE, OS_SIZE};

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's view of memory (banked by PORTB).
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// ANTIC's view of memory (DMA reads): the same as the CPU's except on the
/// 130XE, where PORTB banks extended RAM for ANTIC separately, and at
/// `$D000–$D7FF`, which ANTIC reads as `$FF`. Read-only.
pub const ANTIC_SPACE: MemSpaceId = MemSpaceId(1);

/// Joystick ports 1–4 (3 and 4 on the 400/800 only).
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);
pub const PORT3: PortId = PortId(2);
pub const PORT4: PortId = PortId(3);

/// Disk drives D1:–D4: (`SlotId` 0–3) and the program slot.
pub const SLOT_D1: SlotId = SlotId(0);
pub const SLOT_PROGRAM: SlotId = SlotId(4);
pub const SLOT_TAPE: SlotId = SlotId(5);
pub const SLOT_CARTRIDGE: SlotId = SlotId(6);
const DRIVE_NAMES: [&str; 4] = ["D1:", "D2:", "D3:", "D4:"];

/// Fields the OPTION key is held after power-up for `basic=off`.
const OPTION_HOLD_FIELDS: u64 = 100;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "atari8",
        name: "Atari 400/800/800XL/65XE/130XE",
        models: &[
            "800xl",
            "800xl-ntsc",
            "65xe",
            "65xe-ntsc",
            "130xe",
            "130xe-ntsc",
            "800",
            "800-ntsc",
            "400",
            "400-ntsc",
        ],
        create: |config| Ok(Box::new(Atari8::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    A400,
    A800,
    A800Xl,
    A65Xe,
    A130Xe,
}

impl Model {
    fn parse(model: Option<&str>) -> Result<(Model, VideoStandard), ConfigError> {
        let Some(model) = model else {
            return Ok((Model::A800Xl, VideoStandard::Pal));
        };
        let (name, standard) = match model.strip_suffix("-ntsc") {
            Some(n) => (n, VideoStandard::Ntsc),
            None => (model, VideoStandard::Pal),
        };
        let m = match name {
            "400" => Model::A400,
            "800" => Model::A800,
            "800xl" => Model::A800Xl,
            "65xe" => Model::A65Xe,
            "130xe" => Model::A130Xe,
            _ => return Err(ConfigError::UnknownModel(model.into())),
        };
        Ok((m, standard))
    }

    /// 400/800: the old board without PORTB banking.
    pub fn is_old(self) -> bool {
        matches!(self, Model::A400 | Model::A800)
    }

    fn ports(self) -> usize {
        if self.is_old() {
            4
        } else {
            2
        }
    }
}

fn cpu_hz(standard: VideoStandard) -> u32 {
    match standard {
        VideoStandard::Pal => 1_773_447,
        VideoStandard::Ntsc => 1_789_773,
    }
}

/// ANTIC's view of memory (the 130XE banks it separately).
struct DmaView<'a>(&'a Memory);

impl AnticMemory for DmaView<'_> {
    fn dma_read(&self, addr: u16) -> u8 {
        self.0.read_dma(addr)
    }
}

/// The chips behind `$D000–$D7FF`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Io {
    antic: Antic,
    gtia: Gtia,
    pokey: Pokey,
    pia: Pia,
    /// PIA port A pins: joystick directions (0 = pressed).
    porta: u8,
    /// PIA port B input pins: joystick ports 3 and 4 on the 400/800
    /// (pulled up elsewhere).
    portb_in: u8,
}

/// PIA register for an address: A0 selects port A/B, A1 data/control
/// (`$D300` PORTA, `$D301` PORTB, `$D302` PACTL, `$D303` PBCTL).
fn pia_reg(addr: u16) -> u8 {
    (((addr & 1) << 1) | ((addr >> 1) & 1)) as u8
}

impl Io {
    fn peek(&self, addr: u16) -> u8 {
        match addr >> 8 {
            0xd0 => self.gtia.peek(addr as u8),
            0xd2 => self.pokey.peek(addr as u8),
            0xd3 => self
                .pia
                .peek(pia_reg(addr), self.porta_pins(), self.portb()),
            0xd4 => self.antic.peek(addr as u8),
            _ => 0xff,
        }
    }

    fn read(&mut self, addr: u16) -> u8 {
        match addr >> 8 {
            0xd3 => self
                .pia
                .read(pia_reg(addr), self.porta_pins(), self.portb()),
            _ => self.peek(addr),
        }
    }

    /// Returns the new PORTB pin levels after a PIA write.
    fn write(&mut self, addr: u16, v: u8) -> Option<u8> {
        match addr >> 8 {
            0xd0 => self.gtia.write(addr as u8, v),
            0xd2 => self.pokey.write(addr as u8, v),
            0xd3 => {
                self.pia.write(pia_reg(addr), v);
                return Some(self.portb());
            }
            0xd4 => self.antic.write(addr as u8, v),
            _ => {}
        }
        None
    }

    /// Port A pins: what the PIA drives (inputs pulled up), pulled low by
    /// the joysticks.
    fn porta_pins(&self) -> u8 {
        self.pia.port_a_out() & self.porta
    }

    /// Port B pins: outputs as driven, inputs pulled up (and pulled low
    /// by the 400/800's joysticks 3 and 4).
    fn portb(&self) -> u8 {
        let b = &self.pia.b;
        (b.or & b.ddr) | (!b.ddr & self.portb_in)
    }
}

struct AtariBus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
}

impl Bus for AtariBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        u32::from(match self.mem.area(a) {
            Area::Io if a >> 8 == 0xd5 => {
                // CCTL: some cartridges switch banks on reads.
                if let Some(c) = &mut self.mem.cart {
                    c.access(a, None);
                }
                0xff
            }
            Area::Io => self.io.read(a),
            _ => self.mem.read_plain(a),
        })
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.mem, self.io, addr as u16))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        match self.mem.area(a) {
            Area::Io if a >> 8 == 0xd5 => {
                if let Some(c) = &mut self.mem.cart {
                    c.access(a, Some(v as u8));
                }
            }
            Area::Io => {
                if let Some(portb) = self.io.write(a, v as u8) {
                    self.mem.portb = portb;
                }
            }
            _ => self.mem.write_plain(a, v as u8),
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

fn peek(mem: &Memory, io: &Io, addr: u16) -> u8 {
    match mem.area(addr) {
        Area::Io => io.peek(addr),
        _ => mem.read_plain(addr),
    }
}

/// Read-only view for disassembly and debugger peeks.
struct PeekBus<'a>(&'a Memory, &'a Io);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.0, self.1, addr as u16))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

fn rom<const N: usize>(config: &MachineConfig, role: &str) -> Result<Box<[u8; N]>, ConfigError> {
    let data = config
        .roms
        .get(role)
        .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
    let boxed: Box<[u8]> = data.clone().into_boxed_slice();
    boxed.try_into().map_err(|_| {
        ConfigError::Invalid(format!("{role} ROM must be {N} bytes, got {}", data.len()))
    })
}

fn load_roms(config: &MachineConfig, model: Model) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => Ok(Roms {
            os: if model.is_old() {
                stub::os()[OS_SIZE - OS_AB_SIZE..].to_vec()
            } else {
                stub::os().to_vec()
            },
            basic: None,
        }),
        Some(other) => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => Ok(Roms {
            os: if model.is_old() {
                rom::<OS_AB_SIZE>(config, "osb")?.to_vec()
            } else {
                rom::<OS_SIZE>(config, "os")?.to_vec()
            },
            basic: match config.roms.get("basic") {
                Some(_) => Some(rom::<BASIC_SIZE>(config, "basic")?),
                None => None,
            },
        }),
    }
}

fn port_option(
    config: &MachineConfig,
    key: &str,
    default: &str,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match config
        .options
        .get(key)
        .map(String::as_str)
        .unwrap_or(default)
    {
        "joystick" => Ok(Some(Box::new(devices_input::Joystick::new(
            PortKind::AtariJoy,
        )))),
        "paddles" => Ok(Some(Box::new(devices_input::Paddles::for_port(
            PortKind::AtariJoy,
        )))),
        "lightpen" => Ok(Some(Box::new(devices_input::LightPen::new(
            devices_input::LightPenKind::Atari,
        )))),
        "none" => Ok(None),
        other => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' (joystick, paddles, lightpen or none)"
        ))),
    }
}

/// A pot line's analog value (0 = no resistance … 65535, open line) as
/// POKEY counts it: 1 … 228.
fn pot_count(analog: u16) -> u8 {
    (1 + u32::from(analog) * 227 / 0xffff) as u8
}

pub struct Atari8 {
    model: Model,
    standard: VideoStandard,
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
    /// Joystick ports: two, or four on the 400/800.
    ports: Vec<Option<Box<dyn PortDevice>>>,
    palette: [Rgb8; 256],
    audio: Decimator,
    samples: Vec<i16>,
    field_done_pending: bool,
    /// `basic=off`: OPTION is held until this field.
    option_until: u64,
    basic_off: bool,
    /// The OPTION key as the user holds it (separate from the boot hold).
    option_key: bool,
    sio: Sio,
    program: ProgramSlot,
    loader: Option<Loader>,
    recorder: Recorder,
    /// The cartridge as inserted; `mem.cart` is the working copy.
    cart: CartSlot<AtariCart>,
    /// Debug overlay recording (not machine state); `None` while every
    /// overlay is off.
    overlay_rec: Option<Box<overlay::Recorder>>,
    /// Where a light pen points (from its port's lines, so not saved
    /// separately) and which palette entries light it.
    pen: Option<PenTarget>,
    pen_lit: Vec<bool>,
}

/// The beam position at which a light pen sees its pixel drawn: line,
/// CPU cycle, colour clock, index in GTIA's picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PenTarget {
    line: u16,
    cycle: u16,
    cc: u16,
    index: usize,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
    audio: Decimator,
}

impl Atari8 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let (model, standard) = Model::parse(config.model.as_deref())?;
        let basic_off = match config.options.get("basic").map(String::as_str) {
            None | Some("on") => false,
            Some("off") => true,
            Some(other) => return Err(ConfigError::Invalid(format!("basic option '{other}'"))),
        };
        let kind = match model {
            Model::A400 | Model::A800 => {
                let default = if model == Model::A400 { "16k" } else { "48k" };
                let ram = match config
                    .options
                    .get("ram")
                    .map(String::as_str)
                    .unwrap_or(default)
                {
                    "8k" => 0x2000,
                    "16k" => 0x4000,
                    "32k" => 0x8000,
                    "48k" => 0xc000,
                    other => {
                        return Err(ConfigError::Invalid(format!(
                            "ram option '{other}' (8k, 16k, 32k or 48k)"
                        )))
                    }
                };
                Kind::Old { ram }
            }
            Model::A800Xl | Model::A65Xe => Kind::Xl,
            Model::A130Xe => Kind::Xe130,
        };
        let mut ports: Vec<Option<Box<dyn PortDevice>>> = Vec::new();
        for (i, key) in ["port1", "port2", "port3", "port4"]
            .into_iter()
            .take(model.ports())
            .enumerate()
        {
            ports.push(port_option(
                config,
                key,
                if i == 0 { "joystick" } else { "none" },
            )?);
        }
        let drives = match config.options.get("drives").map(String::as_str) {
            None => 1,
            Some(n) => n
                .parse::<usize>()
                .ok()
                .filter(|&n| n <= 4)
                .ok_or_else(|| ConfigError::Invalid(format!("drives option '{n}' (0-4)")))?,
        };
        let drive_model = match config.options.get("drive").map(String::as_str) {
            None => DriveModel::A1050,
            Some(m) => DriveModel::from_option(m).ok_or_else(|| {
                ConfigError::Invalid(format!("drive option '{m}' (810, 1050 or xf551)"))
            })?,
        };
        let mut sio = Sio::new(cpu_hz(standard));
        for d in sio.drives.iter_mut().take(drives) {
            *d = Some(SioDrive::new(drive_model));
        }
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Sally,
            ..CpuConfig::default()
        });
        cpu.reset();
        let hz = cpu_hz(standard);
        let mut roms = load_roms(config, model)?;
        // 400/800: BASIC is a cartridge in the left slot.
        let cart_slot = devices_cartridge::atari::slot();
        let mut cart = None;
        if model.is_old() {
            if let (false, Some(basic)) = (basic_off, roms.basic.take()) {
                let c = AtariCart::parse(&basic[..])
                    .map_err(|e| ConfigError::Invalid(format!("BASIC cartridge: {e:?}")))?;
                cart = Some(c);
            }
        }
        let mut gtia = Gtia::new(standard);
        gtia.set_trig3_is_cartridge(!model.is_old());
        let mut mem = Memory::new(kind, roms);
        mem.cart = cart;
        let mut m = Self {
            model,
            standard,
            cpu,
            mem,
            io: Io {
                antic: Antic::new(standard),
                gtia,
                pokey: Pokey::new(),
                pia: Pia::new(),
                porta: 0xff,
                portb_in: 0xff,
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            ports,
            palette: chips_atari::palette::palette(standard),
            audio: Decimator::new(hz, emu_core::SAMPLE_RATE)
                .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
            samples: Vec::new(),
            field_done_pending: false,
            overlay_rec: None,
            pen: None,
            pen_lit: Vec::new(),
            option_until: if basic_off && !model.is_old() {
                OPTION_HOLD_FIELDS
            } else {
                0
            },
            basic_off: basic_off && !model.is_old(),
            option_key: false,
            sio,
            program: ProgramSlot::default(),
            loader: None,
            recorder: Recorder::new(hz),
            cart: cart_slot,
        };
        m.refresh_ports();
        m.refresh_console();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let a = &self.io.antic;
        BeamPos {
            frame: a.frame(),
            line: a.line(),
            cycle: a.cycle(),
            hpos: (a.cycle() * 4).saturating_sub(gtia::FIRST_CC * 2),
        }
    }

    /// One CPU cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        // A cartridge went in or out: power up again.
        if self.cart.take_changed() {
            self.mem.cart = self.cart.cart().cloned();
            self.reset(ResetKind::Hard);
        }
        // A program went in: power up and boot it.
        if self.program.take_changed() {
            if let Some(xex) = self.program.pending.take() {
                self.loader = Some(Loader::new(xex));
                self.reset(ResetKind::Hard);
            }
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        if self
            .loader
            .as_ref()
            .is_some_and(|l| self.cpu.at_instruction_boundary() && self.cpu.pc() == l.trap())
        {
            self.load_step();
        }
        // ANTIC's DMA comes first and decides whether the CPU runs.
        let out = self.io.antic.tick(&DmaView(&self.mem));
        if let Some((which, v)) = out.pm {
            let single = self.io.antic.dmactl & 0x10 != 0;
            self.io.gtia.dma(which, v, self.io.antic.line(), single);
        }
        self.cpu.set_halt(out.halt);
        self.cpu.set_rdy(!out.wsync);
        // ANTIC's NMI is a pulse; the 6502 latches the edge.
        self.cpu.set_nmi(out.nmi);
        self.cpu.set_irq(self.io.pokey.irq() || self.io.pia.irq());
        let beam = self.beam_pos();
        let result = {
            let mut bus = AtariBus {
                mem: &mut self.mem,
                io: &mut self.io,
            };
            let mut hooked = HookedBus {
                inner: &mut bus,
                dbg: dbg.as_deref_mut(),
                space: CPU_SPACE,
                beam,
            };
            self.cpu.tick(&mut hooked)
        };
        let entered = self.cpu.take_interrupt();
        // GTIA draws this cycle's two colour clocks.
        let (line, c) = (self.io.antic.line(), self.io.antic.cycle());
        let pf = [
            self.io.antic.playfield(c * 2),
            self.io.antic.playfield(c * 2 + 1),
        ];
        if self.io.antic.hires_in_vblank() {
            self.io.gtia.render_cycle_vblank(c);
        } else {
            self.io.gtia.render_cycle(line, c, pf);
        }
        if let Some(pen) = self.pen {
            if pen.cycle == c && pen.line == line {
                self.pen_sees(pen);
            }
        }
        if let Some(rec) = &mut self.overlay_rec {
            rec.cycle(&self.io.antic, &self.io.gtia, out.nmi);
        }
        self.io.pokey.tick();
        // SIO: bytes POKEY sent, the COMMAND line (PIA CB2, low =
        // asserted), and what the devices send back.
        let now = self.sched.now().0;
        // The program recorder's motor: PIA CA2 low.
        self.recorder.set_motor(!self.io.pia.ca2_out(), now);
        for b in self.io.pokey.take_sent() {
            self.sio.byte_from_computer(b, now);
            self.recorder.byte_from_computer(b, now);
        }
        self.sio.set_command(!self.io.pia.cb2_out(), now);
        let (a, b) = (self.sio.tick(now), self.recorder.tick(now));
        let serial = SerialIn {
            level: a.level && b.level,
            busy: a.busy || b.busy,
            byte: a.byte.or(b.byte),
        };
        self.io.pokey.set_serial_line(serial.level, serial.busy);
        if let Some(b) = serial.byte {
            self.io.pokey.receive(b);
        }
        if !self.model.is_old() {
            self.io
                .gtia
                .set_cartridge(self.mem.cart.as_ref().is_some_and(AtariCart::rd5));
        }
        let level =
            i32::from(self.io.pokey.output()) * 400 + if self.io.gtia.speaker() { 3000 } else { 0 };
        if let Some(s) = self.audio.push(level) {
            self.samples.push(s);
        }
        let new_field = self.io.antic.end_cycle();
        self.sched.tick();
        if new_field {
            self.refresh_console();
            if let Some(rec) = &mut self.overlay_rec {
                rec.end_field();
            }
        }

        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            if let CycleResult::InstrDone { next_pc } = result {
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
                let b = self.beam_pos();
                dbg.check_raster(b.line, b.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                self.field_done_pending |= new_field;
                return Some(stop);
            }
        }
        match result {
            CycleResult::Jammed => Some(StopReason::CpuJam {
                device: CPU,
                pc: u32::from(self.cpu.pc()),
            }),
            _ if new_field => Some(StopReason::FrameDone),
            _ => None,
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match ev.target {
            InputTarget::Keyboard => {
                if let InputEvent::Key { machine_key, down } = ev.event {
                    self.key(machine_key.0, down);
                }
            }
            InputTarget::Port(p) => {
                let i = usize::from(p.0);
                if let Some(Some(dev)) = self.ports.get_mut(i) {
                    dev.input(&ev.event);
                }
                self.refresh_ports();
            }
            InputTarget::Panel => {
                if let emu_core::input::InputEvent::Control { id, value } = ev.event {
                    self.control(id.0, value);
                }
            }
        }
    }

    /// A console control from the panel (power, RESET, the START, SELECT and OPTION keys).
    fn control(&mut self, id: u16, value: u8) {
        match id {
            0 if value == 1 => emu_core::Machine::reset(self, emu_core::ResetKind::Hard),
            1 if value == 1 => self.reset_key(),
            2..=4 => self.key(keys::START.0 + id - 2, value == 1),
            _ => {}
        }
    }

    fn key(&mut self, code: u16, down: bool) {
        let pokey = &mut self.io.pokey;
        match code {
            0..=63 => pokey.set_key(code as u8, down),
            c if c == keys::SHIFT.0 => pokey.set_shift(down),
            c if c == keys::CONTROL.0 => pokey.set_control(down),
            c if c == keys::BREAK.0 && down => pokey.press_break(),
            c if (keys::START.0..=keys::OPTION.0).contains(&c) => {
                let bit = 1 << (c - keys::START.0);
                if c == keys::OPTION.0 {
                    self.option_key = down;
                }
                let g = &mut self.io.gtia;
                if down {
                    g.console_keys |= bit;
                } else {
                    g.console_keys &= !bit;
                }
                self.refresh_console();
            }
            c if c == keys::RESET.0 && down => self.reset_key(),
            _ => {}
        }
    }

    /// The RESET key: the system reset line on the XL/XE, an NMI through
    /// ANTIC on the 400/800 (the OS warm-starts in its handler).
    fn reset_key(&mut self) {
        if self.model.is_old() {
            self.io.antic.reset_key();
        } else {
            self.reset(ResetKind::Soft);
        }
    }

    /// OPTION as the user holds it, plus the boot hold of `basic=off`.
    fn refresh_console(&mut self) {
        let hold = self.io.antic.frame() < self.option_until;
        let g = &mut self.io.gtia;
        if hold || self.option_key {
            g.console_keys |= 0x04;
        } else {
            g.console_keys &= !0x04;
        }
    }

    /// Joystick lines into PIA port A (directions of ports 1 and 2), port
    /// B (ports 3 and 4 on the 400/800), GTIA (triggers) and POKEY (pots).
    fn refresh_ports(&mut self) {
        let now = self.sched.now();
        let mut pins = [0xffu8; 2];
        for (i, dev) in self.ports.iter().enumerate() {
            let lines = dev.as_ref().map(|d| d.read_lines(now));
            let low = lines.map_or(0, |l| l.pulled_low);
            // Up, down, left, right on bits 0–3 (ports 1, 3) / 4–7 (2, 4).
            pins[i / 2] &= !(((low & 0x0f) as u8) << (4 * (i % 2)));
            self.io.gtia.set_trigger(i, low & 0x20 != 0);
            // Pins 9 and 5: POT0/1 (port 1) … POT6/7 (port 4).
            let analog = lines.map_or([u16::MAX; 2], |l| l.analog);
            for (k, a) in analog.into_iter().enumerate() {
                self.io.pokey.set_pot(2 * i + k, pot_count(a));
            }
        }
        self.io.porta = pins[0];
        self.io.portb_in = pins[1];
        self.sync_light_pen();
        if !self.mem.banks_with_portb() {
            self.mem.portb = self.io.portb();
        }
    }

    /// Where a light pen in a joystick port points (the first port with
    /// one): its pixel's line, cycle and colour clock. The pen's pulse goes
    /// to the trigger line (pin 6), ANTIC's light-pen input.
    fn sync_light_pen(&mut self) {
        let now = self.sched.now();
        let aim = self
            .ports
            .iter()
            .flatten()
            .find_map(|d| devices_input::lightpen::aim(&d.read_lines(now)));
        self.pen = aim.and_then(|a| {
            if u32::from(a.x) >= gtia::WIDTH || u32::from(a.y) >= gtia::HEIGHT {
                return None;
            }
            let cc = a.x / 2 + gtia::FIRST_CC;
            Some(PenTarget {
                line: a.y + chips_atari::antic::FIRST_LINE,
                cycle: cc / 2,
                cc,
                index: usize::from(a.y) * gtia::WIDTH as usize + usize::from(a.x),
            })
        });
        if self.pen.is_some() && self.pen_lit.is_empty() {
            self.pen_lit = devices_input::lightpen::lit_colours(&self.palette);
        }
    }

    /// GTIA drew the pen's pixel in this cycle: a bright one pulses ANTIC's
    /// light-pen input.
    fn pen_sees(&mut self, pen: PenTarget) {
        let colour = self.io.gtia.pixels()[pen.index];
        if self.pen_lit.get(usize::from(colour)) == Some(&true) {
            self.io.antic.light_pen(pen.cc);
        }
    }

    /// The CPU reached the loader's trap: load segments until one asks
    /// for its INITAD routine (called, returning to [`INIT_TRAP`]), then
    /// start the program.
    fn load_step(&mut self) {
        let Some(mut l) = self.loader.take() else {
            return;
        };
        if l.phase == Phase::Boot {
            self.write_word(RUNAD, 0);
            // The OS has looked at OPTION (BASIC off); release it for the
            // program.
            self.option_until = 0;
            self.refresh_console();
        }
        while l.next < l.segments.len() {
            let seg = l.segments[l.next].clone();
            l.next += 1;
            self.write_word(INITAD, 0);
            for (i, &b) in seg.data.iter().enumerate() {
                let addr = u32::from(seg.start) + i as u32;
                AtariBus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                }
                .write(addr & 0xffff, Size::Byte, u32::from(b));
            }
            let init = self.read_word(INITAD);
            if seg.covers(INITAD) && init != 0 {
                // JSR init: RTS returns to INIT_TRAP.
                let ret = INIT_TRAP - 1;
                let sp = self.cpu.regs().get("S").unwrap_or(0xff) as u8;
                self.mem.ram[0x100 + usize::from(sp)] = (ret >> 8) as u8;
                self.mem.ram[0x100 + usize::from(sp.wrapping_sub(1))] = ret as u8;
                self.cpu.set_reg("S", u32::from(sp.wrapping_sub(2)));
                self.cpu.set_reg("PC", u32::from(init));
                l.phase = Phase::Init;
                self.loader = Some(l);
                return;
            }
        }
        let run = match self.read_word(RUNAD) {
            0 => l.segments.first().map_or(0, |s| s.start),
            a => a,
        };
        self.cpu.set_reg("PC", u32::from(run));
        self.sio.set_boot_sector(None);
    }

    fn read_word(&self, addr: u16) -> u16 {
        u16::from_le_bytes([
            peek(&self.mem, &self.io, addr),
            peek(&self.mem, &self.io, addr.wrapping_add(1)),
        ])
    }

    fn write_word(&mut self, addr: u16, v: u16) {
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            AtariBus {
                mem: &mut self.mem,
                io: &mut self.io,
            }
            .write(u32::from(addr) + i as u32, Size::Byte, u32::from(b));
        }
    }

    fn type_text(&mut self, text: &str) {
        let (events, _) = keyboard::typed_events(text, self.sched.now().0);
        for e in events {
            self.input.push(e);
        }
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            if f.space != CPU_SPACE {
                continue;
            }
            for i in 0..f.size.bytes() {
                let addr = f.addr.wrapping_add(i) & 0xffff;
                let v = (f.value >> (8 * i)) as u8;
                AtariBus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                }
                .write(addr, Size::Byte, u32::from(v));
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: cpu_hz(self.standard) * 4,
            active_pixels: gtia::WIDTH,
            total_pixels: u32::from(antic::CYCLES_PER_LINE) * 4,
            lines_per_field: u32::from(antic::lines(self.standard)),
            standard: self.standard,
            // The monitor jack carries composite and separate luma/chroma.
            output: OutputType::SVideo,
            field: FieldParity::Progressive,
        }
    }
}

/// Typing for tests and tools: queue key presses for `text`.
pub fn type_text(m: &mut Atari8, text: &str) {
    m.type_text(text);
}

fn regs(
    name: &'static str,
    base: u32,
    names: &[&'static str],
    value: impl Fn(u8) -> u8,
) -> ChipInfo {
    ChipInfo {
        name,
        regs: names
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.is_empty())
            .map(|(i, &n)| ChipReg {
                name: n,
                addr: Some(base + i as u32),
                value: u32::from(value(i as u8)),
                bits: 8,
            })
            .collect(),
    }
}

impl Machine for Atari8 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        let b = self.beam_pos();
        if b.line == 0 && b.cycle == 0 {
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

    /// RESET (the XL's RESET key is the system reset line): CPU, PIA (the
    /// OS ROM banks back in), ANTIC's NMIEN. Power-up also clears RAM and
    /// restarts the chips.
    fn reset(&mut self, kind: ResetKind) {
        let hard = kind == ResetKind::Hard;
        self.cpu.reset();
        self.io.pia.reset();
        self.mem.reset(hard);
        self.io.antic.reset();
        if hard {
            let s = self.standard;
            self.io.antic = Antic::new(s);
            self.io.gtia = Gtia::new(s);
            self.io.pokey = Pokey::new();
            // A program boots without BASIC, like with OPTION held.
            self.option_until = if self.basic_off || self.loader.is_some() {
                OPTION_HOLD_FIELDS
            } else {
                0
            };
            self.sio.power_on();
            self.sio
                .set_boot_sector(self.loader.as_ref().map(|_| loader::boot_sector()));
            if let Some(l) = &mut self.loader {
                l.next = 0;
                l.phase = Phase::Boot;
            }
        }
        self.input.clear();
        self.field_done_pending = false;
        self.refresh_ports();
        self.refresh_console();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: gtia::WIDTH,
            height: gtia::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.io.gtia.pixels(),
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.samples);
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
                big_endian: false,
            },
            MemSpace {
                id: ANTIC_SPACE,
                name: "antic",
                device: CPU,
                addr_bits: 16,
                big_endian: false,
            },
        ]
    }

    fn overlays(&self) -> Vec<OverlayInfo> {
        overlay::infos()
    }

    fn set_overlays(&mut self, ids: &[&str]) {
        self.overlay_rec = overlay::Recorder::new(ids).map(Box::new);
    }

    fn overlay(&self, id: &str) -> Option<Overlay> {
        let b = self.beam_pos();
        let mid_field = b.line != 0 || b.cycle != 0;
        self.overlay_rec.as_ref()?.overlay(id, mid_field)
    }

    fn frame_row(&self, line: u16) -> Option<u32> {
        overlay::frame_row(line)
    }

    fn gfx_views(&self) -> Vec<GfxViewInfo> {
        antic::gfx::views()
    }

    /// ANTIC/GTIA views, decoded from what ANTIC's DMA sees (reported in
    /// [`ANTIC_SPACE`]); `addr` options are ANTIC addresses.
    fn gfx_view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        let info = self.gfx_views().into_iter().find(|v| v.id == id)?;
        let ctx = antic::gfx::GfxContext {
            mem: &DmaView(&self.mem),
            space: ANTIC_SPACE,
            io_space: CPU_SPACE,
            palette: &self.palette,
        };
        Some(
            self.io
                .antic
                .gfx_view(&self.io.gtia, info.id, info.kind, info.name, &ctx, opts),
        )
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let io = &self.io;
        let a = &io.antic;
        let antic = ChipInfo {
            name: "antic",
            regs: [
                ("DMACTL", 0x0, a.dmactl),
                ("CHACTL", 0x1, a.chactl),
                ("DLISTL", 0x2, a.dlist as u8),
                ("DLISTH", 0x3, (a.dlist >> 8) as u8),
                ("HSCROL", 0x4, a.hscrol),
                ("VSCROL", 0x5, a.vscrol),
                ("PMBASE", 0x7, a.pmbase),
                ("CHBASE", 0x9, a.chbase),
                ("VCOUNT", 0xb, a.peek(0xb)),
                ("NMIEN", 0xe, a.nmien),
                ("NMIST", 0xf, a.peek(0xf)),
            ]
            .into_iter()
            .map(|(name, off, v)| ChipReg {
                name,
                addr: Some(0xd400 + off),
                value: u32::from(v),
                bits: 8,
            })
            .collect(),
        };
        let g = &io.gtia;
        let gtia_w: Vec<(&'static str, u8)> = vec![
            ("HPOSP0", g.hposp[0]),
            ("HPOSP1", g.hposp[1]),
            ("HPOSP2", g.hposp[2]),
            ("HPOSP3", g.hposp[3]),
            ("HPOSM0", g.hposm[0]),
            ("HPOSM1", g.hposm[1]),
            ("HPOSM2", g.hposm[2]),
            ("HPOSM3", g.hposm[3]),
            ("SIZEP0", g.sizep[0]),
            ("SIZEP1", g.sizep[1]),
            ("SIZEP2", g.sizep[2]),
            ("SIZEP3", g.sizep[3]),
            ("SIZEM", g.sizem),
            ("GRAFP0", g.grafp[0]),
            ("GRAFP1", g.grafp[1]),
            ("GRAFP2", g.grafp[2]),
            ("GRAFP3", g.grafp[3]),
            ("GRAFM", g.grafm),
            ("COLPM0", g.colpm[0]),
            ("COLPM1", g.colpm[1]),
            ("COLPM2", g.colpm[2]),
            ("COLPM3", g.colpm[3]),
            ("COLPF0", g.colpf[0]),
            ("COLPF1", g.colpf[1]),
            ("COLPF2", g.colpf[2]),
            ("COLPF3", g.colpf[3]),
            ("COLBK", g.colbk),
            ("PRIOR", g.prior),
            ("VDELAY", g.vdelay),
            ("GRACTL", g.gractl),
            ("", 0),
            ("CONSOL", g.peek(0x1f)),
        ];
        // Read side: collisions, triggers, PAL (same addresses).
        const GTIA_R: [&str; 21] = [
            "M0PF", "M1PF", "M2PF", "M3PF", "P0PF", "P1PF", "P2PF", "P3PF", "M0PL", "M1PL", "M2PL",
            "M3PL", "P0PL", "P1PL", "P2PL", "P3PL", "TRIG0", "TRIG1", "TRIG2", "TRIG3", "PAL",
        ];
        let mut gtia_regs: Vec<ChipReg> = gtia_w
            .into_iter()
            .enumerate()
            .filter(|(_, (n, _))| !n.is_empty())
            .map(|(i, (name, v))| ChipReg {
                name,
                addr: Some(0xd000 + i as u32),
                value: u32::from(v),
                bits: 8,
            })
            .collect();
        gtia_regs.extend(GTIA_R.iter().enumerate().map(|(i, &name)| ChipReg {
            name,
            addr: Some(0xd000 + i as u32),
            value: u32::from(g.peek(i as u8)),
            bits: 8,
        }));
        let gtia = ChipInfo {
            name: "gtia",
            regs: gtia_regs,
        };
        let p = &io.pokey;
        let pokey = ChipInfo {
            name: "pokey",
            regs: [
                ("AUDF1", 0x0, p.audf[0]),
                ("AUDC1", 0x1, p.audc[0]),
                ("AUDF2", 0x2, p.audf[1]),
                ("AUDC2", 0x3, p.audc[1]),
                ("AUDF3", 0x4, p.audf[2]),
                ("AUDC3", 0x5, p.audc[2]),
                ("AUDF4", 0x6, p.audf[3]),
                ("AUDC4", 0x7, p.audc[3]),
                ("AUDCTL", 0x8, p.audctl),
                ("KBCODE", 0x9, p.peek(0x9)),
                ("RANDOM", 0xa, p.peek(0xa)),
                ("SERIN", 0xd, p.serin),
                ("IRQEN", 0xe, p.irqen),
                ("IRQST", 0xe, p.peek(0xe)),
                ("SKCTL", 0xf, p.skctl),
                ("SKSTAT", 0xf, p.peek(0xf)),
                ("POT0", 0x0, p.peek(0x0)),
                ("POT1", 0x1, p.peek(0x1)),
                ("POT2", 0x2, p.peek(0x2)),
                ("POT3", 0x3, p.peek(0x3)),
                ("POT4", 0x4, p.peek(0x4)),
                ("POT5", 0x5, p.peek(0x5)),
                ("POT6", 0x6, p.peek(0x6)),
                ("POT7", 0x7, p.peek(0x7)),
                ("ALLPOT", 0x8, p.peek(0x8)),
            ]
            .into_iter()
            .map(|(name, off, v)| ChipReg {
                name,
                addr: Some(0xd200 + off),
                value: u32::from(v),
                bits: 8,
            })
            .collect(),
        };
        let pia = regs("pia", 0xd300, &["PORTA", "PORTB", "PACTL", "PBCTL"], |i| {
            io.pia.peek(pia_reg(u16::from(i)), io.porta_pins(), 0xff)
        });
        vec![antic, gtia, pokey, pia]
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
        use emu_core::ControlInfo;
        vec![
            ControlInfo::button(0, "POWER", false),
            ControlInfo::button(1, "RESET", false),
            ControlInfo::button(2, "START", self.io.gtia.console_keys & 1 != 0),
            ControlInfo::button(3, "SELECT", self.io.gtia.console_keys & 2 != 0),
            ControlInfo::button(4, "OPTION", self.option_key),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        const NAMES: [&str; 4] = [
            "joystick port 1",
            "joystick port 2",
            "joystick port 3",
            "joystick port 4",
        ];
        self.ports
            .iter()
            .enumerate()
            .map(|(i, dev)| PortInfo {
                id: PortId(i as u8),
                kind: PortKind::AtariJoy,
                name: NAMES[i],
                connected: dev.is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::AtariJoy {
            return Err(PortError::Incompatible {
                port: PortKind::AtariJoy,
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
        let mut slots: Vec<MediaSlotInfo> = self
            .sio
            .drives
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                d.as_ref().map(|d| MediaSlotInfo {
                    id: SlotId(i as u8),
                    name: DRIVE_NAMES[i],
                    kind: MediaKind::Disk,
                    inserted: d.has_disk(),
                })
            })
            .collect();
        slots.push(MediaSlotInfo {
            id: SLOT_PROGRAM,
            name: "program",
            kind: MediaKind::Program,
            inserted: self.program.inserted(),
        });
        slots.push(MediaSlotInfo {
            id: SLOT_TAPE,
            name: "tape",
            kind: MediaKind::Tape,
            inserted: self.recorder.has_tape(),
        });
        slots.push(MediaSlotInfo {
            id: SLOT_CARTRIDGE,
            name: "cartridge",
            kind: MediaKind::Cartridge,
            inserted: self.cart.is_inserted(),
        });
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_PROGRAM => Some(&mut self.program),
            SLOT_TAPE => Some(&mut self.recorder),
            SLOT_CARTRIDGE => Some(&mut self.cart),
            SlotId(i @ 0..=3) => self.sio.drives[usize::from(i)]
                .as_mut()
                .map(|d| d as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        // Version 4: the model and PIA port B inputs; one port chunk per
        // joystick port (four on the 400/800).
        w.chunk(*b"A8XL", 4, |w| {
            w.bool(self.field_done_pending);
            w.u64(self.option_until);
            w.bool(self.option_key);
            w.u8(self.io.porta);
            self.audio.save(w);
            w.u8(self.model as u8);
            w.u8(self.io.portb_in);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.io.antic.save(&mut w);
        self.io.gtia.save(&mut w);
        self.io.pokey.save(&mut w);
        self.io.pia.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"POR1", *b"POR2", *b"POR3", *b"POR4"]
            .into_iter()
            .take(self.ports.len())
            .enumerate()
        {
            w.chunk(tag, 1, |w| {
                w.bytes(
                    &self.ports[i]
                        .as_ref()
                        .map(|d| d.save_state())
                        .unwrap_or_default(),
                );
            });
        }
        self.sio.save(&mut w);
        Loader::save(&self.loader, &mut w);
        self.recorder.save(&mut w);
        w.chunk(*b"CART", 1, |w| w.bool(self.mem.cart.is_some()));
        if let Some(c) = &self.mem.cart {
            c.save(&mut w);
        }
        w.into_bytes()
    }

    /// Atomic: components load into copies; the port devices (not
    /// clonable) are restored from a backup if a later part fails.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            mem: self.mem.clone(),
            io: self.io.clone(),
            sched: self.sched.clone(),
            input: self.input.clone(),
            audio: self.audio,
        };
        let mut r = StateReader::new(s);
        let (version, mut c) = r.chunk(*b"A8XL", 4)?;
        let field_done_pending = c.bool()?;
        let option_until = c.u64()?;
        let option_key = c.bool()?;
        next.io.porta = c.u8()?;
        next.audio.load(&mut c)?;
        if version >= 4 {
            if c.u8()? != self.model as u8 {
                return Err(StateError::Invalid("model differs"));
            }
            next.io.portb_in = c.u8()?;
        } else if self.model != Model::A800Xl {
            return Err(StateError::Invalid("model differs"));
        }
        next.cpu.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.antic.load(&mut r)?;
        next.io.gtia.load(&mut r)?;
        next.io.pokey.load(&mut r)?;
        next.io.pia.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"POR1", *b"POR2", *b"POR3", *b"POR4"]
            .into_iter()
            .take(self.ports.len())
        {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        // Version 2: the SIO bus and the program loader. The bus loads
        // last (it has no copy), after everything that can fail.
        let (sio_state, loader) = if version >= 2 {
            let start = r.remaining();
            let mut probe = Sio::new(cpu_hz(self.standard));
            for (p, d) in probe.drives.iter_mut().zip(&self.sio.drives) {
                *p = d.as_ref().map(|d| SioDrive::new(d.model()));
            }
            probe.load(&mut r)?;
            let used = start - r.remaining();
            let loader = Loader::load(&mut r)?;
            (Some((s.len() - start, used)), loader)
        } else {
            (None, None)
        };
        // Version 3: the program recorder and the cartridge's banking.
        let recorder_state = if version >= 3 {
            let start = r.remaining();
            let mut probe = Recorder::new(cpu_hz(self.standard));
            probe.load(&mut r)?;
            let at = s.len() - start;
            let has_cart = r.chunk(*b"CART", 1).and_then(|(_, mut c)| c.bool())?;
            if has_cart != next.mem.cart.is_some() {
                return Err(StateError::Invalid("cartridge differs"));
            }
            if let Some(cart) = &mut next.mem.cart {
                cart.load(&mut r)?;
            }
            Some((at, start - r.remaining()))
        } else {
            None
        };
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
        if let Some((at, len)) = sio_state {
            self.sio.load(&mut StateReader::new(&s[at..at + len]))?;
        }
        if let Some((at, len)) = recorder_state {
            self.recorder
                .load(&mut StateReader::new(&s[at..at + len]))?;
        }
        self.loader = loader;
        // PORTB banking follows the restored PIA.
        next.mem.portb = next.io.portb();
        self.cpu = next.cpu;
        self.mem = next.mem;
        self.io = next.io;
        self.sched = next.sched;
        self.input = next.input;
        self.audio = next.audio;
        self.samples.clear();
        self.field_done_pending = field_done_pending;
        self.option_until = option_until;
        self.option_key = option_key;
        if let Some(rec) = &mut self.overlay_rec {
            rec.clear();
        }
        self.refresh_ports();
        Ok(())
    }
}

impl DebugTarget for Atari8 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space == ANTIC_SPACE {
            let mut v = 0;
            for i in 0..size.bytes() {
                let a = (addr.wrapping_add(i) & 0xffff) as u16;
                v |= u32::from(self.mem.read_dma(a)) << (8 * i);
            }
            return Some(v);
        }
        if space != CPU_SPACE {
            return None;
        }
        let bus = PeekBus(&self.mem, &self.io);
        let mut v = 0;
        for i in 0..size.bytes() {
            v |= bus.peek(addr.wrapping_add(i) & 0xffff, Size::Byte) << (8 * i);
        }
        Some(v)
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = (addr.wrapping_add(i) & 0xffff) as u16;
            match self.mem.area(a) {
                Area::Ram | Area::Extended(..) | Area::Io => AtariBus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                }
                .write(u32::from(a), Size::Byte, (v >> (8 * i)) & 0xff),
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.mem, &self.io), addr)
    }
}

#[cfg(test)]
mod tests;
