//! Commodore 64.
//!
//! Phase 2 builds this up chip by chip. Present so far: 6510 with its I/O
//! port, PLA banking, RAM, ROMs, colour RAM, VIC-II, both CIAs
//! the IRQ/NMI/RDY wiring, keyboard, control ports, a true-emulated 1541 as
//! drive 8, a "program" slot for PRG/T64 and the SID (`options["sid"]` =
//! "6581" or "8580"), producing 48 kHz mono audio, the datasette and the
//! expansion port's "cartridge" slot (CRT images, `devices_cartridge`;
//! inserting or removing one powers the machine up again).
//!
//! Drive 8 (`options["drive8"]`): "1541" (true drive, `roms["dos1541"]`,
//! 16 KB; the default when given), "1571" (`roms["dos1571"]`, 32 KB; in its
//! 1541 mode, a C64 has no fast serial port), "1581" (`roms["dos1581"]`, D81
//! disks), "virtual" (the default without
//! DOS ROMs) or "none"; `drive8_ppm` sets the drive crystal's deviation.
//! `options["autostart"] = "on"` loads and runs the disk in drive 8 after boot.
//!
//! Models: "pal" (default; 6569, 985 248 Hz, 312 lines × 63 cycles, TOD 50 Hz)
//! and "ntsc" (6567R8, 1 022 727 Hz, 263 lines × 65 cycles, TOD 60 Hz). The
//! KERNAL tells them apart by itself (raster line count at boot); the stub
//! KERNAL always sets up PAL timer values.
//!
//! CIA revision: options["cia"] = "6526" (default, original C64) or
//! "6526a" (C64C).
//!
//! ROMs come from `MachineConfig::roms` ("kernal", "basic", "chargen";
//! user-supplied, never in the repo). With `options["roms"] = "stub"` a
//! tiny built-in KERNAL replacement is used instead, for tests without ROMs.

#![forbid(unsafe_code)]

pub mod drive;
pub mod io;
pub mod keyboard;
pub mod loader;
pub mod memory;
pub use chips_cbm::pla;
pub mod stub;

use chips_cbm::cia::CiaModel;
use chips_cbm::sid::SidModel;
use chips_cbm::vic2;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use emu_core::frame::{FrameRef, PixelData, Rgb8};
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

use devices_cartridge::slot::ExpansionSlot;
use devices_cartridge::{CartLines, ExpansionCart};
use drive::{Drive8, DRIVE8, DRIVE_SPACE};
use drives_cbm::{DriveModel, DriveRoms};
use drives_other::Datasette;
use io::{Io, VicView};
use loader::{Autostart, ProgramSlot, BOOT_FRAMES};
use media::tap::TapMachine;
use memory::{Memory, Roms, BASIC_SIZE, CHARGEN_SIZE};
use pla::Bank;

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's banked view of memory.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The 64 KB of RAM, independent of banking.
pub const RAM_SPACE: MemSpaceId = MemSpaceId(1);
/// The 1 KB × 4 bit colour RAM.
pub const COLOR_SPACE: MemSpaceId = MemSpaceId(2);

/// VIC-II register names $D000–$D02E (C64 Programmer's Reference Guide).
const VIC_REGS: [&str; 47] = [
    "M0X", "M0Y", "M1X", "M1Y", "M2X", "M2Y", "M3X", "M3Y", "M4X", "M4Y", "M5X", "M5Y", "M6X",
    "M6Y", "M7X", "M7Y", "MSBX", "CR1", "RASTER", "LPX", "LPY", "MXE", "CR2", "MXYE", "MEMPTR",
    "IRR", "IMR", "MXDP", "MXMC", "MXXE", "MXM", "MXD", "EC", "B0C", "B1C", "B2C", "B3C", "MM0",
    "MM1", "M0C", "M1C", "M2C", "M3C", "M4C", "M5C", "M6C", "M7C",
];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "c64",
        name: "Commodore 64",
        models: &["pal", "ntsc"],
        create: |config| Ok(Box::new(C64::new(config)?)),
    }
}

/// Interrupt sources on the open-collector IRQ and NMI lines.
pub mod irq {
    pub const VIC: u8 = 1 << 0;
    pub const CIA1: u8 = 1 << 1;
    pub const CARTRIDGE: u8 = 1 << 2;
}

pub mod nmi {
    pub const CIA2: u8 = 1 << 0;
    pub const RESTORE: u8 = 1 << 1;
    pub const CARTRIDGE: u8 = 1 << 2;
}

/// Wired-OR interrupt lines: a line is active while any source pulls it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interrupts {
    pub irq: u8,
    pub nmi: u8,
}

impl Interrupts {
    pub fn set_irq(&mut self, source: u8, active: bool) {
        if active {
            self.irq |= source;
        } else {
            self.irq &= !source;
        }
    }

    pub fn set_nmi(&mut self, source: u8, active: bool) {
        if active {
            self.nmi |= source;
        } else {
            self.nmi &= !source;
        }
    }
}

/// I/O1 and I/O2 (`$DE00–$DFFF`) belong to the cartridge port.
fn is_cart_io(addr: u16) -> bool {
    (0xde00..=0xdfff).contains(&addr)
}

/// Side-effect-free CPU-view read.
fn cpu_peek(mem: &Memory, io: &Io, cart: Option<&dyn ExpansionCart>, addr: u16) -> u8 {
    let from_cart = match mem.bank(addr) {
        Bank::RomL => cart.and_then(|c| c.peek_roml(addr)),
        Bank::RomH => cart.and_then(|c| c.peek_romh(addr)),
        Bank::Io if is_cart_io(addr) => cart.and_then(|c| c.peek_io(addr)),
        _ => None,
    };
    match mem.read(addr) {
        Some(v) => v,
        None => from_cart.unwrap_or_else(|| match mem.bank(addr) {
            Bank::Io if !is_cart_io(addr) => io.peek(mem, addr),
            _ => io.open_bus(),
        }),
    }
}

/// The CPU's bus: PLA-resolved memory, I/O and the cartridge.
struct C64Bus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
    cart: Option<&'a mut (dyn ExpansionCart + 'static)>,
}

impl Bus for C64Bus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        if let Some(v) = self.mem.read(a) {
            return u32::from(v);
        }
        let bank = self.mem.bank(a);
        let from_cart = match (&mut self.cart, bank) {
            (Some(c), Bank::RomL) => c.read_roml(a),
            (Some(c), Bank::RomH) => c.read_romh(a),
            (Some(c), Bank::Io) if is_cart_io(a) => c.read_io(a),
            _ => None,
        };
        u32::from(from_cart.unwrap_or_else(|| match bank {
            Bank::Io if !is_cart_io(a) => self.io.read(self.mem, a),
            _ => self.io.open_bus(),
        }))
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(cpu_peek(
            self.mem,
            self.io,
            self.cart.as_deref(),
            addr as u16,
        ))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let (a, v) = (addr as u16, v as u8);
        let bank = self.mem.bank(a);
        if let Some(c) = &mut self.cart {
            match bank {
                Bank::RomL => c.write_roml(a, v),
                Bank::RomH => c.write_romh(a, v),
                Bank::Io if is_cart_io(a) => c.write_io(a, v),
                _ => {}
            }
        }
        if !self.mem.write(a, v) {
            self.io.write(self.mem, a, v);
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// The physical cell a CPU read of `a` gets its value from, if hooks can
/// be bound to it: RAM (not the 6510 port at `$00`/`$01`) or colour RAM.
fn physical_read(mem: &Memory, a: u16) -> Option<(MemSpaceId, u32)> {
    match mem.bank(a) {
        Bank::Ram if a >= 2 => Some((RAM_SPACE, u32::from(a))),
        Bank::Io if (0xd800..0xdc00).contains(&a) => Some((COLOR_SPACE, u32::from(a & 0x3ff))),
        _ => None,
    }
}

/// The physical cell a CPU write of `a` lands in: RAM also under ROM
/// (writes go through), colour RAM in the I/O area.
fn physical_write(mem: &Memory, a: u16) -> Option<(MemSpaceId, u32)> {
    match mem.bank(a) {
        Bank::Ram | Bank::Basic | Bank::Kernal | Bank::Char => Some((RAM_SPACE, u32::from(a))),
        Bank::Io if (0xd800..0xdc00).contains(&a) => Some((COLOR_SPACE, u32::from(a & 0x3ff))),
        _ => None,
    }
}

/// The CPU bus with debug hooks on the CPU view and on the physical memory
/// an access lands in, so freezes and watchpoints on RAM and colour RAM
/// hold whatever the banking ("freezes respect banking"). Where both
/// apply, the CPU view is outermost. With nothing armed this costs a few
/// length checks per access (golden rule 5).
struct HookedC64Bus<'a, 'b> {
    bus: &'a mut C64Bus<'b>,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl HookedC64Bus<'_, '_> {
    fn physical_armed(dbg: &DebugHooks, write: bool) -> bool {
        if write {
            dbg.write_armed(RAM_SPACE) || dbg.write_armed(COLOR_SPACE)
        } else {
            dbg.read_armed(RAM_SPACE) || dbg.read_armed(COLOR_SPACE)
        }
    }
}

impl Bus for HookedC64Bus<'_, '_> {
    #[inline]
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let mut v = self.bus.read(addr, size);
        let Some(dbg) = self.dbg.as_deref_mut() else {
            return v;
        };
        if Self::physical_armed(dbg, false) {
            if let Some((space, pa)) = physical_read(self.bus.mem, addr as u16) {
                if dbg.read_armed(space) {
                    v = dbg.on_read(space, pa, size, v);
                    dbg.trace_read(space, pa, size, v, self.beam);
                }
            }
        }
        if dbg.read_armed(CPU_SPACE) {
            v = dbg.on_read(CPU_SPACE, addr, size, v);
            dbg.trace_read(CPU_SPACE, addr, size, v, self.beam);
        }
        v
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.bus.peek(addr, size)
    }

    #[inline]
    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            let v = v & size.mask();
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, size, v, self.beam);
                if !dbg.on_write(CPU_SPACE, addr, size, v) {
                    return;
                }
            }
            if Self::physical_armed(dbg, true) {
                if let Some((space, pa)) = physical_write(self.bus.mem, addr as u16) {
                    if dbg.write_armed(space) {
                        dbg.trace_write(space, pa, size, v, self.beam);
                        if !dbg.on_write(space, pa, size, v) {
                            return;
                        }
                    }
                }
            }
        }
        self.bus.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Read-only view for disassembly and debugger peeks.
struct PeekBus<'a>(&'a Memory, &'a Io, Option<&'a dyn ExpansionCart>);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(cpu_peek(self.0, self.1, self.2, addr as u16))
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

fn load_roms(config: &MachineConfig) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => Ok(Roms {
            kernal: stub::kernal(),
            basic: Box::new([0xff; BASIC_SIZE]),
            chargen: Box::new([0; CHARGEN_SIZE]),
        }),
        Some(other) => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => Ok(Roms {
            kernal: rom(config, "kernal")?,
            basic: rom(config, "basic")?,
            chargen: rom(config, "chargen")?,
        }),
    }
}

/// Control port 1 and 2 (`PortId(0)`, `PortId(1)`).
pub const CONTROL_PORT_1: PortId = PortId(0);
pub const CONTROL_PORT_2: PortId = PortId(1);
const PORT_NAMES: [&str; 2] = ["control port 1", "control port 2"];

pub const SLOT_DRIVE8: SlotId = SlotId(0);
pub const SLOT_PROGRAM: SlotId = SlotId(1);
pub const SLOT_TAPE: SlotId = SlotId(2);
/// The expansion port (C64 CRT images).
pub const SLOT_CARTRIDGE: SlotId = SlotId(3);

pub struct C64 {
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    ints: Interrupts,
    sched: Scheduler,
    input: InputQueue,
    ports: [Option<Box<dyn PortDevice>>; 2],
    drive: Option<Drive8>,
    program: ProgramSlot,
    /// The datasette on the cassette port.
    tape: Datasette,
    /// The expansion port.
    cart: ExpansionSlot,
    autostart: Autostart,
    palette: [Rgb8; 16],
    field_done_pending: bool,
    /// VIC-II frame of the last reset; programs wait [`BOOT_FRAMES`] from here.
    boot_frame: u64,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    ints: Interrupts,
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
}

impl C64 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let vic = match config.model.as_deref() {
            None | Some("pal") => vic2::Model::Mos6569,
            Some("ntsc") => vic2::Model::Mos6567R8,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let cpu_hz = vic.timing().cpu_hz();
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Mos6510,
            ..CpuConfig::default()
        });
        cpu.reset();
        let cia = match config.options.get("cia").map(String::as_str) {
            None | Some("6526") => CiaModel::Mos6526,
            Some("6526a") => CiaModel::Mos6526A,
            Some(other) => return Err(ConfigError::Invalid(format!("cia option '{other}'"))),
        };
        // Devices: `options["port1"]`/`["port2"]` = "joystick", "paddles",
        // "mouse1351" or "none"; by default a joystick in control port 2.
        let option = |name: &str| config.options.get(name).map(String::as_str);
        let port1 = devices_input::control_port_option("port1", option("port1"), "none", true)?;
        let port2 = devices_input::control_port_option("port2", option("port2"), "joystick", true)?;
        let sid = match config.options.get("sid").map(String::as_str) {
            None | Some("6581") => SidModel::Mos6581,
            Some("8580") => SidModel::Mos8580,
            Some(other) => return Err(ConfigError::Invalid(format!("sid option '{other}'"))),
        };
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
            cpu_hz,
        )?;
        if let Some(d) = &mut drive {
            let ppm = drives_cbm::serial::crystal_ppm(
                config.options.get("drive8_ppm").map(String::as_str),
            )?;
            d.set_crystal_ppm(ppm, cpu_hz);
        }
        let autostart = match config.options.get("autostart").map(String::as_str) {
            None | Some("off") => Autostart::Off,
            Some("on") => Autostart::Armed,
            Some(other) => return Err(ConfigError::Invalid(format!("autostart option '{other}'"))),
        };
        let mut m = Self {
            cpu,
            mem: Memory::new(load_roms(config)?),
            io: Io::new(vic, cia, sid),
            ints: Interrupts::default(),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            ports: [port1, port2],
            drive,
            program: ProgramSlot::default(),
            tape: Datasette::new(TapMachine::C64, cpu_hz),
            cart: devices_cartridge::slot::expansion_slot(),
            autostart,
            palette: vic2::palette::palette(),
            field_done_pending: false,
            boot_frame: 0,
        };
        // The devices' lines from the start (paddles sit at mid travel).
        m.refresh_port(PortId(0));
        m.refresh_port(PortId(1));
        Ok(m)
    }

    pub fn interrupts_mut(&mut self) -> &mut Interrupts {
        &mut self.ints
    }

    fn beam_pos(&self) -> BeamPos {
        self.io.vic.beam()
    }

    /// The cartridge after the CPU access: it ticks, and its lines reach
    /// the PLA and the interrupt inputs.
    fn cartridge_cycle(&mut self) {
        let lines = match self.cart.cart_mut() {
            Some(c) => {
                c.tick();
                c.lines()
            }
            None => CartLines::default(),
        };
        // Line levels: true = high (not pulled).
        if (self.mem.game, self.mem.exrom) != (!lines.game, !lines.exrom) {
            self.mem.set_cartridge_lines(!lines.game, !lines.exrom);
        }
        self.ints.set_irq(irq::CARTRIDGE, lines.irq);
        self.ints.set_nmi(nmi::CARTRIDGE, lines.nmi);
    }

    /// One CPU cycle (φ2). Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        // A cartridge went in or out: the machine powers up again.
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        // φ1: VIC-II access; its BA decides whether the CPU may read.
        let bank = self.io.vic_bank();
        self.io.vic.phi1(&VicView {
            mem: &self.mem,
            bank,
            cart: self.cart.cart().map(|c| c.as_ref()),
        });
        self.cpu.set_rdy(self.io.vic.ba());
        // CIAs clock before the CPU access: a read sees this cycle's state.
        self.io.tick_cias();
        self.io.tick_sid();
        // Lines are sampled after the CIA clock: an NMI raised now is an edge
        // even if the CPU's ICR read in this cycle releases it (Lorenz icr01).
        self.update_interrupts();
        self.cpu.set_irq(self.ints.irq != 0);
        self.cpu.set_nmi(self.ints.nmi != 0);
        let beam = self.beam_pos();
        // Without AEC the halted CPU does not reach the bus at all.
        let result = if !self.io.vic.aec() {
            CycleResult::Busy
        } else {
            let mut bus = C64Bus {
                mem: &mut self.mem,
                io: &mut self.io,
                cart: self.cart.cart_mut().map(|c| c.as_mut()),
            };
            let mut hooked = HookedC64Bus {
                bus: &mut bus,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.tick(&mut hooked)
        };
        let entered = self.cpu.take_interrupt();
        // φ2: VIC-II's own phi2 accesses, pixel output, beam advance.
        let bank = self.io.vic_bank();
        let new_field = self.io.vic.phi2(&VicView {
            mem: &self.mem,
            bank,
            cart: self.cart.cart().map(|c| c.as_ref()),
        });
        self.cartridge_cycle();
        // Serial bus: the drive sees what the C64 drives now.
        let c64 = drive::c64_lines(self.io.cia2.port_a_out());
        let mut drive_stop = None;
        self.io.iec = match &mut self.drive {
            Some(d) => {
                let (bus, stop) = d.run(c64, dbg.as_deref_mut());
                drive_stop = stop;
                bus
            }
            None => c64,
        };
        // Datasette: port bit 3 WRITE, bit 5 MOTOR (output low = on), bit 4
        // SENSE; READ on CIA1 /FLAG.
        let port = &self.mem.port;
        let out = port.outputs();
        self.tape.set_motor(port.ddr & 0x20 != 0 && out & 0x20 == 0);
        self.tape.set_write(out & 0x08 != 0);
        self.tape.tick();
        self.io.cia1.set_flag(self.tape.read());
        self.mem.cassette_sense = self.tape.sense();
        self.update_interrupts();
        self.sched.tick();
        if new_field {
            self.frame_tasks();
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
        if drive_stop.is_some() {
            self.field_done_pending |= new_field;
            return drive_stop;
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
        let i = usize::from(p.0);
        if i < 2 {
            let lines = self.ports[i].as_ref().map(|d| d.read_lines(now));
            self.io.port_lines[i] = lines.map_or(0, |l| l.pulled_low);
            self.io.port_analog[i] = lines.map_or([u16::MAX; 2], |l| l.analog);
        }
    }

    /// Program injection and autostart, once BASIC has booted.
    fn frame_tasks(&mut self) {
        if self.io.vic.beam().frame.saturating_sub(self.boot_frame) < BOOT_FRAMES {
            return;
        }
        if let Some(prg) = self.program.pending.take() {
            self.inject(&prg);
            if prg.load == 0x0801 {
                self.type_text("RUN\r");
            }
        }
        match self.autostart {
            Autostart::Armed => {
                let has_disk = self.drive.as_ref().is_some_and(|d| d.has_disk());
                if has_disk {
                    self.type_text("LOAD\"*\",8,1\r");
                    self.autostart = Autostart::Loading;
                } else {
                    self.autostart = Autostart::Done;
                }
            }
            Autostart::Loading => {
                if loader::loaded(&self.mem.ram[0x0400..0x07e8]) {
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

    /// Put a program into RAM like the KERNAL LOAD does and set the BASIC
    /// end-of-program pointers (VARTAB/ARYTAB/STREND, and $AE/$AF).
    fn inject(&mut self, prg: &media::Prg) {
        let load = usize::from(prg.load);
        self.mem.ram[load..load + prg.data.len()].copy_from_slice(&prg.data);
        let end = prg.end() as u16;
        for ptr in [0x2d, 0x2f, 0x31, 0xae] {
            self.mem.ram[ptr..ptr + 2].copy_from_slice(&end.to_le_bytes());
        }
    }

    fn update_interrupts(&mut self) {
        let (vic, cia1, cia2) = (self.io.vic.irq(), self.io.cia1.irq(), self.io.cia2.irq());
        self.ints.set_irq(irq::VIC, vic);
        self.ints.set_irq(irq::CIA1, cia1);
        self.ints.set_nmi(nmi::CIA2, cia2);
        self.ints.set_nmi(nmi::RESTORE, self.io.keyboard.restore);
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            for i in 0..f.size.bytes() {
                let addr = f.addr.wrapping_add(i);
                let v = (f.value >> (8 * i)) as u8;
                match f.space {
                    CPU_SPACE => C64Bus {
                        mem: &mut self.mem,
                        io: &mut self.io,
                        cart: self.cart.cart_mut().map(|c| c.as_mut()),
                    }
                    .write(addr & 0xffff, Size::Byte, u32::from(v)),
                    RAM_SPACE => self.mem.ram[(addr & 0xffff) as usize] = v,
                    COLOR_SPACE => self.mem.color[(addr & 0x3ff) as usize] = v & 0x0f,
                    _ => {}
                }
            }
        }
    }
}

impl Machine for C64 {
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

    /// The RESET line reaches the 6510, both CIAs, the SID and (through the
    /// serial bus) drive 8, but not the VIC-II.
    fn reset(&mut self, kind: ResetKind) {
        let hard = kind == ResetKind::Hard;
        self.cpu.reset();
        self.mem.reset(hard);
        self.io.reset(hard);
        if let Some(d) = &mut self.drive {
            d.reset(hard);
        }
        self.ints = Interrupts::default();
        if let Some(c) = self.cart.cart_mut() {
            c.reset();
        }
        self.cartridge_cycle();
        self.input.clear();
        if self.autostart == Autostart::Loading {
            self.autostart = Autostart::Done;
        }
        self.field_done_pending = false;
        self.boot_frame = self.io.vic.beam().frame;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        let t = self.io.vic.timing();
        FrameRef {
            width: t.width,
            height: t.height,
            pixels: PixelData::Indexed {
                pixels: self.io.vic.pixels(),
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: t.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.io.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let space = |id, name, addr_bits| MemSpace {
            id,
            name,
            device: CPU,
            addr_bits,
            big_endian: false,
        };
        let mut spaces = vec![
            space(CPU_SPACE, "cpu", 16),
            space(RAM_SPACE, "ram", 16),
            space(COLOR_SPACE, "colorram", 10),
        ];
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
        let cia = |name, base: u32, c: &chips_cbm::cia::Cia| {
            let r16 = |name, addr: u32, value: u16| ChipReg {
                name,
                addr: Some(addr),
                value: u32::from(value),
                bits: 16,
            };
            ChipInfo {
                name,
                regs: vec![
                    reg("PRA", Some(base), c.pra, 8),
                    reg("PRB", Some(base + 1), c.prb, 8),
                    reg("DDRA", Some(base + 2), c.ddra, 8),
                    reg("DDRB", Some(base + 3), c.ddrb, 8),
                    r16("TA", base + 4, c.ta.counter),
                    r16("TA_LATCH", base + 4, c.ta.latch),
                    r16("TB", base + 6, c.tb.counter),
                    r16("TB_LATCH", base + 6, c.tb.latch),
                    reg("TOD_10THS", Some(base + 8), c.tod.time[0], 8),
                    reg("TOD_SEC", Some(base + 9), c.tod.time[1], 8),
                    reg("TOD_MIN", Some(base + 10), c.tod.time[2], 8),
                    reg("TOD_HR", Some(base + 11), c.tod.time[3], 8),
                    reg("ICR", Some(base + 13), c.peek(13, 0xff, 0xff), 8),
                    reg("IMR", None, c.mask(), 8),
                    reg("CRA", Some(base + 14), c.ta.cr, 8),
                    reg("CRB", Some(base + 15), c.tb.cr, 8),
                ],
            }
        };
        vec![
            ChipInfo {
                name: "pla",
                regs: vec![
                    reg("DDR", Some(0), self.mem.port.ddr, 8),
                    reg("DATA", Some(1), self.mem.port.data, 8),
                    reg("PINS", None, self.mem.port.outputs(), 6),
                    reg("MODE", None, self.mem.mode(), 5),
                    reg("GAME", None, u8::from(self.mem.game), 1),
                    reg("EXROM", None, u8::from(self.mem.exrom), 1),
                ],
            },
            cia("cia1", 0xdc00, &self.io.cia1),
            cia("cia2", 0xdd00, &self.io.cia2),
            ChipInfo {
                name: "vic2",
                regs: VIC_REGS
                    .iter()
                    .enumerate()
                    .map(|(i, &name)| {
                        reg(name, Some(0xd000 + i as u32), self.io.vic.peek(i as u8), 8)
                    })
                    .collect(),
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
        w.chunk(*b"C64S", 6, |w| {
            w.u8(self.ints.irq);
            w.u8(self.ints.nmi);
            w.bool(self.field_done_pending);
            w.u8(self.autostart.to_u8());
            w.u64(self.boot_frame);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.io.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        self.tape.save(&mut w);
        w.chunk(*b"CART", 1, |w| w.bool(self.cart.is_inserted()));
        if let Some(c) = self.cart.cart() {
            c.save(&mut w);
        }
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

    /// Atomic: components load into copies; port devices (not clonable)
    /// are restored from their own backup if a later one fails.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            mem: self.mem.clone(),
            io: self.io.clone(),
            ints: self.ints,
            sched: self.sched.clone(),
            input: self.input.clone(),
            field_done_pending: self.field_done_pending,
        };
        let mut r = StateReader::new(s);
        let (version, mut c) = r.chunk(*b"C64S", 6)?;
        if version < 3 {
            return Err(StateError::UnsupportedVersion {
                tag: *b"C64S",
                version,
            });
        }
        next.ints = Interrupts {
            irq: c.u8()?,
            nmi: c.u8()?,
        };
        next.field_done_pending = c.bool()?;
        let autostart =
            Autostart::from_u8(c.u8()?).ok_or(StateError::Invalid("autostart state"))?;
        // Version 3 states predate resets: booted at power-on.
        let boot_frame = if version >= 4 { c.u64()? } else { 0 };
        next.cpu.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;
        // The datasette (not clonable): from version 5, restored if a later
        // part fails.
        let tape_backup = {
            let mut w = StateWriter::new();
            self.tape.save(&mut w);
            w.into_bytes()
        };
        if version >= 5 {
            self.tape.load(&mut r)?;
        }
        // The cartridge likewise, from version 6.
        let cart_backup = self.cart.cart().map(|c| {
            let mut w = StateWriter::new();
            c.save(&mut w);
            w.into_bytes()
        });
        let restore_tape = |tape: &mut Datasette, cart: &mut ExpansionSlot| {
            let _ = tape.load(&mut StateReader::new(&tape_backup));
            if let (Some(c), Some(b)) = (cart.cart_mut(), &cart_backup) {
                let _ = c.load(&mut StateReader::new(b));
            }
        };
        if version >= 6 {
            let present = r.chunk(*b"CART", 1).and_then(|(_, mut c)| c.bool());
            let loaded = match (present, self.cart.cart_mut()) {
                (Ok(true), Some(c)) => c.load(&mut r),
                (Ok(false), None) => Ok(()),
                (Err(e), _) => Err(e),
                _ => Err(StateError::Invalid("cartridge differs")),
            };
            if let Err(e) = loaded {
                restore_tape(&mut self.tape, &mut self.cart);
                return Err(e);
            }
        }

        // Drive 8 (not clonable): keep a backup in case a later part fails.
        let drive_chunk = r.chunk(*b"DRV8", 1).and_then(|(_, mut c)| c.bool());
        if drive_chunk != Ok(self.drive.is_some()) {
            restore_tape(&mut self.tape, &mut self.cart);
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
                restore_tape(&mut self.tape, &mut self.cart);
                return Err(e);
            }
        }
        let restore_drive = |drive: &mut Option<Drive8>| {
            if let (Some(d), Some(b)) = (drive, &drive_backup) {
                let _ = d.load(&mut StateReader::new(b));
            }
        };

        let Ok((_, mut c)) = r.chunk(*b"PORT", 1) else {
            restore_drive(&mut self.drive);
            restore_tape(&mut self.tape, &mut self.cart);
            return Err(StateError::Missing(*b"PORT"));
        };
        let port_states = match (c.bytes(), c.bytes()) {
            (Ok(a), Ok(b)) => [a, b],
            (Err(e), _) | (_, Err(e)) => {
                restore_drive(&mut self.drive);
                restore_tape(&mut self.tape, &mut self.cart);
                return Err(e);
            }
        };
        let backups: Vec<Vec<u8>> = self
            .ports
            .iter()
            .map(|p| p.as_ref().map(|d| d.save_state()).unwrap_or_default())
            .collect();
        let mut failed = None;
        for (dev, state) in self.ports.iter_mut().zip(port_states) {
            if let (Some(dev), false) = (dev, state.is_empty()) {
                if let Err(e) = dev.load_state(state) {
                    failed = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = failed {
            // A device that failed kept its state; restore the others.
            for (dev, backup) in self.ports.iter_mut().zip(&backups) {
                if let Some(d) = dev {
                    let _ = d.load_state(backup);
                }
            }
            restore_drive(&mut self.drive);
            restore_tape(&mut self.tape, &mut self.cart);
            return Err(e);
        }
        self.cpu = next.cpu;
        self.mem = next.mem;
        self.mem.cassette_sense = self.tape.sense();
        self.io = next.io;
        self.ints = next.ints;
        self.sched = next.sched;
        self.input = next.input;
        self.field_done_pending = next.field_done_pending;
        self.autostart = autostart;
        self.boot_frame = boot_frame;
        Ok(())
    }
}

impl DebugTarget for C64 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE => Some(
                    PeekBus(&self.mem, &self.io, self.cart.cart().map(|c| c.as_ref()))
                        .peek(a & 0xffff, Size::Byte),
                ),
                RAM_SPACE => Some(u32::from(self.mem.ram[(a & 0xffff) as usize])),
                COLOR_SPACE => Some(u32::from(self.mem.color[(a & 0x3ff) as usize])),
                _ => None,
            }
        };
        let mut v = 0;
        for i in 0..size.bytes() {
            v |= byte(addr.wrapping_add(i))? << (8 * i);
        }
        Some(v)
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let b = (v >> (8 * i)) as u8;
            match space {
                CPU_SPACE => {
                    let a = (a & 0xffff) as u16;
                    match self.mem.bank(a) {
                        Bank::Basic | Bank::Kernal | Bank::Char | Bank::RomL | Bank::RomH => {
                            return false
                        }
                        _ => C64Bus {
                            mem: &mut self.mem,
                            io: &mut self.io,
                            cart: self.cart.cart_mut().map(|c| c.as_mut()),
                        }
                        .write(u32::from(a), Size::Byte, u32::from(b)),
                    }
                }
                RAM_SPACE => self.mem.ram[(a & 0xffff) as usize] = b,
                COLOR_SPACE => self.mem.color[(a & 0x3ff) as usize] = b & 0x0f,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(
            &PeekBus(&self.mem, &self.io, self.cart.cart().map(|c| c.as_ref())),
            addr,
        )
    }
}

#[cfg(test)]
mod tests;
