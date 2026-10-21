//! Commodore PET.
//!
//! 6502 at 1 MHz, two 6520 PIAs (keyboard, IEEE-488), a 6522 VIA (IEEE-488
//! handshake, user port, character set, CB2 sound), video RAM at `$8000`
//! and, on the 4032/8032, a 6545 CRTC.
//!
//! Models (`MachineConfig::model`):
//!
//! | Model  | ROMs                          | Video                   | Default RAM |
//! | ------ | ----------------------------- | ----------------------- | ----------- |
//! | "2001" | BASIC 1, editor 1, kernal 1   | 40 columns, fixed timing | 8 KB       |
//! | "3032" | BASIC 2, editor 2, kernal 2   | 40 columns, fixed timing | 32 KB      |
//! | "4032" | BASIC 4, editor 4-40, kernal 4 | 40 columns, CRTC (default model) | 32 KB |
//! | "8032" | BASIC 4, editor 4-80, kernal 4 | 80 columns, CRTC       | 32 KB       |
//!
//! Options: `ram` ("8k", "16k", "32k"); `keyboard` ("graphics" or
//! "business"; the 8032 and the editor ROMs decide what exists); `hz` ("50"
//! default, or "60": which 4.0 editor, i.e. the CRTC refresh rate);
//! `drive8` ("virtual" default, or "none"); `roms` = "stub" for tests
//! without ROMs.
//!
//! ROM roles (`MachineConfig::roms`, user-supplied): `basic1`/`basic2`
//! (8 KB at `$C000`), `basic4` (12 KB at `$B000`), `kernal1/2/4` (4 KB at
//! `$F000`), the editors (2 KB at `$E000`) `edit1n`, `edit2n`, `edit2b`,
//! `edit4-40n-50`/`-60`, `edit4-40b-50`/`-60`, `edit4-80b-50`/`-60`, and
//! the character ROMs `chargen1` (2001) and `chargen2`.
//!
//! The IEEE-488 bus carries the PET's lines and those of a virtual disk
//! drive as device 8 (slot "drive 8": D64, D71, D81, D80 or D82 images;
//! see [`drives_cbm::IeeeDrive`]). Programs also come in through the
//! "program" slot and the datasette on cassette port 1.

#![forbid(unsafe_code)]

pub mod keyboard;
pub mod loader;
pub mod stub;
pub mod video;

use chips_cbm::pia::Pia;
use chips_cbm::via::Via;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use drives_cbm::{IeeeDrive, IeeeLines};
use drives_other::Datasette;
use emu_core::frame::{FrameRef, PixelData, Rgb8, VideoStandard};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::Cycle;
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, CycleResult, DebugHooks, DebugTarget, Decimator, DeviceId, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason, SAMPLE_RATE,
};
use media::tap::TapMachine;

use keyboard::{Keyboard, Layout};
use loader::{ProgramSlot, BASIC_START, BOOT_CYCLES};
use video::Video;

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's view of memory (the PET has no banking).
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

pub const KERNAL_SIZE: usize = 0x1000;
pub const EDITOR_SIZE: usize = 0x800;
pub const CHARGEN_SIZE: usize = 0x800;

const CPU_HZ: u32 = 1_000_000;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "pet",
        name: "Commodore PET",
        models: &["4032", "2001", "3032", "8032"],
        create: |config| Ok(Box::new(Pet::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Pet2001,
    Pet3032,
    Pet4032,
    Pet8032,
}

impl Model {
    fn crtc(self) -> bool {
        matches!(self, Model::Pet4032 | Model::Pet8032)
    }

    fn columns(self) -> u32 {
        if self == Model::Pet8032 {
            80
        } else {
            40
        }
    }

    /// BASIC 1 keeps its pointers elsewhere in zero page.
    fn basic1(self) -> bool {
        self == Model::Pet2001
    }
}

/// The ROMs of one model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roms {
    pub basic: Vec<u8>,
    pub basic_base: u16,
    pub editor: Vec<u8>,
    pub kernal: Vec<u8>,
    pub chargen: Vec<u8>,
}

fn rom(config: &MachineConfig, role: &str, size: usize) -> Result<Vec<u8>, ConfigError> {
    let data = config
        .roms
        .get(role)
        .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
    if data.len() != size {
        return Err(ConfigError::Invalid(format!(
            "{role} ROM must be {size} bytes, got {}",
            data.len()
        )));
    }
    Ok(data.clone())
}

fn load_roms(
    config: &MachineConfig,
    model: Model,
    layout: Layout,
    hz: u32,
) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => {
            return Ok(Roms {
                basic: vec![0xff; 0x2000],
                basic_base: 0xc000,
                editor: vec![0xff; EDITOR_SIZE],
                kernal: stub::kernal(),
                chargen: vec![0; CHARGEN_SIZE],
            })
        }
        Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => {}
    }
    let business = layout == Layout::Business;
    let (basic, basic_size, basic_base, kernal, editor, chargen) = match model {
        Model::Pet2001 if business => {
            return Err(ConfigError::Invalid(
                "the 2001 has no business keyboard editor".into(),
            ))
        }
        Model::Pet2001 => (
            "basic1",
            0x2000,
            0xc000,
            "kernal1",
            "edit1n".to_string(),
            "chargen1",
        ),
        Model::Pet3032 => {
            let edit = if business { "edit2b" } else { "edit2n" };
            (
                "basic2",
                0x2000,
                0xc000,
                "kernal2",
                edit.to_string(),
                "chargen2",
            )
        }
        Model::Pet4032 => {
            let kb = if business { "b" } else { "n" };
            let edit = format!("edit4-40{kb}-{hz}");
            ("basic4", 0x3000, 0xb000, "kernal4", edit, "chargen2")
        }
        Model::Pet8032 if !business => {
            return Err(ConfigError::Invalid(
                "the 8032 editor needs the business keyboard".into(),
            ))
        }
        Model::Pet8032 => {
            let edit = format!("edit4-80b-{hz}");
            ("basic4", 0x3000, 0xb000, "kernal4", edit, "chargen2")
        }
    };
    Ok(Roms {
        basic: rom(config, basic, basic_size)?,
        basic_base,
        editor: rom(config, &editor, EDITOR_SIZE)?,
        kernal: rom(config, kernal, KERNAL_SIZE)?,
        chargen: rom(config, chargen, CHARGEN_SIZE)?,
    })
}

/// What a CPU address selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Area {
    Ram,
    Video,
    Basic,
    Editor,
    Io,
    Kernal,
    /// Nothing there: the data bus floats.
    None,
}

/// Memory map: RAM `$0000–$7FFF` (as fitted), video RAM `$8000–$8FFF`
/// (1 or 2 KB, mirrored), expansion ROM sockets `$9000–$AFFF` (empty),
/// BASIC, editor `$E000–$E7FF`, I/O `$E800–$E8FF`, kernal `$F000–$FFFF`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub ram: Vec<u8>,
    pub vram: Vec<u8>,
    pub roms: Roms,
}

impl Memory {
    fn area(&self, a: u16) -> Area {
        match a {
            0x0000..=0x7fff if usize::from(a) < self.ram.len() => Area::Ram,
            0x8000..=0x8fff => Area::Video,
            0xb000..=0xdfff if a >= self.roms.basic_base => Area::Basic,
            0xe000..=0xe7ff => Area::Editor,
            0xe800..=0xe8ff => Area::Io,
            0xf000..=0xffff => Area::Kernal,
            _ => Area::None,
        }
    }

    fn vram_index(&self, a: u16) -> usize {
        usize::from(a) & (self.vram.len() - 1)
    }
}

/// I/O chips at `$E800–$E8FF`: PIA1 where A4 = 1 (`$E810`), PIA2 where
/// A5 = 1 (`$E820`), VIA where A6 = 1 (`$E840`), CRTC where A7 = 1
/// (`$E880`, CRTC models).
///
/// Wiring (PET schematics, Commodore PET user manuals):
/// - PIA1: PA0–3 keyboard row select, PA4/PA5 cassette sense, PA6 EOI in,
///   PA7 diagnostic sense; PB keyboard columns; CA2 EOI out; CB1 retrace
///   interrupt (vertical sync); CA1 cassette 1 read; CB2 cassette 1 motor
///   (low = on).
/// - PIA2: PA IEEE data in, PB IEEE data out; CA1 ATN in, CA2 NDAC out;
///   CB1 SRQ in, CB2 DAV out.
/// - VIA: PB0 NDAC in, PB1 NRFD out, PB2 ATN out, PB3 cassette write,
///   PB4 cassette 2 motor, PB5 retrace in, PB6 NRFD in, PB7 DAV in; CA2
///   character set (high = text); CB2 sound (shift register).
///
/// All three IRQ outputs drive /IRQ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Io {
    pub pia1: Pia,
    pub pia2: Pia,
    pub via: Via,
    pub keyboard: Keyboard,
    /// Vertical sync from the video timing.
    pub retrace: bool,
    /// A key of the datasette on cassette port 1 is down (PIA1 PA4 low).
    pub cassette_sense: bool,
    /// IEEE-488 bus levels after the last update (`true` = low).
    pub ieee: IeeeLines,
}

impl Io {
    fn pia1_pins(&self) -> (u8, u8) {
        let mut ext_a = 0xff;
        if self.ieee.eoi {
            ext_a &= !0x40; // EOI in
        }
        if self.cassette_sense {
            ext_a &= !0x10;
        }
        let pa = self.pia1.port_a_out() & ext_a;
        let pb = self.pia1.port_b_out() & self.keyboard.columns(pa & 0x0f);
        (pa, pb)
    }

    /// PIA2 port A reads the IEEE-488 data lines (low = 1 bit, so a pulled
    /// line reads 0), port B drives them.
    fn pia2_pins(&self) -> (u8, u8) {
        (self.pia2.port_a_out() & !self.ieee.data, 0xff)
    }

    /// What the PET pulls on the IEEE-488 bus: VIA PB2 ATN, PB1 NRFD,
    /// PIA2 CA2 NDAC, CB2 DAV, PIA1 CA2 EOI (output low = line low), PIA2
    /// port B the data lines (a 0 bit pulls the line).
    fn ieee_out(&self) -> IeeeLines {
        let pb = self.via.port_b_out();
        IeeeLines {
            atn: pb & 0x04 == 0,
            nrfd: pb & 0x02 == 0,
            ndac: !self.pia2.ca2_out(),
            dav: !self.pia2.cb2_out(),
            eoi: !self.pia1.ca2_out(),
            data: !self.pia2.port_b_out(),
        }
    }

    fn via_pins(&self) -> (u8, u8) {
        let pb_out = self.via.port_b_out();
        let mut ext = 0xff;
        if self.ieee.ndac {
            ext &= !0x01;
        }
        if self.retrace {
            ext &= !0x20;
        }
        if self.ieee.nrfd {
            ext &= !0x40;
        }
        if self.ieee.dav {
            ext &= !0x80;
        }
        (self.via.port_a_out(), pb_out & ext)
    }
}

/// The CPU's bus.
struct PetBus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
    video: &'a mut Video,
    crtc: bool,
}

impl PetBus<'_> {
    /// An I/O read with its side effects (flags cleared by reading).
    fn io_read(&mut self, a: u16) -> u8 {
        let reg = a as u8;
        let mut v = 0xff;
        let mut selected = false;
        if a & 0x10 != 0 {
            let (pa, pb) = self.io.pia1_pins();
            v &= self.io.pia1.read(reg, pa, pb);
            selected = true;
        }
        if a & 0x20 != 0 {
            let (pa, pb) = self.io.pia2_pins();
            v &= self.io.pia2.read(reg, pa, pb);
            selected = true;
        }
        if a & 0x40 != 0 {
            let (pa, pb) = self.io.via_pins();
            v &= self.io.via.read(reg, pa, pb);
            selected = true;
        }
        if a & 0x80 != 0 && self.crtc {
            v &= self.video.crtc.peek(reg);
            selected = true;
        }
        if selected {
            v
        } else {
            (a >> 8) as u8
        }
    }
}

impl Bus for PetBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        u32::from(match self.mem.area(a) {
            Area::Io => self.io_read(a),
            _ => peek_byte(self.mem, self.io, self.video, self.crtc, a),
        })
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_byte(
            self.mem,
            self.io,
            self.video,
            self.crtc,
            addr as u16,
        ))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        let v = v as u8;
        match self.mem.area(a) {
            Area::Ram => self.mem.ram[usize::from(a)] = v,
            Area::Video => {
                let i = self.mem.vram_index(a);
                self.mem.vram[i] = v;
            }
            Area::Io => {
                let reg = a as u8;
                if a & 0x10 != 0 {
                    self.io.pia1.write(reg, v);
                }
                if a & 0x20 != 0 {
                    self.io.pia2.write(reg, v);
                }
                if a & 0x40 != 0 {
                    self.io.via.write(reg, v);
                }
                if a & 0x80 != 0 && self.crtc {
                    self.video.crtc.write(reg, v);
                }
            }
            _ => {}
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Side-effect-free read of any address (the debugger's and the CPU's
/// reads outside I/O).
fn peek_byte(mem: &Memory, io: &Io, video: &Video, crtc: bool, a: u16) -> u8 {
    let i = usize::from(a);
    match mem.area(a) {
        Area::Ram => mem.ram[i],
        Area::Video => mem.vram[mem.vram_index(a)],
        Area::Basic => mem.roms.basic[i - usize::from(mem.roms.basic_base)],
        Area::Editor => mem.roms.editor[i & 0x7ff],
        Area::Kernal => mem.roms.kernal[i & 0xfff],
        Area::Io => {
            let reg = a as u8;
            let mut v = 0xff;
            let mut selected = false;
            if a & 0x10 != 0 {
                let (pa, pb) = io.pia1_pins();
                v &= io.pia1.peek(reg, pa, pb);
                selected = true;
            }
            if a & 0x20 != 0 {
                let (pa, pb) = io.pia2_pins();
                v &= io.pia2.peek(reg, pa, pb);
                selected = true;
            }
            if a & 0x40 != 0 {
                let (pa, pb) = io.via_pins();
                v &= io.via.peek(reg, pa, pb);
                selected = true;
            }
            if a & 0x80 != 0 && crtc {
                v &= video.crtc.peek(reg);
                selected = true;
            }
            if selected {
                v
            } else {
                (a >> 8) as u8
            }
        }
        // The high address byte lingers on the floating data bus.
        Area::None => (a >> 8) as u8,
    }
}

/// Read-only view for disassembly.
struct PeekBus<'a> {
    mem: &'a Memory,
    io: &'a Io,
    video: &'a Video,
    crtc: bool,
}

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_byte(
            self.mem,
            self.io,
            self.video,
            self.crtc,
            addr as u16,
        ))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

pub const SLOT_PROGRAM: SlotId = SlotId(0);
/// The datasette on cassette port 1.
pub const SLOT_TAPE: SlotId = SlotId(1);
/// The virtual IEEE-488 drive, device 8.
pub const SLOT_DRIVE8: SlotId = SlotId(2);

/// Video levels: off, on. The phosphor colour (white 2001, green later
/// models) comes from the monitor profile (`pet-white`, `pet-green`).
const PALETTE: [Rgb8; 2] = [
    Rgb8 { r: 0, g: 0, b: 0 },
    Rgb8 {
        r: 0xff,
        g: 0xff,
        b: 0xff,
    },
];

pub struct Pet {
    model: Model,
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    video: Video,
    sched: Scheduler,
    input: InputQueue,
    program: ProgramSlot,
    /// The datasette on cassette port 1.
    tape: Datasette,
    /// The virtual disk drive on the IEEE-488 bus.
    drive: Option<IeeeDrive>,
    resampler: Decimator,
    /// Audio produced but not yet taken (output, not machine state).
    samples: Vec<i16>,
    field_done_pending: bool,
    /// Cycle of the last reset; programs wait [`BOOT_CYCLES`] from here.
    boot_cycle: u64,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    video: Video,
    sched: Scheduler,
    input: InputQueue,
    resampler: Decimator,
}

impl Pet {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("4032") => Model::Pet4032,
            Some("2001") => Model::Pet2001,
            Some("3032") => Model::Pet3032,
            Some("8032") => Model::Pet8032,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let layout = match config.options.get("keyboard").map(String::as_str) {
            None if model == Model::Pet8032 => Layout::Business,
            None | Some("graphics") => Layout::Graphics,
            Some("business") => Layout::Business,
            Some(other) => return Err(ConfigError::Invalid(format!("keyboard option '{other}'"))),
        };
        let hz = match config.options.get("hz").map(String::as_str) {
            None | Some("50") => 50,
            Some("60") => 60,
            Some(other) => return Err(ConfigError::Invalid(format!("hz option '{other}'"))),
        };
        let ram = match config.options.get("ram").map(String::as_str) {
            None if model == Model::Pet2001 => 0x2000,
            None | Some("32k") => 0x8000,
            Some("8k") => 0x2000,
            Some("16k") => 0x4000,
            Some(other) => return Err(ConfigError::Invalid(format!("ram option '{other}'"))),
        };
        let drive = match config.options.get("drive8").map(String::as_str) {
            None | Some("virtual") => Some(IeeeDrive::new(8)),
            Some("none") => None,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "drive8 option '{other}' (virtual or none)"
                )))
            }
        };
        let roms = load_roms(config, model, layout, hz)?;
        let (regs, standard) = match (model.crtc(), model.columns(), hz) {
            (false, _, _) => (video::FIXED_TIMING, VideoStandard::Ntsc),
            (true, 80, 50) => (video::CRTC_80_50HZ, VideoStandard::Pal),
            (true, _, 50) => (video::CRTC_40_50HZ, VideoStandard::Pal),
            (true, _, _) => (video::CRTC_40_60HZ, VideoStandard::Ntsc),
        };
        let cpc = model.columns() / 40;
        let char_lines = if model.crtc() { 10 } else { 8 };
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Nmos6502,
            ..CpuConfig::default()
        });
        cpu.reset();
        Ok(Self {
            model,
            cpu,
            mem: Memory {
                ram: vec![0; ram],
                vram: vec![0; 0x400 * cpc as usize],
                roms,
            },
            io: Io {
                pia1: Pia::new(),
                pia2: Pia::new(),
                via: Via::new(),
                keyboard: Keyboard::new(layout),
                retrace: false,
                cassette_sense: false,
                ieee: IeeeLines::default(),
            },
            video: Video::new(regs, cpc, char_lines, standard),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            program: ProgramSlot::default(),
            tape: Datasette::new(TapMachine::Pet, 1_000_000),
            drive,
            resampler: Decimator::new(CPU_HZ, SAMPLE_RATE).expect("valid rates"),
            samples: Vec::new(),
            field_done_pending: false,
            boot_cycle: 0,
        })
    }

    pub fn layout(&self) -> Layout {
        self.io.keyboard.layout
    }

    /// One CPU cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        self.io.via.tick();
        // Datasette on cassette port 1: PIA1 CB2 MOTOR (low = on), VIA PB3
        // WRITE, PIA1 PA4 SENSE, READ on PIA1 CA1.
        self.tape.set_motor(!self.io.pia1.cb2_out());
        self.tape.set_write(self.io.via.port_b_out() & 0x08 != 0);
        self.tape.tick();
        self.io.cassette_sense = self.tape.sense();
        self.io.pia1.set_ca1(self.tape.read());
        let text = self.io.via.ca2_out();
        let video = self
            .video
            .tick(&self.mem.vram, &self.mem.roms.chargen, text);
        self.io.retrace = video.vsync;
        // CB1 sees the retrace; the editor interrupts on its falling edge.
        self.io.pia1.set_cb1(!video.vsync);
        // IEEE-488: the drive sees what the PET drives now; ATN in on
        // PIA2 CA1.
        let pet = self.io.ieee_out();
        self.io.ieee = match &mut self.drive {
            Some(d) => {
                d.tick(IeeeLines::combine(&[pet, d.ieee_out()]));
                IeeeLines::combine(&[pet, d.ieee_out()])
            }
            None => pet,
        };
        self.io.pia2.set_ca1(!self.io.ieee.atn);
        let irq = self.io.pia1.irq() || self.io.pia2.irq() || self.io.via.irq();
        self.cpu.set_irq(irq);

        let beam = self.video.beam();
        let result = {
            let mut bus = PetBus {
                mem: &mut self.mem,
                io: &mut self.io,
                video: &mut self.video,
                crtc: self.model.crtc(),
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
        self.sound();
        self.sched.tick();
        let new_field = video.frame_end;
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
                    dbg.trace_instr(CPU, next_pc, self.video.beam());
                }
                if dbg.instr_step_armed() {
                    dbg.check_instr_step();
                }
                if dbg.exec_armed(CPU_SPACE) {
                    dbg.check_exec(CPU_SPACE, next_pc);
                }
            }
            if dbg.raster_armed() {
                let b = self.video.beam();
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

    /// CB2 sound: the shift register output, silent unless it runs.
    fn sound(&mut self) {
        let s = if self.io.via.acr & 0x1c != 0x10 {
            0
        } else if self.io.via.cb2_out() {
            6000
        } else {
            -6000
        };
        if let Some(out) = self.resampler.push(s) {
            // Bound the backlog if nobody drains it.
            if self.samples.len() < SAMPLE_RATE as usize * 4 {
                self.samples.push(out);
            }
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, emu_core::input::InputEvent::Key { machine_key, down }) => {
                self.io.keyboard.set(machine_key, down);
            }
            (InputTarget::Panel, emu_core::input::InputEvent::Control { id, value }) => {
                self.control(id.0, value);
            }
            _ => {}
        }
    }

    /// A console control from the panel (power).
    fn control(&mut self, id: u16, value: u8) {
        match id {
            0 if value == 1 => emu_core::Machine::reset(self, emu_core::ResetKind::Hard),
            _ => {}
        }
    }

    /// Program injection once BASIC has booted.
    fn frame_tasks(&mut self) {
        if self.sched.now().0.saturating_sub(self.boot_cycle) < BOOT_CYCLES {
            return;
        }
        if let Some(prg) = self.program.pending.take() {
            if self.inject(&prg) {
                let (events, _) =
                    keyboard::typed_events(self.layout(), "RUN\r", self.sched.now().0);
                for e in events {
                    self.input.push(e);
                }
            }
        }
    }

    /// Put a program into RAM at its load address and set BASIC's
    /// end-of-program pointers (VARTAB, ARYTAB, STREND). Returns whether it
    /// is a BASIC program (to be RUN).
    fn inject(&mut self, prg: &media::Prg) -> bool {
        let start = usize::from(prg.load);
        let ram = &mut self.mem.ram;
        let len = prg.data.len().min(ram.len().saturating_sub(start));
        if len == 0 {
            return false;
        }
        ram[start..start + len].copy_from_slice(&prg.data[..len]);
        let end = ((start + len) as u16).to_le_bytes();
        let pointers: [usize; 3] = if self.model.basic1() {
            [0x7c, 0x7e, 0x80]
        } else {
            [0x2a, 0x2c, 0x2e]
        };
        for p in pointers {
            ram[p..p + 2].copy_from_slice(&end);
        }
        prg.load == BASIC_START
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            if f.space != CPU_SPACE {
                continue;
            }
            for i in 0..f.size.bytes() {
                let addr = f.addr.wrapping_add(i) & 0xffff;
                let v = (f.value >> (8 * i)) as u8;
                PetBus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                    video: &mut self.video,
                    crtc: self.model.crtc(),
                }
                .write(addr, Size::Byte, u32::from(v));
            }
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

fn pia_regs(name: &'static str, base: u32, p: &Pia) -> ChipInfo {
    ChipInfo {
        name,
        regs: vec![
            reg("ORA", Some(base), u32::from(p.a.or), 8),
            reg("DDRA", Some(base), u32::from(p.a.ddr), 8),
            reg("CRA", Some(base + 1), u32::from(p.a.cr), 8),
            reg("ORB", Some(base + 2), u32::from(p.b.or), 8),
            reg("DDRB", Some(base + 2), u32::from(p.b.ddr), 8),
            reg("CRB", Some(base + 3), u32::from(p.b.cr), 8),
        ],
    }
}

impl Machine for Pet {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        let b = self.video.beam();
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

    /// The RESET line reaches the 6502, the PIAs, the VIA and the CRTC
    /// (which has no reset input on the 6545 and keeps its registers).
    fn reset(&mut self, kind: ResetKind) {
        self.cpu.reset();
        self.io.pia1.reset();
        self.io.pia2.reset();
        self.io.via.reset();
        // The reset line is IFC on the IEEE-488 bus: the drive starts over.
        if let Some(d) = &mut self.drive {
            d.reset();
        }
        if kind == ResetKind::Hard {
            self.mem.ram.fill(0);
            self.mem.vram.fill(0);
        }
        self.input.clear();
        self.field_done_pending = false;
        self.boot_cycle = self.sched.now().0;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: self.video.width(),
            height: self.video.height(),
            pixels: PixelData::Indexed {
                pixels: self.video.pixels(),
                palette: &PALETTE,
            },
            luma_chroma: None,
            signal: self.video.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.samples);
        emu_core::AudioFormat {
            sample_rate: SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.video.beam())
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
        let v = &self.io.via;
        let mut chips = vec![
            pia_regs("pia1", 0xe810, &self.io.pia1),
            pia_regs("pia2", 0xe820, &self.io.pia2),
            ChipInfo {
                name: "via",
                regs: vec![
                    reg("ORB", Some(0xe840), u32::from(v.orb), 8),
                    reg("ORA", Some(0xe841), u32::from(v.ora), 8),
                    reg("DDRB", Some(0xe842), u32::from(v.ddrb), 8),
                    reg("DDRA", Some(0xe843), u32::from(v.ddra), 8),
                    reg("T1", Some(0xe844), u32::from(v.t1_counter), 16),
                    reg("T2", Some(0xe848), u32::from(v.t2_counter), 16),
                    reg("SR", Some(0xe84a), u32::from(v.sr), 8),
                    reg("ACR", Some(0xe84b), u32::from(v.acr), 8),
                    reg("PCR", Some(0xe84c), u32::from(v.pcr), 8),
                    reg("IFR", Some(0xe84d), u32::from(v.ifr), 8),
                    reg("IER", Some(0xe84e), u32::from(v.ier), 8),
                ],
            },
        ];
        if self.model.crtc() {
            const NAMES: [&str; 18] = [
                "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12",
                "R13", "R14", "R15", "R16", "R17",
            ];
            chips.push(ChipInfo {
                name: "crtc",
                regs: NAMES
                    .iter()
                    .zip(self.video.crtc.regs)
                    .map(|(&n, v)| reg(n, None, u32::from(v), 8))
                    .collect(),
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

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        use emu_core::ControlInfo;
        vec![ControlInfo::button(0, "POWER", false)]
    }

    fn ports(&self) -> Vec<PortInfo> {
        Vec::new()
    }

    fn connect(&mut self, port: PortId, _dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        Err(PortError::NoSuchPort(port))
    }

    fn disconnect(&mut self, _port: PortId) -> Option<Box<dyn PortDevice>> {
        None
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut slots = vec![
            MediaSlotInfo {
                id: SLOT_PROGRAM,
                name: "program",
                kind: MediaKind::Program,
                inserted: self.program.inserted(),
            },
            MediaSlotInfo {
                id: SLOT_TAPE,
                name: "tape",
                kind: MediaKind::Tape,
                inserted: self.tape.has_tape(),
            },
        ];
        if let Some(d) = &self.drive {
            slots.push(MediaSlotInfo {
                id: SLOT_DRIVE8,
                name: "drive 8",
                kind: MediaKind::Disk,
                inserted: d.has_disk(),
            });
        }
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_PROGRAM => Some(&mut self.program),
            SLOT_TAPE => Some(&mut self.tape),
            SLOT_DRIVE8 => self.drive.as_mut().map(|d| d as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"PETS", 3, |w| {
            w.bool(self.field_done_pending);
            w.u64(self.boot_cycle);
            w.u32(self.mem.ram.len() as u32);
            w.raw(&self.mem.ram);
            w.u32(self.mem.vram.len() as u32);
            w.raw(&self.mem.vram);
            for row in self.io.keyboard.rows {
                w.u8(row);
            }
            w.bool(self.io.retrace);
            let b = self.io.ieee;
            for v in [b.atn, b.eoi, b.dav, b.nrfd, b.ndac] {
                w.bool(v);
            }
            w.u8(b.data);
            self.resampler.save(w);
        });
        self.cpu.save(&mut w);
        self.io.pia1.save(&mut w);
        self.io.pia2.save(&mut w);
        self.io.via.save(&mut w);
        self.video.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        self.tape.save(&mut w);
        w.chunk(*b"DRV8", 1, |w| w.bool(self.drive.is_some()));
        if let Some(d) = &self.drive {
            d.save(&mut w);
        }
        w.into_bytes()
    }

    /// Atomic: every part loads into a copy first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            mem: self.mem.clone(),
            io: self.io.clone(),
            video: self.video.clone(),
            sched: self.sched.clone(),
            input: self.input.clone(),
            resampler: self.resampler,
        };
        let mut r = StateReader::new(s);
        let (version, mut c) = r.chunk(*b"PETS", 3)?;
        let field_done_pending = c.bool()?;
        let boot_cycle = c.u64()?;
        if c.u32()? as usize != next.mem.ram.len() {
            return Err(StateError::Invalid("RAM size differs"));
        }
        c.raw_into(&mut next.mem.ram)?;
        if c.u32()? as usize != next.mem.vram.len() {
            return Err(StateError::Invalid("video RAM size differs"));
        }
        c.raw_into(&mut next.mem.vram)?;
        for row in &mut next.io.keyboard.rows {
            *row = c.u8()?;
        }
        next.io.retrace = c.bool()?;
        if version >= 3 {
            next.io.ieee = IeeeLines {
                atn: c.bool()?,
                eoi: c.bool()?,
                dav: c.bool()?,
                nrfd: c.bool()?,
                ndac: c.bool()?,
                data: c.u8()?,
            };
        }
        next.resampler.load(&mut c)?;
        next.cpu.load(&mut r)?;
        next.io.pia1.load(&mut r)?;
        next.io.pia2.load(&mut r)?;
        next.io.via.load(&mut r)?;
        next.video.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;
        // The datasette last: its load is atomic, so a failure leaves all
        // as it was.
        // The datasette and the drive load atomically; the drive comes back
        // if the tape fails after it.
        let tape_backup = {
            let mut w = StateWriter::new();
            self.tape.save(&mut w);
            w.into_bytes()
        };
        if version >= 2 {
            self.tape.load(&mut r)?;
        }
        if version >= 3 {
            let drive_chunk = r.chunk(*b"DRV8", 1).and_then(|(_, mut c)| c.bool());
            let loaded = match drive_chunk {
                Ok(present) if present == self.drive.is_some() => match &mut self.drive {
                    Some(d) => d.load(&mut r),
                    None => Ok(()),
                },
                Ok(_) => Err(StateError::Invalid("drive 8 configuration differs")),
                Err(e) => Err(e),
            };
            if let Err(e) = loaded {
                let _ = self.tape.load(&mut StateReader::new(&tape_backup));
                return Err(e);
            }
        }
        next.io.cassette_sense = self.tape.sense();
        self.cpu = next.cpu;
        self.mem = next.mem;
        self.io = next.io;
        self.video = next.video;
        self.sched = next.sched;
        self.input = next.input;
        self.resampler = next.resampler;
        self.field_done_pending = field_done_pending;
        self.boot_cycle = boot_cycle;
        Ok(())
    }
}

impl DebugTarget for Pet {
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
            let b = peek_byte(&self.mem, &self.io, &self.video, self.model.crtc(), a);
            v |= u32::from(b) << (8 * i);
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
                Area::Ram | Area::Video | Area::Io => PetBus {
                    mem: &mut self.mem,
                    io: &mut self.io,
                    video: &mut self.video,
                    crtc: self.model.crtc(),
                }
                .write(u32::from(a), Size::Byte, (v >> (8 * i)) & 0xff),
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(
            &PeekBus {
                mem: &self.mem,
                io: &self.io,
                video: &self.video,
                crtc: self.model.crtc(),
            },
            addr,
        )
    }
}

#[cfg(test)]
mod tests;
