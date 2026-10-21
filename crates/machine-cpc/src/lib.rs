//! Amstrad CPC 464, 664 and 6128.
//!
//! Z80 at 4 MHz, Gate Array (video, palette, interrupts, wait states),
//! CRTC 6845, 8255 PPI, AY-3-8912 (sound at 1 MHz, keyboard port),
//! 64 KB RAM (128 KB on the 6128) with the OS and BASIC ROMs and, on the
//! 664/6128, the AMSDOS disc ROM as upper ROM 7 (see [`memory`]).
//!
//! Models: "6128" (default), "664", "464". Options:
//! - `crtc`: CRTC type "0", "1" or "2" (default 0 on the 464, 1 otherwise).
//! - `dos`: upper ROM 7 on the disc models, "amsdos" (default), "parados"
//!   or "none".
//! - `roms`: "stub" runs a tiny built-in program instead of the OS (tests
//!   without ROMs).
//! - `joy1`: the joystick port: "joystick" (default; Amstrad JY2, two
//!   buttons), "amx" (AMX mouse) or "none"; `joy2`: a second joystick
//!   daisy-chained through the first, "joystick" or "none" (default).
//!   Joystick 0 reads as keyboard line 9, joystick 1 as line 6.
//!
//! Media slots: "drive A" and "drive B" (664/6128: DSK images on the
//! µPD765A, ports &FA7E motor, &FB7E status, &FB7F data) and "tape"
//! (CDT/TZX on the cassette deck: motor on PPI port C bit 4, write bit 5,
//! read on port B bit 7).
//!
//! ROMs (`MachineConfig::roms`, see `media-backends`): "os464"/"basic464",
//! "os664"/"basic664", "os6128"/"basic6128" (16 KB each), or the 32 KB
//! "rom464"/"rom664"/"rom6128" with both, "amsdos", "parados". User-supplied,
//! never in the repository.
//!
//! **Timing.** The Z80 runs whole instructions with [`Z80::step_timed`];
//! before each of its bus accesses the bus brings the CRTC and the Gate
//! Array up to that T-state and adds the Gate Array's wait states, so I/O
//! and video memory see the CPU's accesses at the right moment. The CRTC
//! and the Gate Array advance once per microsecond (every 4th T-state).
//! [`Machine::step_cycle`] runs one instruction.

#![forbid(unsafe_code)]

pub mod keyboard;
pub mod memory;
pub mod stub;

use chips_amstrad::ay::Ay;
use chips_amstrad::crtc::{Crtc, CrtcType};
use chips_amstrad::fdc::Fdc;
use chips_amstrad::gate_array::{self, GateArray};
use chips_amstrad::ppi::Ppi;
use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use emu_core::audio::Decimator;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, InterruptKind,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

use drives_other::cpc_drive::CpcDrive;
use drives_other::cpc_tape::CpcTape;
use memory::{Memory, Rom, Roms, ROM_SIZE};

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–&FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

/// Joystick 0 (the port) and joystick 1 (daisy-chained through it).
pub const JOY0: PortId = PortId(0);
pub const JOY1: PortId = PortId(1);

/// T-states one step of AMX mouse motion holds its direction line (1 ms).
const AMX_STEP: u64 = 4000;

/// Disc drives A and B (664/6128) and the cassette deck.
pub const SLOT_A: SlotId = SlotId(0);
pub const SLOT_B: SlotId = SlotId(1);
pub const SLOT_TAPE: SlotId = SlotId(2);

/// The FDC's ports: A10 = 0 and A7 = 0; A8 = 0 the motor latch, else A0
/// picks the status (0) or data (1) register.
fn fdc_port(port: u16) -> Option<u8> {
    if port & 0x0480 != 0 {
        return None;
    }
    Some(match (port >> 8 & 1, port & 1) {
        (0, _) => 0,
        (_, 0) => 1,
        _ => 2,
    })
}

/// Z80 T-states per second.
pub const CPU_HZ: u32 = 4_000_000;
/// T-states per microsecond (one CRTC character).
const T_PER_US: u64 = 4;
/// The T-state of each microsecond on which the CRTC and the Gate Array
/// advance: its first, where the Gate Array also releases /WAIT. Checked
/// by the Arnold acid tests "ld a,r/ld a,i p/v int test" (an interrupt
/// right after LD A,I hits on exactly two NOP positions) and "CPC int
/// (HSYNC width)"; phase 3 failed both.
const CHIP_PHASE: u64 = 0;

/// PPI port B inputs other than VSYNC and the cassette (7): printer not
/// ready (6), no expansion (5), 50 Hz (4), distributor Amstrad (3–1).
const PORT_B: u8 = 0x7e;
/// PPI port C pins when nothing drives them (Arnold acid test `ppi`).
const PORT_C_PULLS: u8 = 0x2f;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "cpc",
        name: "Amstrad CPC",
        models: &["6128", "664", "464"],
        create: |config| Ok(Box::new(Cpc::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Cpc464,
    Cpc664,
    Cpc6128,
}

impl Model {
    fn id(self) -> u8 {
        match self {
            Self::Cpc464 => 0,
            Self::Cpc664 => 1,
            Self::Cpc6128 => 2,
        }
    }

    fn rom_names(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Cpc464 => ("os464", "basic464", "rom464"),
            Self::Cpc664 => ("os664", "basic664", "rom664"),
            Self::Cpc6128 => ("os6128", "basic6128", "rom6128"),
        }
    }
}

fn port_option(
    config: &MachineConfig,
    key: &str,
    default: &str,
    mouse: bool,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match config
        .options
        .get(key)
        .map(String::as_str)
        .unwrap_or(default)
    {
        "joystick" => Ok(Some(Box::new(devices_input::Joystick::new(
            PortKind::CpcJoy,
        )))),
        "amx" if mouse => Ok(Some(Box::new(devices_input::AmxMouse::new(AMX_STEP)))),
        "none" => Ok(None),
        other => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' ({})",
            if mouse {
                "joystick, amx or none"
            } else {
                "joystick or none"
            }
        ))),
    }
}

/// Port lines as keyboard matrix bits: up, down, left, right, fire 2 (pin
/// 6, the main button), fire 1 (pin 7), fire 3 (pin 5) — BASIC's JOY()
/// bits 0–5.
fn joystick_bits(lines: PortLines) -> u8 {
    use devices_input::joystick::pin;
    [
        pin::UP,
        pin::DOWN,
        pin::LEFT,
        pin::RIGHT,
        pin::FIRE,
        pin::CPC_FIRE1,
        pin::BUTTON3,
    ]
    .iter()
    .enumerate()
    .filter(|(_, &p)| lines.pulled_low & p != 0)
    .fold(0, |v, (bit, _)| v | 1 << bit)
}

fn rom16(data: &[u8], role: &str) -> Result<Rom, ConfigError> {
    let boxed: Box<[u8]> = data.to_vec().into_boxed_slice();
    boxed.try_into().map_err(|_| {
        ConfigError::Invalid(format!(
            "{role} ROM must be 16384 bytes, got {}",
            data.len()
        ))
    })
}

fn load_roms(config: &MachineConfig, model: Model) -> Result<Roms, ConfigError> {
    if let Some(v) = config.options.get("roms") {
        return match v.as_str() {
            "stub" => Ok(stub::roms()),
            other => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        };
    }
    let (os_role, basic_role, both_role) = model.rom_names();
    let (os, basic) = match (config.roms.get(os_role), config.roms.get(basic_role)) {
        (Some(os), Some(basic)) => (rom16(os, os_role)?, rom16(basic, basic_role)?),
        _ => {
            let both = config
                .roms
                .get(both_role)
                .ok_or_else(|| ConfigError::MissingRom(format!("{os_role} + {basic_role}")))?;
            if both.len() != 2 * ROM_SIZE {
                return Err(ConfigError::Invalid(format!(
                    "{both_role} ROM must be 32768 bytes"
                )));
            }
            (
                rom16(&both[..ROM_SIZE], both_role)?,
                rom16(&both[ROM_SIZE..], both_role)?,
            )
        }
    };
    let dos = if model == Model::Cpc464 {
        None
    } else {
        match config.options.get("dos").map(String::as_str) {
            None | Some("amsdos") => match config.roms.get("amsdos") {
                Some(r) => Some(rom16(r, "amsdos")?),
                None => None,
            },
            Some("parados") => Some(rom16(
                config
                    .roms
                    .get("parados")
                    .ok_or_else(|| ConfigError::MissingRom("parados".into()))?,
                "parados",
            )?),
            Some("none") => None,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "dos option '{other}' (amsdos, parados or none)"
                )))
            }
        }
    };
    Ok(Roms { os, basic, dos })
}

/// The chips on the I/O bus.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Io {
    crtc: Crtc,
    ga: GateArray,
    ppi: Ppi,
    ay: Ay,
    fdc: Fdc,
    /// The disc interface is fitted (664/6128).
    has_fdc: bool,
    /// Keyboard matrix lines, a 0 bit = key down.
    keys: [u8; 10],
    /// Joystick 0 and 1 as matrix bits of lines 9 and 6 (1 = closed).
    joy: [u8; 2],
    /// The CRTC's VSYNC (PPI port B bit 0) and the cassette's READ (bit 7).
    vsync: bool,
    cas_in: bool,
    /// The PAL's RAM configurations exist (6128).
    has_pal: bool,
}

impl Io {
    /// The keyboard line selected by PPI port C.
    fn key_line(&self) -> u8 {
        let line = usize::from(self.ppi.c_pins() & 0x0f);
        let joy = match line {
            9 => self.joy[0],
            6 => self.joy[1],
            _ => 0,
        };
        self.keys.get(line).copied().unwrap_or(0xff) & !joy
    }

    /// What PPI port A reads: the AY's bus in a read cycle.
    fn ppi_a_in(&self) -> u8 {
        if self.ppi.c_pins() >> 6 == 1 {
            self.ay.read(self.key_line())
        } else {
            0xff
        }
    }

    fn port_b(&self) -> u8 {
        PORT_B | u8::from(self.vsync) | u8::from(self.cas_in) << 7
    }

    /// An I/O read (no side effects on the CPC's chips).
    fn read(&self, port: u16) -> u8 {
        let mut v = 0xff;
        if port & 0x4000 == 0 {
            match (port >> 8) & 3 {
                2 => v &= self.crtc.status(),
                3 => v &= self.crtc.read(),
                _ => {}
            }
        }
        if port & 0x0800 == 0 {
            v &= self
                .ppi
                .read((port >> 8) as u8 & 3, self.ppi_a_in(), self.port_b());
        }
        if self.has_fdc {
            match fdc_port(port) {
                Some(1) => v &= self.fdc.status(),
                Some(2) => v &= self.fdc.peek_data(),
                _ => {}
            }
        }
        v
    }

    /// An I/O write; several chips may be selected at once.
    fn write(&mut self, mem: &mut Memory, port: u16, v: u8) {
        if port & 0xc000 == 0x4000 {
            if let Some(mmr) = self.ga.write(v) {
                if self.has_pal {
                    mem.config = mmr & 7;
                }
            }
        }
        if port & 0x4000 == 0 {
            match (port >> 8) & 3 {
                0 => self.crtc.select(v),
                1 => self.crtc.write(v),
                _ => {}
            }
        }
        if port & 0x2000 == 0 {
            mem.upper = v;
        }
        if self.has_fdc && fdc_port(port) == Some(0) {
            self.fdc.motor = v & 1 != 0;
        }
        if port & 0x0800 == 0 {
            self.ppi.write((port >> 8) as u8 & 3, v);
            // BDIR/BC1 on port C drive the AY with port A's value.
            let function = self.ppi.c_pins() >> 6;
            if function >= 2 {
                self.ay.bus_write(function, self.ppi.a_out());
            }
        }
    }

    /// One microsecond of video.
    fn tick_us(&mut self, mem: &Memory) {
        let out = self.crtc.tick();
        self.vsync = out.vsync;
        self.ga.tick(out, mem.video());
    }
}

/// The Z80's bus for one instruction: brings the chips up to each access,
/// adds the wait states, routes debugger hooks.
struct CpcBus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
    /// T-states the chips have run.
    chips_now: &'a mut u64,
    /// T-state the instruction started on.
    start: u64,
    side: Side<'a>,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

/// What runs beside the chips: the disc drives, the cassette deck and the
/// audio.
struct Side<'a> {
    drives: &'a mut [CpcDrive; 2],
    tape: &'a mut CpcTape,
    audio: &'a mut Decimator,
    samples: &'a mut Vec<i16>,
}

/// Run the chips up to (not including) T-state `to`.
fn catch_up(mem: &Memory, io: &mut Io, now: &mut u64, to: u64, side: &mut Side<'_>) {
    while *now < to {
        if *now % T_PER_US == CHIP_PHASE {
            io.tick_us(mem);
            io.ay.tick();
            if io.has_fdc {
                for d in side.drives.iter_mut() {
                    d.set_motor(io.fdc.motor);
                }
                io.fdc.tick(side.drives);
            }
            let c = io.ppi.c_pins();
            side.tape.set_motor(c & 0x10 != 0);
            side.tape.set_write(c & 0x20 != 0);
            side.tape.tick();
            io.cas_in = side.tape.read();
            // Channels A, B and C mixed into mono, as on the speaker.
            let [a, b, c] = io.ay.output();
            let level = (i32::from(a) + i32::from(b) + i32::from(c)) * 3 / 4;
            if let Some(s) = side.audio.push(level) {
                side.samples.push(s);
            }
        }
        *now += 1;
    }
}

impl CpcBus<'_> {
    fn raw_read(&mut self, addr: u32) -> u8 {
        if addr >= IO_BASE {
            let port = addr as u16;
            let v = self.io.read(port);
            if port & 0x4300 == 0x0300 {
                self.io.crtc.after_read();
            }
            if self.io.has_fdc && fdc_port(port) == Some(2) {
                // The read itself moves the FDC on (the value is the same
                // as the peek above).
                self.io.fdc.read_data();
            }
            v
        } else {
            self.mem
                .read(addr as u16, self.io.ga.lower_rom(), self.io.ga.upper_rom())
        }
    }
}

impl Bus for CpcBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = u32::from(self.raw_read(addr));
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
        u32::from(peek(self.mem, self.io, addr))
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
            let port = addr as u16;
            self.io.write(self.mem, port, v as u8);
            if self.io.has_fdc && fdc_port(port) == Some(2) {
                self.io.fdc.write_data(v as u8, self.side.drives);
            }
        } else {
            self.mem.write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for CpcBus<'_> {
    fn wait_states(&mut self, _access: Access, t: u32) -> u32 {
        let at = self.start + u64::from(t);
        let w = gate_array::cpu_wait_states(at);
        catch_up(
            self.mem,
            self.io,
            self.chips_now,
            at + u64::from(w),
            &mut self.side,
        );
        w
    }
}

fn peek(mem: &Memory, io: &Io, addr: u32) -> u8 {
    if addr >= IO_BASE {
        io.read(addr as u16)
    } else {
        mem.read(addr as u16, io.ga.lower_rom(), io.ga.upper_rom())
    }
}

struct PeekBus<'a>(&'a Memory, &'a Io);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.0, self.1, addr))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

pub struct Cpc {
    model: Model,
    cpu: Z80,
    mem: Memory,
    io: Io,
    /// T-states since power-on (the CPU's and the chips' time base).
    sched: Scheduler,
    input: InputQueue,
    palette: [Rgb8; 32],
    audio: Decimator,
    samples: Vec<i16>,
    field_done_pending: bool,
    ports: [Option<Box<dyn PortDevice>>; 2],
    /// T-state of the next port refresh.
    ports_due: u64,
    drives: [CpcDrive; 2],
    tape: CpcTape,
}

/// The clonable part of the machine, for atomic savestate loading.
struct Snapshot {
    cpu: Z80,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
    audio: Decimator,
}

impl Cpc {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("6128") => Model::Cpc6128,
            Some("664") => Model::Cpc664,
            Some("464") => Model::Cpc464,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let crtc = match config.options.get("crtc").map(String::as_str) {
            None => {
                if model == Model::Cpc464 {
                    CrtcType::T0
                } else {
                    CrtcType::T1
                }
            }
            Some(n) => n
                .parse::<u8>()
                .ok()
                .and_then(CrtcType::from_number)
                .ok_or_else(|| ConfigError::Invalid(format!("crtc option '{n}' (0, 1 or 2)")))?,
        };
        let ram_kb = if model == Model::Cpc6128 { 128 } else { 64 };
        let mut m = Self {
            model,
            cpu: Z80::new(),
            mem: Memory::new(load_roms(config, model)?, ram_kb),
            io: Io {
                crtc: Crtc::new(crtc),
                ga: GateArray::new(),
                ppi: Ppi::with_c_pulls(PORT_C_PULLS),
                ay: Ay::new(),
                fdc: Fdc::new(),
                has_fdc: model != Model::Cpc464,
                keys: [0xff; 10],
                joy: [0; 2],
                vsync: false,
                cas_in: false,
                has_pal: model == Model::Cpc6128,
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            palette: chips_amstrad::palette::palette(),
            audio: Decimator::new(1_000_000, emu_core::SAMPLE_RATE)
                .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
            samples: Vec::new(),
            field_done_pending: false,
            ports: [
                port_option(config, "joy1", "joystick", true)?,
                port_option(config, "joy2", "none", false)?,
            ],
            ports_due: 0,
            drives: [CpcDrive::new(), CpcDrive::new()],
            tape: CpcTape::new(),
        };
        m.cpu.set_int_vector(0xff);
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let (line, us, fields) = self.io.ga.beam();
        BeamPos {
            frame: fields,
            line,
            cycle: u16::from(us),
            hpos: u16::from(us) * 16,
        }
    }

    /// Read the joystick port's lines into the keyboard matrix.
    fn refresh_ports(&mut self) {
        let now = self.sched.now();
        for (i, dev) in self.ports.iter_mut().enumerate() {
            self.io.joy[i] = match dev {
                Some(d) => {
                    d.write_lines(PortLines::default(), now);
                    joystick_bits(d.read_lines(now))
                }
                None => 0,
            };
        }
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        // Lines that change with time (AMX mouse): once a raster line.
        let now = self.sched.now().0;
        if now >= self.ports_due && self.ports.iter().any(Option::is_some) {
            self.ports_due = now + 256;
            self.refresh_ports();
        }
        self.cpu.set_int(self.io.ga.int_pending);
        let start = self.sched.now().0;
        let mut chips_now = start;
        let beam = self.beam_pos();
        let t = {
            let mut bus = CpcBus {
                mem: &mut self.mem,
                io: &mut self.io,
                chips_now: &mut chips_now,
                start,
                side: Side {
                    drives: &mut self.drives,
                    tape: &mut self.tape,
                    audio: &mut self.audio,
                    samples: &mut self.samples,
                },
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step_timed(&mut bus)
        };
        let end = start + u64::from(t);
        catch_up(
            &self.mem,
            &mut self.io,
            &mut chips_now,
            end,
            &mut Side {
                drives: &mut self.drives,
                tape: &mut self.tape,
                audio: &mut self.audio,
                samples: &mut self.samples,
            },
        );
        self.sched.advance(u64::from(t));
        let entered = self.cpu.take_interrupt();
        if entered == Some(InterruptKind::Irq) {
            self.io.ga.acknowledge();
        }
        let new_field = self.io.ga.take_field_done();
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
                self.field_done_pending |= new_field;
                return Some(stop);
            }
        }
        new_field.then_some(StopReason::FrameDone)
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.key(machine_key.0, down);
            }
            (InputTarget::Port(p), event) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&event);
                }
                self.refresh_ports();
            }
            _ => {}
        }
    }

    fn key(&mut self, code: u16, down: bool) {
        let (line, bit) = (usize::from(code / 8), code % 8);
        if let Some(l) = self.io.keys.get_mut(line) {
            if down {
                *l &= !(1 << bit);
            } else {
                *l |= 1 << bit;
            }
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
                let addr = f.addr.wrapping_add(i);
                let v = (f.value >> (8 * i)) as u8;
                if addr >= IO_BASE {
                    self.io.write(&mut self.mem, addr as u16, v);
                } else {
                    self.mem.write(addr as u16, v);
                }
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: 16_000_000,
            active_pixels: gate_array::WIDTH,
            total_pixels: 64 * 16,
            lines_per_field: 312,
            standard: VideoStandard::Pal,
            output: OutputType::Rgb,
            field: FieldParity::Progressive,
        }
    }
}

/// Typing for tests and tools: queue key presses for `text`.
pub fn type_text(m: &mut Cpc, text: &str) {
    m.type_text(text);
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Cpc, addr: u16) -> u8 {
    peek(&m.mem, &m.io, u32::from(addr))
}

fn regs(name: &'static str, base: Option<u32>, values: &[(&'static str, u8)]) -> ChipInfo {
    ChipInfo {
        name,
        regs: values
            .iter()
            .enumerate()
            .map(|(i, &(n, v))| ChipReg {
                name: n,
                addr: base.map(|b| b + i as u32),
                value: u32::from(v),
                bits: 8,
            })
            .collect(),
    }
}

impl Machine for Cpc {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        let b = self.beam_pos();
        if b.line == 0 && b.cycle == 0 {
            self.apply_frame_freezes(dbg);
        }
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.instruction(None);
        StopReason::StepDone
    }

    /// RESET (power-on; the CPC has no reset button): CPU, Gate Array,
    /// PPI, AY and the upper ROM / RAM configuration latches. Power-up
    /// also clears RAM.
    fn reset(&mut self, kind: ResetKind) {
        let hard = kind == ResetKind::Hard;
        self.cpu.reset();
        self.io.ga.reset();
        self.io.ppi.reset();
        self.io.ay.reset();
        self.io.fdc.reset();
        self.mem.reset(hard);
        if hard {
            self.io.crtc = Crtc::new(self.io.crtc.kind);
            self.io.keys = [0xff; 10];
        }
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: gate_array::WIDTH,
            height: gate_array::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.io.ga.pixels(),
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
            addr_bits: 17,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let c = &self.io.crtc;
        const CRTC_NAMES: [&str; 18] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14", "R15", "R16", "R17",
        ];
        let crtc: Vec<(&'static str, u8)> = CRTC_NAMES
            .iter()
            .zip(c.regs)
            .map(|(&n, v)| (n, v))
            .collect();
        let ga = &self.io.ga;
        const PENS: [&str; 17] = [
            "PEN0", "PEN1", "PEN2", "PEN3", "PEN4", "PEN5", "PEN6", "PEN7", "PEN8", "PEN9",
            "PEN10", "PEN11", "PEN12", "PEN13", "PEN14", "PEN15", "BORDER",
        ];
        let mut gav: Vec<(&'static str, u8)> =
            PENS.iter().zip(ga.inks).map(|(&n, v)| (n, v)).collect();
        gav.extend([
            ("RMR", ga.rmr),
            ("MODE", ga.mode),
            ("INTCNT", ga.int_counter),
            ("RAMCFG", self.mem.config),
            ("UPPER", self.mem.upper),
        ]);
        let p = &self.io.ppi;
        const AY_NAMES: [&str; 16] = [
            "AFINE", "ACOARSE", "BFINE", "BCOARSE", "CFINE", "CCOARSE", "NOISE", "MIXER", "AVOL",
            "BVOL", "CVOL", "EFINE", "ECOARSE", "ESHAPE", "PORTA", "PORTB",
        ];
        let ay: Vec<(&'static str, u8)> = AY_NAMES
            .iter()
            .zip(self.io.ay.regs)
            .map(|(&n, v)| (n, v))
            .collect();
        let (hcc, vlc, vcc) = c.counters();
        let mut crtc = crtc;
        crtc.extend([
            ("HCC", hcc),
            ("VLC", vlc),
            ("VCC", vcc),
            ("TYPE", c.kind.number()),
        ]);
        let f = &self.io.fdc;
        let fdc = [
            ("MSR", f.status()),
            ("MOTOR", u8::from(f.motor)),
            ("PCN_A", f.cylinder(0)),
            ("PCN_B", f.cylinder(1)),
        ];
        let mut chips = vec![
            regs("crtc", None, &crtc),
            regs("gate_array", None, &gav),
            regs(
                "ppi",
                None,
                &[
                    ("PORTA", p.a),
                    ("PORTB", self.io.port_b()),
                    ("PORTC", p.c),
                    ("CONTROL", p.control),
                ],
            ),
            regs("ay", None, &ay),
        ];
        if self.io.has_fdc {
            chips.push(regs("fdc", None, &fdc));
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

    fn ports(&self) -> Vec<PortInfo> {
        [(JOY0, "joystick port"), (JOY1, "joystick 2 (daisy chain)")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::CpcJoy,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::CpcJoy {
            return Err(PortError::Incompatible {
                port: PortKind::CpcJoy,
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
        let mut slots = Vec::new();
        if self.io.has_fdc {
            for (i, (id, name)) in [(SLOT_A, "drive A"), (SLOT_B, "drive B")]
                .into_iter()
                .enumerate()
            {
                slots.push(MediaSlotInfo {
                    id,
                    name,
                    kind: MediaKind::Disk,
                    inserted: self.drives[i].has_disc(),
                });
            }
        }
        slots.push(MediaSlotInfo {
            id: SLOT_TAPE,
            name: "tape",
            kind: MediaKind::Tape,
            inserted: self.tape.has_tape(),
        });
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_A | SLOT_B if self.io.has_fdc => {
                Some(&mut self.drives[usize::from(slot.0)] as &mut dyn MediaSlot)
            }
            SLOT_TAPE => Some(&mut self.tape),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"CPCM", 1, |w| {
            w.u8(self.model.id());
            w.bool(self.field_done_pending);
            w.raw(&self.io.keys);
            w.raw(&self.io.joy);
            w.u64(self.ports_due);
            w.bool(self.io.vsync);
            w.bool(self.io.cas_in);
            self.audio.save(w);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.io.crtc.save(&mut w);
        self.io.ga.save(&mut w);
        self.io.ppi.save(&mut w);
        self.io.ay.save(&mut w);
        self.io.fdc.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        self.tape.save(&mut w);
        for d in &self.drives {
            d.save(&mut w);
        }
        for (i, tag) in [*b"JOY0", *b"JOY1"].into_iter().enumerate() {
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
        let mut next = Snapshot {
            cpu: self.cpu.clone(),
            mem: self.mem.clone(),
            io: self.io.clone(),
            sched: self.sched.clone(),
            input: self.input.clone(),
            audio: self.audio,
        };
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"CPCM", 1)?;
        if c.u8()? != self.model.id() {
            return Err(StateError::Invalid("CPC model differs"));
        }
        let field_done_pending = c.bool()?;
        c.raw_into(&mut next.io.keys)?;
        c.raw_into(&mut next.io.joy)?;
        let ports_due = c.u64()?;
        next.io.vsync = c.bool()?;
        next.io.cas_in = c.bool()?;
        next.audio.load(&mut c)?;
        next.cpu.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.crtc.load(&mut r)?;
        next.io.ga.load(&mut r)?;
        next.io.ppi.load(&mut r)?;
        next.io.ay.load(&mut r)?;
        next.io.fdc.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;
        // The deck loads last (it has no copy), after everything that can
        // fail: check its chunk first.
        let at = s.len() - r.remaining();
        CpcTape::new().load(&mut r)?;
        for _ in 0..2 {
            CpcDrive::new().load(&mut r)?;
        }
        let mut port_states = Vec::new();
        for tag in [*b"JOY0", *b"JOY1"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
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
        let mut rest = StateReader::new(&s[at..]);
        self.tape.load(&mut rest)?;
        for d in &mut self.drives {
            d.load(&mut rest)?;
        }
        self.cpu = next.cpu;
        self.mem = next.mem;
        self.io = next.io;
        self.sched = next.sched;
        self.input = next.input;
        self.audio = next.audio;
        self.samples.clear();
        self.field_done_pending = field_done_pending;
        self.ports_due = ports_due;
        Ok(())
    }
}

impl DebugTarget for Cpc {
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
            let a = addr.wrapping_add(i) & 0x1_ffff;
            v |= u32::from(peek(&self.mem, &self.io, a)) << (8 * i);
        }
        Some(v)
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i) & 0x1_ffff;
            let b = (v >> (8 * i)) as u8;
            if a >= IO_BASE {
                self.io.write(&mut self.mem, a as u16, b);
            } else {
                self.mem.write(a as u16, b);
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
