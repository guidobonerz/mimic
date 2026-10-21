//! Atari 2600 (Video Computer System).
//!
//! A 6507 CPU (13 address lines, no interrupts), the TIA (video, sound,
//! fire buttons and paddles; [`chips_atari::tia`]), a 6532 RIOT (128 bytes
//! RAM, joystick and console switch ports, timer; [`chips_atari::riot`])
//! and the cartridge. There is no system ROM: the program comes from the
//! cartridge.
//!
//! Memory map (A0–A12; A13–A15 are not connected):
//!
//! | Address        | Select          | Device                         |
//! | -------------- | --------------- | ------------------------------ |
//! | `$1000–$1FFF`  | A12 = 1         | cartridge                      |
//! | `$0000–$007F`  | A12 = 0, A7 = 0 | TIA (mirrored)                 |
//! | `$0080–$00FF`  | A7 = 1, A9 = 0  | RIOT RAM (mirrored at `$0180`) |
//! | `$0280–$029F`  | A7 = 1, A9 = 1  | RIOT ports and timer           |
//!
//! TIA reads drive D7 and D6 only; the other data lines keep what the
//! last bus cycle left on them.
//!
//! Models: "ntsc" (default; 3.579545 MHz colour clock, 262 lines) and
//! "pal" (3.546894 MHz, 312 lines); the CPU runs at a third of the colour
//! clock. Per CPU cycle: the CPU (held by WSYNC through RDY), the RIOT
//! timer, three TIA colour clocks.
//!
//! Options:
//! - `port1`, `port2` (left and right controller port): "joystick"
//!   (default), "paddles" (a pair; the host mouse turns paddle 0, the one
//!   on pin 5), "keypad", "driving" or "none".
//! - `tv`: "color" (default) or "bw"; `left`, `right`: difficulty switch
//!   "b" (default) or "a".
//! - `bank`: the cartridge's bank-switching scheme instead of the
//!   detected one: 2k, 4k, f8, f6, f4 (each also with "sc" for the
//!   Superchip, e.g. f8sc), fa, fe, e0, e7, 3f (see
//!   [`devices_cartridge::atari2600`]).
//! - `roms`: "stub" inserts a built-in test cartridge ([`stub`]).
//!
//! Controllers (pins of the 9-pin port → chips; left port / right port):
//! directions pins 1–4 → SWCHA D4–D7 / D0–D3 (also the keypad rows, which
//! the program drives as outputs); fire pin 6 → INPT4 / INPT5; pin 5 →
//! INPT0 / INPT2 and pin 9 → INPT1 / INPT3 (paddle pots, keypad columns).
//!
//! Console switches on SWCHB (keys, see [`keys`]): D0 RESET and D1 SELECT
//! (low while pressed), D3 colour (1) / black-and-white (0), D6 left and
//! D7 right difficulty (1 = A).
//!
//! Media slot: "cartridge" (raw `.a26`/`.bin` dumps, 2 KB–512 KB, bank
//! switching detected; inserting or removing one powers the console up
//! again). Without a cartridge the CPU has no
//! program and stays idle; the TIA free-runs a black picture.
//!
//! Sources: Steve Wright, "Stella Programmer's Guide" (1979); Kevin
//! Horton, "Cart Information"
//! (<https://www.qotile.net/minidig/docs/sizes.txt>); "Atari 2600
//! Specifications" (<https://www.astro.physik.uni-potsdam.de/~mwendt/2600scene/specs.html>).

#![forbid(unsafe_code)]

pub mod stub;

use chips_atari::riot::Riot;
use chips_atari::tia::{self, Tia};
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use devices_cartridge::atari2600::{Cart2600, Scheme};
use devices_cartridge::CartSlot;
use devices_input::joystick::pin;
use emu_core::audio::Decimator;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputEvent, InputQueue, InputTarget, MachineKey, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::Cycle;
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

/// Left and right controller ports.
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const SLOT_CARTRIDGE: SlotId = SlotId(0);

/// Machine key codes ([`InputEvent::Key`] on the keyboard target): the
/// console switches and the keypads in the ports.
pub mod keys {
    use emu_core::input::MachineKey;

    pub const RESET: MachineKey = MachineKey(0);
    pub const SELECT: MachineKey = MachineKey(1);
    /// Toggles: TV type, left and right difficulty.
    pub const TV_TYPE: MachineKey = MachineKey(2);
    pub const LEFT_DIFFICULTY: MachineKey = MachineKey(3);
    pub const RIGHT_DIFFICULTY: MachineKey = MachineKey(4);
    /// Keypad keys in reading order (1 2 3 4 5 6 7 8 9 * 0 #): left port
    /// from 16, right port from 32.
    pub const LEFT_KEYPAD: u16 = 16;
    pub const RIGHT_KEYPAD: u16 = 32;
}

/// SWCHB with the switches at rest: RESET and SELECT up, the unused
/// inputs high; colour and difficulties come from the options.
const SWITCHES_IDLE: u8 = 0x37;
const SWCHB_COLOUR: u8 = 0x08;
const SWCHB_LEFT_A: u8 = 0x40;
const SWCHB_RIGHT_A: u8 = 0x80;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "atari2600",
        name: "Atari 2600",
        models: &["ntsc", "pal"],
        create: |config| Ok(Box::new(Atari2600::new(config)?)),
    }
}

fn colour_clock_hz(standard: VideoStandard) -> u32 {
    match standard {
        VideoStandard::Ntsc => 3_579_545,
        VideoStandard::Pal => 3_546_894,
    }
}

fn cpu_hz(standard: VideoStandard) -> u32 {
    colour_clock_hz(standard) / 3
}

/// The chips behind the 6507's address lines.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Board {
    tia: Tia,
    riot: Riot,
    /// The cartridge as the CPU sees it (banking state); `None`: empty.
    cart: Option<Cart2600>,
    /// What the last bus cycle left on the data lines.
    data_bus: u8,
    /// SWCHA or SWACNT was written: the keypad rows may have changed.
    porta_written: bool,
}

impl Board {
    fn peek(&self, addr: u16) -> u8 {
        let a = addr & 0x1fff;
        if a & 0x1000 != 0 {
            self.cart.as_ref().map_or(self.data_bus, |c| c.peek(a))
        } else if a & 0x80 == 0 {
            (self.tia.peek(a as u8) & 0xc0) | (self.data_bus & 0x3f)
        } else {
            self.riot.peek(a)
        }
    }

    fn read(&mut self, addr: u16) -> u8 {
        let a = addr & 0x1fff;
        let from_cart = self.cart.as_mut().and_then(|c| c.access(a, None));
        let v = if a & 0x1000 != 0 {
            from_cart.unwrap_or(self.data_bus)
        } else if a & 0x80 == 0 {
            (self.tia.peek(a as u8) & 0xc0) | (self.data_bus & 0x3f)
        } else {
            self.riot.read(a)
        };
        if let Some(c) = &mut self.cart {
            c.snoop(a, v);
        }
        self.data_bus = v;
        v
    }

    fn write(&mut self, addr: u16, v: u8) {
        let a = addr & 0x1fff;
        if let Some(c) = &mut self.cart {
            c.access(a, Some(v));
        }
        if a & 0x1000 == 0 {
            if a & 0x80 == 0 {
                self.tia.write(a as u8, v);
            } else {
                self.riot.write(a, v);
                // A9 = 1, A2 = 0, A1 = 0: SWCHA or SWACNT.
                self.porta_written |= a & 0x206 == 0x200;
            }
        }
        if let Some(c) = &mut self.cart {
            c.snoop(a, v);
        }
        self.data_bus = v;
    }
}

impl Bus for Board {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        u32::from(Board::read(self, addr as u16))
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(Board::peek(self, addr as u16))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        Board::write(self, addr as u16, v as u8);
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

fn port_option(
    config: &MachineConfig,
    key: &str,
    hz: u32,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    let kind = PortKind::AtariJoy;
    match config
        .options
        .get(key)
        .map(String::as_str)
        .unwrap_or("joystick")
    {
        "joystick" => Ok(Some(Box::new(devices_input::Joystick::new(kind)))),
        // Paddle 0 of a pair is the one on pin 5: the mouse turns that one.
        "paddles" => Ok(Some(Box::new(
            devices_input::Paddles::for_port(kind).mouse_turns(1),
        ))),
        "keypad" => Ok(Some(Box::new(devices_input::Keypad::new()))),
        // About two turns (32 code steps) a second while held.
        "driving" => Ok(Some(Box::new(devices_input::Driving::new(
            u64::from(hz) / 32,
        )))),
        "none" => Ok(None),
        other => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' (joystick, paddles, keypad, driving or none)"
        ))),
    }
}

/// A two-way option (`key`): `on` sets `bit`, `off` clears it.
fn switch_option(
    config: &MachineConfig,
    key: &str,
    [on, off]: [&str; 2],
    default_on: bool,
    bit: u8,
) -> Result<u8, ConfigError> {
    match config.options.get(key).map(String::as_str) {
        None => Ok(if default_on { bit } else { 0 }),
        Some(v) if v == on => Ok(bit),
        Some(v) if v == off => Ok(0),
        Some(v) => Err(ConfigError::Invalid(format!(
            "{key} option '{v}' ({on} or {off})"
        ))),
    }
}

pub struct Atari2600 {
    standard: VideoStandard,
    cpu: Cpu6502,
    board: Board,
    sched: Scheduler,
    input: InputQueue,
    ports: [Option<Box<dyn PortDevice>>; 2],
    palette: [Rgb8; 128],
    audio: Decimator,
    samples: Vec<i16>,
    field_done_pending: bool,
    /// The cartridge as inserted; `board.cart` is the working copy.
    cart: CartSlot<Cart2600>,
    /// `roms=stub`: the built-in cartridge while the slot is empty.
    stub: Option<Cart2600>,
    /// Console switch levels (SWCHB inputs).
    switches: u8,
    /// `bank` option: the scheme for inserted cartridges.
    scheme: Option<(Scheme, bool)>,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Cpu6502,
    board: Board,
    sched: Scheduler,
    input: InputQueue,
    audio: Decimator,
}

impl Atari2600 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let standard = match config.model.as_deref() {
            None | Some("ntsc") => VideoStandard::Ntsc,
            Some("pal") => VideoStandard::Pal,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let stub = match config.options.get("roms").map(String::as_str) {
            None => false,
            Some("stub") => true,
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        };
        let scheme = match config.options.get("bank") {
            None => None,
            Some(name) => Some(Scheme::from_name(name).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "bank option '{name}' (2k, 4k, f8, f6, f4, f8sc, f6sc, f4sc, fa, fe, e0, e7, 3f)"
                ))
            })?),
        };
        let hz = cpu_hz(standard);
        let ports = [
            port_option(config, "port1", hz)?,
            port_option(config, "port2", hz)?,
        ];
        let switches = SWITCHES_IDLE
            | switch_option(config, "tv", ["color", "bw"], true, SWCHB_COLOUR)?
            | switch_option(config, "left", ["a", "b"], false, SWCHB_LEFT_A)?
            | switch_option(config, "right", ["a", "b"], false, SWCHB_RIGHT_A)?;
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Mos6507,
            ..CpuConfig::default()
        });
        cpu.reset();
        let mut riot = Riot::new();
        riot.set_port_b_input(switches);
        let mut m = Self {
            standard,
            cpu,
            board: Board {
                tia: Tia::new(standard),
                riot,
                cart: None,
                data_bus: 0,
                porta_written: false,
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            ports,
            palette: chips_atari::palette::tia(standard),
            audio: Decimator::new(cpu_hz(standard), emu_core::SAMPLE_RATE)
                .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
            samples: Vec::new(),
            field_done_pending: false,
            cart: devices_cartridge::atari2600::slot(),
            stub: None,
            switches,
            scheme,
        };
        if stub {
            m.stub = Some(
                Cart2600::parse(&stub::cartridge())
                    .map_err(|e| ConfigError::Invalid(format!("stub cartridge: {e}")))?,
            );
            m.board.cart = m.power_on_cart();
        }
        m.refresh_ports();
        Ok(m)
    }

    /// The cartridge at power-up: the inserted one (with the scheme of
    /// the `bank` option, if it fits), else the stub.
    fn power_on_cart(&self) -> Option<Cart2600> {
        let inserted = self.cart.cart().map(|c| {
            let mut c = match self.scheme {
                Some((s, sc)) => c.with_scheme(s, sc).unwrap_or_else(|_| c.clone()),
                None => c.clone(),
            };
            c.power_on();
            c
        });
        inserted.or_else(|| self.stub.clone())
    }

    fn beam_pos(&self) -> BeamPos {
        let t = &self.board.tia;
        BeamPos {
            frame: t.frame(),
            line: t.line(),
            cycle: u16::from(t.clock() / 3),
            hpos: u16::from(t.clock().saturating_sub(tia::HBLANK)),
        }
    }

    /// One CPU cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        // A cartridge went in or out: power up again.
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let result = if self.board.cart.is_some() {
            self.cpu.set_rdy(self.board.tia.rdy());
            let beam = self.beam_pos();
            let mut hooked = HookedBus {
                inner: &mut self.board,
                dbg: dbg.as_deref_mut(),
                space: CPU_SPACE,
                beam,
            };
            self.cpu.tick(&mut hooked)
        } else {
            CycleResult::Busy
        };
        self.board.riot.tick();
        let mut new_field = false;
        for _ in 0..3 {
            new_field |= self.board.tia.tick();
        }
        // Keypad rows right after the program sets them; time-dependent
        // devices (driving controller) once per line.
        if std::mem::take(&mut self.board.porta_written) || self.board.tia.clock() < 3 {
            self.refresh_ports();
        }
        if let Some(s) = self
            .audio
            .push(i32::from(self.board.tia.audio_level()) * 1000)
        {
            self.samples.push(s);
        }
        self.sched.tick();

        if let Some(dbg) = dbg {
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
                    self.control(id.0, value);
                }
            }
        }
        self.refresh_ports();
    }

    /// A console control from the panel: power, the RESET and SELECT
    /// switches (held while `value` is 1), the TV type and difficulty
    /// switches (set to position `value`).
    fn control(&mut self, id: u16, value: u8) {
        let set = |s: &mut u8, bit: u8, on: bool| {
            if on {
                *s |= bit;
            } else {
                *s &= !bit;
            }
        };
        match id {
            0 if value == 1 => emu_core::Machine::reset(self, ResetKind::Hard),
            1 => set(&mut self.switches, 0x01, value == 0),
            2 => set(&mut self.switches, 0x02, value == 0),
            3 => set(&mut self.switches, SWCHB_COLOUR, value == 0),
            4 => set(&mut self.switches, SWCHB_LEFT_A, value == 1),
            5 => set(&mut self.switches, SWCHB_RIGHT_A, value == 1),
            _ => {}
        }
    }

    /// A console switch, or a key of a keypad in a port.
    fn key(&mut self, key: MachineKey, down: bool) {
        let momentary = |s: &mut u8, bit: u8| {
            if down {
                *s &= !bit;
            } else {
                *s |= bit;
            }
        };
        match key {
            keys::RESET => momentary(&mut self.switches, 0x01),
            keys::SELECT => momentary(&mut self.switches, 0x02),
            keys::TV_TYPE if down => self.switches ^= SWCHB_COLOUR,
            keys::LEFT_DIFFICULTY if down => self.switches ^= SWCHB_LEFT_A,
            keys::RIGHT_DIFFICULTY if down => self.switches ^= SWCHB_RIGHT_A,
            MachineKey(k) => {
                let (port, base) = match k {
                    16..=27 => (0, keys::LEFT_KEYPAD),
                    32..=43 => (1, keys::RIGHT_KEYPAD),
                    _ => return,
                };
                if let Some(dev) = &mut self.ports[port] {
                    dev.input(&InputEvent::Key {
                        machine_key: MachineKey(k - base),
                        down,
                    });
                }
            }
        }
    }

    /// Controller pins into RIOT port A and the TIA inputs, the program's
    /// port A outputs (keypad rows) out to the controllers, the console
    /// switches into port B.
    fn refresh_ports(&mut self) {
        let now = self.sched.now();
        let riot = &self.board.riot;
        let driven_low = riot.ddra & !riot.ora;
        let mut swcha = 0xffu8;
        for i in 0..2 {
            // Up, down, left, right: D4–D7 (left port), D0–D3 (right).
            let shift = 4 - 4 * i;
            let Some(dev) = &mut self.ports[i] else {
                for n in [2 * i, 2 * i + 1] {
                    self.board.tia.set_pot(n as u8, u16::MAX);
                    self.board.tia.set_input(n as u8, true);
                }
                self.board.tia.set_input(4 + i as u8, true);
                continue;
            };
            dev.write_lines(
                PortLines {
                    pulled_low: u32::from((driven_low >> shift) & 0x0f),
                    analog: [u16::MAX; 2],
                },
                now,
            );
            let lines = dev.read_lines(now);
            let low = lines.pulled_low;
            swcha &= !(((low & 0x0f) as u8) << shift);
            let tia = &mut self.board.tia;
            tia.set_input(4 + i as u8, low & pin::FIRE == 0);
            // Pin 5 → INPT0/2, pin 9 → INPT1/3.
            let (pin5, pin9) = (2 * i as u8, 2 * i as u8 + 1);
            tia.set_pot(pin5, lines.analog[1]);
            tia.set_input(pin5, low & pin::BUTTON3 == 0);
            tia.set_pot(pin9, lines.analog[0]);
            tia.set_input(pin9, low & pin::BUTTON2 == 0);
        }
        self.board.riot.set_port_a_input(swcha);
        self.board.riot.set_port_b_input(self.switches);
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            if f.space != CPU_SPACE {
                continue;
            }
            for i in 0..f.size.bytes() {
                let addr = (f.addr.wrapping_add(i) & 0xffff) as u16;
                self.board.write(addr, (f.value >> (8 * i)) as u8);
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: colour_clock_hz(self.standard),
            active_pixels: tia::WIDTH,
            total_pixels: u32::from(tia::CLOCKS_PER_LINE),
            lines_per_field: tia::height(self.standard),
            standard: self.standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

fn chip(name: &'static str, regs: Vec<(&'static str, u32, u8)>) -> ChipInfo {
    ChipInfo {
        name,
        regs: regs
            .into_iter()
            .map(|(name, addr, v)| ChipReg {
                name,
                addr: Some(addr),
                value: u32::from(v),
                bits: 8,
            })
            .collect(),
    }
}

impl Machine for Atari2600 {
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

    /// The 2600 has no reset line (the RESET switch is read by the
    /// program): a soft reset restarts the CPU at its RESET vector, a hard
    /// reset powers the console up again.
    fn reset(&mut self, kind: ResetKind) {
        self.cpu.reset();
        if kind == ResetKind::Hard {
            self.board.tia.reset();
            self.board.riot.reset();
            self.board.cart = self.power_on_cart();
            self.board.data_bus = 0;
        }
        self.input.clear();
        self.field_done_pending = false;
        self.refresh_ports();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: tia::WIDTH,
            height: tia::height(self.standard),
            pixels: PixelData::Indexed {
                pixels: self.board.tia.pixels(),
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
        vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 16,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let t = &self.board.tia;
        let mut tia_regs: Vec<(&'static str, u32, u8)> = tia::W
            .iter()
            .enumerate()
            .filter(|(i, _)| ![2, 3, 0x10, 0x11, 0x12, 0x13, 0x14, 0x2a, 0x2b, 0x2c].contains(i))
            .map(|(i, &n)| (n, i as u32, t.regs[i]))
            .collect();
        tia_regs.extend(
            tia::R
                .iter()
                .enumerate()
                .map(|(i, &n)| (n, i as u32, Board::peek(&self.board, i as u16) & 0xc0)),
        );
        let r = &self.board.riot;
        let riot = chip(
            "riot",
            vec![
                ("SWCHA", 0x280, r.port_a()),
                ("SWACNT", 0x281, r.ddra),
                ("SWCHB", 0x282, r.port_b()),
                ("SWBCNT", 0x283, r.ddrb),
                ("INTIM", 0x284, r.timer),
                ("TIMINT", 0x285, r.flags()),
            ],
        );
        vec![chip("tia", tia_regs), riot]
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
            ControlInfo::button(1, "RESET", self.switches & 0x01 == 0),
            ControlInfo::button(2, "SELECT", self.switches & 0x02 == 0),
            ControlInfo::switch(
                3,
                "TV TYPE",
                &["COLOR", "B/W"],
                u8::from(self.switches & SWCHB_COLOUR == 0),
            ),
            ControlInfo::switch(
                4,
                "LEFT DIFFICULTY",
                &["B", "A"],
                u8::from(self.switches & SWCHB_LEFT_A != 0),
            ),
            ControlInfo::switch(
                5,
                "RIGHT DIFFICULTY",
                &["B", "A"],
                u8::from(self.switches & SWCHB_RIGHT_A != 0),
            ),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        [(PORT1, "left controller"), (PORT2, "right controller")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::AtariJoy,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
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
        vec![MediaSlotInfo {
            id: SLOT_CARTRIDGE,
            name: "cartridge",
            kind: MediaKind::Cartridge,
            inserted: self.cart.is_inserted(),
        }]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        (slot == SLOT_CARTRIDGE).then_some(&mut self.cart as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"A26M", 1, |w| {
            w.bool(self.field_done_pending);
            w.u8(self.board.data_bus);
            w.u8(self.switches);
            self.audio.save(w);
        });
        self.cpu.save(&mut w);
        self.board.tia.save(&mut w);
        self.board.riot.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"POR1", *b"POR2"].into_iter().enumerate() {
            w.chunk(tag, 1, |w| {
                w.bytes(
                    &self.ports[i]
                        .as_ref()
                        .map(|d| d.save_state())
                        .unwrap_or_default(),
                );
            });
        }
        w.chunk(*b"CART", 1, |w| w.bool(self.board.cart.is_some()));
        if let Some(c) = &self.board.cart {
            c.save(&mut w);
        }
        w.into_bytes()
    }

    /// Atomic: components load into copies; the port devices (not
    /// clonable) are restored from a backup if one of them fails.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            board: self.board.clone(),
            sched: self.sched.clone(),
            input: self.input.clone(),
            audio: self.audio,
        };
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"A26M", 1)?;
        let field_done_pending = c.bool()?;
        next.board.data_bus = c.u8()?;
        let switches = c.u8()?;
        next.audio.load(&mut c)?;
        next.cpu.load(&mut r)?;
        next.board.tia.load(&mut r)?;
        next.board.riot.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"POR1", *b"POR2"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        let has_cart = r.chunk(*b"CART", 1).and_then(|(_, mut c)| c.bool())?;
        if has_cart != next.board.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        if let Some(cart) = &mut next.board.cart {
            cart.load(&mut r)?;
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
        self.cpu = next.cpu;
        self.board = next.board;
        self.sched = next.sched;
        self.input = next.input;
        self.audio = next.audio;
        self.samples.clear();
        self.field_done_pending = field_done_pending;
        self.switches = switches;
        self.refresh_ports();
        Ok(())
    }
}

impl DebugTarget for Atari2600 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != CPU_SPACE {
            return None;
        }
        let mut v = 0;
        for i in 0..size.bytes() {
            let a = (addr.wrapping_add(i) & 0xffff) as u16;
            v |= u32::from(self.board.peek(a)) << (8 * i);
        }
        Some(v)
    }

    /// RAM, TIA and RIOT registers; the cartridge ROM cannot be changed.
    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = (addr.wrapping_add(i) & 0xffff) as u16;
            if a & 0x1000 != 0 {
                return false;
            }
            self.board.write(a, (v >> (8 * i)) as u8);
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&self.board, addr)
    }
}

#[cfg(test)]
mod tests;
