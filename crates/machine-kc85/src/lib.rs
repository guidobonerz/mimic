//! Robotron KC85/2, KC85/3, KC85/4 and KC85/5 (VEB Mikroelektronik
//! "Wilhelm Pieck" Mühlhausen; the KC85/5 is the KC-Club's extension of
//! the KC85/4).
//!
//! A U880 (Z80) at 1.75 MHz (KC85/4, /5: 1.7734475 MHz), a U857 (CTC) and
//! a U855 (PIO) in interrupt mode 2 ([`chips_zilog`]), the IRM video RAM
//! with 320 × 256 pixels ([`video`]), RAM and ROM switched by the PIO and
//! two output ports ([`memory`]), two module slots ([`modules`]); the
//! floppy unit D004 on top ([`d004`]).
//!
//! I/O ports (the lower address byte; the upper one is the slot for port
//! `$80`):
//!
//! | Port | |
//! | --- | --- |
//! | `$80` | module control ([`modules`]) |
//! | `$84` | KC85/4: IRM plane and image, colour resolution, RAM8 block |
//! | `$86` | KC85/4: RAM4, CAOS ROM C, USER ROM segment |
//! | `$88`, `$89` | PIO data A, B |
//! | `$8A`, `$8B` | PIO control A, B |
//! | `$8C–$8F` | CTC channels 0–3 |
//! | `$F0–$F4` | D004: Koppel-RAM, control ([`d004`]) |
//!
//! PIO A: bit 0 CAOS ROM E, 1 RAM0, 2 IRM, 3 RAM0 writable, 5 tape LED, 6
//! tape motor, 7 BASIC ROM. PIO B: bits 4–1 the volume (inverted: all
//! ones is silence), 7 blink allowed; KC85/4: bit 5 RAM8, bit 6 RAM8
//! writable. PIO A's strobe is the cassette input ([`tape`]: a strobe
//! per zero crossing), PIO B's the keyboard's pulses ([`keyboard`]).
//!
//! CTC: the outputs of channels 0 and 1 each toggle a flip-flop: the two
//! sound channels (CAOS writes the cassette signal with channel 1);
//! channel 2's toggles the blink flip-flop (CAOS also times the tone's
//! length and the tape's half waves with it). Channel 3 times the
//! keyboard's pulses. Channels 0 and 1 count picture lines at their
//! CLK/TRG inputs, channels 2 and 3 fields (50 Hz). Interrupt priority:
//! CTC, then PIO.
//!
//! Media slot "tape" (kind Tape): a KC-TAP or KCC image; LOAD starts the
//! motor. Saving to tape is not built yet. With the D004: "drive 0" and
//! "drive 1" (kind Disk): DSK images or raw MicroDOS dumps (`.img`).
//!
//! Power-on starts the CPU at `$F000`, the RESET button at `$E000` (the
//! hardware forces these addresses; RAM survives a RESET).
//!
//! Models: "4" (KC85/4, default), "2", "3", "5". Options:
//! - `slot8`, `slotc`: a module in the base unit's slots: "none"
//!   (default) or one of [`modules::KINDS`] ("m022", "m011", "m006",
//!   "m012", "m026", "m027", "m033"); ROM modules need their image as ROM
//!   role of the same name.
//! - `d004`: "on" puts the floppy unit D004 on top (ROM role "d004", the
//!   8 KB ROM 2.0, KCemu's `floppy20.rom`); `JUMP FC` starts it. Default
//!   "off".
//! - `roms`: "stub" runs a small built-in program ([`stub`]).
//!
//! ROMs user-supplied, never in the repository: CAOS per model (roles
//! "caos2", "caos3", "caos4e" + "caos4c", "caos5e" + "caos5c"), BASIC
//! ("basic", 8 KB, KC85/3 and /4; "basic5", 32 KB USER ROM of the
//! KC85/5).
//!
//! Sources: KC85/3 Systemhandbuch, KC85/4 Systemhandbuch (VEB
//! Mikroelektronik Mühlhausen: ports, PIO and CTC wiring, memory, IRM);
//! Zilog's CTC and PIO documentation (see [`chips_zilog`]). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

pub mod d004;
pub mod keyboard;
pub mod memory;
pub mod modules;
pub mod stub;
pub mod tape;
pub mod video;

use chips_zilog::{pio, Ctc, Pio};
use cpu_z80::{IO_BASE, Z80};
use d004::D004;
use memory::{Memory, Roms};
use mimic_core::audio::Decimator;
use mimic_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use mimic_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use mimic_core::machine::ChipReg;
use mimic_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use mimic_core::port::{PortDevice, PortError, PortId, PortInfo};
use mimic_core::registry::{ConfigError, MachineConfig, MachineEntry};
use mimic_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, InterruptKind,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

pub const SLOT_TAPE: SlotId = SlotId(0);
/// The D004's drives.
pub const SLOT_DRIVE0: SlotId = SlotId(1);
pub const SLOT_DRIVE1: SlotId = SlotId(2);

/// KC85/2, /3: CPU clock.
pub const CPU_HZ_3: u32 = 1_750_000;
/// KC85/4, /5: 17.734475 MHz / 10.
pub const CPU_HZ_4: u32 = 1_773_447;

/// Panel controls.
pub const CONTROL_RESET: u16 = 0;
pub const CONTROL_POWER: u16 = 1;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "kc85",
        name: "Robotron KC85",
        models: &["4", "2", "3", "5"],
        video_formats: &[],
        create: |config| Ok(Box::new(Kc85::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Model {
    Kc852 = 2,
    Kc853 = 3,
    Kc854 = 4,
    Kc855 = 5,
}

impl Model {
    /// A KC85/4 or /5: planes in the IRM, ports `$84` and `$86`.
    pub fn four(self) -> bool {
        self >= Model::Kc854
    }

    pub fn cpu_hz(self) -> u32 {
        if self.four() {
            CPU_HZ_4
        } else {
            CPU_HZ_3
        }
    }

    /// Half CPU clocks per picture line (112 and 113.5 clocks).
    fn line_half_t(self) -> u64 {
        if self.four() {
            227
        } else {
            224
        }
    }
}

/// The base unit's I/O: what a port reads. No side effects.
fn port_peek(mem: &Memory, pio: &Pio, ctc: &Ctc, d004: Option<&D004>, port: u16) -> u8 {
    match port as u8 {
        0x80 => mem.modules.id((port >> 8) as u8),
        0x88 => pio.read_data(pio::A),
        0x89 => pio.read_data(pio::B),
        p @ 0x8c..=0x8f => ctc.read(usize::from(p & 3)),
        0xf0..=0xf3 if mem.modules.d004_connected() => {
            d004.and_then(|d| d.kc_read(port)).unwrap_or(0xff)
        }
        _ => 0xff,
    }
}

fn peek_at(mem: &Memory, pio: &Pio, ctc: &Ctc, d004: Option<&D004>, addr: u32) -> u8 {
    if addr >= IO_BASE {
        port_peek(mem, pio, ctc, d004, addr as u16)
    } else {
        mem.read(addr as u16)
    }
}

/// The CPU's bus for one instruction.
struct KcBus<'a> {
    four: bool,
    mem: &'a mut Memory,
    pio: &'a mut Pio,
    ctc: &'a mut Ctc,
    d004: Option<&'a mut D004>,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus for KcBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = u32::from(peek_at(
            self.mem,
            self.pio,
            self.ctc,
            self.d004.as_deref(),
            addr,
        ));
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
        u32::from(peek_at(
            self.mem,
            self.pio,
            self.ctc,
            self.d004.as_deref(),
            addr,
        ))
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
        let v = v as u8;
        if addr < IO_BASE {
            self.mem.write(addr as u16, v);
            return;
        }
        match addr as u8 {
            0x80 => self.mem.modules.control((addr >> 8) as u8, v),
            0x84 | 0x85 if self.four => self.mem.port84 = v,
            0x86 | 0x87 if self.four => self.mem.port86 = v,
            0x88 => self.pio.write_data(pio::A, v),
            0x89 => self.pio.write_data(pio::B, v),
            0x8a => self.pio.write_control(pio::A, v),
            0x8b => self.pio.write_control(pio::B, v),
            p @ 0x8c..=0x8f => self.ctc.write(usize::from(p & 3), v),
            0xf0..=0xf4 if self.mem.modules.d004_connected() => {
                if let Some(d) = self.d004.as_deref_mut() {
                    d.kc_write(addr as u16, v);
                }
            }
            _ => {}
        }
        // The PIO's lines switch the memory.
        self.mem.pio_a = self.pio.pins(pio::A, 0xff);
        self.mem.pio_b = self.pio.pins(pio::B, 0xff);
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

struct PeekBus<'a>(&'a Memory, &'a Pio, &'a Ctc, Option<&'a D004>);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_at(self.0, self.1, self.2, self.3, addr))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

/// The two sound flip-flops and their way to the loudspeaker.
#[derive(Clone)]
struct Sound {
    /// The flip-flops behind CTC channels 0 and 1.
    level: [bool; 2],
    /// CPU clocks into the current sound step.
    phase: u32,
    audio: Decimator,
    samples: Vec<i16>,
}

/// CPU clocks per sound step.
const SOUND_STEP: u32 = 8;

impl Sound {
    fn new(cpu_hz: u32) -> Self {
        Self {
            level: [false; 2],
            phase: 0,
            audio: Decimator::new(cpu_hz / SOUND_STEP, mimic_core::SAMPLE_RATE)
                .expect("the sound clock is above the sample rate"),
            samples: Vec::new(),
        }
    }

    /// Run `t` clocks; `toggles` are the CTC outputs in this span
    /// (channel, clocks into it), sorted by time; `sounding`: the
    /// channel's counter runs (a stopped one is silent, wherever its
    /// flip-flop came to rest). `volume`: 0 to 15.
    fn run(&mut self, t: u32, toggles: &[(u8, u32)], sounding: [bool; 2], volume: i32) {
        let mut next = toggles.iter().peekable();
        for at in 1..=t {
            while let Some(&&(ch, _)) = next.peek().filter(|e| e.1 <= at) {
                self.level[usize::from(ch)] ^= true;
                next.next();
            }
            self.phase += 1;
            if self.phase == SOUND_STEP {
                self.phase = 0;
                let level: i32 = (0..2)
                    .filter(|&ch| sounding[ch])
                    .map(|ch| if self.level[ch] { 1 } else { -1 })
                    .sum();
                if let Some(s) = self.audio.push(level * 400 * volume) {
                    self.samples.push(s);
                }
            }
        }
    }
}

pub struct Kc85 {
    model: Model,
    cpu: Z80,
    mem: Memory,
    pio: Pio,
    ctc: Ctc,
    /// The blink flip-flop behind CTC channel 2.
    blink: bool,
    sound: Sound,
    /// CPU clocks since power-on.
    sched: Scheduler,
    input: InputQueue,
    keyboard: keyboard::Keyboard,
    tape: tape::Tape,
    /// The picture line being drawn (0–311) and the half clock it ends at.
    line: u32,
    line_end: u64,
    fields: u64,
    pixels: Vec<u8>,
    field_done_pending: bool,
    /// CTC outputs of the current instruction (scratch).
    toggles: Vec<(u8, u32)>,
    /// The floppy unit on top.
    d004: Option<Box<D004>>,
}

impl Kc85 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("4") => Model::Kc854,
            Some("2") => Model::Kc852,
            Some("3") => Model::Kc853,
            Some("5") => Model::Kc855,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let opt = |k: &str| config.options.get(k).map(String::as_str);
        let rom = |role: &str, size: usize| -> Result<Vec<u8>, ConfigError> {
            let r = config
                .roms
                .get(role)
                .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
            if r.len() != size {
                return Err(ConfigError::Invalid(format!(
                    "{role} ROM must be {size} bytes, got {}",
                    r.len()
                )));
            }
            Ok(r.clone())
        };
        let roms = match opt("roms") {
            Some("stub") => Roms {
                caos_e: stub::rom(model),
                ..Roms::default()
            },
            Some(o) => return Err(ConfigError::Invalid(format!("roms option '{o}'"))),
            None => match model {
                Model::Kc852 => Roms {
                    caos_e: rom("caos2", 0x2000)?,
                    ..Roms::default()
                },
                Model::Kc853 => Roms {
                    caos_e: rom("caos3", 0x2000)?,
                    basic: rom("basic", 0x2000)?,
                    ..Roms::default()
                },
                Model::Kc854 => Roms {
                    caos_e: rom("caos4e", 0x2000)?,
                    caos_c: rom("caos4c", 0x1000)?,
                    basic: rom("basic", 0x2000)?,
                },
                Model::Kc855 => Roms {
                    caos_e: rom("caos5e", 0x2000)?,
                    caos_c: rom("caos5c", 0x2000)?,
                    basic: rom("basic5", 0x8000)?,
                },
            },
        };
        let mut mem = Memory::new(model, roms);
        for (i, key) in ["slot8", "slotc"].into_iter().enumerate() {
            let name = match opt(key) {
                None | Some("none") => continue,
                Some(n) => n,
            };
            let kind = modules::kind(name).ok_or_else(|| {
                let known: Vec<_> = modules::KINDS.iter().map(|k| k.name).collect();
                ConfigError::Invalid(format!(
                    "{key} option '{name}' (none, {})",
                    known.join(", ")
                ))
            })?;
            let image = config.roms.get(name).map(Vec::as_slice);
            let module = modules::Module::new(kind, image).map_err(|size| match image {
                None => ConfigError::MissingRom(name.into()),
                Some(r) => ConfigError::Invalid(format!(
                    "{name} ROM must be {size} bytes, got {}",
                    r.len()
                )),
            })?;
            mem.modules.slot[i] = Some(module);
        }
        let d004 = match opt("d004") {
            None | Some("off") => None,
            Some("on") => {
                let image = rom("d004", 0x2000)?;
                mem.modules.slot[modules::SLOT_D004] = Some(
                    modules::Module::new(modules::D004, Some(&image))
                        .expect("the size was checked"),
                );
                Some(Box::new(D004::new()))
            }
            Some(o) => return Err(ConfigError::Invalid(format!("d004 option '{o}' (on, off)"))),
        };
        let mut m = Self {
            model,
            cpu: Z80::new(),
            mem,
            pio: Pio::new(),
            ctc: Ctc::new(),
            blink: false,
            sound: Sound::new(model.cpu_hz()),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            keyboard: keyboard::Keyboard::new(model.cpu_hz()),
            tape: tape::Tape::default(),
            line: 0,
            line_end: model.line_half_t(),
            fields: 0,
            pixels: vec![0; (video::WIDTH * video::HEIGHT) as usize],
            field_done_pending: false,
            toggles: Vec::new(),
            d004,
        };
        m.cpu.pc = 0xf000;
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let len = self.model.line_half_t();
        let into = (self.sched.now().0 * 2 + len).saturating_sub(self.line_end) / 2;
        BeamPos {
            frame: self.fields,
            line: self.line as u16,
            cycle: into as u16,
            hpos: (into * 4) as u16,
        }
    }

    /// /INT and the vector of the device that would be acknowledged.
    fn interrupt(&self) -> Option<u8> {
        self.ctc
            .vector()
            .or_else(|| self.ctc.chain.ieo().then(|| self.pio.vector()).flatten())
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        // Every pulse of the keyboard is a strobe at PIO port B.
        if self.keyboard.run(self.sched.now().0) {
            self.pio.strobe(pio::B);
        }
        let beam = self.beam_pos();
        let vector = self.interrupt();
        self.cpu.set_int(vector.is_some());
        self.cpu.set_int_vector(vector.unwrap_or(0xff));
        // The devices of the interrupt chain watch the bus for RETI.
        let reti = !self.cpu.halted
            && self.mem.read(self.cpu.pc) == 0xed
            && self.mem.read(self.cpu.pc.wrapping_add(1)) == 0x4d;
        let t = {
            let mut bus = KcBus {
                four: self.model.four(),
                mem: &mut self.mem,
                pio: &mut self.pio,
                ctc: &mut self.ctc,
                d004: self.d004.as_deref_mut(),
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step(&mut bus)
        };
        // The floppy unit runs the same time.
        if let Some(d) = &mut self.d004 {
            d.run(t, self.model.cpu_hz(), dbg.as_deref_mut());
        }
        let entered = self.cpu.take_interrupt();
        // The acknowledge and the RETI go to the first device of the
        // chain they belong to.
        if entered == Some(InterruptKind::Irq) {
            if self.ctc.chain.acknowledge().is_none() {
                self.pio.chain.acknowledge();
            }
        } else if entered.is_none() && reti && !self.ctc.chain.reti() {
            self.pio.chain.reti();
        }
        self.sched.advance(u64::from(t));
        // Every zero crossing on the tape is a strobe at PIO port A.
        let motor = self.mem.pio_a & memory::A_TAPE_MOTOR != 0;
        if self.tape.run(u64::from(t), motor) > 0 {
            self.pio.strobe(pio::A);
        }
        self.toggles.clear();
        let (toggles, blink) = (&mut self.toggles, &mut self.blink);
        self.ctc.advance(t, |ch, at| match ch {
            0 | 1 => toggles.push((ch as u8, at)),
            2 => *blink ^= true,
            _ => {}
        });
        self.toggles.sort_by_key(|e| e.1);
        // PIO B bits 4-1: the volume, all ones is silence.
        let volume = 15 - i32::from(self.mem.pio_b >> 1 & 15);
        let sounding = [
            self.ctc.channel(0).0 & 1 != 0,
            self.ctc.channel(1).0 & 1 != 0,
        ];
        self.sound.run(t, &self.toggles, sounding, volume);
        let mut field = false;
        while self.sched.now().0 * 2 >= self.line_end {
            self.line_end += self.model.line_half_t();
            self.end_of_line();
            field |= self.line == 0;
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

    /// The beam reached the end of its line: draw it, count it.
    fn end_of_line(&mut self) {
        let y = self.line as usize;
        if y < video::HEIGHT as usize {
            let blink = self.blink && self.mem.pio_b & memory::B_BLINK != 0;
            let out = &mut self.pixels[y * video::WIDTH as usize..][..video::WIDTH as usize];
            if self.model.four() {
                video::line_4(&self.mem.irm, self.mem.port84, y, blink, out);
            } else {
                video::line_3(&self.mem.irm, y, blink, out);
            }
        }
        self.line += 1;
        // The line pulse clocks CTC channels 0 and 1, the field pulse
        // channels 2 and 3.
        for ch in 0..2 {
            if self.ctc.trigger(ch) {
                self.sound.level[ch] ^= true;
            }
        }
        if self.line == video::HEIGHT {
            if self.ctc.trigger(2) {
                self.blink ^= true;
            }
            self.ctc.trigger(3);
        }
        if self.line == video::LINES {
            self.line = 0;
            self.fields += 1;
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.keyboard.key(machine_key.0, down);
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) => match id.0 {
                CONTROL_RESET => Machine::reset(self, ResetKind::Soft),
                CONTROL_POWER => Machine::reset(self, ResetKind::Hard),
                _ => {}
            },
            _ => {}
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: self.model.cpu_hz() * 4,
            active_pixels: video::WIDTH,
            total_pixels: (self.model.line_half_t() * 2) as u32,
            lines_per_field: video::LINES,
            standard: VideoStandard::Pal,
            output: OutputType::Rgb,
            field: FieldParity::Progressive,
        }
    }

    /// The 32 lines of CAOS' character buffer (`$B200`, 40 columns) as
    /// text, unprintable codes as `.`, for tests and tools.
    pub fn screen_text(&self) -> String {
        let mut s = String::new();
        for row in 0..32u16 {
            for col in 0..40u16 {
                let c = self.mem.irm0(0xb200 + row * 40 + col);
                s.push(match c {
                    0 => ' ',
                    0x20..=0x7e => c as char,
                    _ => '.',
                });
            }
            s.push('\n');
        }
        s
    }

    /// A byte as the CPU reads it now (tests and tools).
    pub fn peek_cpu(&self, addr: u16) -> u8 {
        self.mem.read(addr)
    }
}

impl Machine for Kc85 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        for f in dbg.per_frame_freezes() {
            if f.space == CPU_SPACE && f.addr < IO_BASE {
                for i in 0..f.size.bytes() {
                    self.mem
                        .poke((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
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
        self.instruction(None);
        StopReason::StepDone
    }

    /// Soft: the RESET button (the CPU restarts at `$E000`, RAM kept).
    /// Hard: power off and on (`$F000`).
    fn reset(&mut self, kind: ResetKind) {
        self.pio.reset();
        self.ctc.reset();
        self.mem.reset();
        self.mem.pio_a = self.pio.pins(pio::A, 0xff);
        self.mem.pio_b = self.pio.pins(pio::B, 0xff);
        self.input.clear();
        self.keyboard.release_all();
        match kind {
            ResetKind::Soft => {
                self.cpu.reset();
                self.cpu.pc = 0xe000;
            }
            ResetKind::Hard => {
                self.cpu = Z80::new();
                self.cpu.pc = 0xf000;
                self.mem.clear();
                self.blink = false;
                self.sound.level = [false; 2];
                self.field_done_pending = false;
                if let Some(d) = &mut self.d004 {
                    d.power_on();
                }
            }
        }
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: video::WIDTH,
            height: video::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.pixels,
                palette: &video::PALETTE,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> mimic_core::AudioFormat {
        out.append(&mut self.sound.samples);
        mimic_core::AudioFormat {
            sample_rate: mimic_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let mut v = vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 17,
            big_endian: false,
        }];
        if self.d004.is_some() {
            v.push(MemSpace {
                id: d004::D004_SPACE,
                name: "d004",
                device: d004::D004_DEVICE,
                addr_bits: 17,
                big_endian: false,
            });
        }
        v
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, addr: Option<u32>, value: u32, bits| ChipReg {
            name,
            addr,
            value,
            bits,
        };
        let (a, b) = (self.pio.registers(pio::A), self.pio.registers(pio::B));
        let mut pio_regs = vec![
            reg("A", Some(IO_BASE + 0x88), u32::from(self.mem.pio_a), 8),
            reg("B", Some(IO_BASE + 0x89), u32::from(self.mem.pio_b), 8),
            reg("A_MODE", None, u32::from(a.0), 2),
            reg("B_MODE", None, u32::from(b.0), 2),
            reg("A_INT", None, u32::from(a.2), 8),
            reg("B_INT", None, u32::from(b.2), 8),
            reg("A_VECTOR", None, u32::from(a.3), 8),
            reg("B_VECTOR", None, u32::from(b.3), 8),
        ];
        if self.model.four() {
            pio_regs.push(reg(
                "PORT_84",
                Some(IO_BASE + 0x84),
                u32::from(self.mem.port84),
                8,
            ));
            pio_regs.push(reg(
                "PORT_86",
                Some(IO_BASE + 0x86),
                u32::from(self.mem.port86),
                8,
            ));
        }
        const CTC_NAMES: [[&str; 3]; 4] = [
            ["CONTROL0", "CONSTANT0", "COUNTER0"],
            ["CONTROL1", "CONSTANT1", "COUNTER1"],
            ["CONTROL2", "CONSTANT2", "COUNTER2"],
            ["CONTROL3", "CONSTANT3", "COUNTER3"],
        ];
        let mut ctc_regs = vec![reg("VECTOR", None, u32::from(self.ctc.vector_base()), 8)];
        for (n, names) in CTC_NAMES.iter().enumerate() {
            let (control, constant, counter) = self.ctc.channel(n);
            ctc_regs.push(reg(names[0], None, u32::from(control), 8));
            ctc_regs.push(reg(names[1], None, u32::from(constant), 9));
            ctc_regs.push(reg(
                names[2],
                Some(IO_BASE + 0x8c + n as u32),
                u32::from(counter),
                9,
            ));
        }
        let mut chips = vec![
            ChipInfo {
                name: "pio",
                regs: pio_regs,
            },
            ChipInfo {
                name: "ctc",
                regs: ctc_regs,
            },
        ];
        if let Some(d) = &self.d004 {
            let mut regs = vec![
                reg("RUNNING", None, u32::from(d.running()), 1),
                // The D004's own ports $F6, $F4, $F0 (its space, not the KC's).
                reg("SELECT", None, u32::from(d.select()), 8),
                reg("STATUS", None, u32::from(d.status()), 8),
                reg("FDC_MSR", None, u32::from(d.fdc().status()), 8),
                reg("PCN0", None, u32::from(d.fdc().cylinder(0)), 8),
                reg("PCN1", None, u32::from(d.fdc().cylinder(1)), 8),
            ];
            regs.push(reg("CTC_VECTOR", None, u32::from(d.ctc().vector_base()), 8));
            const NAMES: [[&str; 2]; 4] = [
                ["CTC_CONTROL0", "CTC_COUNTER0"],
                ["CTC_CONTROL1", "CTC_COUNTER1"],
                ["CTC_CONTROL2", "CTC_COUNTER2"],
                ["CTC_CONTROL3", "CTC_COUNTER3"],
            ];
            for (n, [control_name, counter_name]) in NAMES.into_iter().enumerate() {
                let (control, _, counter) = d.ctc().channel(n);
                regs.push(reg(control_name, None, u32::from(control), 8));
                regs.push(reg(counter_name, None, u32::from(counter), 9));
            }
            chips.push(ChipInfo { name: "d004", regs });
        }
        chips
    }

    fn devices(&self) -> Vec<DeviceId> {
        let mut v = vec![CPU];
        if self.d004.is_some() {
            v.push(d004::D004_DEVICE);
        }
        v
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if dev == CPU {
            return Some(self as &mut dyn DebugTarget);
        }
        match self.d004.as_deref_mut() {
            Some(d) if dev == d004::D004_DEVICE => Some(d as &mut dyn DebugTarget),
            _ => None,
        }
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    /// Text as key presses, with a pause behind ENTER for CAOS or BASIC
    /// to take the line in.
    fn typed_keys(&self, text: &str, start: Cycle) -> Option<(Vec<TimedInput>, Cycle)> {
        let hz = self.model.cpu_hz();
        Some(mimic_core::input::typed_lines(
            text,
            start,
            u64::from(hz),
            |line, from| keyboard::typed_events(line, from, hz),
        ))
    }

    fn controls(&self) -> Vec<mimic_core::ControlInfo> {
        vec![
            mimic_core::ControlInfo::button(CONTROL_RESET, "RESET", false),
            mimic_core::ControlInfo::button(CONTROL_POWER, "POWER", false),
        ]
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
        let mut v = Vec::new();
        if self.d004.is_some() {
            for (i, (id, name)) in [(SLOT_DRIVE0, "drive 0"), (SLOT_DRIVE1, "drive 1")]
                .into_iter()
                .enumerate()
            {
                v.push(MediaSlotInfo {
                    id,
                    name,
                    kind: MediaKind::Disk,
                    inserted: self.d004.as_ref().is_some_and(|d| d.drives[i].has_disc()),
                });
            }
        }
        v.push(MediaSlotInfo {
            id: SLOT_TAPE,
            name: "tape",
            kind: MediaKind::Tape,
            inserted: self.tape.inserted(),
        });
        v
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_TAPE => Some(&mut self.tape),
            SLOT_DRIVE0 | SLOT_DRIVE1 => self
                .d004
                .as_deref_mut()
                .map(|d| &mut d.drives[usize::from(slot.0 - 1)] as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"KC85", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.blink);
            w.bool(self.sound.level[0]);
            w.bool(self.sound.level[1]);
            w.u32(self.sound.phase);
            self.sound.audio.save(w);
            w.u32(self.line);
            w.u64(self.line_end);
            w.u64(self.fields);
            w.bool(self.field_done_pending);
            w.raw(&self.pixels);
            self.keyboard.save(w);
            self.tape.save(w);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.pio.save(&mut w);
        self.ctc.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        if let Some(d) = &self.d004 {
            d.save(&mut w);
        }
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"KC85", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let blink = c.bool()?;
        let mut sound = self.sound.clone();
        sound.level = [c.bool()?, c.bool()?];
        sound.phase = c.u32()?;
        sound.audio.load(&mut c)?;
        let line = c.u32()?;
        let line_end = c.u64()?;
        let fields = c.u64()?;
        let field_done_pending = c.bool()?;
        let mut pixels = vec![0u8; self.pixels.len()];
        c.raw_into(&mut pixels)?;
        let mut keyboard = self.keyboard.clone();
        keyboard.load(&mut c)?;
        let tape = tape::Tape::load(&mut c)?;
        if sound.phase >= SOUND_STEP
            || line >= video::LINES
            || pixels
                .iter()
                .any(|&p| usize::from(p) >= video::PALETTE.len())
        {
            return Err(StateError::Invalid("KC85 picture or sound state"));
        }
        let mut cpu = self.cpu.clone();
        let mut mem = self.mem.clone();
        let mut pio = self.pio.clone();
        let mut ctc = self.ctc.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        mem.load(&mut r)?;
        pio.load(&mut r)?;
        ctc.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let now = sched.now().0 * 2;
        if line_end <= now || line_end > now + self.model.line_half_t() + 64 {
            return Err(StateError::Invalid("KC85 line clock"));
        }
        // Last (it is atomic on its own).
        if let Some(d) = &mut self.d004 {
            d.load(&mut r)?;
        }
        self.cpu = cpu;
        self.mem = mem;
        self.pio = pio;
        self.ctc = ctc;
        self.sched = sched;
        self.input = input;
        self.blink = blink;
        self.sound = sound;
        self.sound.samples.clear();
        self.line = line;
        self.line_end = line_end;
        self.fields = fields;
        self.field_done_pending = field_done_pending;
        self.pixels = pixels;
        self.keyboard = keyboard;
        // The tape's contents stay; its position loads.
        self.tape.restore(tape);
        Ok(())
    }
}

impl DebugTarget for Kc85 {
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
        let bus = PeekBus(&self.mem, &self.pio, &self.ctc, self.d004.as_deref());
        Some((0..size.bytes()).fold(0, |v, i| {
            v | bus.peek(addr.wrapping_add(i), Size::Byte) << (8 * i)
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE || addr >= IO_BASE {
            return false;
        }
        (0..size.bytes()).all(|i| {
            self.mem
                .poke(addr.wrapping_add(i) as u16, (v >> (8 * i)) as u8)
        })
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(
            &PeekBus(&self.mem, &self.pio, &self.ctc, self.d004.as_deref()),
            addr,
        )
    }
}

#[cfg(test)]
mod tests;
