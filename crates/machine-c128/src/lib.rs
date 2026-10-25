//! Commodore 128.
//!
//! 8502 (1 or 2 MHz) and Z80 sharing the bus under the MMU 8722, VIC-IIe
//! (40 columns) and VDC 8563 (80 columns, its own 16 or 64 KB RAM), SID,
//! two CIAs, 128 KB RAM, C64 mode, drive 8 (a true-emulated 1571 or 1581
//! with fast serial transfers, a 1541, or the virtual drive), a "program"
//! slot, the datasette and a "cartridge" slot for C64 cartridges (the
//! KERNAL sees GAME/EXROM in the MMU and starts C64 mode).
//!
//! At power-on the Z80 runs its BIOS (KERNAL ROM `$D000–$DFFF`), which
//! hands over to the 8502 through MCR bit 0 (or enters C64 mode when C= is
//! held). The Z80 runs at 2 T-states per system cycle (its 4 MHz clock less
//! the VIC's half); in 2 MHz mode (`$D030` bit 0) the 8502 runs two cycles
//! per VIC cycle.
//!
//! Two screens ([`Machine::screens`]): "vic" (the main one) and "vdc"; the
//! 40/80 DISPLAY key decides which is active. Time ([`Machine::now`])
//! counts system (VIC) cycles.
//!
//! Models: "pal" (8566, 985 248 Hz) and "ntsc" (8564, 1 022 727 Hz).
//! Options: `display` ("40" default, "80": the 40/80 key down at power-on),
//! `vdc_ram` ("16k" default, "64k"), `sid` ("6581"/"8580"), `drive8`
//! ("1571" with `roms["dos1571"]`, the default when given; "1541" with
//! `roms["dos1541"]`; "1581" with `roms["dos1581"]`; "virtual"; "none"),
//! `drive8_ppm` (drive crystal deviation, see [`drives_cbm::serial`]),
//! `autostart` ("on"), `port1`/`port2` ("joystick", "paddles",
//! "mouse1351", "none"; port 1 also "lightpen": control port 1 pin 6 is
//! the VIC-IIe's LP input, as on the C64), `roms` = "stub".
//!
//! ROMs (`MachineConfig::roms`, user-supplied): "kernal" (16 KB, with the
//! screen editor and the Z80 BIOS), "basic-lo", "basic-hi" (16 KB each),
//! "chargen" (8 KB), "c64" (16 KB C64-mode BASIC + KERNAL, or "basic64"
//! and "kernal64" of 8 KB each), optional "function" (32 KB internal
//! function ROM).

#![forbid(unsafe_code)]

pub mod drive;
pub mod io;
pub mod keyboard;
pub mod loader;
pub mod memory;
pub mod stub;

use chips_cbm::sid::SidModel;
use chips_cbm::{vdc, vic2};
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use cpu_z80::{IO_BASE, Z80};
use emu_core::frame::{FrameRef, PixelData, Rgb8, ScreenId, ScreenInfo};
use emu_core::gfx::{GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::overlay::{Overlay, OverlayInfo};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr,
    InterruptKind, Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler,
    Size, StateError, StateReader, StateWriter, StopReason,
};

use devices_cartridge::slot::ExpansionSlot;
use devices_cartridge::{CartLines, ExpansionCart};
use drive::{Drive8, DRIVE8, DRIVE_SPACE};
use drives_cbm::{DriveModel, DriveRoms, IecLines};
use drives_other::Datasette;
use io::Io;
use loader::{Autostart, ProgramSlot, BOOT_FRAMES};
use media::tap::TapMachine;
use memory::{GfxVicView, Memory, Read, Roms, VicView, Write, CHARGEN_SIZE, ROM_16K};

pub const CPU: DeviceId = DeviceId("cpu");
pub const Z80_DEV: DeviceId = DeviceId("z80");
/// The 8502's view of memory (MMU or C64 map).
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// RAM banks 0 and 1 as one 128 KB space (bank in bit 16).
pub const RAM_SPACE: MemSpaceId = MemSpaceId(1);
/// The Z80's view of memory.
pub const Z80_SPACE: MemSpaceId = MemSpaceId(4);
/// The VDC's own RAM.
pub const VDC_SPACE: MemSpaceId = MemSpaceId(5);

pub const VIC_SCREEN: ScreenId = ScreenId(0);
pub const VDC_SCREEN: ScreenId = ScreenId(1);

/// VIC-II register names $D000–$D02E (C64 Programmer's Reference Guide).
const VIC_REGS: [&str; 47] = [
    "M0X", "M0Y", "M1X", "M1Y", "M2X", "M2Y", "M3X", "M3Y", "M4X", "M4Y", "M5X", "M5Y", "M6X",
    "M6Y", "M7X", "M7Y", "MSBX", "CR1", "RASTER", "LPX", "LPY", "MXE", "CR2", "MXYE", "MEMPTR",
    "IRR", "IMR", "MXDP", "MXMC", "MXXE", "MXM", "MXD", "EC", "B0C", "B1C", "B2C", "B3C", "MM0",
    "MM1", "M0C", "M1C", "M2C", "M3C", "M4C", "M5C", "M6C", "M7C",
];

/// Graphics view option: which 64 KB RAM bank the VIC-IIe views read.
const OPT_RAMBANK: GfxOptionInfo = GfxOptionInfo {
    name: "rambank",
    help: "RAM bank 0/1 the VIC-IIe reads (default: the MMU's RCR bits 6-7; bit 16 of addr also selects it)",
};

const MMU_REGS: [&str; 12] = [
    "CR", "PCRA", "PCRB", "PCRC", "PCRD", "MCR", "RCR", "P0L", "P0H", "P1L", "P1H", "VR",
];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "c128",
        name: "Commodore 128",
        models: &["pal", "ntsc"],
        create: |config| Ok(Box::new(C128::new(config)?)),
    }
}

/// Interrupt sources on the open-collector lines (as on the C64).
mod irq {
    pub const VIC: u8 = 1 << 0;
    pub const CIA1: u8 = 1 << 1;
    pub const CARTRIDGE: u8 = 1 << 2;
}

mod nmi {
    pub const CIA2: u8 = 1 << 0;
    pub const RESTORE: u8 = 1 << 1;
    pub const CARTRIDGE: u8 = 1 << 2;
}

/// I/O1 and I/O2, the expansion port's I/O pages.
fn is_cart_io(addr: u16) -> bool {
    (0xde00..=0xdfff).contains(&addr)
}

/// The 8502's bus: MMU/PLA-resolved memory, I/O and the cartridge.
struct CpuBus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
    cart: Option<&'a mut (dyn ExpansionCart + 'static)>,
}

impl Bus for CpuBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let r = self.mem.cpu_read(addr as u16);
        let from_cart = match (r, self.cart.as_deref_mut()) {
            (Read::Value(v), _) => return u32::from(v),
            (Read::RomL(a), Some(c)) => c.read_roml(a),
            (Read::RomH(a), Some(c)) => c.read_romh(a),
            (Read::Io(a), Some(c)) if is_cart_io(a) => c.read_io(a),
            _ => None,
        };
        u32::from(from_cart.unwrap_or_else(|| match r {
            Read::Io(a) if !is_cart_io(a) => self.io.read(self.mem, a),
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
        match (self.mem.cpu_write(a, v), self.cart.as_deref_mut()) {
            (Write::Io, Some(c)) if is_cart_io(a) => c.write_io(a, v),
            (Write::Io, _) => self.io.write(self.mem, a, v),
            (Write::RomL, Some(c)) => c.write_roml(a, v),
            (Write::RomH, Some(c)) => c.write_romh(a, v),
            _ => {}
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Side-effect-free 8502-view read.
fn cpu_peek(mem: &Memory, io: &Io, cart: Option<&dyn ExpansionCart>, a: u16) -> u8 {
    let r = mem.cpu_read(a);
    let from_cart = match (r, cart) {
        (Read::Value(v), _) => return v,
        (Read::RomL(a), Some(c)) => c.peek_roml(a),
        (Read::RomH(a), Some(c)) => c.peek_romh(a),
        (Read::Io(a), Some(c)) if is_cart_io(a) => c.peek_io(a),
        _ => None,
    };
    from_cart.unwrap_or_else(|| match r {
        Read::Io(a) if !is_cart_io(a) => io.peek(mem, a),
        _ => io.open_bus(),
    })
}

/// The Z80's bus: memory, and I/O ports `$D000–$DFFF` at [`IO_BASE`].
struct Z80Bus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
}

impl Bus for Z80Bus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        u32::from(match addr.checked_sub(IO_BASE) {
            Some(port @ 0xd000..=0xdfff) => self.io.read(self.mem, port as u16),
            Some(_) => 0xff,
            None => self.mem.z80_read(addr as u16),
        })
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(z80_peek(self.mem, self.io, addr))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        match addr.checked_sub(IO_BASE) {
            Some(port @ 0xd000..=0xdfff) => self.io.write(self.mem, port as u16, v as u8),
            Some(_) => {}
            None => self.mem.z80_write(addr as u16, v as u8),
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

fn z80_peek(mem: &Memory, io: &Io, addr: u32) -> u8 {
    match addr.checked_sub(IO_BASE) {
        Some(port @ 0xd000..=0xdfff) => io.peek(mem, port as u16),
        Some(_) => 0xff,
        None => mem.z80_read(addr as u16),
    }
}

/// Read-only views for disassembly.
struct PeekBus<'a> {
    mem: &'a Memory,
    io: &'a Io,
    cart: Option<&'a dyn ExpansionCart>,
    z80: bool,
}

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(if self.z80 {
            z80_peek(self.mem, self.io, addr)
        } else {
            cpu_peek(self.mem, self.io, self.cart, addr as u16)
        })
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

/// The C64-mode ROM: "c64" (16 KB), or "basic64" and "kernal64" (8 KB each).
fn c64_rom(config: &MachineConfig) -> Result<Box<[u8; ROM_16K]>, ConfigError> {
    if config.roms.contains_key("c64") || !config.roms.contains_key("basic64") {
        return rom(config, "c64");
    }
    let basic: Box<[u8; 0x2000]> = rom(config, "basic64")?;
    let kernal: Box<[u8; 0x2000]> = rom(config, "kernal64")?;
    let mut both = Box::new([0u8; ROM_16K]);
    both[..0x2000].copy_from_slice(&basic[..]);
    both[0x2000..].copy_from_slice(&kernal[..]);
    Ok(both)
}

fn load_roms(config: &MachineConfig) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => Ok(Roms {
            kernal: stub::kernal(),
            basic_lo: Box::new([0xff; ROM_16K]),
            basic_hi: Box::new([0xff; ROM_16K]),
            chargen: Box::new([0; CHARGEN_SIZE]),
            c64: Box::new([0xff; ROM_16K]),
            function: None,
        }),
        Some(other) => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => Ok(Roms {
            kernal: rom(config, "kernal")?,
            basic_lo: rom(config, "basic-lo")?,
            basic_hi: rom(config, "basic-hi")?,
            chargen: rom(config, "chargen")?,
            c64: c64_rom(config)?,
            function: match config.roms.get("function") {
                Some(_) => Some(rom(config, "function")?),
                None => None,
            },
        }),
    }
}

pub const CONTROL_PORT_1: PortId = PortId(0);
pub const CONTROL_PORT_2: PortId = PortId(1);
const PORT_NAMES: [&str; 2] = ["control port 1", "control port 2"];

pub const SLOT_DRIVE8: SlotId = SlotId(0);
pub const SLOT_PROGRAM: SlotId = SlotId(1);
pub const SLOT_TAPE: SlotId = SlotId(2);
pub const SLOT_CARTRIDGE: SlotId = SlotId(3);

pub struct C128 {
    cpu: Cpu6502,
    z80: Z80,
    mem: Memory,
    io: Io,
    ints: (u8, u8),
    sched: Scheduler,
    input: InputQueue,
    ports: [Option<Box<dyn PortDevice>>; 2],
    drive: Option<Drive8>,
    program: ProgramSlot,
    /// The datasette on the cassette port.
    tape: Datasette,
    /// The expansion port (C64 cartridges: the KERNAL starts C64 mode when
    /// one pulls GAME or EXROM).
    cart: ExpansionSlot,
    autostart: Autostart,
    vic_palette: [Rgb8; 16],
    vdc_palette: [Rgb8; 16],
    field_done_pending: bool,
    /// VIC frame of the last reset; programs wait [`BOOT_FRAMES`] from here.
    boot_frame: u64,
    /// BASIC has shown READY. since the last reset.
    booted: bool,
    /// The device the debugger addresses through [`DebugTarget`].
    debug_dev: DeviceId,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Cpu6502,
    z80: Z80,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
}

impl C128 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let vic = match config.model.as_deref() {
            None | Some("pal") => vic2::Model::Mos6569,
            Some("ntsc") => vic2::Model::Mos6567R8,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let cpu_hz = vic.timing().cpu_hz();
        let option = |name: &str| config.options.get(name).map(String::as_str);
        let sid = match option("sid") {
            None | Some("8580") => SidModel::Mos8580,
            Some("6581") => SidModel::Mos6581,
            Some(other) => return Err(ConfigError::Invalid(format!("sid option '{other}'"))),
        };
        let vdc_kb = match option("vdc_ram") {
            None | Some("16k") => 16,
            Some("64k") => 64,
            Some(other) => return Err(ConfigError::Invalid(format!("vdc_ram option '{other}'"))),
        };
        let display_80 = match option("display") {
            None | Some("40") => false,
            Some("80") => true,
            Some(other) => return Err(ConfigError::Invalid(format!("display option '{other}'"))),
        };
        let port1 = devices_input::cbm_port_option("port1", option("port1"), "none", true, true)?;
        let port2 = devices_input::control_port_option("port2", option("port2"), "joystick", true)?;
        let stub = option("roms") == Some("stub");
        let drive_roms = if stub {
            DriveRoms::default()
        } else {
            DriveRoms::from_map(&config.roms)
        };
        let mut drive = Drive8::from_option(
            config.options.get("drive8").map(String::as_str),
            drive_roms,
            DriveModel::C1571,
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
        let autostart = match option("autostart") {
            None | Some("off") => Autostart::Off,
            Some("on") => Autostart::Armed,
            Some(other) => return Err(ConfigError::Invalid(format!("autostart option '{other}'"))),
        };
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Mos8502,
            ..CpuConfig::default()
        });
        cpu.reset();
        let mut io = Io::new(vic, sid, vdc_kb);
        io.keyboard.display_80 = display_80;
        let mut m = Self {
            cpu,
            z80: Z80::new(),
            mem: Memory::new(load_roms(config)?),
            io,
            ints: (0, 0),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            ports: [port1, port2],
            drive,
            program: ProgramSlot::default(),
            tape: Datasette::new(TapMachine::C64, cpu_hz),
            cart: devices_cartridge::slot::expansion_slot(),
            autostart,
            vic_palette: vic2::palette::palette(),
            vdc_palette: vdc::palette(),
            field_done_pending: false,
            boot_frame: 0,
            booted: false,
            debug_dev: CPU,
        };
        // The devices' lines from the start (paddles sit at mid travel).
        m.refresh_ports();
        Ok(m)
    }

    fn beam_pos(&self) -> BeamPos {
        self.io.vic.beam()
    }

    fn update_interrupts(&mut self) {
        let mut irq_lines = 0;
        if self.io.vic.irq() {
            irq_lines |= irq::VIC;
        }
        if self.io.cia1.irq() {
            irq_lines |= irq::CIA1;
        }
        let mut nmi_lines = 0;
        if self.io.cia2.irq() {
            nmi_lines |= nmi::CIA2;
        }
        if self.io.keyboard.restore {
            nmi_lines |= nmi::RESTORE;
        }
        let cart = self.cart.cart().map_or(CartLines::default(), |c| c.lines());
        if cart.irq {
            irq_lines |= irq::CARTRIDGE;
        }
        if cart.nmi {
            nmi_lines |= nmi::CARTRIDGE;
        }
        self.ints = (irq_lines, nmi_lines);
    }

    /// The cartridge's clock and its GAME/EXROM lines into the PLA.
    fn cartridge_cycle(&mut self) {
        let lines = match self.cart.cart_mut() {
            Some(c) => {
                c.tick();
                c.lines()
            }
            None => CartLines::default(),
        };
        // Line levels: true = high (not pulled).
        self.mem.game = !lines.game;
        self.mem.exrom = !lines.exrom;
    }

    /// One 8502 cycle on the hooked bus.
    fn cpu_cycle(&mut self, dbg: Option<&mut DebugHooks>) -> (CycleResult, Option<InterruptKind>) {
        let beam = self.beam_pos();
        let mut bus = CpuBus {
            mem: &mut self.mem,
            io: &mut self.io,
            cart: self.cart.cart_mut().map(|c| c.as_mut()),
        };
        let mut hooked = HookedBus {
            inner: &mut bus,
            dbg,
            space: CPU_SPACE,
            beam,
        };
        let r = self.cpu.tick(&mut hooked);
        (r, self.cpu.take_interrupt())
    }

    /// One Z80 T-state on the hooked bus.
    fn z80_cycle(&mut self, dbg: Option<&mut DebugHooks>) -> (CycleResult, Option<InterruptKind>) {
        let beam = self.beam_pos();
        let mut bus = Z80Bus {
            mem: &mut self.mem,
            io: &mut self.io,
        };
        let mut hooked = HookedBus {
            inner: &mut bus,
            dbg,
            space: Z80_SPACE,
            beam,
        };
        let r = self.z80.tick(&mut hooked);
        (r, self.z80.take_interrupt())
    }

    /// One system (VIC) cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        // A cartridge went in or out: the machine powers up again.
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let base = self.io.vic_base();
        self.io.vic.phi1(&VicView {
            mem: &self.mem,
            base,
            cart: self.cart.cart().map(|c| c.as_ref()),
        });
        let ba = self.io.vic.ba();
        self.io.tick_chips();
        self.update_interrupts();
        let (irq_on, nmi_on) = (self.ints.0 != 0, self.ints.1 != 0);

        // The CPU steps of this cycle: one or two 8502 cycles, or two Z80
        // T-states. A debug stop ends the cycle's steps early.
        let m8502 = self.mem.mmu.m8502();
        let steps = match (m8502, self.io.fast(), ba) {
            (true, true, _) => 2,
            (true, false, _) => 1,
            (false, _, true) => 2,
            (false, _, false) => 0,
        };
        let mut jam = None;
        for step in 0..steps {
            let (dev, space, (result, entered)) = if m8502 {
                // 2 MHz: the second cycle in the VIC's half.
                self.cpu.set_rdy(ba || step > 0);
                self.cpu.set_irq(irq_on);
                self.cpu.set_nmi(nmi_on);
                // Without AEC the halted CPU does not reach the bus at all.
                let result = if step == 0 && !self.io.vic.aec() {
                    (CycleResult::Busy, None)
                } else {
                    self.cpu_cycle(dbg.as_deref_mut())
                };
                (CPU, CPU_SPACE, result)
            } else {
                self.z80.set_int(irq_on);
                self.z80.set_nmi(nmi_on);
                (Z80_DEV, Z80_SPACE, self.z80_cycle(dbg.as_deref_mut()))
            };
            if result == CycleResult::Jammed {
                jam = Some(StopReason::CpuJam {
                    device: dev,
                    pc: u32::from(self.cpu.pc()),
                });
            }
            if let Some(dbg) = dbg.as_deref_mut() {
                if let Some(kind) = entered {
                    if dbg.interrupt_armed(kind) {
                        dbg.check_interrupt(kind);
                    }
                }
                if let CycleResult::InstrDone { next_pc } = result {
                    if dbg.trace_instr_armed() {
                        dbg.trace_instr(dev, next_pc, self.beam_pos());
                    }
                    if dbg.instr_step_armed() {
                        dbg.check_instr_step();
                    }
                    if dbg.exec_armed(space) {
                        dbg.check_exec(space, next_pc);
                    }
                }
                if dbg.stop_pending() {
                    break;
                }
            }
        }

        let base = self.io.vic_base();
        let new_field = self.io.vic.phi2(&VicView {
            mem: &self.mem,
            base,
            cart: self.cart.cart().map(|c| c.as_ref()),
        });
        self.cartridge_cycle();
        // Serial bus: the drive sees what CIA2 (and CIA1 as fast serial
        // sender) drives now.
        let fsdir = self.mem.mmu.mcr & 0x08 != 0;
        let host = IecLines::combine(&[
            drive::c64_lines(self.io.cia2.port_a_out()),
            drive::fast_serial_lines(&self.io.cia1, fsdir),
        ]);
        let mut drive_stop = None;
        self.io.iec = match &mut self.drive {
            Some(d) => {
                let (bus, stop) = d.run(host, dbg.as_deref_mut());
                drive_stop = stop;
                bus
            }
            None => host,
        };
        // FSDIR clear: CIA1 receives on DATA/SRQ.
        let iec = self.io.iec;
        self.io.cia1.set_sp(fsdir || !iec.data);
        self.io.cia1.set_cnt(fsdir || !iec.srq);
        // Datasette: port bit 3 WRITE, bit 5 MOTOR (output low = on), bit 4
        // SENSE; READ shares CIA1 /FLAG with SRQ.
        let port = &self.mem.port;
        let out = port.outputs();
        self.tape.set_motor(port.ddr & 0x20 != 0 && out & 0x20 == 0);
        self.tape.set_write(out & 0x08 != 0);
        self.tape.tick();
        self.mem.cassette_sense = self.tape.sense();
        self.io.cia1.set_flag(!iec.srq && self.tape.read());
        self.update_interrupts();
        self.sched.tick();
        if new_field {
            self.frame_tasks();
        }

        if let Some(dbg) = dbg {
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
        match jam {
            Some(stop) => Some(stop),
            None if new_field => Some(StopReason::FrameDone),
            None => None,
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match ev.target {
            InputTarget::Keyboard => {
                if let emu_core::input::InputEvent::Key { machine_key, down } = ev.event {
                    self.io.keyboard.set(machine_key, down);
                    self.mem.caps_lock = self.io.keyboard.caps_lock;
                }
            }
            InputTarget::Port(p) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&ev.event);
                }
                self.refresh_port(p);
            }
            InputTarget::Panel => {
                if let emu_core::input::InputEvent::Control { id, value } = ev.event {
                    self.control(id.0, value);
                }
            }
        }
    }

    /// A console control from the panel (power, reset, RESTORE, the locking 40/80 DISPLAY and CAPS LOCK keys).
    fn control(&mut self, id: u16, value: u8) {
        match id {
            0 if value == 1 => emu_core::Machine::reset(self, emu_core::ResetKind::Hard),
            1 if value == 1 => emu_core::Machine::reset(self, emu_core::ResetKind::Soft),
            2 => {
                self.io.keyboard.set(keyboard::RESTORE, value == 1);
            }
            3 if (value == 1) != self.io.keyboard.display_80 => {
                self.io.keyboard.set(keyboard::DISPLAY_4080, true);
            }
            4 if (value == 1) != self.io.keyboard.caps_lock => {
                self.io.keyboard.set(keyboard::CAPS_LOCK, true);
                self.mem.caps_lock = self.io.keyboard.caps_lock;
            }
            _ => {}
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
        if i == 0 {
            // Control port 1 pin 6 is the VIC-IIe's LP input: the fire
            // button latches the beam position.
            let fire = self.io.port_lines[0] & devices_input::joystick::pin::FIRE != 0;
            self.io.vic.set_lp_line(fire);
            self.sync_light_pen();
        }
    }

    /// Tell the VIC-IIe where a light pen in control port 1 points (frame
    /// pixels of the VIC screen) and which colours light it; no pen:
    /// `None`. The VDC's light-pen input is not wired to the port.
    fn sync_light_pen(&mut self) {
        let lines = self.ports[0]
            .as_ref()
            .map(|d| d.read_lines(self.sched.now()));
        let aim = lines.as_ref().and_then(devices_input::lightpen::aim);
        let lit = self
            .vic_palette
            .iter()
            .enumerate()
            .filter(|(_, &c)| devices_input::lightpen::lit(c))
            .fold(0u16, |m, (n, _)| m | 1 << n);
        self.io.vic.set_light_pen(aim.map(|a| (a.x, a.y)), lit);
    }

    fn refresh_ports(&mut self) {
        self.refresh_port(PortId(0));
        self.refresh_port(PortId(1));
    }

    /// The text screens: VIC (`$0400` of bank 0) and VDC (its RAM at 0).
    fn screens_text(&self) -> (Vec<u8>, Vec<u8>) {
        let vic = self.mem.ram[0x0400..0x07e8].to_vec();
        let vdc = self.io.vdc.ram()[..2000].to_vec();
        (vic, vdc)
    }

    /// BASIC's start in the current mode.
    fn basic_start(&self) -> u16 {
        if self.mem.mmu.c64_mode() {
            0x0801
        } else {
            0x1c01
        }
    }

    /// Program injection and autostart, once BASIC has booted.
    fn frame_tasks(&mut self) {
        if !self.booted {
            if self.io.vic.beam().frame.saturating_sub(self.boot_frame) < BOOT_FRAMES {
                return;
            }
            let (vic, vdc) = self.screens_text();
            let active = if self.io.keyboard.display_80 {
                vdc
            } else {
                vic
            };
            if !loader::ready(&active) {
                return;
            }
            self.booted = true;
        }
        if let Some(prg) = self.program.pending.take() {
            self.inject(&prg);
            if prg.load == self.basic_start() {
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
                let (vic, vdc) = self.screens_text();
                if loader::loaded(&vic) || loader::loaded(&vdc) {
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

    /// Put a program into RAM bank 0 like the KERNAL LOAD and set BASIC's
    /// end pointers: TEXT_TOP (`$1210`) in C128 mode, VARTAB/ARYTAB/STREND
    /// in C64 mode, and the load end at `$AE/$AF`.
    fn inject(&mut self, prg: &media::Prg) {
        let start = usize::from(prg.load);
        let len = prg.data.len().min(0x10000 - start);
        self.mem.ram[start..start + len].copy_from_slice(&prg.data[..len]);
        let end = ((start + len) as u16).to_le_bytes();
        let pointers: &[usize] = if self.mem.mmu.c64_mode() {
            &[0x2d, 0x2f, 0x31, 0xae]
        } else {
            &[0x1210, 0xae]
        };
        for &p in pointers {
            self.mem.ram[p..p + 2].copy_from_slice(&end);
        }
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            for i in 0..f.size.bytes() {
                let addr = f.addr.wrapping_add(i);
                let v = (f.value >> (8 * i)) as u8;
                match f.space {
                    CPU_SPACE => CpuBus {
                        mem: &mut self.mem,
                        io: &mut self.io,
                        cart: self.cart.cart_mut().map(|c| c.as_mut()),
                    }
                    .write(addr & 0xffff, Size::Byte, u32::from(v)),
                    RAM_SPACE => self.mem.ram[(addr & 0x1ffff) as usize] = v,
                    _ => {}
                }
            }
        }
    }

    fn vic_frame(&self) -> FrameRef<'_> {
        let t = self.io.vic.timing();
        FrameRef {
            width: t.width,
            height: t.height,
            pixels: PixelData::Indexed {
                pixels: self.io.vic.pixels(),
                palette: &self.vic_palette,
            },
            luma_chroma: None,
            signal: t.signal(),
        }
    }

    fn vdc_frame(&self) -> FrameRef<'_> {
        FrameRef {
            width: vdc::WIDTH,
            height: vdc::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.io.vdc.pixels(),
                palette: &self.vdc_palette,
            },
            luma_chroma: None,
            signal: self.io.vdc.signal(),
        }
    }
}

fn reg(name: &'static str, addr: Option<u32>, value: u32, bits: u8) -> ChipReg {
    ChipReg {
        name,
        addr,
        value,
        bits,
    }
}

impl Machine for C128 {
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

    /// RESET reaches both CPUs, the MMU (back to the Z80), CIAs, SID and
    /// drive 8; the VIC-IIe and the VDC restart only on power-up.
    fn reset(&mut self, kind: ResetKind) {
        let hard = kind == ResetKind::Hard;
        self.cpu.reset();
        self.z80.reset();
        self.mem.reset(hard);
        self.io.reset(hard);
        if let Some(d) = &mut self.drive {
            d.reset(hard);
        }
        if let Some(c) = self.cart.cart_mut() {
            c.reset();
        }
        self.ints = (0, 0);
        self.cartridge_cycle();
        self.input.clear();
        if self.autostart == Autostart::Loading {
            self.autostart = Autostart::Done;
        }
        self.field_done_pending = false;
        self.boot_frame = self.io.vic.beam().frame;
        self.booted = false;
        self.sync_light_pen();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        self.vic_frame()
    }

    fn screens(&self) -> Vec<ScreenInfo> {
        let vdc = self.io.keyboard.display_80;
        vec![
            ScreenInfo {
                id: VIC_SCREEN,
                name: "vic",
                active: !vdc,
            },
            ScreenInfo {
                id: VDC_SCREEN,
                name: "vdc",
                active: vdc,
            },
        ]
    }

    fn screen(&self, id: ScreenId) -> Option<FrameRef<'_>> {
        match id {
            VIC_SCREEN => Some(self.vic_frame()),
            VDC_SCREEN => Some(self.vdc_frame()),
            _ => None,
        }
    }

    fn overlays(&self) -> Vec<OverlayInfo> {
        vic2::overlay::overlays()
    }

    fn set_overlays(&mut self, ids: &[&str]) {
        self.io.vic.set_overlays(ids);
    }

    fn overlay(&self, id: &str) -> Option<Overlay> {
        self.io.vic.overlay(id)
    }

    fn frame_row(&self, line: u16) -> Option<u32> {
        self.io.vic.frame_row(line)
    }

    /// The VIC-IIe's views (as on the C64) and the VDC's (`vdc-*`).
    fn gfx_views(&self) -> Vec<GfxViewInfo> {
        let mut views = vic2::gfx::views(
            "VIC-IIe",
            ["sprites", "charset", "bitmap", "screen", "palette"],
        );
        for v in &mut views {
            if v.kind != GfxKind::Palette {
                v.options.push(OPT_RAMBANK);
            }
        }
        views.extend(vdc::gfx::views());
        views
    }

    /// VIC-IIe views: option `bank` (or bits 14–15 of `addr`) selects the
    /// 16 KB bank instead of CIA2's, `rambank` (or bit 16 of `addr`) the
    /// 64 KB RAM bank instead of the MMU's; addresses are reported in the
    /// `ram` space (17 bits). VDC views read VDC RAM (`vdcram` space).
    fn gfx_view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        if id.starts_with("vdc-") {
            let ctx = vdc::gfx::GfxContext {
                space: VDC_SPACE,
                palette: &self.vdc_palette,
            };
            return self.io.vdc.gfx_view(id, &ctx, opts);
        }
        let info = self.gfx_views().into_iter().find(|v| v.id == id)?;
        let bank = match (opts.get("bank"), opts.get("addr")) {
            (Some(b), _) => (b as u16 & 3) << 14,
            (None, Some(a)) => a as u16 & 0xc000,
            (None, None) => self.io.vic_base(),
        };
        let view = VicView {
            mem: &self.mem,
            base: bank,
            cart: self.cart.cart().map(|c| c.as_ref()),
        };
        let ram_bank = match (opts.get("rambank"), opts.get("addr")) {
            (Some(b), _) => (b & 1) as u8,
            (None, Some(a)) if a > 0xffff => ((a >> 16) & 1) as u8,
            _ => view.ram_bank(),
        };
        let view = GfxVicView { view, ram_bank };
        let ctx = vic2::gfx::GfxContext {
            mem: &view,
            bank_base: bank,
            space: RAM_SPACE,
            palette: &self.vic_palette,
        };
        let mut v = self
            .io
            .vic
            .gfx_view(info.id, info.kind, info.name, &ctx, opts);
        if v.kind != GfxKind::Palette {
            v.addr |= u32::from(ram_bank) << 16;
            v.params.insert(1, ("rambank", u32::from(ram_bank)));
        }
        Some(v)
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
        let space = |id, name, device, addr_bits| MemSpace {
            id,
            name,
            device,
            addr_bits,
            big_endian: false,
        };
        let mut spaces = vec![
            space(CPU_SPACE, "cpu", CPU, 16),
            space(RAM_SPACE, "ram", CPU, 17),
            space(Z80_SPACE, "z80", Z80_DEV, 16),
            space(
                VDC_SPACE,
                "vdcram",
                CPU,
                if self.io.vdc.ram().len() > 0x4000 {
                    16
                } else {
                    14
                },
            ),
        ];
        if self.drive.as_ref().is_some_and(|d| d.is_true()) {
            spaces.push(space(DRIVE_SPACE, "drive8", DRIVE8, 16));
        }
        spaces
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let r8 = |name, addr: u32, v: u8| reg(name, Some(addr), u32::from(v), 8);
        let cia = |name, base: u32, c: &chips_cbm::cia::Cia| ChipInfo {
            name,
            regs: vec![
                r8("PRA", base, c.pra),
                r8("PRB", base + 1, c.prb),
                r8("DDRA", base + 2, c.ddra),
                r8("DDRB", base + 3, c.ddrb),
                reg("TA", Some(base + 4), u32::from(c.ta.counter), 16),
                reg("TB", Some(base + 6), u32::from(c.tb.counter), 16),
                r8("ICR", base + 13, c.peek(13, 0xff, 0xff)),
                r8("CRA", base + 14, c.ta.cr),
                r8("CRB", base + 15, c.tb.cr),
            ],
        };
        let mmu = &self.mem.mmu;
        vec![
            ChipInfo {
                name: "vic2",
                regs: VIC_REGS
                    .iter()
                    .enumerate()
                    .map(|(i, &name)| r8(name, 0xd000 + i as u32, self.io.vic.peek(i as u8)))
                    .chain([
                        r8("KEYLINES", 0xd02f, self.io.d02f),
                        r8("CLOCK", 0xd030, self.io.d030),
                    ])
                    .collect(),
            },
            cia("cia1", 0xdc00, &self.io.cia1),
            cia("cia2", 0xdd00, &self.io.cia2),
            ChipInfo {
                name: "mmu",
                regs: MMU_REGS
                    .iter()
                    .enumerate()
                    .map(|(i, &name)| {
                        let v = mmu.peek(
                            i as u8,
                            self.io.keyboard.display_80,
                            !self.mem.game,
                            !self.mem.exrom,
                        );
                        r8(name, 0xd500 + i as u32, v)
                    })
                    .collect(),
            },
            ChipInfo {
                name: "vdc",
                regs: self
                    .io
                    .vdc
                    .regs
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| reg(VDC_NAMES[i], None, u32::from(v), 8))
                    .collect(),
            },
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        let mut d = vec![CPU, Z80_DEV];
        if self.drive.as_ref().is_some_and(|d| d.is_true()) {
            d.push(DRIVE8);
        }
        d
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if dev == CPU || dev == Z80_DEV {
            self.debug_dev = dev;
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

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        use emu_core::ControlInfo;
        vec![
            ControlInfo::button(0, "POWER", false),
            ControlInfo::button(1, "RESET", false),
            ControlInfo::button(2, "RESTORE", self.io.keyboard.restore),
            ControlInfo::switch(
                3,
                "40/80 DISPLAY",
                &["40", "80"],
                self.io.keyboard.display_80 as u8,
            ),
            ControlInfo::switch(
                4,
                "CAPS LOCK",
                &["OFF", "ON"],
                self.io.keyboard.caps_lock as u8,
            ),
        ]
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
        w.chunk(*b"C128", 3, |w| {
            w.u8(self.ints.0);
            w.u8(self.ints.1);
            w.bool(self.field_done_pending);
            w.u8(self.autostart.to_u8());
            w.u64(self.boot_frame);
            w.bool(self.booted);
        });
        self.cpu.save(&mut w);
        self.z80.save(&mut w);
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

    /// Atomic: components load into copies; the drive and the port devices
    /// (not clonable) are restored from backups if a later part fails.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            z80: self.z80.clone(),
            mem: self.mem.clone(),
            io: self.io.clone(),
            sched: self.sched.clone(),
            input: self.input.clone(),
        };
        let mut r = StateReader::new(s);
        let (version, mut c) = r.chunk(*b"C128", 3)?;
        let ints = (c.u8()?, c.u8()?);
        let field_done_pending = c.bool()?;
        let autostart =
            Autostart::from_u8(c.u8()?).ok_or(StateError::Invalid("autostart state"))?;
        let boot_frame = c.u64()?;
        let booted = c.bool()?;
        next.cpu.load(&mut r)?;
        next.z80.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.load(&mut r)?;
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
        // The cartridge (version 3 on), likewise.
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
        if version >= 3 {
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
        let port_states = match r
            .chunk(*b"PORT", 1)
            .and_then(|(_, mut c)| Ok([c.bytes()?, c.bytes()?]))
        {
            Ok(p) => p,
            Err(e) => {
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
        for (dev, state) in self.ports.iter_mut().zip(port_states) {
            if let (Some(dev), false) = (dev, state.is_empty()) {
                if let Err(e) = dev.load_state(state) {
                    for (dev, backup) in self.ports.iter_mut().zip(&backups) {
                        if let Some(d) = dev {
                            let _ = d.load_state(backup);
                        }
                    }
                    restore_drive(&mut self.drive);
                    restore_tape(&mut self.tape, &mut self.cart);
                    return Err(e);
                }
            }
        }
        self.cpu = next.cpu;
        self.z80 = next.z80;
        self.mem = next.mem;
        self.mem.cassette_sense = self.tape.sense();
        self.io = next.io;
        self.sched = next.sched;
        self.input = next.input;
        self.ints = ints;
        self.field_done_pending = field_done_pending;
        self.autostart = autostart;
        self.boot_frame = boot_frame;
        self.booted = booted;
        self.sync_light_pen();
        Ok(())
    }
}

/// VDC register names R0–R36.
const VDC_NAMES: [&str; 37] = [
    "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13", "R14",
    "R15", "R16", "R17", "R18", "R19", "R20", "R21", "R22", "R23", "R24", "R25", "R26", "R27",
    "R28", "R29", "R30", "R31", "R32", "R33", "R34", "R35", "R36",
];

/// The debugger reaches the 8502 or the Z80, whichever
/// [`Machine::debug_target`] selected last; memory spaces are shared.
impl DebugTarget for C128 {
    fn regs(&self) -> RegisterSet {
        if self.debug_dev == Z80_DEV {
            self.z80.regs()
        } else {
            self.cpu.regs()
        }
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        if self.debug_dev == Z80_DEV {
            self.z80.set_reg(name, v);
        } else {
            self.cpu.set_reg(name, v);
        }
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            Some(u32::from(match space {
                CPU_SPACE => cpu_peek(
                    &self.mem,
                    &self.io,
                    self.cart.cart().map(|c| c.as_ref()),
                    (a & 0xffff) as u16,
                ),
                RAM_SPACE => self.mem.ram[(a & 0x1ffff) as usize],
                Z80_SPACE => z80_peek(&self.mem, &self.io, a & 0xffff),
                VDC_SPACE => {
                    let ram = self.io.vdc.ram();
                    ram[a as usize & (ram.len() - 1)]
                }
                _ => return None,
            }))
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
                CPU_SPACE => CpuBus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                    cart: self.cart.cart_mut().map(|c| c.as_mut()),
                }
                .write(a & 0xffff, Size::Byte, u32::from(b)),
                RAM_SPACE => self.mem.ram[(a & 0x1ffff) as usize] = b,
                Z80_SPACE => self.mem.z80_write((a & 0xffff) as u16, b),
                VDC_SPACE => self.io.vdc.poke_ram((a & 0xffff) as u16, b),
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        let bus = PeekBus {
            mem: &self.mem,
            io: &self.io,
            cart: self.cart.cart().map(|c| c.as_ref()),
            z80: self.debug_dev == Z80_DEV,
        };
        if self.debug_dev == Z80_DEV {
            self.z80.disassemble(&bus, addr)
        } else {
            self.cpu.disassemble(&bus, addr)
        }
    }
}

#[cfg(test)]
mod tests;
