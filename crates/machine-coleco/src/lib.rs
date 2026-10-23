//! ColecoVision (NTSC and PAL).
//!
//! Z80A at 3.579545 MHz (PAL: 3.546893 MHz), the TMS9918A video display
//! processor (PAL: TMS9929A) with 16 KB VRAM, the SN76489AN sound
//! generator (`chips-ti`), 8 KB BIOS ROM, 1 KB RAM, two controller ports
//! and the cartridge slot.
//!
//! **Memory.** $0000–$1FFF BIOS; $2000–$5FFF the expansion port (open:
//! reads $FF); $6000–$7FFF the 1 KB RAM, repeated eight times; $8000–$FFFF
//! the cartridge (open without one).
//!
//! **I/O** (address bits 7–5; the low bits as noted): writes to $80–$9F
//! select the controllers' keypad segment, $C0–$DF the joystick segment;
//! $A0–$BF is the VDP (A0 = 0 data, 1 control/status); writes to $E0–$FF
//! go to the SN76489, reads there return a controller (A1 = 0 port 1, 1
//! port 2). Other reads return $FF.
//!
//! **Controllers** (`devices_input::coleco`): a read returns, active low,
//! bit 0 pin 1 (up / keypad code bit 0), bit 1 pin 4 (right / code bit 1),
//! bit 2 pin 2 (down / code bit 2), bit 3 pin 3 (left / code bit 3), bit 4
//! pin 9 and bit 5 pin 7 (the quadrature phases B and A of a spinner,
//! wheel or trackball), bit 6 pin 6 (left fire in the joystick segment,
//! right fire in the keypad segment), bit 7 high. A rising edge of phase A
//! on either port pulls the Z80's /INT low until that port is read; the
//! ports are sampled once a raster line (228 T-states). This interrupt
//! behaviour is this project's reading (see `docs/sources.md`); the BIOS
//! counts spinner steps in its interrupt routine. A Roller Controller in
//! port 1 also drives port 2's fire and phase lines through its second
//! cable.
//!
//! **Interrupts.** The VDP's /INT drives the Z80's /NMI (edge-triggered:
//! a program that does not read the VDP status gets no further NMI).
//!
//! **Timing.** The Z80 runs whole instructions with [`Z80::step_timed`]
//! and no wait states; before each bus access the chips are brought up to
//! that T-state: the VDP runs three pixel clocks per two T-states (both
//! derive from the colour subcarrier crystal), the SN76489 one input clock
//! per T-state. A field: 262 (PAL 313) lines of 228 T-states.
//!
//! Models: "ntsc" (default) and "pal". Options:
//! - `port1`, `port2`: "controller" (default), "superaction", "driving",
//!   "roller" or "none".
//! - `roms`: "stub" runs a small built-in BIOS ([`stub`]): a cartridge
//!   whose header starts with $AA55 or $55AA is started directly.
//!
//! Media slot "cartridge": a ROM image up to 32 KB or a MegaCart image
//! (`devices_cartridge::coleco`). Inserting or removing one powers the
//! console up again.
//!
//! **Keyboard** (machine keys, mapped by the host keymap): the keypads of
//! port 1 from 16 (1 2 3 4 5 6 7 8 9 * 0 #, then the Super Action
//! purple and blue buttons and the right fire button) and of port 2 from
//! 32. The host keymap puts port 1's keypad on the digit row (- for *, =
//! for #, [ ] purple and blue, X right fire) and port 2's on the numeric
//! keypad (* and / for * and #, + right fire); the joystick of port 1 is
//! on the cursor keys with Space as the left fire button.
//!
//! ROM: role "coleco" (8 KB BIOS), user-supplied, never in the repository.
//!
//! Sources: pvcollib wiki, "Complete Colecovision Specs" (memory and I/O
//! map, cartridge header, NMI from the VDP); Texas Instruments TMS9918A and
//! SN76489AN data (see `chips-ti`). The implementation is this project's
//! own.

#![forbid(unsafe_code)]

pub mod stub;

use chips_ti::{vdp, Sn76489, Tms9918, VdpModel};
use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use devices_cartridge::coleco::ColecoCart;
use devices_cartridge::CartSlot;
use devices_input::coleco::pin;
use devices_input::{ColecoController, ColecoKind};
use emu_core::audio::Decimator;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::input::{InputEvent, InputQueue, InputTarget, MachineKey, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The VDP's 16 KB of video RAM.
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const CPU_HZ_NTSC: u32 = 3_579_545;
pub const CPU_HZ_PAL: u32 = 3_546_893;
/// T-states per raster line (342 pixel clocks × 2/3).
pub const LINE_T: u64 = 228;

/// Machine keys: the keypads.
pub mod keys {
    /// Port 1's keys 0–14 (1 2 3 4 5 6 7 8 9 * 0 # purple blue, then the
    /// right fire button).
    pub const PORT1_KEYPAD: u16 = 16;
    /// Port 2's keys.
    pub const PORT2_KEYPAD: u16 = 32;
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "coleco",
        name: "ColecoVision",
        models: &["ntsc", "pal"],
        create: |config| Ok(Box::new(Coleco::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Ntsc,
    Pal,
}

/// The device a port option names.
fn port_option(
    config: &MachineConfig,
    key: &str,
    cycles_per_step: u64,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    let kind = match config.options.get(key).map(String::as_str) {
        None | Some("controller") => ColecoKind::Standard,
        Some("superaction") => ColecoKind::SuperAction,
        Some("driving") => ColecoKind::Driving,
        Some("roller") => ColecoKind::Roller,
        Some("none") => return Ok(None),
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "{key} option '{other}' (controller, superaction, driving, roller or none)"
            )))
        }
    };
    Ok(Some(Box::new(ColecoController::new(kind, cycles_per_step))))
}

/// Everything on the bus except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    bios: Vec<u8>,
    ram: Vec<u8>,
    /// The working copy of the inserted cartridge.
    cart: Option<ColecoCart>,
    vdp: Tms9918,
    psg: Sn76489,
    /// The keypad segment is selected (else the joystick one).
    keypad: bool,
    /// Phase A of each port's quadrature input at the last sample (high).
    spin_a: [bool; 2],
    /// A spinner step waits for its port to be read (/INT low).
    spin_int: [bool; 2],
    /// T-states the chips have run.
    chips_t: u64,
    /// T-state of the next port sample.
    sample_due: u64,
    /// A field ended since the last check.
    field_done: bool,
    audio: Decimator,
    /// DC blocker: last input and output (16.16).
    dc: [i32; 2],
    samples: Vec<i16>,
}

impl Board {
    /// One T-state of the chips: the SN76489 one input clock, the VDP one
    /// or two pixel clocks (three per two T-states).
    fn tick(&mut self) {
        let level = self.psg.tick();
        if let Some(s) = self.audio.push(level - 16_382) {
            // A first-order high-pass (the console's AC coupling).
            let x = i32::from(s) << 16;
            let y = x - self.dc[0] + ((i64::from(self.dc[1]) * 65_208) >> 16) as i32;
            self.dc = [x, y];
            self.samples
                .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        }
        let pixels = 1 + (self.chips_t & 1);
        for _ in 0..pixels {
            self.field_done |= self.vdp.tick();
        }
        self.chips_t += 1;
    }

    /// Run the chips up to (not including) T-state `to`.
    fn run_to(&mut self, to: u64) {
        while self.chips_t < to {
            self.tick();
        }
    }

    fn mem_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1fff => self.bios[usize::from(addr)],
            0x2000..=0x5fff => 0xff,
            0x6000..=0x7fff => self.ram[usize::from(addr & 0x3ff)],
            _ => self.cart.as_mut().map_or(0xff, |c| c.access(addr)),
        }
    }

    fn mem_peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1fff => self.bios[usize::from(addr)],
            0x2000..=0x5fff => 0xff,
            0x6000..=0x7fff => self.ram[usize::from(addr & 0x3ff)],
            _ => self.cart.as_ref().map_or(0xff, |c| c.peek(addr)),
        }
    }

    fn mem_write(&mut self, addr: u16, v: u8) {
        match addr {
            0x6000..=0x7fff => self.ram[usize::from(addr & 0x3ff)] = v,
            0x8000..=0xffff => {
                if let Some(c) = &mut self.cart {
                    c.access(addr);
                }
            }
            _ => {}
        }
    }
}

/// What a port's lines give as the controller byte (see the crate doc).
fn controller_byte(low: u32) -> u8 {
    let bit = |p: u32, b: u8| if low & p != 0 { 0 } else { 1 << b };
    0x80 | bit(pin::UP, 0)
        | bit(pin::RIGHT, 1)
        | bit(pin::DOWN, 2)
        | bit(pin::LEFT, 3)
        | bit(pin::SPIN_B, 4)
        | bit(pin::SPIN_A, 5)
        | bit(pin::FIRE, 6)
}

/// The lines of port `n` (0 or 1) at `now`: its device's, or for port 2
/// the second cable of a Roller Controller in port 1.
fn port_lines(ports: &[Option<Box<dyn PortDevice>>; 2], n: usize, now: Cycle) -> u32 {
    if n == 1 {
        if let Some(d) = &ports[0] {
            let low = d.read_lines(now).pulled_low;
            if low & pin::Y_CABLE != 0 {
                let map = [
                    (pin::Y_FIRE, pin::FIRE),
                    (pin::Y_A, pin::SPIN_A),
                    (pin::Y_B, pin::SPIN_B),
                ];
                let mut y = 0;
                for (from, to) in map {
                    if low & from != 0 {
                        y |= to;
                    }
                }
                if let Some(d2) = &ports[1] {
                    y |= d2.read_lines(now).pulled_low & !(pin::SPIN_A | pin::SPIN_B);
                }
                return y;
            }
        }
    }
    ports[n]
        .as_ref()
        .map_or(0, |d| d.read_lines(now).pulled_low)
}

/// The Z80's bus for one instruction.
struct CvBus<'a> {
    b: &'a mut Board,
    ports: &'a mut [Option<Box<dyn PortDevice>>; 2],
    /// T-state the instruction started on.
    start: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl CvBus<'_> {
    fn io_read(&mut self, port: u8) -> u8 {
        match port & 0xe0 {
            0xa0 => {
                if port & 1 == 0 {
                    self.b.vdp.read_data()
                } else {
                    self.b.vdp.read_status()
                }
            }
            0xe0 => {
                let n = usize::from(port >> 1 & 1);
                self.b.spin_int[n] = false;
                controller_byte(port_lines(self.ports, n, Cycle(self.b.chips_t)))
            }
            _ => 0xff,
        }
    }

    fn io_write(&mut self, port: u8, v: u8) {
        match port & 0xe0 {
            0x80 | 0xc0 => {
                self.b.keypad = port & 0xe0 == 0x80;
                select_segment(self.ports, self.b.keypad, Cycle(self.b.chips_t));
            }
            0xa0 => {
                if port & 1 == 0 {
                    self.b.vdp.write_data(v);
                } else {
                    self.b.vdp.write_control(v);
                }
            }
            0xe0 => self.b.psg.write(v),
            _ => {}
        }
    }
}

/// Drive the segment commons of both ports.
fn select_segment(ports: &mut [Option<Box<dyn PortDevice>>; 2], keypad: bool, now: Cycle) {
    let common = if keypad {
        pin::KEYPAD_COMMON
    } else {
        pin::JOY_COMMON
    };
    for d in ports.iter_mut().flatten() {
        d.write_lines(
            PortLines {
                pulled_low: common,
                analog: [0; 2],
            },
            now,
        );
    }
}

impl Bus for CvBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = if addr >= IO_BASE {
            u32::from(self.io_read(addr as u8))
        } else {
            u32::from(self.b.mem_read(addr as u16))
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
        u32::from(peek(self.b, addr))
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
            self.io_write(addr as u8, v as u8);
        } else {
            self.b.mem_write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for CvBus<'_> {
    fn wait_states(&mut self, _access: Access, t: u32) -> u32 {
        self.b.run_to(self.start + u64::from(t));
        0
    }
}

/// Side-effect-free CPU view (I/O: VDP buffer/status, $FF elsewhere).
fn peek(b: &Board, addr: u32) -> u8 {
    if addr >= IO_BASE {
        let port = addr as u8;
        return match port & 0xe0 {
            0xa0 => {
                let (data, status) = b.vdp.peek();
                if port & 1 == 0 {
                    data
                } else {
                    status
                }
            }
            _ => 0xff,
        };
    }
    b.mem_peek(addr as u16)
}

struct PeekBus<'a>(&'a Board);

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

pub struct Coleco {
    model: Model,
    cpu: Z80,
    board: Board,
    ports: [Option<Box<dyn PortDevice>>; 2],
    cart: CartSlot<ColecoCart>,
    /// T-states since power-on.
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
}

impl Coleco {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("ntsc") => Model::Ntsc,
            Some("pal") => Model::Pal,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let bios = match config.options.get("roms").map(String::as_str) {
            Some("stub") => stub::rom(),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => {
                let r = config
                    .roms
                    .get("coleco")
                    .ok_or_else(|| ConfigError::MissingRom("coleco".into()))?;
                if r.len() != 0x2000 {
                    return Err(ConfigError::Invalid(format!(
                        "coleco BIOS must be 8192 bytes, got {}",
                        r.len()
                    )));
                }
                r.clone()
            }
        };
        let hz = cpu_hz(model);
        // About 400 steps a second: the wheel while a direction is held,
        // host motion at most (a line is 228 T-states, a step 8949).
        let wheel = u64::from(hz) / 400;
        let ports = [
            port_option(config, "port1", wheel)?,
            port_option(config, "port2", wheel)?,
        ];
        let vdp_model = match model {
            Model::Ntsc => VdpModel::Ntsc,
            Model::Pal => VdpModel::Pal,
        };
        let mut m = Self {
            model,
            cpu: Z80::new(),
            board: Board {
                bios,
                ram: vec![0; 0x400],
                cart: None,
                vdp: Tms9918::new(vdp_model),
                psg: Sn76489::new(),
                keypad: false,
                spin_a: [true; 2],
                spin_int: [false; 2],
                chips_t: 0,
                sample_due: 0,
                field_done: false,
                audio: Decimator::new(hz, emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                dc: [0; 2],
                samples: Vec::new(),
            },
            ports,
            cart: devices_cartridge::coleco::slot(),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
        };
        m.cpu.reset();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.vdp;
        BeamPos {
            frame: v.fields,
            line: v.line,
            cycle: v.dot * 2 / 3,
            hpos: v.dot,
        }
    }

    /// Sample the ports' quadrature inputs: a rising edge of phase A
    /// requests the maskable interrupt until the port is read.
    fn sample_ports(&mut self) {
        let now = Cycle(self.board.chips_t);
        select_segment(&mut self.ports, self.board.keypad, now);
        for n in 0..2 {
            let low = port_lines(&self.ports, n, now);
            let a = low & pin::SPIN_A == 0;
            if a && !self.board.spin_a[n] {
                self.board.spin_int[n] = true;
            }
            self.board.spin_a[n] = a;
        }
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let start = self.sched.now().0;
        let beam = self.beam_pos();
        let t = {
            let mut bus = CvBus {
                b: &mut self.board,
                ports: &mut self.ports,
                start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step_timed(&mut bus)
        };
        let end = start + u64::from(t);
        self.board.run_to(end);
        self.sched.advance(u64::from(t));
        if end >= self.board.sample_due {
            self.board.sample_due = end + LINE_T;
            self.sample_ports();
        }
        self.cpu.set_nmi(self.board.vdp.int());
        self.cpu
            .set_int(self.board.spin_int[0] || self.board.spin_int[1]);
        let entered = self.cpu.take_interrupt();
        let field = std::mem::take(&mut self.board.field_done);
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
        match ev.target {
            InputTarget::Port(p) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&ev.event);
                }
            }
            InputTarget::Keyboard => {
                if let InputEvent::Key { machine_key, down } = ev.event {
                    self.key(machine_key, down);
                }
            }
            InputTarget::Panel => {
                if let InputEvent::Control { id, value: 1 } = ev.event {
                    match id.0 {
                        0 => self.reset(ResetKind::Hard),
                        1 => self.reset(ResetKind::Soft),
                        _ => {}
                    }
                }
            }
        }
    }

    /// A keyboard key: the keypads of both ports.
    fn key(&mut self, key: MachineKey, down: bool) {
        let (port, k) = match key.0 {
            k @ 16..=30 => (0, k - keys::PORT1_KEYPAD),
            k @ 32..=46 => (1, k - keys::PORT2_KEYPAD),
            _ => return,
        };
        if let Some(dev) = &mut self.ports[port] {
            dev.input(&InputEvent::Key {
                machine_key: MachineKey(k),
                down,
            });
        }
    }

    fn signal(&self) -> SignalInfo {
        let standard = match self.model {
            Model::Ntsc => VideoStandard::Ntsc,
            Model::Pal => VideoStandard::Pal,
        };
        SignalInfo {
            pixel_clock_hz: cpu_hz(self.model) * 3 / 2,
            active_pixels: vdp::WIDTH,
            total_pixels: u32::from(vdp::LINE),
            lines_per_field: u32::from(self.board.vdp.model.lines()),
            standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

fn cpu_hz(model: Model) -> u32 {
    match model {
        Model::Ntsc => CPU_HZ_NTSC,
        Model::Pal => CPU_HZ_PAL,
    }
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Coleco, addr: u16) -> u8 {
    m.board.mem_peek(addr)
}

/// The VDP (tests and tools).
pub fn vdp(m: &Coleco) -> &Tms9918 {
    &m.board.vdp
}

impl Machine for Coleco {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        if self.board.vdp.line == 0 && self.board.vdp.dot < 3 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE && f.addr < IO_BASE {
                    for i in 0..f.size.bytes() {
                        self.board
                            .mem_write((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
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
        self.instruction(None);
        StopReason::StepDone
    }

    /// Hard: power cycle (RAM and VRAM cleared, the cartridge's banking
    /// restarts). Soft: the RESET button (the Z80 and the VDP's registers).
    fn reset(&mut self, kind: ResetKind) {
        self.cpu = Z80::new();
        self.cpu.reset();
        self.board.vdp.reset();
        if kind == ResetKind::Hard {
            self.board.ram.fill(0);
            self.board.vdp = Tms9918::new(self.board.vdp.model);
            self.board.psg = Sn76489::new();
            self.board.cart = self.cart.cart().cloned();
            if let Some(c) = &mut self.board.cart {
                c.reset();
            }
        }
        self.board.spin_int = [false; 2];
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: vdp::WIDTH,
            height: self.board.vdp.model.height(),
            pixels: PixelData::Indexed {
                pixels: self.board.vdp.frame(),
                palette: &vdp::PALETTE,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![
            MemSpace {
                id: CPU_SPACE,
                name: "cpu",
                device: CPU,
                addr_bits: 17,
                big_endian: false,
            },
            MemSpace {
                id: VRAM_SPACE,
                name: "vram",
                device: CPU,
                addr_bits: 14,
                big_endian: false,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let v = &self.board.vdp;
        let p = &self.board.psg;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        const R: [&str; 8] = ["R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7"];
        let mut vdp_regs: Vec<ChipReg> = R
            .iter()
            .zip(v.regs)
            .map(|(n, val)| reg(*n, u32::from(val), 8))
            .collect();
        vdp_regs.extend([
            reg("STATUS", u32::from(v.peek().1), 8),
            reg("ADDR", u32::from(v.addr), 14),
            reg("LINE", u32::from(v.line), 9),
            reg("DOT", u32::from(v.dot), 9),
        ]);
        let psg_regs = vec![
            reg("TONE0", u32::from(p.period[0]), 10),
            reg("TONE1", u32::from(p.period[1]), 10),
            reg("TONE2", u32::from(p.period[2]), 10),
            reg("NOISE", u32::from(p.noise), 3),
            reg("ATT0", u32::from(p.atten[0]), 4),
            reg("ATT1", u32::from(p.atten[1]), 4),
            reg("ATT2", u32::from(p.atten[2]), 4),
            reg("ATT3", u32::from(p.atten[3]), 4),
        ];
        let b = &self.board;
        vec![
            ChipInfo {
                name: "vdp",
                regs: vdp_regs,
            },
            ChipInfo {
                name: "psg",
                regs: psg_regs,
            },
            ChipInfo {
                name: "controllers",
                regs: vec![
                    reg("KEYPAD", u32::from(b.keypad), 1),
                    reg("SPIN_INT1", u32::from(b.spin_int[0]), 1),
                    reg("SPIN_INT2", u32::from(b.spin_int[1]), 1),
                    reg("BANK", b.cart.as_ref().map_or(0, |c| c.bank as u32), 6),
                ],
            },
        ]
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
        vec![
            emu_core::ControlInfo::button(0, "POWER", false),
            emu_core::ControlInfo::button(1, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        [(PORT1, "controller 1"), (PORT2, "controller 2")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::ColecoControl,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, mut dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::ColecoControl {
            return Err(PortError::Incompatible {
                port: PortKind::ColecoControl,
                device: dev.kind(),
            });
        }
        if slot.is_some() {
            return Err(PortError::Occupied(port));
        }
        let common = if self.board.keypad {
            pin::KEYPAD_COMMON
        } else {
            pin::JOY_COMMON
        };
        dev.write_lines(
            PortLines {
                pulled_low: common,
                analog: [0; 2],
            },
            Cycle(self.board.chips_t),
        );
        *slot = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        self.ports.get_mut(usize::from(port.0))?.take()
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
        let b = &self.board;
        w.chunk(*b"CVIS", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.field_done_pending);
            w.bytes(&b.ram);
            w.bool(b.keypad);
            for n in 0..2 {
                w.bool(b.spin_a[n]);
                w.bool(b.spin_int[n]);
            }
            w.u64(b.chips_t);
            w.u64(b.sample_due);
            w.bool(b.field_done);
            b.audio.save(w);
            w.u32(b.dc[0] as u32);
            w.u32(b.dc[1] as u32);
            w.bool(b.cart.is_some());
        });
        self.cpu.save(&mut w);
        b.vdp.save(&mut w);
        b.psg.save(&mut w);
        if let Some(c) = &b.cart {
            c.save(&mut w);
        }
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"CVP1", *b"CVP2"].into_iter().enumerate() {
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
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"CVIS", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let field_done_pending = c.bool()?;
        let mut board = self.board.clone();
        let ram = c.bytes()?;
        if ram.len() != board.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        board.ram = ram.to_vec();
        board.keypad = c.bool()?;
        for n in 0..2 {
            board.spin_a[n] = c.bool()?;
            board.spin_int[n] = c.bool()?;
        }
        board.chips_t = c.u64()?;
        board.sample_due = c.u64()?;
        board.field_done = c.bool()?;
        board.audio.load(&mut c)?;
        board.dc = [c.u32()? as i32, c.u32()? as i32];
        let has_cart = c.bool()?;
        if has_cart != board.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        board.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        board.vdp.load(&mut r)?;
        board.psg.load(&mut r)?;
        if let Some(cart) = &mut board.cart {
            cart.load(&mut r)?;
        }
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"CVP1", *b"CVP2"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        for (dev, st) in self.ports.iter().zip(&port_states) {
            if dev.is_none() != st.is_empty() {
                return Err(StateError::Invalid("controllers differ"));
            }
        }
        for (dev, st) in self.ports.iter_mut().zip(&port_states) {
            if let Some(d) = dev {
                d.load_state(st)?;
            }
        }
        self.cpu = cpu;
        self.board = board;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for Coleco {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE => Some(u32::from(peek(&self.board, a))),
                VRAM_SPACE => Some(u32::from(self.board.vdp.vram[(a & 0x3fff) as usize])),
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            match space {
                CPU_SPACE if (0x6000..0x8000).contains(&a) => {
                    self.board.ram[(a & 0x3ff) as usize] = byte;
                }
                VRAM_SPACE => self.board.vdp.vram[(a & 0x3fff) as usize] = byte,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.board), addr)
    }
}

#[cfg(test)]
mod tests;
