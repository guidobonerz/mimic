//! Sinclair ZX Spectrum 48K, 128K and +2.
//!
//! Z80A at 3.5 MHz (48K) or 3.5469 MHz (128K, +2), the ULA
//! (`chips_sinclair::spectrum`: display, contended memory, port `$FE`,
//! the frame interrupt), 16 KB ROM and 48 KB RAM, or two ROMs and eight
//! RAM banks paged by port `$7FFD` ([`memory`]) with an AY-3-8912 sound
//! chip (`chips_amstrad::ay`, clocked at half the CPU clock; register
//! select `$FFFD`, data `$BFFD`, decoded by A15/A14/A1).
//!
//! Models: "48k" (default), "128k", "plus2" (the Amstrad-built +2: the
//! 128K's hardware with its own ROMs). Options:
//! - `traps`: "on" (default) — when the ROM's LD-BYTES routine (`$0556`
//!   in the 48 BASIC ROM) is entered and the tape has a standard-speed
//!   block, the block is copied straight into memory; "off" always plays
//!   the tape. Either way entering LD-BYTES presses PLAY.
//! - `issue`: "3" (default) or "2": which board's EAR feedback port `$FE`
//!   bit 6 shows (issue 2: MIC too).
//! - `joystick`: "kempston" (default), "sinclair1", "sinclair2",
//!   "cursor", "none": the interface the default joystick is plugged into
//!   (the others stay free for more).
//! - `mouse`: "kempston" plugs a Kempston mouse in (default "none").
//! - `autostart`: "on" — once the ROM is up (frame 100), with a tape in,
//!   type LOAD "" (48K) or pick the menu's Tape Loader (128K, +2).
//! - `multiface`: "on" plugs in a Multiface 128 (ROM role "mf128", 8 KB;
//!   see [`memory`]); its red button is the panel's "MULTIFACE".
//! - `if1`: "on" plugs in an Interface 1 (ROM role "if1", 8 KB; [`if1`])
//!   with `microdrives` drives (1–8, default 2): media slots
//!   "microdrive 1" … with MDR cartridges.
//! - `roms`: "stub" runs a small built-in program ([`stub`]).
//!
//! Ports: "kempston" (port `$1F`: bits 0–4 right, left, down, up, fire,
//! active high), "sinclair 1" and "sinclair 2" (the Interface 2: keys
//! 6–0 and 1–5), "cursor" (keys 5–8 and 0), all of kind
//! [`PortKind::SpectrumJoy`], and "kempston mouse".
//!
//! Media slots: "tape" (TAP, TZX; [`drives_other::spectrum_tape`]) and
//! "snapshot" (kind Program: SNA or Z80, loaded at the next instruction,
//! [`snapshot`]).
//!
//! ROMs (user-supplied, never in the repository): role "spectrum48"
//! (16 KB), "spectrum128" and "spectrumplus2" (32 KB: the 128 editor ROM,
//! then 48 BASIC).
//!
//! **Timing.** The Z80 runs whole instructions with [`Z80::step_timed`]
//! and reports every M-cycle and every internal T-state with its address
//! ([`Z80::idle_cycles`]); the bus adds the ULA's contention there. The
//! picture is drawn lazily: before a write to the shown screen, a border
//! change, a screen switch and at the frame's end the ULA catches up to
//! that T-state. Sound and tape run in steps of two T-states (the AY's
//! clock) and catch up before every port access that hears or changes
//! them. [`Machine::step_cycle`] runs one instruction.
//!
//! Sources: Sinclair ZX Spectrum Service Manual and ZX Spectrum 128
//! Service Manual; the comp.sys.sinclair FAQ and World of Spectrum FAQ
//! (contention, 128K paging, AY ports, Kempston); Ian Logan and Frank
//! O'Hara, *The Complete Spectrum ROM Disassembly* (LD-BYTES $0556,
//! SA/LD-RET $053F). The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod gfx;
pub mod if1;
pub mod keyboard;
pub mod memory;
pub mod snapshot;
pub mod stub;

use chips_amstrad::ay::Ay;
use chips_sinclair::spectrum::{self as ula, SpectrumUla, Timing};
use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use drives_other::spectrum_tape::SpectrumTape;
use emu_core::audio::Decimator;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{
    DriveStatus, Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId,
};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use if1::Interface1;
use memory::{Memory, BANK, ROM_SIZE};
use snapshot::Snapshot;

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The eight RAM banks one after another (128 KB; the 48K uses 5, 2, 0).
pub const RAM_SPACE: MemSpaceId = MemSpaceId(1);
/// The ROMs one after another.
pub const ROM_SPACE: MemSpaceId = MemSpaceId(2);

pub const SLOT_TAPE: SlotId = SlotId(0);
pub const SLOT_SNAPSHOT: SlotId = SlotId(1);
/// Microdrive n (1–8) is slot `SLOT_MICRODRIVE + n − 1`.
pub const SLOT_MICRODRIVE: u8 = 2;
const MICRODRIVE_NAMES: [&str; 8] = [
    "microdrive 1",
    "microdrive 2",
    "microdrive 3",
    "microdrive 4",
    "microdrive 5",
    "microdrive 6",
    "microdrive 7",
    "microdrive 8",
];

pub const PORT_KEMPSTON: PortId = PortId(0);
pub const PORT_SINCLAIR1: PortId = PortId(1);
pub const PORT_SINCLAIR2: PortId = PortId(2);
pub const PORT_CURSOR: PortId = PortId(3);
pub const PORT_MOUSE: PortId = PortId(4);
const PORT_NAMES: [&str; 5] = [
    "kempston",
    "sinclair 1",
    "sinclair 2",
    "cursor",
    "kempston mouse",
];

pub const HZ_48K: u32 = 3_500_000;
pub const HZ_128K: u32 = 3_546_900;

/// ROM points of the tape trap: LD-BYTES and SA/LD-RET, and LD-BYTES'
/// first bytes (INC D; EX AF,AF'; DEC D; DI) to recognise the ROM.
pub const ROM_LD_BYTES: u16 = 0x0556;
pub const ROM_SA_LD_RET: u16 = 0x053f;
const LD_BYTES_CODE: [u8; 4] = [0x14, 0x08, 0x15, 0xf3];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "spectrum",
        name: "Sinclair ZX Spectrum",
        models: &["48k", "128k", "plus2"],
        create: |config| Ok(Box::new(Spectrum::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    S48k,
    S128k,
    Plus2,
}

impl Model {
    pub fn is_128(self) -> bool {
        self != Model::S48k
    }

    fn timing(self) -> Timing {
        if self.is_128() {
            ula::TIMING_128K
        } else {
            ula::TIMING_48K
        }
    }

    pub fn cpu_hz(self) -> u32 {
        if self.is_128() {
            HZ_128K
        } else {
            HZ_48K
        }
    }
}

/// Sound and tape, run in steps of two T-states.
#[derive(Clone)]
struct Sound {
    /// T-state up to which they ran.
    now: u64,
    audio: Decimator,
    samples: Vec<i16>,
}

impl Sound {
    /// Run up to T-state `to`.
    fn run_to(&mut self, to: u64, ula: &SpectrumUla, ay: &mut Option<Ay>, tape: &mut SpectrumTape) {
        let beeper = i32::from(ula.ear) * 6000 + i32::from(ula.mic) * 600;
        while self.now + 2 <= to {
            self.now += 2;
            tape.advance(2);
            let mut level = beeper + i32::from(tape.level()) * 1200;
            if let Some(ay) = ay {
                ay.tick();
                let [a, b, c] = ay.output();
                level += (i32::from(a) + i32::from(b) + i32::from(c)) / 2;
            }
            if let Some(s) = self.audio.push(level) {
                self.samples.push(s);
            }
        }
    }
}

/// The snapshot slot: a parsed file waits for the next instruction.
#[derive(Default)]
struct SnapSlot {
    media: Option<Media>,
    pending: Option<Snapshot>,
}

impl MediaSlot for SnapSlot {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Program {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        self.pending = Some(snapshot::parse(&bytes)?);
        self.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.pending = None;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus::default()
    }
}

type Ports = [Option<Box<dyn PortDevice>>; 5];

/// Peripherals reached through I/O ports.
struct Io<'a> {
    ay: &'a mut Option<Ay>,
    sound: &'a mut Sound,
    tape: &'a mut SpectrumTape,
    keys: &'a [u8; 8],
    ports: &'a mut Ports,
    if1: &'a mut Option<Interface1>,
}

/// The Z80's bus for one instruction.
struct SpecBus<'a> {
    mem: &'a mut Memory,
    ula: &'a mut SpectrumUla,
    io: Io<'a>,
    /// T-state the instruction started on, and the frame's start.
    start: u64,
    frame_start: u64,
    frame_len: u64,
    /// T-state of the current M-cycle's T1 after its wait states.
    at: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl SpecBus<'_> {
    #[inline]
    fn ft(&self, t: u64) -> u32 {
        ((t - self.frame_start) % self.frame_len) as u32
    }

    /// The keys the selected half-rows (address lines A8–A15 low) show,
    /// with the Sinclair and cursor joysticks.
    fn keys(&self, port: u16) -> u8 {
        let mut rows = *self.io.keys;
        for (k, dev) in self.io.ports[1..4].iter().enumerate() {
            let Some(dev) = dev else { continue };
            let low = dev.read_lines(Cycle(self.at)).pulled_low;
            let pins = [JOY_UP, JOY_DOWN, JOY_LEFT, JOY_RIGHT, JOY_FIRE];
            for (pin, key) in pins.iter().zip(keyboard::joystick_keys(k)) {
                if low & pin != 0 {
                    rows[usize::from(key / 5)] |= 1 << (key % 5);
                }
            }
        }
        let mut v = 0;
        for (row, r) in rows.iter().enumerate() {
            if port & (0x100 << row) == 0 {
                v |= r;
            }
        }
        v
    }

    fn io_read(&mut self, port: u16) -> u8 {
        let at = self.at;
        if let Some(v) = self.mem.mf_in(port) {
            return v;
        }
        if let (Some(if1), true) = (self.io.if1.as_mut(), Interface1::claims(port)) {
            return if1.read(port);
        }
        if port & 1 == 0 {
            // The EAR level as the ULA samples it.
            self.io
                .sound
                .run_to(at + 3, self.ula, self.io.ay, self.io.tape);
            let keys = self.keys(port);
            return self.ula.read_fe(keys, self.io.tape.level());
        }
        if port & 0xff == 0xdf {
            if let Some(mouse) = &self.io.ports[usize::from(PORT_MOUSE.0)] {
                let l = mouse.read_lines(Cycle(at));
                return if port & 0x100 == 0 {
                    // Buttons: bit 0 right, bit 1 left, 0 = pressed.
                    let pressed = (l.pulled_low >> 1 & 1) | (l.pulled_low & 1) << 1;
                    !(pressed as u8)
                } else if port & 0x400 == 0 {
                    l.analog[0] as u8
                } else {
                    l.analog[1] as u8
                };
            }
        }
        if self.io.ay.is_some() && port & 0xc002 == 0xc000 {
            return self.io.ay.as_ref().map_or(0xff, |ay| ay.read(0xff));
        }
        if port & 0x20 == 0 {
            if let Some(joy) = &self.io.ports[usize::from(PORT_KEMPSTON.0)] {
                let low = joy.read_lines(Cycle(at)).pulled_low;
                let mut v = 0;
                for (bit, pin) in [JOY_RIGHT, JOY_LEFT, JOY_DOWN, JOY_UP, JOY_FIRE]
                    .iter()
                    .enumerate()
                {
                    if low & pin != 0 {
                        v |= 1 << bit;
                    }
                }
                return v;
            }
        }
        let t = self.ft(at + 3);
        let v = self.ula.floating(t, self.mem.screen());
        if self.mem.banked && port & 0x8002 == 0 {
            // The 128K's paging port is write-only, but a read strobes it
            // too: the byte on the bus (the floating value) is latched
            // (Fuse Test "0x7ffd read", "0x3ffd read").
            self.ula.render_to(t, self.mem.screen());
            self.mem.write_7ffd(v);
        }
        v
    }

    fn io_write(&mut self, port: u16, v: u8) {
        let at = self.at;
        self.mem.mf_out(port);
        if let (Some(if1), true) = (self.io.if1.as_mut(), Interface1::claims(port)) {
            if1.write(port, v);
        }
        if port & 1 == 0 {
            let t = self.ft(at + 3);
            self.ula.render_to(t, self.mem.screen());
            self.io
                .sound
                .run_to(at + 3, self.ula, self.io.ay, self.io.tape);
            self.ula.write_fe(v);
        }
        if self.mem.banked && port & 0x8002 == 0 {
            let t = self.ft(at + 3);
            self.ula.render_to(t, self.mem.screen());
            self.mem.write_7ffd(v);
        }
        if self.io.ay.is_some() {
            match port & 0xc002 {
                0xc000 => {
                    if let Some(ay) = self.io.ay {
                        ay.bus_write(3, v);
                    }
                }
                0x8000 => {
                    self.io
                        .sound
                        .run_to(at + 3, self.ula, self.io.ay, self.io.tape);
                    if let Some(ay) = self.io.ay {
                        ay.bus_write(2, v);
                    }
                }
                _ => {}
            }
        }
    }
}

use devices_input::joystick::pin::{
    DOWN as JOY_DOWN, FIRE as JOY_FIRE, LEFT as JOY_LEFT, RIGHT as JOY_RIGHT, UP as JOY_UP,
};

impl Bus for SpecBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = if addr >= IO_BASE {
            u32::from(self.io_read(addr as u16))
        } else {
            u32::from(self.mem.read(addr as u16))
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
        u32::from(peek(self.mem, addr))
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
            self.io_write(addr as u16, v as u8);
            return;
        }
        let a = addr as u16;
        if self.mem.hits_screen(a) {
            let t = self.ft(self.at + 2);
            self.ula.render_to(t, self.mem.screen());
        }
        self.mem.write(a, v as u8);
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for SpecBus<'_> {
    fn wait_states(&mut self, _access: Access, _t: u32) -> u32 {
        0
    }

    #[inline]
    fn wait_states_at(&mut self, access: Access, t: u32, addr: u16) -> u32 {
        let (strobe, io) = match access {
            Access::Fetch | Access::Read | Access::Write => (1, false),
            Access::IoRead | Access::IoWrite => (2, true),
            Access::IntAck => {
                self.at = self.start + u64::from(t) - 3;
                return 0;
            }
        };
        let t1 = self.start + u64::from(t) - strobe;
        if access == Access::Fetch {
            self.mem.m1(addr);
        }
        let contended = self.mem.contended(addr);
        let w = if io {
            self.ula.io_delay(self.ft(t1), addr, contended)
        } else if contended {
            self.ula.delay(self.ft(t1))
        } else {
            0
        };
        self.at = t1 + u64::from(w);
        w
    }

    #[inline]
    fn idle(&mut self, t: u32, addr: u16) -> u32 {
        if self.mem.contended(addr) {
            self.ula.delay(self.ft(self.start + u64::from(t)))
        } else {
            0
        }
    }
}

/// Side-effect-free read in the CPU's view (ports read `$FF`).
fn peek(mem: &Memory, addr: u32) -> u8 {
    if addr >= IO_BASE {
        0xff
    } else {
        mem.read(addr as u16)
    }
}

struct PeekBus<'a>(&'a Memory);

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

pub struct Spectrum {
    model: Model,
    cpu: Z80,
    mem: Memory,
    ula: SpectrumUla,
    ay: Option<Ay>,
    /// Keyboard half-rows, bits 0–4 (1 = pressed).
    keys: [u8; 8],
    sched: Scheduler,
    input: InputQueue,
    /// T-state the current frame started on.
    frame_start: u64,
    sound: Sound,
    tape: SpectrumTape,
    traps: bool,
    ports: Ports,
    snap: SnapSlot,
    if1: Option<Interface1>,
    field_done_pending: bool,
    /// `autostart=on` and not yet done.
    autostart: bool,
}

/// Frame by which the ROMs are ready for input.
const AUTOSTART_FRAME: u64 = 100;

fn option<'a>(config: &'a MachineConfig, name: &str) -> Option<&'a str> {
    config.options.get(name).map(String::as_str)
}

impl Spectrum {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("48k") | Some("48") => Model::S48k,
            Some("128k") | Some("128") => Model::S128k,
            Some("plus2") | Some("+2") => Model::Plus2,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let invalid = |name: &str, v: &str| ConfigError::Invalid(format!("{name} option '{v}'"));
        let traps = match option(config, "traps") {
            None | Some("on") => true,
            Some("off") => false,
            Some(v) => return Err(invalid("traps", v)),
        };
        let issue3 = match option(config, "issue") {
            None | Some("3") => true,
            Some("2") => false,
            Some(v) => return Err(invalid("issue", v)),
        };
        let joy = match option(config, "joystick") {
            None | Some("kempston") => Some(PORT_KEMPSTON),
            Some("sinclair1") => Some(PORT_SINCLAIR1),
            Some("sinclair2") => Some(PORT_SINCLAIR2),
            Some("cursor") => Some(PORT_CURSOR),
            Some("none") => None,
            Some(v) => return Err(invalid("joystick", v)),
        };
        let mouse = match option(config, "mouse") {
            None | Some("none") => false,
            Some("kempston") => true,
            Some(v) => return Err(invalid("mouse", v)),
        };
        let autostart = match option(config, "autostart") {
            None | Some("off") => false,
            Some("on") => true,
            Some(v) => return Err(invalid("autostart", v)),
        };
        let multiface = match option(config, "multiface") {
            None | Some("off") => None,
            Some("on") => {
                let r = config
                    .roms
                    .get("mf128")
                    .ok_or_else(|| ConfigError::MissingRom("mf128".into()))?;
                if r.len() != 0x2000 {
                    return Err(ConfigError::Invalid(format!(
                        "mf128 ROM must be 8192 bytes, got {}",
                        r.len()
                    )));
                }
                Some(r.clone())
            }
            Some(v) => return Err(invalid("multiface", v)),
        };
        let if1_rom = match option(config, "if1") {
            None | Some("off") => None,
            Some("on") => {
                let r = config
                    .roms
                    .get("if1")
                    .ok_or_else(|| ConfigError::MissingRom("if1".into()))?;
                if r.len() != 0x2000 {
                    return Err(ConfigError::Invalid(format!(
                        "if1 ROM must be 8192 bytes, got {}",
                        r.len()
                    )));
                }
                Some(r.clone())
            }
            Some(v) => return Err(invalid("if1", v)),
        };
        let microdrives = match option(config, "microdrives") {
            None => 2,
            Some(v) => match v.parse::<usize>() {
                Ok(n @ 1..=8) => n,
                _ => return Err(invalid("microdrives", v)),
            },
        };
        let if1 = if1_rom.is_some().then(|| Interface1::new(microdrives));
        let count = if model.is_128() { 2 } else { 1 };
        let roms = match option(config, "roms") {
            Some("stub") => stub::roms(count),
            Some(v) => return Err(invalid("roms", v)),
            None => {
                let role = match model {
                    Model::S48k => "spectrum48",
                    Model::S128k => "spectrum128",
                    Model::Plus2 => "spectrumplus2",
                };
                let r = config
                    .roms
                    .get(role)
                    .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
                if r.len() != count * ROM_SIZE {
                    return Err(ConfigError::Invalid(format!(
                        "{role} ROM must be {} bytes, got {}",
                        count * ROM_SIZE,
                        r.len()
                    )));
                }
                r.chunks(ROM_SIZE).map(<[u8]>::to_vec).collect()
            }
        };
        let hz = model.cpu_hz();
        let mut ula = SpectrumUla::new(model.timing());
        ula.issue3 = issue3;
        let mut ports: Ports = Default::default();
        if let Some(p) = joy {
            ports[usize::from(p.0)] = Some(Box::new(devices_input::Joystick::new(
                PortKind::SpectrumJoy,
            )));
        }
        if mouse {
            ports[usize::from(PORT_MOUSE.0)] = Some(Box::new(devices_input::KempstonMouse::new()));
        }
        let mut cpu = Z80::new();
        cpu.idle_cycles = true;
        cpu.set_int_vector(0xff);
        let mut mem = Memory::new(roms, model.is_128());
        mem.mf_rom = multiface;
        mem.if1_rom = if1_rom;
        Ok(Self {
            model,
            cpu,
            mem,
            ula,
            ay: model.is_128().then(Ay::new),
            keys: [0; 8],
            sched: Scheduler::new(),
            input: InputQueue::new(),
            frame_start: 0,
            sound: Sound {
                now: 0,
                audio: Decimator::new(hz / 2, emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                samples: Vec::new(),
            },
            tape: SpectrumTape::new(hz, !model.is_128()),
            traps,
            ports,
            snap: SnapSlot::default(),
            if1,
            field_done_pending: false,
            autostart,
        })
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn frame_len(&self) -> u64 {
        u64::from(self.ula.timing.frame_len())
    }

    /// T-state within the frame.
    fn frame_t(&self) -> u32 {
        (self.sched.now().0 - self.frame_start) as u32
    }

    fn beam_pos(&self) -> BeamPos {
        let (line, cycle) = self.ula.beam(self.frame_t());
        BeamPos {
            frame: self.ula.frames,
            line: line as u16,
            cycle: cycle as u16,
            hpos: self.ula.picture_column(cycle).unwrap_or(0) as u16,
        }
    }

    /// The ROM's LD-BYTES: copy the tape's next block (traps on) or press
    /// PLAY.
    fn trap(&mut self) {
        if self.cpu.pc != ROM_LD_BYTES || !self.mem.basic_rom() {
            return;
        }
        if (0..4).any(|i| self.mem.read(ROM_LD_BYTES + i) != LD_BYTES_CODE[usize::from(i)]) {
            return;
        }
        if self.traps {
            if let Some(block) = self.tape.take_block() {
                self.fast_load(&block);
                return;
            }
        }
        self.tape.play();
    }

    /// LD-BYTES on one block: A the flag expected, IX the address, DE the
    /// length, carry set to load (clear to verify). Returns through
    /// SA/LD-RET with carry set on success, as the ROM does.
    fn fast_load(&mut self, block: &[u8]) {
        const C: u8 = 0x01;
        let verify = self.cpu.f & C == 0;
        let (mut ix, mut de) = (self.cpu.ix, self.cpu.de());
        let mut ok = false;
        if block.len() >= 2 && block[0] == self.cpu.a {
            let mut parity = block[0];
            let mut matched = true;
            let mut i = 1;
            while de > 0 && i < block.len() {
                let b = block[i];
                parity ^= b;
                if verify {
                    matched &= self.mem.read(ix) == b;
                } else {
                    self.mem.write(ix, b);
                }
                ix = ix.wrapping_add(1);
                de -= 1;
                i += 1;
            }
            // The checksum byte follows the data.
            if de == 0 && i < block.len() {
                parity ^= block[i];
                ok = parity == 0 && matched;
            }
        }
        self.cpu.ix = ix;
        self.cpu.set_de(de);
        self.cpu.f = if ok { self.cpu.f | C } else { self.cpu.f & !C };
        self.cpu.pc = ROM_SA_LD_RET;
    }

    fn apply_snapshot(&mut self, s: Snapshot) {
        if s.paging.is_some() && !self.model.is_128() {
            // A 128K snapshot cannot run on the 48K; ignore it.
            return;
        }
        let mut cpu = s.cpu;
        cpu.idle_cycles = true;
        cpu.set_int_vector(0xff);
        self.cpu = cpu;
        self.mem.reset();
        for (bank, data) in &s.banks {
            self.mem.bank_mut(*bank).copy_from_slice(data);
        }
        self.mem.paging = match (s.paging, self.model.is_128()) {
            (Some(p), _) => p,
            // A 48K program on a 128K: 48 BASIC, paging locked.
            (None, true) => 0x30,
            (None, false) => 0,
        };
        self.ula.write_fe(s.border);
        if let (Some(ay), Some((sel, regs))) = (&mut self.ay, s.ay) {
            for (r, v) in regs.iter().enumerate() {
                ay.bus_write(3, r as u8);
                ay.bus_write(2, *v);
            }
            ay.bus_write(3, sel);
        }
        // Continue just after the frame interrupt.
        let now = self.sched.now().0;
        self.frame_start = now.saturating_sub(u64::from(self.ula.timing.int_len));
        self.sound.now = now;
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        self.trap();
        let start = self.sched.now().0;
        let beam = if dbg.is_some() {
            self.beam_pos()
        } else {
            BeamPos::default()
        };
        let frame_len = self.frame_len();
        let t = {
            let mut bus = SpecBus {
                mem: &mut self.mem,
                ula: &mut self.ula,
                io: Io {
                    ay: &mut self.ay,
                    sound: &mut self.sound,
                    tape: &mut self.tape,
                    keys: &self.keys,
                    ports: &mut self.ports,
                    if1: &mut self.if1,
                },
                start,
                frame_start: self.frame_start,
                frame_len,
                at: start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step_timed(&mut bus)
        };
        let end = start + u64::from(t);
        self.sched.advance(u64::from(t));
        let mut field = false;
        if end - self.frame_start >= frame_len {
            let frame_end = self.frame_start + frame_len;
            self.sound
                .run_to(frame_end, &self.ula, &mut self.ay, &mut self.tape);
            self.ula.end_frame(self.mem.screen());
            self.frame_start = frame_end;
            field = true;
        }
        self.cpu.set_int(self.ula.int_active(self.frame_t()));
        let entered = self.cpu.take_interrupt();
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
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.key(machine_key.0, down);
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) if id.0 == 0 => {
                Machine::reset(self, ResetKind::Soft);
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 })
                if id.0 == 1 && self.mem.mf_rom.is_some() =>
            {
                // The red button: /NMI; the Multiface pages in at the
                // acknowledge's fetch from $0066.
                self.mem.mf_armed = true;
                self.cpu.set_nmi(true);
                self.cpu.set_nmi(false);
            }
            (InputTarget::Port(p), ev) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&ev);
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, code: u16, down: bool) {
        let (row, col) = (usize::from(code / 5), code % 5);
        if let (Some(r), true) = (self.keys.get_mut(row), col < 5) {
            if down {
                *r |= 1 << col;
            } else {
                *r &= !(1 << col);
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        let t = self.ula.timing;
        SignalInfo {
            pixel_clock_hz: self.model.cpu_hz() * ula::PIXELS_PER_T,
            active_pixels: ula::WIDTH,
            total_pixels: t.t_per_line * ula::PIXELS_PER_T,
            lines_per_field: t.lines,
            standard: VideoStandard::Pal,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }

    /// `autostart=on`: start the tape's program once the ROM is up.
    fn autostart(&mut self) {
        if !self.autostart || self.ula.frames < AUTOSTART_FRAME || !self.tape.has_tape() {
            return;
        }
        self.autostart = false;
        let text = if self.model.is_128() {
            "
"
        } else {
            "j\"\"
"
        };
        type_text(self, text);
    }

    fn take_snapshot(&mut self) {
        if let Some(s) = self.snap.pending.take() {
            self.apply_snapshot(s);
        }
    }
}

/// Typing for tests and tools: queue key presses for `text`.
pub fn type_text(m: &mut Spectrum, text: &str) {
    let hz = u64::from(m.model.cpu_hz());
    let (events, _) = keyboard::typed_events(text, m.sched.now().0, hz);
    for e in events {
        m.input.push(e);
    }
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Spectrum, addr: u16) -> u8 {
    m.mem.read(addr)
}

impl Machine for Spectrum {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        self.take_snapshot();
        self.autostart();
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        if self.frame_t() == 0 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE && f.addr < IO_BASE {
                    for i in 0..f.size.bytes() {
                        self.mem
                            .write((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
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
        self.take_snapshot();
        self.instruction(None);
        StopReason::StepDone
    }

    /// Soft: the 128K's reset button (RAM kept); hard: power off and on.
    fn reset(&mut self, kind: ResetKind) {
        self.cpu.reset();
        if kind == ResetKind::Hard {
            self.cpu = Z80::new();
            self.cpu.idle_cycles = true;
            self.cpu.set_int_vector(0xff);
            self.mem.clear();
        } else {
            self.mem.reset();
        }
        self.ula.reset();
        if let Some(i) = &mut self.if1 {
            i.reset();
        }
        if let Some(ay) = &mut self.ay {
            ay.reset();
        }
        self.keys = [0; 8];
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: ula::WIDTH,
            height: ula::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.ula.pixels(),
                palette: &ula::PALETTE,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn gfx_views(&self) -> Vec<emu_core::GfxViewInfo> {
        self.views()
    }

    fn gfx_view(&self, id: &str, opts: &emu_core::GfxOptions) -> Option<emu_core::GfxView> {
        self.view(id, opts)
    }

    fn frame_row(&self, line: u16) -> Option<u32> {
        self.ula.picture_row(u32::from(line))
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.sound.samples);
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
        vec![
            space(CPU_SPACE, "cpu", 17),
            space(RAM_SPACE, "ram", 17),
            space(ROM_SPACE, "rom", 15),
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let (line, cycle) = self.ula.beam(self.frame_t());
        let u = &self.ula;
        let mut chips = vec![ChipInfo {
            name: "ula",
            regs: vec![
                reg("LINE", line, 16),
                reg("T", cycle, 8),
                reg("FRAME_T", self.frame_t(), 17),
                reg("BORDER", u32::from(u.border), 3),
                reg("MIC", u32::from(u.mic), 1),
                reg("EAR", u32::from(u.ear), 1),
                reg("FLASH", u32::from(u.frames & 16 != 0), 1),
            ],
        }];
        if self.model.is_128() {
            chips.push(ChipInfo {
                name: "paging",
                regs: vec![
                    reg("7FFD", u32::from(self.mem.paging), 6),
                    reg("BANK", u32::from(self.mem.paging & 7), 3),
                    reg("SCREEN", self.mem.screen_bank() as u32, 3),
                    reg("ROM", self.mem.rom_index() as u32, 1),
                    reg("LOCK", u32::from(self.mem.paging >> 5 & 1), 1),
                ],
            });
        }
        if let Some(ay) = &self.ay {
            const NAMES: [&str; 16] = [
                "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12",
                "R13", "R14", "R15",
            ];
            let mut regs: Vec<ChipReg> = NAMES
                .iter()
                .zip(ay.regs)
                .map(|(n, v)| reg(n, u32::from(v), 8))
                .collect();
            regs.push(reg("SELECT", u32::from(ay.addr), 8));
            chips.push(ChipInfo { name: "ay", regs });
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
        let mut c = vec![emu_core::ControlInfo::button(0, "RESET", false)];
        if self.mem.mf_rom.is_some() {
            c.push(emu_core::ControlInfo::button(1, "MULTIFACE", false));
        }
        c
    }

    fn ports(&self) -> Vec<PortInfo> {
        (0..5)
            .map(|i| PortInfo {
                id: PortId(i as u8),
                kind: if i == 4 {
                    PortKind::KempstonMouse
                } else {
                    PortKind::SpectrumJoy
                },
                name: PORT_NAMES[i],
                connected: self.ports[i].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let i = usize::from(port.0);
        let kind = match i {
            0..=3 => PortKind::SpectrumJoy,
            4 => PortKind::KempstonMouse,
            _ => return Err(PortError::NoSuchPort(port)),
        };
        if dev.kind() != kind {
            return Err(PortError::Incompatible {
                port: kind,
                device: dev.kind(),
            });
        }
        if self.ports[i].is_some() {
            return Err(PortError::Occupied(port));
        }
        self.ports[i] = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        self.ports.get_mut(usize::from(port.0))?.take()
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let drives = self.if1.iter().flat_map(|i| {
            i.drives.iter().enumerate().map(|(n, d)| MediaSlotInfo {
                id: SlotId(SLOT_MICRODRIVE + n as u8),
                name: MICRODRIVE_NAMES[n],
                kind: MediaKind::Disk,
                inserted: d.has_cartridge(),
            })
        });
        let mut slots = vec![
            MediaSlotInfo {
                id: SLOT_TAPE,
                name: "tape",
                kind: MediaKind::Tape,
                inserted: self.tape.has_tape(),
            },
            MediaSlotInfo {
                id: SLOT_SNAPSHOT,
                name: "snapshot",
                kind: MediaKind::Program,
                inserted: self.snap.media.is_some(),
            },
        ];
        slots.extend(drives);
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_TAPE => Some(&mut self.tape as &mut dyn MediaSlot),
            SLOT_SNAPSHOT => Some(&mut self.snap as &mut dyn MediaSlot),
            SlotId(n) if n >= SLOT_MICRODRIVE => self
                .if1
                .as_mut()?
                .drives
                .get_mut(usize::from(n - SLOT_MICRODRIVE))
                .map(|d| d as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"ZXSP", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.field_done_pending);
            w.raw(&self.keys);
            w.u64(self.frame_start);
            w.u64(self.sound.now);
            self.sound.audio.save(w);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.ula.save(&mut w);
        if let Some(ay) = &self.ay {
            ay.save(&mut w);
        }
        self.sched.save(&mut w);
        self.input.save(&mut w);
        self.tape.save(&mut w);
        w.chunk(*b"PORT", 1, |w| {
            for p in &self.ports {
                w.bytes(&p.as_ref().map(|d| d.save_state()).unwrap_or_default());
            }
        });
        if let Some(i) = &self.if1 {
            i.save(&mut w);
        }
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"ZXSP", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("Spectrum model differs"));
        }
        let field_done_pending = c.bool()?;
        let mut keys = [0u8; 8];
        c.raw_into(&mut keys)?;
        let frame_start = c.u64()?;
        let sound_now = c.u64()?;
        let mut audio = self.sound.audio;
        audio.load(&mut c)?;
        let mut cpu = self.cpu.clone();
        let mut mem = self.mem.clone();
        let mut ula = self.ula.clone();
        let mut ay = self.ay.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        cpu.idle_cycles = true;
        mem.load(&mut r)?;
        ula.load(&mut r)?;
        if let Some(ay) = &mut ay {
            ay.load(&mut r)?;
        }
        sched.load(&mut r)?;
        input.load(&mut r)?;
        if frame_start > sched.now().0
            || sched.now().0 - frame_start > u64::from(ula.timing.frame_len())
        {
            return Err(StateError::Invalid("frame position"));
        }
        let tape_at = s.len() - r.remaining();
        SpectrumTape::new(1, true).load(&mut r)?;
        let (_, mut c) = r.chunk(*b"PORT", 1)?;
        let mut states = Vec::new();
        for _ in 0..5 {
            states.push(c.bytes()?.to_vec());
        }
        // The Interface 1 holds media: check its chunk now, load it last.
        let if1_at = s.len() - r.remaining();
        if let Some(i) = &self.if1 {
            Interface1::new(i.drives.len()).load(&mut r)?;
        }
        let backups: Vec<Option<Vec<u8>>> = self
            .ports
            .iter()
            .map(|d| d.as_ref().map(|d| d.save_state()))
            .collect();
        for (i, state) in states.iter().enumerate() {
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
        self.tape.load(&mut StateReader::new(&s[tape_at..]))?;
        if let Some(i) = &mut self.if1 {
            i.load(&mut StateReader::new(&s[if1_at..]))?;
        }
        self.cpu = cpu;
        self.mem = mem;
        self.ula = ula;
        self.ay = ay;
        self.sched = sched;
        self.input = input;
        self.keys = keys;
        self.frame_start = frame_start;
        self.sound.now = sound_now;
        self.sound.audio = audio;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for Spectrum {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u8> {
            match space {
                CPU_SPACE => Some(peek(&self.mem, a)),
                RAM_SPACE => self.mem.ram.get(a as usize).copied(),
                ROM_SPACE => {
                    let i = a as usize;
                    self.mem.roms.get(i / ROM_SIZE).map(|r| r[i % ROM_SIZE])
                }
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0, |v, i| {
            Some(v | u32::from(byte(addr.wrapping_add(i))?) << (8 * i))
        })
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let b = (v >> (8 * i)) as u8;
            let ok = match space {
                CPU_SPACE if a < IO_BASE => self.mem.write(a as u16, b),
                RAM_SPACE if (a as usize) < 8 * BANK => {
                    self.mem.ram[a as usize] = b;
                    true
                }
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.mem), addr)
    }
}

#[cfg(test)]
mod tests;
