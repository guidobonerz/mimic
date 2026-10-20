//! Commodore VIC-20.
//!
//! 6502, VIC 6561 (PAL) or 6560 (NTSC) for video and sound, two 6522 VIAs
//! (keyboard, joystick, serial bus, RESTORE), 5 KB RAM plus expansions, the
//! control port, a true-emulated 1541 as drive 8 and a "program" slot for
//! PRG/T64.
//!
//! Models: "pal" (default; 1 108 404 Hz, 312 lines × 71 cycles) and "ntsc"
//! (1 022 727 Hz, 261 lines × 65 cycles). The NTSC model uses
//! `roms["kernal-ntsc"]` if given, else `roms["kernal"]`.
//!
//! Options:
//! - `ram`: "none" (default), "3k", "8k", "16k", "24k" or "all" (3 KB plus
//!   blocks 1, 2, 3 and 5) — see [`memory`].
//! - `port1`: "joystick" (default) or "none".
//! - `drive8`: "1541" (default when `roms["dos1541"]` is given) or "none".
//! - `autostart`: "on" loads and runs the disk in drive 8 after boot.
//! - `roms`: "stub" uses a tiny built-in KERNAL, for tests without ROMs.
//!
//! ROMs come from `MachineConfig::roms` ("kernal", "basic", "chargen";
//! user-supplied, never in the repo).

#![forbid(unsafe_code)]

pub mod drive;
pub mod io;
pub mod keyboard;
pub mod loader;
pub mod memory;
pub mod stub;

use chips_cbm::via::Via;
use chips_cbm::vic;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use devices_input::Joystick;
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

use drive::{Drive8, DRIVE8, DRIVE_SPACE};
use io::Io;
use loader::{Autostart, ProgramSlot, BASIC_STARTS, BOOT_FRAMES};
use memory::{Area, Memory, Roms, VicView, BASIC_SIZE, CHARGEN_SIZE};

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's view of memory (the VIC-20 has no banking).
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

/// VIC register names `$9000–$900F` (VIC-20 Programmer's Reference Guide).
const VIC_REGS: [&str; 16] = [
    "HORIZ", "VERT", "COLS", "ROWS", "RASTER", "MEMPTR", "LPX", "LPY", "POTX", "POTY", "BASS",
    "ALTO", "SOPRANO", "NOISE", "VOLUME", "COLOR",
];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "vic20",
        name: "Commodore VIC-20",
        models: &["pal", "ntsc"],
        create: |config| Ok(Box::new(Vic20::new(config)?)),
    }
}

/// Side-effect-free CPU-view read.
fn cpu_peek(mem: &Memory, io: &Io, addr: u16) -> u8 {
    match mem.area(addr) {
        Area::Io => io.peek(addr),
        _ => plain_read(mem, io, addr),
    }
}

/// Everything but I/O, which reads the same with or without side effects.
fn plain_read(mem: &Memory, io: &Io, addr: u16) -> u8 {
    let a = usize::from(addr);
    match mem.area(addr) {
        Area::Ram => mem.ram[a],
        Area::Chargen => mem.roms.chargen[a & 0x0fff],
        // Colour RAM: only 4 bits exist, the upper nibble is open bus.
        Area::Color => (io.open_bus() & 0xf0) | mem.color[a & 0x3ff],
        Area::Basic => mem.roms.basic[a & 0x1fff],
        Area::Kernal => mem.roms.kernal[a & 0x1fff],
        Area::Io | Area::None => io.open_bus(),
    }
}

/// The CPU's bus.
struct Vic20Bus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
}

impl Bus for Vic20Bus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        u32::from(match self.mem.area(a) {
            Area::Io => self.io.read(a),
            _ => plain_read(self.mem, self.io, a),
        })
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(cpu_peek(self.mem, self.io, addr as u16))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        match self.mem.area(a) {
            Area::Ram => self.mem.ram[usize::from(a)] = v as u8,
            Area::Color => self.mem.color[usize::from(a & 0x3ff)] = v as u8 & 0x0f,
            Area::Io => self.io.write(a, v as u8),
            _ => {}
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Read-only view for disassembly and debugger peeks.
struct PeekBus<'a>(&'a Memory, &'a Io);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(cpu_peek(self.0, self.1, addr as u16))
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

fn load_roms(config: &MachineConfig, model: vic::Model) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => Ok(Roms {
            kernal: stub::kernal(),
            basic: Box::new([0xff; BASIC_SIZE]),
            chargen: Box::new([0; CHARGEN_SIZE]),
        }),
        Some(other) => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => {
            let kernal = if model == vic::Model::Mos6560 && config.roms.contains_key("kernal-ntsc")
            {
                "kernal-ntsc"
            } else {
                "kernal"
            };
            Ok(Roms {
                kernal: rom(config, kernal)?,
                basic: rom(config, "basic")?,
                chargen: rom(config, "chargen")?,
            })
        }
    }
}

/// The control port (`PortId(0)`).
pub const CONTROL_PORT: PortId = PortId(0);

pub const SLOT_DRIVE8: SlotId = SlotId(0);
pub const SLOT_PROGRAM: SlotId = SlotId(1);

pub struct Vic20 {
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
    port: Option<Box<dyn PortDevice>>,
    drive: Option<Drive8>,
    program: ProgramSlot,
    autostart: Autostart,
    palette: [Rgb8; 16],
    field_done_pending: bool,
    /// VIC frame of the last reset; programs wait [`BOOT_FRAMES`] from here.
    boot_frame: u64,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
}

impl Vic20 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("pal") => vic::Model::Mos6561,
            Some("ntsc") => vic::Model::Mos6560,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let cpu_hz = model.timing().cpu_hz();
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Nmos6502,
            ..CpuConfig::default()
        });
        cpu.reset();
        let expansion = match config.options.get("ram") {
            None => 0,
            Some(r) => memory::expansion_for(r)
                .ok_or_else(|| ConfigError::Invalid(format!("ram option '{r}'")))?,
        };
        let port: Option<Box<dyn PortDevice>> =
            match config.options.get("port1").map(String::as_str) {
                None | Some("joystick") => Some(Box::new(Joystick::new(PortKind::CbmControl))),
                Some("none") => None,
                Some(other) => return Err(ConfigError::Invalid(format!("port1 option '{other}'"))),
            };
        let stub = config.options.get("roms").is_some_and(|r| r == "stub");
        let drive = match config.options.get("drive8").map(String::as_str) {
            Some("none") => None,
            None | Some("1541") if !stub && config.roms.contains_key("dos1541") => {
                Some(Drive8::new(rom(config, "dos1541")?, cpu_hz))
            }
            None | Some("1541") => None,
            Some(other) => return Err(ConfigError::Invalid(format!("drive8 option '{other}'"))),
        };
        let autostart = match config.options.get("autostart").map(String::as_str) {
            None | Some("off") => Autostart::Off,
            Some("on") => Autostart::Armed,
            Some(other) => return Err(ConfigError::Invalid(format!("autostart option '{other}'"))),
        };
        Ok(Self {
            cpu,
            mem: Memory::new(load_roms(config, model)?, expansion),
            io: Io::new(model),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            port,
            drive,
            program: ProgramSlot::default(),
            autostart,
            palette: vic::palette::palette(),
            field_done_pending: false,
            boot_frame: 0,
        })
    }

    fn beam_pos(&self) -> BeamPos {
        self.io.vic.beam()
    }

    /// One CPU cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        // VIAs clock before the CPU access: a read sees this cycle's state.
        self.io.tick_vias();
        self.cpu.set_irq(self.io.via2.irq());
        self.cpu.set_nmi(self.io.via1.irq());
        let beam = self.beam_pos();
        let result = {
            let mut bus = Vic20Bus {
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
        // The VIC fetches in φ1 of the next cycle; it never stops the CPU.
        let new_field = self.io.vic.tick(&VicView(&self.mem));
        // Serial bus: the drive sees what the VIC-20 drives now.
        let host = self.io.iec_out();
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
            InputTarget::Port(p) if p == CONTROL_PORT => {
                if let Some(dev) = &mut self.port {
                    dev.input(&ev.event);
                }
                self.refresh_port();
            }
            InputTarget::Port(_) => {}
        }
    }

    fn refresh_port(&mut self) {
        let now = self.sched.now();
        self.io.port_lines = self
            .port
            .as_ref()
            .map_or(0, |d| d.read_lines(now).pulled_low);
    }

    /// The text screen as the VIC shows it (for autostart).
    fn screen(&self) -> Vec<u8> {
        let view = VicView(&self.mem);
        let cells = u16::from(self.io.vic.columns()) * u16::from(self.io.vic.rows());
        let base = self.io.vic.screen_addr();
        (0..cells.min(0x400))
            .map(|i| chips_cbm::vic::VicMemory::read(&view, (base + i) & 0x3fff))
            .collect()
    }

    /// Program injection and autostart, once BASIC has booted.
    fn frame_tasks(&mut self) {
        if self.io.vic.beam().frame.saturating_sub(self.boot_frame) < BOOT_FRAMES {
            return;
        }
        if let Some(prg) = self.program.pending.take() {
            if self.inject(&prg) {
                self.type_text("RUN\r");
            }
        }
        match self.autostart {
            Autostart::Armed => {
                let has_disk = self.drive.as_ref().is_some_and(|d| d.drive.has_disk());
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

    /// Put a program into RAM and set the BASIC end-of-program pointers
    /// (VARTAB/ARYTAB/STREND, and `$AE/$AF`). A BASIC program goes to the
    /// current start of BASIC (TXTTAB) with its links rebuilt. Returns
    /// whether it is a BASIC program (to be RUN).
    fn inject(&mut self, prg: &media::Prg) -> bool {
        let ram = &mut self.mem.ram;
        let basic = BASIC_STARTS.contains(&prg.load);
        let start = if basic {
            u16::from_le_bytes([ram[0x2b], ram[0x2c]])
        } else {
            prg.load
        };
        let len = prg.data.len().min(0x10000 - usize::from(start));
        let at = usize::from(start);
        ram[at..at + len].copy_from_slice(&prg.data[..len]);
        if basic {
            loader::relink(ram, start);
        }
        let end = (at + len) as u16;
        for ptr in [0x2d, 0x2f, 0x31, 0xae] {
            ram[ptr..ptr + 2].copy_from_slice(&end.to_le_bytes());
        }
        basic
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            if f.space != CPU_SPACE {
                continue;
            }
            for i in 0..f.size.bytes() {
                let addr = f.addr.wrapping_add(i) & 0xffff;
                let v = (f.value >> (8 * i)) as u8;
                Vic20Bus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                }
                .write(addr, Size::Byte, u32::from(v));
            }
        }
    }
}

fn via_regs(name: &'static str, base: u32, v: &Via, pins: (u8, u8)) -> ChipInfo {
    let reg = |name, addr: u32, value: u32, bits| ChipReg {
        name,
        addr: Some(addr),
        value,
        bits,
    };
    let byte = |r: u8| u32::from(v.peek(r, pins.0, pins.1));
    ChipInfo {
        name,
        regs: vec![
            reg("ORB", base, u32::from(v.orb), 8),
            reg("ORA", base + 1, u32::from(v.ora), 8),
            reg("DDRB", base + 2, u32::from(v.ddrb), 8),
            reg("DDRA", base + 3, u32::from(v.ddra), 8),
            reg("T1", base + 4, u32::from(v.t1_counter), 16),
            reg("T1_LATCH", base + 6, u32::from(v.t1_latch), 16),
            reg("T2", base + 8, u32::from(v.t2_counter), 16),
            reg("SR", base + 10, u32::from(v.sr), 8),
            reg("ACR", base + 11, u32::from(v.acr), 8),
            reg("PCR", base + 12, u32::from(v.pcr), 8),
            reg("IFR", base + 13, byte(13), 8),
            reg("IER", base + 14, byte(14), 8),
        ],
    }
}

impl Machine for Vic20 {
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

    /// The RESET line reaches the 6502, both VIAs and (through the serial
    /// bus) drive 8, but not the VIC.
    fn reset(&mut self, kind: ResetKind) {
        let hard = kind == ResetKind::Hard;
        self.cpu.reset();
        self.mem.reset(hard);
        self.io.reset(hard);
        if let Some(d) = &mut self.drive {
            d.drive.reset(hard);
        }
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
            width: t.width(),
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
        out.append(&mut self.io.vic.samples);
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
        if self.drive.is_some() {
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
        vec![
            ChipInfo {
                name: "vic",
                regs: VIC_REGS
                    .iter()
                    .enumerate()
                    .map(|(i, &name)| ChipReg {
                        name,
                        addr: Some(0x9000 + i as u32),
                        value: u32::from(self.io.vic.peek(i as u8)),
                        bits: 8,
                    })
                    .collect(),
            },
            via_regs("via1", 0x9110, &self.io.via1, self.io.via1_pins()),
            via_regs("via2", 0x9120, &self.io.via2, self.io.via2_pins()),
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        let mut d = vec![CPU];
        if self.drive.is_some() {
            d.push(DRIVE8);
        }
        d
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if dev == CPU {
            return Some(self as &mut dyn DebugTarget);
        }
        match &mut self.drive {
            Some(d) if dev == DRIVE8 => Some(&mut d.drive as &mut dyn DebugTarget),
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
        vec![PortInfo {
            id: CONTROL_PORT,
            kind: PortKind::CbmControl,
            name: "control port",
            connected: self.port.is_some(),
        }]
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        if port != CONTROL_PORT {
            return Err(PortError::NoSuchPort(port));
        }
        if dev.kind() != PortKind::CbmControl {
            return Err(PortError::Incompatible {
                port: PortKind::CbmControl,
                device: dev.kind(),
            });
        }
        if self.port.is_some() {
            return Err(PortError::Occupied(port));
        }
        self.port = Some(dev);
        self.refresh_port();
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        if port != CONTROL_PORT {
            return None;
        }
        let dev = self.port.take();
        self.refresh_port();
        dev
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut slots = Vec::new();
        if let Some(d) = &self.drive {
            slots.push(MediaSlotInfo {
                id: SLOT_DRIVE8,
                name: "drive 8",
                kind: MediaKind::Disk,
                inserted: d.drive.has_disk(),
            });
        }
        slots.push(MediaSlotInfo {
            id: SLOT_PROGRAM,
            name: "program",
            kind: MediaKind::Program,
            inserted: self.program.inserted(),
        });
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_DRIVE8 => self
                .drive
                .as_mut()
                .map(|d| &mut d.drive as &mut dyn MediaSlot),
            SLOT_PROGRAM => Some(&mut self.program),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"V20S", 1, |w| {
            w.bool(self.field_done_pending);
            w.u8(self.autostart.to_u8());
            w.u64(self.boot_frame);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.io.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        w.chunk(*b"DRV8", 1, |w| w.bool(self.drive.is_some()));
        if let Some(d) = &self.drive {
            d.save(&mut w);
        }
        w.chunk(*b"PORT", 1, |w| {
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

    /// Atomic: components load into copies; the drive and the port device
    /// (not clonable) are restored from a backup if a later part fails.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            mem: self.mem.clone(),
            io: self.io.clone(),
            sched: self.sched.clone(),
            input: self.input.clone(),
        };
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"V20S", 1)?;
        let field_done_pending = c.bool()?;
        let autostart =
            Autostart::from_u8(c.u8()?).ok_or(StateError::Invalid("autostart state"))?;
        let boot_frame = c.u64()?;
        next.cpu.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;

        let (_, mut c) = r.chunk(*b"DRV8", 1)?;
        if c.bool()? != self.drive.is_some() {
            return Err(StateError::Invalid("drive 8 configuration differs"));
        }
        let drive_backup = self.drive.as_ref().map(|d| {
            let mut w = StateWriter::new();
            d.save(&mut w);
            w.into_bytes()
        });
        if let Some(d) = &mut self.drive {
            d.load(&mut r)?;
        }
        let restore_drive = |drive: &mut Option<Drive8>| {
            if let (Some(d), Some(b)) = (drive, &drive_backup) {
                let _ = d.load(&mut StateReader::new(b));
            }
        };
        let port_state = match r.chunk(*b"PORT", 1).and_then(|(_, mut c)| c.bytes()) {
            Ok(p) => p,
            Err(e) => {
                restore_drive(&mut self.drive);
                return Err(e);
            }
        };
        if let (Some(dev), false) = (&mut self.port, port_state.is_empty()) {
            if let Err(e) = dev.load_state(port_state) {
                restore_drive(&mut self.drive);
                return Err(e);
            }
        }
        self.cpu = next.cpu;
        self.mem = next.mem;
        self.io = next.io;
        self.sched = next.sched;
        self.input = next.input;
        self.field_done_pending = field_done_pending;
        self.autostart = autostart;
        self.boot_frame = boot_frame;
        Ok(())
    }
}

impl DebugTarget for Vic20 {
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
                Area::Ram | Area::Color | Area::Io => Vic20Bus {
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
