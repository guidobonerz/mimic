//! Commodore C16 and Plus/4: 7501/8501 CPU and the TED (video, sound,
//! timers, CPU clock).
//!
//! Machine ids: "c16" (16 KB RAM) and "plus4" (64 KB RAM, 3-plus-1
//! function ROM, ACIA and user port). Models: "pal" (default; TED 8360,
//! 1 773 447 Hz double clock, 312 lines) and "ntsc" (TED 7360,
//! 1 789 772 Hz, 262 lines); the NTSC model uses `roms["kernal-ntsc"]` if
//! given. Time ([`Machine::now`]) counts TED double clocks; the CPU runs in
//! the slots the TED leaves it (see `chips_cbm::ted`). The "cartridge"
//! slot holds the expansion port's C1/C2 ROMs (ROM banks 2 and 3).
//!
//! I/O at `$FD00–$FF3F`:
//!
//! | Range         | Chip                                                     |
//! | ------------- | -------------------------------------------------------- |
//! | `$FD00–$FD0F` | 6551 ACIA (Plus/4; registers stored, no serial line yet) |
//! | `$FD10–$FD1F` | 6529 user port latch (Plus/4)                            |
//! | `$FD30–$FD3F` | keyboard column latch                                    |
//! | `$FDD0–$FDDF` | ROM bank select (write)                                  |
//! | `$FF00–$FF3F` | TED; `$FF08` keyboard/joystick, `$FF3E/$FF3F` ROM/RAM    |
//!
//! CPU port (`$01`, Plus/4 schematic): bit 0 serial DATA out, 1 serial CLK
//! out, 2 serial ATN out (through 7406 inverters: 1 = line low), 3 cassette
//! motor, 4 cassette read, 6 serial CLK in, 7 serial DATA in (line level).
//!
//! Options: `ram` ("16k", "64k"; defaults per machine), `drive8` ("1541"
//! when `roms["dos1541"]` is given, "1571" with `roms["dos1571"]`, "1581"
//! with `roms["dos1581"]`,
//! "virtual" or "none"), `drive8_ppm` (drive crystal deviation),
//! `autostart` ("on"), `port1` ("joystick" default, "none"), `roms` = "stub".

#![forbid(unsafe_code)]

pub mod drive;
pub mod keyboard;
pub mod loader;
pub mod memory;
pub mod stub;

use chips_cbm::ted::{self, Ted};
use cpu_6502::{Config as CpuConfig, Cpu6502, IoPort, Variant};
use drives_cbm::IecLines;
use emu_core::frame::{FrameRef, PixelData, Rgb8};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::Cycle;
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

use devices_cartridge::slot::CartSlot;
use devices_cartridge::ted::TedCart;
use drive::{Drive8, DRIVE8, DRIVE_SPACE};
use drives_cbm::{DriveModel, DriveRoms};
use drives_other::Datasette;
use keyboard::Keyboard;
use loader::{Autostart, ProgramSlot, BASIC_START, BOOT_FRAMES};
use media::tap::TapMachine;
use memory::{Area, Memory, Roms, TedView, ROM_SIZE};

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's banked view of memory.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

/// TED register names `$FF00–$FF1F`.
const TED_REGS: [&str; 32] = [
    "T1LO",
    "T1HI",
    "T2LO",
    "T2HI",
    "T3LO",
    "T3HI",
    "CTRL1",
    "CTRL2",
    "KEYB",
    "IRR",
    "IMR",
    "RASTERCMP",
    "CURHI",
    "CURLO",
    "V1FREQ",
    "V2FREQ",
    "V2HI",
    "SOUND",
    "BITMAP",
    "CHARBASE",
    "MATRIX",
    "BG0",
    "BG1",
    "BG2",
    "BG3",
    "BORDER",
    "CPOSHI",
    "CPOSLO",
    "LINEHI",
    "LINELO",
    "HPOS",
    "VSUB",
];

fn entry_for(id: &'static str, name: &'static str) -> MachineEntry {
    MachineEntry {
        id,
        name,
        models: &["pal", "ntsc"],
        create: match id {
            "c16" => |config| Ok(Box::new(TedMachine::new(Variant16::C16, config)?)),
            _ => |config| Ok(Box::new(TedMachine::new(Variant16::Plus4, config)?)),
        },
    }
}

pub fn c16_entry() -> MachineEntry {
    entry_for("c16", "Commodore 16")
}

pub fn plus4_entry() -> MachineEntry {
    entry_for("plus4", "Commodore Plus/4")
}

/// Which machine of the family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant16 {
    C16,
    Plus4,
}

fn rom(config: &MachineConfig, role: &str) -> Result<Box<[u8; ROM_SIZE]>, ConfigError> {
    let data = config
        .roms
        .get(role)
        .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
    let boxed: Box<[u8]> = data.clone().into_boxed_slice();
    boxed.try_into().map_err(|_| {
        ConfigError::Invalid(format!(
            "{role} ROM must be {ROM_SIZE} bytes, got {}",
            data.len()
        ))
    })
}

fn load_roms(
    config: &MachineConfig,
    variant: Variant16,
    model: ted::Model,
) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => Ok(Roms {
            kernal: stub::kernal(),
            basic: Box::new([0xff; ROM_SIZE]),
            function: None,
        }),
        Some(other) => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => {
            let kernal = if model == ted::Model::Mos7360 && config.roms.contains_key("kernal-ntsc")
            {
                "kernal-ntsc"
            } else {
                "kernal"
            };
            let function = match variant {
                Variant16::Plus4 => {
                    Some((rom(config, "function-lo")?, rom(config, "function-hi")?))
                }
                Variant16::C16 => None,
            };
            Ok(Roms {
                kernal: rom(config, kernal)?,
                basic: rom(config, "basic")?,
                function,
            })
        }
    }
}

/// The ACIA's registers (data, status, command, control): stored only,
/// transmitter always empty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Acia {
    regs: [u8; 4],
}

impl Acia {
    fn peek(&self, reg: u8) -> u8 {
        match reg & 3 {
            // Status: transmit data register empty.
            1 => 0x10,
            r => self.regs[usize::from(r)],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Io {
    pub ted: Ted,
    pub keyboard: Keyboard,
    /// `$FD30` latch: keyboard columns (0 = selected).
    pub key_select: u8,
    /// Last value written to `$FF08` (joystick select).
    pub joy_select: u8,
    /// Pins pulled low by the devices in the two joystick ports.
    pub port_lines: [u32; 2],
    /// Serial bus levels after the last drive update.
    pub iec: IecLines,
    acia: Acia,
    user_port: u8,
    /// Datasette READ (CPU port bit 4).
    pub cassette_read: bool,
    /// A datasette key is down (`$FD10` bit 2 low).
    pub cassette_sense: bool,
}

impl Io {
    /// `$FF08`: keyboard rows of the selected columns, with the selected
    /// joysticks' lines.
    fn keyboard_read(&self) -> u8 {
        let joy = |lines: u32, fire: u8| -> u8 {
            let dirs = (lines & 0x0f) as u8;
            let fired = if lines & (1 << 5) != 0 { fire } else { 0 };
            !(dirs | fired)
        };
        let mut rows = 0xff;
        if self.joy_select & 0x04 == 0 {
            rows &= joy(self.port_lines[0], 0x40);
        }
        if self.joy_select & 0x02 == 0 {
            rows &= joy(self.port_lines[1], 0x80);
        }
        self.keyboard.rows(self.key_select, rows)
    }
}

/// Side-effect-free read (also the CPU's read outside the TED and port).
fn peek_byte(mem: &Memory, io: &Io, port: &IoPort, plus4: bool, a: u16) -> u8 {
    match mem.area(a) {
        Area::Port => port.read(a as u8, port_pins(io)),
        Area::Ram => mem.ram_read(a),
        Area::Rom => mem.rom_read(a),
        Area::Io => match a {
            0xfd00..=0xfd0f if plus4 => io.acia.peek(a as u8),
            // The cassette sense line reads on bit 2 (KERNAL `$E31C`),
            // through the 6529 on the Plus/4.
            0xfd10..=0xfd1f => {
                let sense = if io.cassette_sense { 0x04 } else { 0 };
                (if plus4 { io.user_port } else { 0xff }) & !sense
            }
            0xfd30..=0xfd3f => io.key_select,
            0xff08 => io.keyboard_read(),
            0xff13 => (io.ted.peek(0x13) & 0xfe) | u8::from(mem.rom_on),
            0xff00..=0xff1f => io.ted.peek(a as u8),
            _ => 0xff,
        },
    }
}

/// Input levels on the CPU port: serial CLK and DATA in, cassette read.
fn port_pins(io: &Io) -> u8 {
    let mut pins = 0xff;
    if !io.cassette_read {
        pins &= !0x10;
    }
    if io.iec.clk {
        pins &= !0x40;
    }
    if io.iec.data {
        pins &= !0x80;
    }
    pins
}

/// The CPU's bus.
struct TedBus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
    port: &'a mut IoPort,
    plus4: bool,
}

impl Bus for TedBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        u32::from(match a {
            0xff00..=0xff1f if a != 0xff08 && a != 0xff13 => self.io.ted.read(a as u8),
            _ => peek_byte(self.mem, self.io, self.port, self.plus4, a),
        })
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_byte(
            self.mem,
            self.io,
            self.port,
            self.plus4,
            addr as u16,
        ))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        let v = v as u8;
        match self.mem.area(a) {
            Area::Port => self.port.write(a as u8, v),
            Area::Ram | Area::Rom => self.mem.ram_write(a, v),
            Area::Io => match a {
                0xfd00..=0xfd0f if self.plus4 => self.io.acia.regs[usize::from(a & 3)] = v,
                0xfd10..=0xfd1f if self.plus4 => self.io.user_port = v,
                0xfd30..=0xfd3f => self.io.key_select = v,
                0xfdd0..=0xfddf => self.mem.banks = (a & 0x0f) as u8,
                0xff08 => self.io.joy_select = v,
                0xff3e => self.mem.rom_on = true,
                0xff3f => self.mem.rom_on = false,
                0xff00..=0xff1f => self.io.ted.write(a as u8, v),
                _ => {}
            },
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Read-only view for disassembly.
struct PeekBus<'a> {
    mem: &'a Memory,
    io: &'a Io,
    port: &'a IoPort,
    plus4: bool,
}

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_byte(
            self.mem,
            self.io,
            self.port,
            self.plus4,
            addr as u16,
        ))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

pub const SLOT_DRIVE8: SlotId = SlotId(0);
pub const SLOT_PROGRAM: SlotId = SlotId(1);
pub const SLOT_TAPE: SlotId = SlotId(2);
pub const SLOT_CARTRIDGE: SlotId = SlotId(3);
const PORT_NAMES: [&str; 2] = ["joystick 1", "joystick 2"];

pub struct TedMachine {
    variant: Variant16,
    cpu: Cpu6502,
    port: IoPort,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
    ports: [Option<Box<dyn PortDevice>>; 2],
    drive: Option<Drive8>,
    program: ProgramSlot,
    /// The datasette (1531) on the cassette port.
    tape: Datasette,
    /// The expansion port's ROM cartridges (a copy in `mem` maps them).
    cart: CartSlot<TedCart>,
    autostart: Autostart,
    palette: [Rgb8; 128],
    field_done_pending: bool,
    /// TED frame of the last reset; programs wait [`BOOT_FRAMES`] from here.
    boot_frame: u64,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Cpu6502,
    port: IoPort,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
}

impl TedMachine {
    pub fn new(variant: Variant16, config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("pal") => ted::Model::Mos8360,
            Some("ntsc") => ted::Model::Mos7360,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let double_hz = model.timing().double_hz();
        let ram_mask = match (config.options.get("ram").map(String::as_str), variant) {
            (None, Variant16::C16) | (Some("16k"), _) => 0x3fff,
            (None, Variant16::Plus4) | (Some("64k"), _) => 0xffff,
            (Some(other), _) => return Err(ConfigError::Invalid(format!("ram option '{other}'"))),
        };
        // The Plus/4 and C16 ports have no pot lines: joysticks only.
        let port1 = devices_input::control_port_option(
            "port1",
            config.options.get("port1").map(String::as_str),
            "joystick",
            false,
        )?;
        let stub = config.options.get("roms").is_some_and(|r| r == "stub");
        let drive_roms = if stub {
            DriveRoms::default()
        } else {
            DriveRoms::from_map(&config.roms)
        };
        let mut drive = Drive8::from_option(
            config.options.get("drive8").map(String::as_str),
            drive_roms,
            DriveModel::C1541,
            8,
            DRIVE8,
            DRIVE_SPACE,
            double_hz,
        )?;
        if let Some(d) = &mut drive {
            let ppm = drives_cbm::serial::crystal_ppm(
                config.options.get("drive8_ppm").map(String::as_str),
            )?;
            d.set_crystal_ppm(ppm, double_hz);
        }
        let autostart = match config.options.get("autostart").map(String::as_str) {
            None | Some("off") => Autostart::Off,
            Some("on") => Autostart::Armed,
            Some(other) => return Err(ConfigError::Invalid(format!("autostart option '{other}'"))),
        };
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Mos7501,
            ..CpuConfig::default()
        });
        cpu.reset();
        Ok(Self {
            variant,
            cpu,
            port: IoPort::new(Variant::Mos7501.io_port_mask().unwrap_or(0xff)),
            mem: Memory::new(load_roms(config, variant, model)?, ram_mask),
            io: Io {
                ted: Ted::new(model),
                keyboard: Keyboard::default(),
                key_select: 0xff,
                joy_select: 0xff,
                port_lines: [0; 2],
                iec: IecLines::default(),
                acia: Acia::default(),
                user_port: 0xff,
                cassette_read: true,
                cassette_sense: false,
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            ports: [port1, None],
            drive,
            program: ProgramSlot::default(),
            tape: Datasette::new(TapMachine::C16, double_hz),
            cart: devices_cartridge::ted::slot(),
            autostart,
            palette: ted::palette::palette(),
            field_done_pending: false,
            boot_frame: 0,
        })
    }

    fn plus4(&self) -> bool {
        self.variant == Variant16::Plus4
    }

    fn beam_pos(&self) -> BeamPos {
        self.io.ted.beam()
    }

    /// What the machine pulls on the serial bus (CPU port bits 0–2).
    fn iec_out(&self) -> IecLines {
        let o = self.port.outputs();
        IecLines {
            atn: o & 0x04 != 0,
            clk: o & 0x02 != 0,
            data: o & 0x01 != 0,
            srq: false,
        }
    }

    /// One TED double clock. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        // A cartridge went in or out: the machine powers up again.
        if self.cart.take_changed() {
            self.mem.cart = self.cart.cart().cloned();
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let out = self.io.ted.tick(&TedView(&self.mem));
        let mut cpu_result = None;
        if out.cpu {
            self.cpu.set_irq(self.io.ted.irq());
            let beam = self.beam_pos();
            let plus4 = self.plus4();
            let mut bus = TedBus {
                mem: &mut self.mem,
                io: &mut self.io,
                port: &mut self.port,
                plus4,
            };
            let mut hooked = HookedBus {
                inner: &mut bus,
                dbg: dbg.as_deref_mut(),
                space: CPU_SPACE,
                beam,
            };
            cpu_result = Some((self.cpu.tick(&mut hooked), self.cpu.take_interrupt()));
        }
        // Datasette: CPU port bit 1 WRITE, bit 3 MOTOR (output low = on),
        // READ on bit 4, SENSE at `$FD10`.
        let port_out = self.port.outputs();
        self.tape
            .set_motor(self.port.ddr & 0x08 != 0 && port_out & 0x08 == 0);
        self.tape.set_write(port_out & 0x02 != 0);
        self.tape.tick();
        self.io.cassette_read = self.tape.read();
        self.io.cassette_sense = self.tape.sense();
        // Serial bus: the drive sees what the machine drives now.
        let host = self.iec_out();
        let mut drive_stop = None;
        self.io.iec = match &mut self.drive {
            Some(d) => {
                let (bus, stop) = d.run(host, dbg.as_deref_mut());
                drive_stop = stop;
                bus
            }
            None => host,
        };
        self.sched.tick();
        let new_field = out.frame_end;
        if new_field {
            self.frame_tasks();
        }

        if let Some(dbg) = dbg {
            if let Some((result, entered)) = cpu_result {
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
        if drive_stop.is_some() {
            self.field_done_pending |= new_field;
            return drive_stop;
        }
        match cpu_result {
            Some((CycleResult::Jammed, _)) => Some(StopReason::CpuJam {
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
                if let emu_core::input::InputEvent::Key { machine_key, down } = ev.event {
                    self.io.keyboard.set(machine_key, down);
                }
            }
            InputTarget::Port(p) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&ev.event);
                }
                self.refresh_port(p);
            }
        }
    }

    fn refresh_port(&mut self, p: PortId) {
        let now = self.sched.now();
        if let Some(lines) = self.io.port_lines.get_mut(usize::from(p.0)) {
            *lines = self.ports[usize::from(p.0)]
                .as_ref()
                .map_or(0, |d| d.read_lines(now).pulled_low);
        }
    }

    /// The 40 × 25 text screen (character codes at matrix base + `$400`).
    fn screen(&self) -> Vec<u8> {
        let base = self.io.ted.matrix_base().wrapping_add(0x400);
        (0..1000u16)
            .map(|i| self.mem.ram_read(base.wrapping_add(i)))
            .collect()
    }

    /// Program injection and autostart, once BASIC has booted.
    fn frame_tasks(&mut self) {
        if self.io.ted.beam().frame.saturating_sub(self.boot_frame) < BOOT_FRAMES {
            return;
        }
        if let Some(prg) = self.program.pending.take() {
            self.inject(&prg);
            if prg.load == BASIC_START {
                self.type_text("RUN\r");
            }
        }
        match self.autostart {
            Autostart::Armed => {
                let has_disk = self.drive.as_ref().is_some_and(|d| d.has_disk());
                if has_disk {
                    self.type_text("LOAD\"*\",8\r");
                    self.autostart = Autostart::Loading;
                } else {
                    self.autostart = Autostart::Done;
                }
            }
            Autostart::Loading => {
                if loader::loaded(&self.screen()) {
                    self.type_text("RUN\r");
                    self.autostart = Autostart::Done;
                }
            }
            Autostart::Off | Autostart::Done => {}
        }
    }

    fn type_text(&mut self, text: &str) {
        let (events, _) = keyboard::typed_events(text, self.sched.now().0);
        for e in events {
            self.input.push(e);
        }
    }

    /// Put a program into RAM like the KERNAL LOAD does and set BASIC's
    /// end-of-program pointers (VARTAB/ARYTAB/STREND, and `$9D/$9E`).
    fn inject(&mut self, prg: &media::Prg) {
        let start = usize::from(prg.load);
        let len = prg.data.len().min(0x10000 - start);
        for (i, &b) in prg.data[..len].iter().enumerate() {
            self.mem.ram_write((start + i) as u16, b);
        }
        let end = ((start + len) as u16).to_le_bytes();
        for ptr in [0x2d, 0x2f, 0x31, 0x9d] {
            self.mem.ram_write(ptr, end[0]);
            self.mem.ram_write(ptr + 1, end[1]);
        }
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        let plus4 = self.plus4();
        for f in dbg.per_frame_freezes() {
            if f.space != CPU_SPACE {
                continue;
            }
            for i in 0..f.size.bytes() {
                let addr = f.addr.wrapping_add(i) & 0xffff;
                let v = (f.value >> (8 * i)) as u8;
                TedBus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                    port: &mut self.port,
                    plus4,
                }
                .write(addr, Size::Byte, u32::from(v));
            }
        }
    }
}

impl Machine for TedMachine {
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

    /// The RESET line reaches the CPU, the memory banking and (through the
    /// serial bus) drive 8. The TED has no reset input; it is new only on
    /// power-up.
    fn reset(&mut self, kind: ResetKind) {
        let hard = kind == ResetKind::Hard;
        self.cpu.reset();
        self.port = IoPort::new(Variant::Mos7501.io_port_mask().unwrap_or(0xff));
        self.mem.reset(hard);
        if hard {
            self.io.ted = Ted::new(self.io.ted.model());
        }
        if let Some(d) = &mut self.drive {
            d.reset(hard);
        }
        self.input.clear();
        if self.autostart == Autostart::Loading {
            self.autostart = Autostart::Done;
        }
        self.field_done_pending = false;
        self.boot_frame = self.io.ted.beam().frame;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        let t = self.io.ted.timing();
        FrameRef {
            width: ted::WIDTH,
            height: t.height,
            pixels: PixelData::Indexed {
                pixels: self.io.ted.pixels(),
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: t.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.io.ted.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let mut spaces = vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 16,
            big_endian: false,
        }];
        if self.drive.as_ref().is_some_and(|d| d.is_true()) {
            spaces.push(MemSpace {
                id: DRIVE_SPACE,
                name: "drive8",
                device: DRIVE8,
                addr_bits: 16,
                big_endian: false,
            });
        }
        spaces
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, addr: Option<u32>, value: u8, bits| ChipReg {
            name,
            addr,
            value: u32::from(value),
            bits,
        };
        vec![
            ChipInfo {
                name: "ted",
                regs: TED_REGS
                    .iter()
                    .enumerate()
                    .map(|(i, &name)| {
                        let a = 0xff00 + i as u16;
                        let v = peek_byte(&self.mem, &self.io, &self.port, self.plus4(), a);
                        reg(name, Some(u32::from(a)), v, 8)
                    })
                    .collect(),
            },
            ChipInfo {
                name: "memory",
                regs: vec![
                    reg("PORT_DDR", Some(0), self.port.ddr, 8),
                    reg("PORT_DATA", Some(1), self.port.data, 8),
                    reg("ROM", None, u8::from(self.mem.rom_on), 1),
                    reg("BANKS", Some(0xfdd0), self.mem.banks, 4),
                    reg("KEYSEL", Some(0xfd30), self.io.key_select, 8),
                ],
            },
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        let mut d = vec![CPU];
        if self.drive.as_ref().is_some_and(|d| d.is_true()) {
            d.push(DRIVE8);
        }
        d
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if dev == CPU {
            return Some(self as &mut dyn DebugTarget);
        }
        match &mut self.drive {
            Some(d) if dev == DRIVE8 => d.debug_target(),
            _ => None,
        }
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    fn ports(&self) -> Vec<PortInfo> {
        (0..2)
            .map(|i| PortInfo {
                id: PortId(i as u8),
                kind: PortKind::CbmControl,
                name: PORT_NAMES[i],
                connected: self.ports[i].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let slot = self
            .ports
            .get_mut(usize::from(port.0))
            .ok_or(PortError::NoSuchPort(port))?;
        if dev.kind() != PortKind::CbmControl {
            return Err(PortError::Incompatible {
                port: PortKind::CbmControl,
                device: dev.kind(),
            });
        }
        if slot.is_some() {
            return Err(PortError::Occupied(port));
        }
        *slot = Some(dev);
        self.refresh_port(port);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        let dev = self.ports.get_mut(usize::from(port.0))?.take();
        self.refresh_port(port);
        dev
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut slots = Vec::new();
        if let Some(d) = &self.drive {
            slots.push(MediaSlotInfo {
                id: SLOT_DRIVE8,
                name: "drive 8",
                kind: MediaKind::Disk,
                inserted: d.has_disk(),
            });
        }
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
            inserted: self.tape.has_tape(),
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
            SLOT_DRIVE8 => self.drive.as_mut().map(|d| d.media_slot()),
            SLOT_PROGRAM => Some(&mut self.program),
            SLOT_TAPE => Some(&mut self.tape),
            SLOT_CARTRIDGE => Some(&mut self.cart),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"TEDS", 2, |w| {
            w.bool(self.field_done_pending);
            w.u8(self.autostart.to_u8());
            w.u64(self.boot_frame);
            w.u8(self.io.key_select);
            w.u8(self.io.joy_select);
            w.u32(self.io.port_lines[0]);
            w.u32(self.io.port_lines[1]);
            w.u64(self.io.keyboard.matrix);
            w.bool(self.io.iec.atn);
            w.bool(self.io.iec.clk);
            w.bool(self.io.iec.data);
            w.raw(&self.io.acia.regs);
            w.u8(self.io.user_port);
        });
        self.cpu.save(&mut w);
        self.port.save(&mut w);
        self.mem.save(&mut w);
        self.io.ted.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        self.tape.save(&mut w);
        w.chunk(*b"DRV8", 1, |w| w.bool(self.drive.is_some()));
        if let Some(d) = &self.drive {
            d.save(&mut w);
        }
        w.chunk(*b"PORT", 1, |w| {
            for p in &self.ports {
                w.bytes(&p.as_ref().map(|d| d.save_state()).unwrap_or_default());
            }
        });
        w.into_bytes()
    }

    /// Atomic: components load into copies; the drive and the port devices
    /// (not clonable) are restored from backups if a later part fails.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            port: self.port,
            mem: self.mem.clone(),
            io: self.io.clone(),
            sched: self.sched.clone(),
            input: self.input.clone(),
        };
        let mut r = StateReader::new(s);
        let (version, mut c) = r.chunk(*b"TEDS", 2)?;
        let field_done_pending = c.bool()?;
        let autostart =
            Autostart::from_u8(c.u8()?).ok_or(StateError::Invalid("autostart state"))?;
        let boot_frame = c.u64()?;
        next.io.key_select = c.u8()?;
        next.io.joy_select = c.u8()?;
        next.io.port_lines = [c.u32()?, c.u32()?];
        next.io.keyboard.matrix = c.u64()?;
        next.io.iec = IecLines {
            atn: c.bool()?,
            clk: c.bool()?,
            data: c.bool()?,
            srq: false,
        };
        c.raw_into(&mut next.io.acia.regs)?;
        next.io.user_port = c.u8()?;
        next.cpu.load(&mut r)?;
        next.port.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.ted.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;
        // The datasette (not clonable; version 2 on): restored if a later
        // part fails.
        let tape_backup = {
            let mut w = StateWriter::new();
            self.tape.save(&mut w);
            w.into_bytes()
        };
        if version >= 2 {
            self.tape.load(&mut r)?;
        }
        let restore_tape = |tape: &mut Datasette| {
            let _ = tape.load(&mut StateReader::new(&tape_backup));
        };

        let drive_chunk = r.chunk(*b"DRV8", 1).and_then(|(_, mut c)| c.bool());
        if drive_chunk != Ok(self.drive.is_some()) {
            restore_tape(&mut self.tape);
            return Err(drive_chunk
                .err()
                .unwrap_or(StateError::Invalid("drive 8 configuration differs")));
        }
        let drive_backup = self.drive.as_ref().map(|d| {
            let mut w = StateWriter::new();
            d.save(&mut w);
            w.into_bytes()
        });
        if let Some(d) = &mut self.drive {
            if let Err(e) = d.load(&mut r) {
                restore_tape(&mut self.tape);
                return Err(e);
            }
        }
        let restore_drive = |drive: &mut Option<Drive8>| {
            if let (Some(d), Some(b)) = (drive, &drive_backup) {
                let _ = d.load(&mut StateReader::new(b));
            }
        };
        let port_states = match r
            .chunk(*b"PORT", 1)
            .and_then(|(_, mut c)| Ok([c.bytes()?, c.bytes()?]))
        {
            Ok(p) => p,
            Err(e) => {
                restore_drive(&mut self.drive);
                restore_tape(&mut self.tape);
                return Err(e);
            }
        };
        let backups: Vec<Vec<u8>> = self
            .ports
            .iter()
            .map(|p| p.as_ref().map(|d| d.save_state()).unwrap_or_default())
            .collect();
        for (dev, state) in self.ports.iter_mut().zip(port_states) {
            if let (Some(dev), false) = (dev, state.is_empty()) {
                if let Err(e) = dev.load_state(state) {
                    for (dev, backup) in self.ports.iter_mut().zip(&backups) {
                        if let Some(d) = dev {
                            let _ = d.load_state(backup);
                        }
                    }
                    restore_drive(&mut self.drive);
                    restore_tape(&mut self.tape);
                    return Err(e);
                }
            }
        }
        self.cpu = next.cpu;
        self.port = next.port;
        self.mem = next.mem;
        self.io = next.io;
        self.io.cassette_read = self.tape.read();
        self.io.cassette_sense = self.tape.sense();
        self.sched = next.sched;
        self.input = next.input;
        self.field_done_pending = field_done_pending;
        self.autostart = autostart;
        self.boot_frame = boot_frame;
        Ok(())
    }
}

impl DebugTarget for TedMachine {
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
            let b = peek_byte(&self.mem, &self.io, &self.port, self.plus4(), a);
            v |= u32::from(b) << (8 * i);
        }
        Some(v)
    }

    /// Pokes into ROM space write the RAM underneath, like the CPU does.
    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        let plus4 = self.plus4();
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i) & 0xffff;
            TedBus {
                mem: &mut self.mem,
                io: &mut self.io,
                port: &mut self.port,
                plus4,
            }
            .write(a, Size::Byte, (v >> (8 * i)) & 0xff);
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(
            &PeekBus {
                mem: &self.mem,
                io: &self.io,
                port: &self.port,
                plus4: self.plus4(),
            },
            addr,
        )
    }
}

#[cfg(test)]
mod tests;
