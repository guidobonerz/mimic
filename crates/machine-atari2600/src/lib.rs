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
//!   (default) or "none".
//! - `roms`: "stub" inserts a built-in test cartridge ([`stub`]).
//!
//! Media slot: "cartridge" (raw `.a26`/`.bin` dumps; inserting or removing
//! one powers the console up again). Without a cartridge the CPU has no
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
use devices_cartridge::atari2600::Cart2600;
use devices_cartridge::CartSlot;
use emu_core::audio::Decimator;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
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

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

/// Left and right controller ports.
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const SLOT_CARTRIDGE: SlotId = SlotId(0);

/// SWCHB with the console switches at rest: RESET and SELECT up (1),
/// colour (bit 3 = 1), both difficulties B (bits 6, 7 = 0); the unused
/// inputs read high.
const SWITCHES: u8 = 0x3f;

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
            }
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
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match config
        .options
        .get(key)
        .map(String::as_str)
        .unwrap_or("joystick")
    {
        "joystick" => Ok(Some(Box::new(devices_input::Joystick::new(
            PortKind::AtariJoy,
        )))),
        "none" => Ok(None),
        other => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' (joystick or none)"
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
        let ports = [port_option(config, "port1")?, port_option(config, "port2")?];
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Mos6507,
            ..CpuConfig::default()
        });
        cpu.reset();
        let mut riot = Riot::new();
        riot.set_port_b_input(SWITCHES);
        let mut m = Self {
            standard,
            cpu,
            board: Board {
                tia: Tia::new(standard),
                riot,
                cart: None,
                data_bus: 0,
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

    /// The cartridge at power-up: the inserted one, else the stub.
    fn power_on_cart(&self) -> Option<Cart2600> {
        self.cart.cart().or(self.stub.as_ref()).cloned()
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
        if let Some(s) = self.audio.push(0) {
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
        if let InputTarget::Port(p) = ev.target {
            if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                dev.input(&ev.event);
            }
            self.refresh_ports();
        }
    }

    /// Joystick lines into RIOT port A (directions) and the TIA's I4/I5
    /// (fire buttons).
    fn refresh_ports(&mut self) {
        let now = self.sched.now();
        let mut swcha = 0xffu8;
        for (i, dev) in self.ports.iter().enumerate() {
            let low = dev.as_ref().map_or(0, |d| d.read_lines(now).pulled_low);
            // Up, down, left, right: bits 4–7 (left port), 0–3 (right).
            swcha &= !(((low & 0x0f) as u8) << (4 - 4 * i));
            self.board.tia.set_input(4 + i as u8, low & 0x20 == 0);
        }
        self.board.riot.set_port_a_input(swcha);
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
