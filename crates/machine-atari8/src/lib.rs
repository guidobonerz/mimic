//! Atari 800XL.
//!
//! 6502C "SALLY", ANTIC (display lists, DMA), GTIA (colours, players and
//! missiles), POKEY (sound, timers, keyboard, serial I/O), a 6520 PIA
//! (joysticks, memory banking) and 64 KB RAM with the OS, self-test and
//! BASIC ROMs banked in by PIA port B (see [`memory`]).
//!
//! Models: "pal" (default; 1 773 447 Hz, 312 lines) and "ntsc"
//! (1 789 773 Hz, 262 lines); 114 cycles per line on both.
//!
//! Options:
//! - `port1`, `port2`: "joystick" (default port 1) or "none" (default
//!   port 2).
//! - `basic`: "on" (default) or "off" — holds OPTION while the OS boots,
//!   like the real machine.
//! - `roms`: "stub" uses a tiny built-in OS, for tests without ROMs.
//!
//! ROMs come from `MachineConfig::roms`: "os" (16 KB XL/XE OS), "basic"
//! (8 KB, optional). User-supplied, never in the repository.
//!
//! Per cycle: ANTIC runs its DMA first (and halts the CPU for it), the CPU
//! runs unless halted, GTIA draws the cycle's two colour clocks, POKEY
//! clocks.

#![forbid(unsafe_code)]

pub mod keyboard;
pub mod memory;
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
use emu_core::hooked::HookedBus;
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::Cycle;
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

use keyboard::keys;
use memory::{Area, Memory, Roms, BASIC_SIZE, OS_SIZE};

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's view of memory (banked by PORTB).
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

/// Joystick ports 1 and 2.
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

/// Fields the OPTION key is held after power-up for `basic=off`.
const OPTION_HOLD_FIELDS: u64 = 100;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "atari800xl",
        name: "Atari 800XL",
        models: &["pal", "ntsc"],
        create: |config| Ok(Box::new(Atari800Xl::new(config)?)),
    }
}

fn cpu_hz(standard: VideoStandard) -> u32 {
    match standard {
        VideoStandard::Pal => 1_773_447,
        VideoStandard::Ntsc => 1_789_773,
    }
}

/// ANTIC's view of memory.
struct DmaView<'a>(&'a Memory);

impl AnticMemory for DmaView<'_> {
    fn dma_read(&self, addr: u16) -> u8 {
        self.0.read_plain(addr)
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
            0xd3 => self.pia.peek(pia_reg(addr), self.porta, 0xff),
            0xd4 => self.antic.peek(addr as u8),
            _ => 0xff,
        }
    }

    fn read(&mut self, addr: u16) -> u8 {
        match addr >> 8 {
            0xd3 => self.pia.read(pia_reg(addr), self.porta, 0xff),
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

    /// Port B pins: outputs as driven, inputs pulled up.
    fn portb(&self) -> u8 {
        let b = &self.pia.b;
        (b.or & b.ddr) | !b.ddr
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

fn load_roms(config: &MachineConfig) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => Ok(Roms {
            os: stub::os(),
            basic: None,
        }),
        Some(other) => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => Ok(Roms {
            os: rom::<OS_SIZE>(config, "os")?,
            basic: match config.roms.get("basic") {
                Some(_) => Some(rom::<BASIC_SIZE>(config, "basic")?),
                None => None,
            },
        }),
    }
}

fn port_option(config: &MachineConfig, key: &str, default: &str) -> Result<bool, ConfigError> {
    match config
        .options
        .get(key)
        .map(String::as_str)
        .unwrap_or(default)
    {
        "joystick" => Ok(true),
        "none" => Ok(false),
        other => Err(ConfigError::Invalid(format!("{key} option '{other}'"))),
    }
}

pub struct Atari800Xl {
    standard: VideoStandard,
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    sched: Scheduler,
    input: InputQueue,
    ports: [Option<Box<dyn PortDevice>>; 2],
    palette: [Rgb8; 256],
    audio: Decimator,
    samples: Vec<i16>,
    field_done_pending: bool,
    /// `basic=off`: OPTION is held until this field.
    option_until: u64,
    basic_off: bool,
    /// The OPTION key as the user holds it (separate from the boot hold).
    option_key: bool,
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

impl Atari800Xl {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let standard = match config.model.as_deref() {
            None | Some("pal") => VideoStandard::Pal,
            Some("ntsc") => VideoStandard::Ntsc,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let basic_off = match config.options.get("basic").map(String::as_str) {
            None | Some("on") => false,
            Some("off") => true,
            Some(other) => return Err(ConfigError::Invalid(format!("basic option '{other}'"))),
        };
        let mut ports: [Option<Box<dyn PortDevice>>; 2] = [None, None];
        for (i, (key, default)) in [("port1", "joystick"), ("port2", "none")]
            .into_iter()
            .enumerate()
        {
            if port_option(config, key, default)? {
                ports[i] = Some(Box::new(devices_input::Joystick::new(PortKind::AtariJoy)));
            }
        }
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Sally,
            ..CpuConfig::default()
        });
        cpu.reset();
        let hz = cpu_hz(standard);
        let mut m = Self {
            standard,
            cpu,
            mem: Memory::new(load_roms(config)?),
            io: Io {
                antic: Antic::new(standard),
                gtia: Gtia::new(standard),
                pokey: Pokey::new(),
                pia: Pia::new(),
                porta: 0xff,
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            ports,
            palette: chips_atari::palette::palette(standard),
            audio: Decimator::new(hz, emu_core::SAMPLE_RATE)
                .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
            samples: Vec::new(),
            field_done_pending: false,
            option_until: if basic_off { OPTION_HOLD_FIELDS } else { 0 },
            basic_off,
            option_key: false,
        };
        m.refresh_ports();
        m.refresh_console();
        Ok(m)
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
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        // ANTIC's DMA comes first and decides whether the CPU runs.
        let out = self.io.antic.tick(&DmaView(&self.mem));
        if let Some((which, v)) = out.pm {
            let single = self.io.antic.dmactl & 0x10 != 0;
            self.io.gtia.dma(which, v, self.io.antic.line(), single);
        }
        self.cpu.set_halt(out.halt);
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
        self.io.gtia.render_cycle(line, c, pf);
        self.io.pokey.tick();
        let level =
            i32::from(self.io.pokey.output()) * 400 + if self.io.gtia.speaker() { 3000 } else { 0 };
        if let Some(s) = self.audio.push(level) {
            self.samples.push(s);
        }
        let new_field = self.io.antic.end_cycle();
        self.sched.tick();
        if new_field {
            self.refresh_console();
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
            c if c == keys::RESET.0 && down => self.reset(ResetKind::Soft),
            _ => {}
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

    /// Joystick lines into PIA port A (directions) and GTIA (triggers).
    fn refresh_ports(&mut self) {
        let now = self.sched.now();
        let mut porta = 0xffu8;
        for (i, dev) in self.ports.iter().enumerate() {
            let low = dev.as_ref().map_or(0, |d| d.read_lines(now).pulled_low);
            // Up, down, left, right on bits 0–3 (port 1) / 4–7 (port 2).
            porta &= !(((low & 0x0f) as u8) << (4 * i));
            self.io.gtia.set_trigger(i, low & 0x20 != 0);
        }
        self.io.porta = porta;
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
pub fn type_text(m: &mut Atari800Xl, text: &str) {
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

impl Machine for Atari800Xl {
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
            self.option_until = if self.basic_off {
                OPTION_HOLD_FIELDS
            } else {
                0
            };
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
        vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 16,
            big_endian: false,
        }]
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
        let gtia = ChipInfo {
            name: "gtia",
            regs: gtia_w
                .into_iter()
                .enumerate()
                .filter(|(_, (n, _))| !n.is_empty())
                .map(|(i, (name, v))| ChipReg {
                    name,
                    addr: Some(0xd000 + i as u32),
                    value: u32::from(v),
                    bits: 8,
                })
                .collect(),
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
            io.pia.peek(pia_reg(u16::from(i)), io.porta, 0xff)
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

    fn ports(&self) -> Vec<PortInfo> {
        [(PORT1, "joystick port 1"), (PORT2, "joystick port 2")]
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
        Vec::new()
    }

    fn media_slot(&mut self, _slot: SlotId) -> Option<&mut dyn MediaSlot> {
        None
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"A8XL", 1, |w| {
            w.bool(self.field_done_pending);
            w.u64(self.option_until);
            w.bool(self.option_key);
            w.u8(self.io.porta);
            self.audio.save(w);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.io.antic.save(&mut w);
        self.io.gtia.save(&mut w);
        self.io.pokey.save(&mut w);
        self.io.pia.save(&mut w);
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
        let (_, mut c) = r.chunk(*b"A8XL", 1)?;
        let field_done_pending = c.bool()?;
        let option_until = c.u64()?;
        let option_key = c.bool()?;
        next.io.porta = c.u8()?;
        next.audio.load(&mut c)?;
        next.cpu.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.antic.load(&mut r)?;
        next.io.gtia.load(&mut r)?;
        next.io.pokey.load(&mut r)?;
        next.io.pia.load(&mut r)?;
        next.sched.load(&mut r)?;
        next.input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"POR1", *b"POR2"] {
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
        self.refresh_ports();
        Ok(())
    }
}

impl DebugTarget for Atari800Xl {
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
                Area::Ram | Area::Io => AtariBus {
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
