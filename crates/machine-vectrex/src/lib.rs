//! Vectrex (GCE / Milton Bradley, 1982).
//!
//! A 68A09 at 1.5 MHz, a 6522 VIA, an AY-3-8912 sound generator, 1 KB RAM,
//! 8 KB system ROM (the executive at $F000 and the built-in game Mine Storm
//! at $E000), a cartridge slot and a built-in vector monitor. The CPU
//! draws by steering the beam with analog hardware the VIA controls.
//!
//! **Memory.** $0000–$7FFF the cartridge ($FF without one); $C800–$CFFF
//! the RAM (1 KB, twice); $D000–$D7FF the VIA (register = address bits
//! 3–0); $D800–$DFFF selects both (writes reach the VIA and the RAM, reads
//! come from the VIA); $E000–$FFFF the ROM. Everything else reads $FF.
//!
//! **The analog side.** Port A drives an 8-bit DAC (two's complement).
//! The DAC feeds the X integrator directly and, through a multiplexer
//! enabled by PB0 low and selected by PB2–1, three sample/holds: 00 the Y
//! integrator's input, 01 the zero reference both integrators work
//! against, 10 the beam intensity, 11 the sound output. While PB7 (/RAMP,
//! driven by timer 1 in one-shot mode) is low the integrators move the
//! beam: X by DAC − reference, Y by Y − reference per cycle. CA2 low
//! (/ZERO) pulls both to the centre. CB2 is /BLANK: the beam is visible
//! while it is high and the intensity is above 0; the executive drives it
//! from the shift register, so a pattern byte draws dashed lines. The
//! integration units are this project's calibration: the executive's ±127
//! at scale $7F reaches the screen's edges ([`HALF_W`], [`HALF_H`]).
//!
//! **Controllers.** The comparator on PB5 is high while the DAC is above
//! the potentiometer the multiplexer's second half selects (PB2–1: 00
//! controller 1 X, 01 controller 1 Y, 10 controller 2 X, 11 controller 2
//! Y; the potentiometers give the lower voltage for right and up); the executive
//! finds the stick by successive approximation. The
//! buttons reach the AY's I/O port A (bits 0–3 controller 1, 4–7
//! controller 2, low = pressed); button 4 of controller 2 is also CA1,
//! where the light pen reports the beam.
//!
//! **Sound.** The AY-3-8912 at 1.5 MHz on port A's data bus, PB4 = BDIR,
//! PB3 = BC1; plus the DAC through the multiplexer's sound channel.
//!
//! **Frames.** The display has no frame of its own: the executive waits
//! for timer 2 (30 000 cycles, 50 Hz) between pictures. The machine cuts
//! the beam's path into frames of [`FRAME_CYCLES`]: [`Machine::vectors`]
//! gives the segments of the last one and [`Machine::framebuffer`] the
//! same rasterised ([`render`]).
//!
//! **3D Imager** (`port2=imager`, `devices_input::Imager`). The machine
//! reports pin 2 to it (high while the sound chip's port A is an input,
//! or as an output with bit 5 set: the motor runs) and colours every
//! segment by the filter in front of the screen while it is drawn. The
//! disc is taken as six equal sectors from the index on: right eye blue,
//! green, red, then left eye blue, green, red (the order a published
//! Imager demo draws in; real discs differ — 3-D Mine Storm's has a large
//! green sector). Segments behind the other eye's filters are left out:
//! option `eye` = "left" (default), "right" or "both".
//!
//! Options: `port1`, `port2` = "controller" (default) or "none"; port 2
//! also "lightpen" or "imager"; `eye` (see above); `lines` = "fine"
//! (default: thin lines with bright ends) or "soft" (even, brighter lines;
//! see [`render`]). `roms` = "stub" runs a built-in test ROM ([`stub`]).
//!
//! Media slot "cartridge": a ROM image up to 32 KB, or 64 KB with PB6
//! bank switching (`devices_cartridge::vectrex`). Inserting or removing
//! one powers the console up again.
//!
//! ROM: role "vectrex", the 8 KB system ROM; a 4 KB image (the executive
//! alone) is accepted and appears in both halves, so without a cartridge
//! the console has no Mine Storm. User-supplied, never in the repository.
//!
//! Sources: roadsidethoughts.com, "Vectrex Programmer Notes": "Hardware
//! Description", "Memory Map", "6522 Interface Adapter", "Programmable
//! Sound Generator" (read 2026-10-24); Motorola MC6809 and MOS 6522 data
//! sheets (see `cpu-6809`, `chips-cbm`). The implementation is this
//! project's own.

#![forbid(unsafe_code)]

pub mod render;
pub mod stub;

use chips_amstrad::ay::Ay;
use chips_cbm::via::Via;
use cpu_6809::{M6809Bus, M6809};
use devices_cartridge::vectrex::VectrexCart;
use devices_cartridge::CartSlot;
use devices_input::vectrex::{pin, CENTRE, NO_AIM};
use devices_input::{Imager, VectrexController, VectrexKind};
use emu_core::audio::Decimator;
use emu_core::frame::VideoStandard;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo};
use emu_core::input::{InputEvent, InputQueue, InputTarget, MachineKey, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, ChipInfo, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine, MemSpace,
    MemSpaceId, RegisterSet, ResetKind, Savestate, Segment, Size, StateError, StateReader,
    StateWriter, StopReason, VectorFrame,
};

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const CPU_HZ: u32 = 1_500_000;
/// Cycles per frame of the vector output (50 Hz).
pub const FRAME_CYCLES: u64 = 30_000;
/// Integrator units per raster pixel.
pub const UNIT: i32 = 41;
/// Half the visible area in integrator units (the tube is 4:5).
pub const HALF_W: i32 = render::WIDTH as i32 * UNIT / 2;
pub const HALF_H: i32 = render::HEIGHT as i32 * UNIT / 2;
/// Most segments kept per frame.
const MAX_SEGMENTS: usize = 65_536;
/// The phosphor: a slightly blue white.
pub const PHOSPHOR: Rgb8 = Rgb8 {
    r: 230,
    g: 240,
    b: 255,
};

/// The 3D Imager's filters as the eye sees the phosphor through them.
pub const FILTER_RED: Rgb8 = Rgb8 {
    r: 255,
    g: 60,
    b: 50,
};
pub const FILTER_GREEN: Rgb8 = Rgb8 {
    r: 60,
    g: 255,
    b: 90,
};
pub const FILTER_BLUE: Rgb8 = Rgb8 {
    r: 90,
    g: 110,
    b: 255,
};

/// Which eye of the 3D Imager the picture shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eye {
    Left,
    Right,
    Both,
}

/// Machine keys: each controller's buttons 1–4, then up, down, left,
/// right.
pub mod keys {
    pub const PORT1: u16 = 16;
    pub const PORT2: u16 = 32;
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "vectrex",
        name: "Vectrex",
        models: &["vectrex"],
        create: |config| Ok(Box::new(Vectrex::new(config)?)),
    }
}

fn port_option(
    config: &MachineConfig,
    key: &str,
    pen: bool,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    let kind = match config.options.get(key).map(String::as_str) {
        None | Some("controller") | Some("joystick") => VectrexKind::Controller,
        Some("lightpen") if pen => VectrexKind::LightPen,
        Some("imager") if pen => return Ok(Some(Box::new(Imager::new(u64::from(CPU_HZ))))),
        Some("none") => return Ok(None),
        Some(other) => {
            let choices = if pen {
                "controller, lightpen, imager or none"
            } else {
                "controller or none"
            };
            return Err(ConfigError::Invalid(format!(
                "{key} option '{other}' ({choices})"
            )));
        }
    };
    Ok(Some(Box::new(VectrexController::new(kind))))
}

/// A segment the beam is drawing now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Open {
    x0: i32,
    y0: i32,
    vx: i32,
    vy: i32,
    z: u8,
    color: Rgb8,
    cycles: u32,
}

/// Integrators, sample/holds and the beam's path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analog {
    /// Beam position in integrator units (+X right, +Y up, 0 = centre).
    pub x: i32,
    pub y: i32,
    pub ysh: i8,
    pub zero_ref: i8,
    pub z: i8,
    pub sound: i8,
    /// The beam is visible now.
    pub lit: bool,
    open: Option<Open>,
    /// Segments of the frame being drawn and of the last complete one.
    segments: Vec<Segment>,
    done: Vec<Segment>,
}

impl Analog {
    /// Close the open segment (if any) into the frame's list.
    fn close(&mut self) {
        if let Some(o) = self.open.take() {
            if self.segments.len() < MAX_SEGMENTS && o.cycles > 0 {
                let c = o.cycles as i32;
                let x1 = o.x0.saturating_add(o.vx.saturating_mul(c));
                let y1 = o.y0.saturating_add(o.vy.saturating_mul(c));
                self.segments.push(Segment {
                    x0: o.x0 + HALF_W,
                    y0: HALF_H - o.y0,
                    x1: x1 + HALF_W,
                    y1: HALF_H - y1,
                    intensity: o.z,
                    color: o.color,
                    cycles: o.cycles,
                });
            }
        }
    }

    /// One cycle with the DAC at `dac`, port B at `pb`, /ZERO and /BLANK;
    /// `color` is what the viewer sees of the phosphor (`None`: hidden by
    /// the 3D Imager's disc).
    fn step(&mut self, dac: i8, pb: u8, zero: bool, blank: bool, color: Option<Rgb8>) {
        if pb & 1 == 0 {
            match (pb >> 1) & 3 {
                0 => self.ysh = dac,
                1 => self.zero_ref = dac,
                2 => self.z = dac,
                _ => self.sound = dac,
            }
        }
        if zero {
            self.x = 0;
            self.y = 0;
        }
        let ramp = pb & 0x80 == 0 && !zero;
        let (vx, vy) = if ramp {
            (
                i32::from(dac) - i32::from(self.zero_ref),
                i32::from(self.ysh) - i32::from(self.zero_ref),
            )
        } else {
            (0, 0)
        };
        self.lit = !blank && self.z > 0;
        let z = (self.z.max(0) as u8) << 1;
        let seen = color.filter(|_| self.lit);
        match self.open {
            Some(ref mut o) if seen == Some(o.color) && o.vx == vx && o.vy == vy && o.z == z => {
                o.cycles += 1;
            }
            _ => {
                self.close();
                if let Some(color) = seen {
                    self.open = Some(Open {
                        x0: self.x,
                        y0: self.y,
                        vx,
                        vy,
                        z,
                        color,
                        cycles: 1,
                    });
                }
            }
        }
        // Far outside the screen the integrators saturate.
        self.x = (self.x + vx).clamp(-4 * HALF_W, 4 * HALF_W);
        self.y = (self.y + vy).clamp(-4 * HALF_H, 4 * HALF_H);
    }

    /// End of a frame: the open segment continues in the next one.
    fn end_frame(&mut self) {
        let open = self.open;
        self.close();
        if let Some(o) = open {
            self.open = Some(Open {
                x0: self.x,
                y0: self.y,
                cycles: 0,
                ..o
            });
        }
        std::mem::swap(&mut self.segments, &mut self.done);
        self.segments.clear();
    }

    /// Where the beam is in the raster picture, in pixels.
    fn pixel(&self) -> (i32, i32) {
        ((self.x + HALF_W) / UNIT, (HALF_H - self.y) / UNIT)
    }
}

/// Everything on the bus except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    rom: Vec<u8>,
    ram: Vec<u8>,
    via: Via,
    ay: Ay,
    cart: Option<VectrexCart>,
    analog: Analog,
    /// BDIR/BC1 and the data of the last AY bus cycle.
    ay_bus: (u8, u8),
    /// Cycles since power-on and the start of the current frame.
    cycle: u64,
    frame_start: u64,
    frames: u64,
    frame_done: bool,
    /// Port 2's button 4 / light pen line as CA1 saw it last.
    ca1: bool,
    /// Pin 2 of port 2 as last reported to the device (the Imager's motor).
    pin2: bool,
    eye: Eye,
    /// How the raster picture draws the beam (option `lines`).
    style: render::Style,
    audio: Decimator,
    dc: [i32; 2],
    samples: Vec<i16>,
    /// The rasterised last frame and scratch space for drawing it.
    pixels: Vec<Rgb8>,
    energy: Vec<[f32; 3]>,
}

/// The potentiometer value (−128…127) of a stick axis.
fn pot(v: u16) -> i32 {
    i32::from(v >> 8) - 128
}

fn lines(ports: &[Option<Box<dyn PortDevice>>; 2], n: usize, now: Cycle) -> PortLines {
    ports[n].as_ref().map_or(
        PortLines {
            pulled_low: 0,
            analog: [CENTRE; 2],
        },
        |d| d.read_lines(now),
    )
}

impl Board {
    /// What the buttons put on the AY's port A (low = pressed), and
    /// whether port 2's button 4 line is low (button or light pen).
    fn buttons(&self, ports: &[Option<Box<dyn PortDevice>>; 2]) -> (u8, bool) {
        let now = Cycle(self.cycle);
        let mut v = 0xff;
        let mut b4 = false;
        for n in 0..2 {
            let l = lines(ports, n, now);
            let mut low = l.pulled_low & pin::BUTTONS;
            if l.pulled_low & pin::PHOTOCELL != 0 && self.pen_sees_beam(l.analog) {
                low |= pin::BUTTON4;
            }
            v &= !((low as u8) << (4 * n));
            if n == 1 {
                b4 = low & pin::BUTTON4 != 0;
            }
        }
        (v, b4)
    }

    /// The light pen sees the beam while it is lit within two pixels.
    fn pen_sees_beam(&self, aim: [u16; 2]) -> bool {
        if aim[0] == NO_AIM || !self.analog.lit {
            return false;
        }
        let (x, y) = self.analog.pixel();
        (x - i32::from(aim[0])).abs() <= 2 && (y - i32::from(aim[1])).abs() <= 2
    }

    /// The comparator (PB5): DAC above the selected potentiometer.
    ///
    /// The potentiometers give the lower voltage for right and up (checked
    /// with Scramble: both axes were reversed the other way, user report
    /// 2026-10-24).
    fn comparator(&self, ports: &[Option<Box<dyn PortDevice>>; 2]) -> bool {
        let pb = self.via.port_b_out();
        let sel = usize::from((pb >> 1) & 3);
        let l = lines(ports, sel >> 1, Cycle(self.cycle));
        let dac = i32::from(self.via.port_a_out() as i8);
        dac > -1 - pot(l.analog[sel & 1])
    }

    /// Port pins as the VIA reads them.
    fn via_pins(&self, ports: &[Option<Box<dyn PortDevice>>; 2]) -> (u8, u8) {
        let pb = self.via.port_b_out();
        let function = (pb >> 3) & 3;
        let pa = if function == 1 {
            self.ay.read(self.buttons(ports).0)
        } else {
            0xff
        };
        let cmp = if self.comparator(ports) { 0x20 } else { 0 };
        (pa, 0xdf | cmp)
    }

    /// What the viewer sees of the phosphor now: through the 3D Imager's
    /// filter in front of the screen, `None` while it covers the eye not
    /// shown.
    fn filter(&self, ports: &[Option<Box<dyn PortDevice>>; 2]) -> Option<Rgb8> {
        let l = lines(ports, 1, Cycle(self.cycle));
        if l.pulled_low & pin::IMAGER == 0 {
            return Some(PHOSPHOR);
        }
        let (right, color) = match (u32::from(l.analog[0]) * 6) >> 16 {
            0 => (true, FILTER_BLUE),
            1 => (true, FILTER_GREEN),
            2 => (true, FILTER_RED),
            3 => (false, FILTER_BLUE),
            4 => (false, FILTER_GREEN),
            _ => (false, FILTER_RED),
        };
        match self.eye {
            Eye::Both => Some(color),
            Eye::Left => (!right).then_some(color),
            Eye::Right => right.then_some(color),
        }
    }

    /// One cycle (1.5 MHz) of everything but the CPU.
    fn tick(&mut self, ports: &mut [Option<Box<dyn PortDevice>>; 2]) {
        self.via.tick();
        let pa = self.via.port_a_out();
        let pb = self.via.port_b_out();
        let (buttons, b4) = self.buttons(ports);
        // CA1: high unless button 4 of controller 2 is held.
        if b4 == self.ca1 {
            self.via.set_ca1(!b4, pa);
            self.ca1 = !b4;
        }
        let function = (pb >> 3) & 3;
        if function >= 2 && (function, pa) != self.ay_bus {
            self.ay.bus_write(function, pa);
        }
        self.ay_bus = (function, pa);
        self.ay.tick();
        if let Some(c) = &mut self.cart {
            c.pb6 = pb & 0x40 != 0;
        }
        let _ = buttons;
        // Pin 2 of port 2: an input floats high, an output follows bit 5.
        let pin2 = self.ay.regs[7] & 0x40 == 0 || self.ay.regs[14] & 0x20 != 0;
        if pin2 != self.pin2 || self.cycle.is_multiple_of(64) {
            self.pin2 = pin2;
            if let Some(d) = &mut ports[1] {
                let lines = PortLines {
                    pulled_low: if pin2 { pin::PIN2_HIGH } else { 0 },
                    analog: [0; 2],
                };
                d.write_lines(lines, Cycle(self.cycle));
            }
        }
        let zero = !self.via.ca2_out();
        let blank = !self.via.cb2_out();
        let color = self.filter(ports);
        self.analog.step(pa as i8, pb, zero, blank, color);
        self.sound();
        self.cycle += 1;
        if self.cycle - self.frame_start >= FRAME_CYCLES {
            self.frame_start = self.cycle;
            self.frames += 1;
            self.analog.end_frame();
            render::draw(
                &self.analog.done,
                UNIT,
                self.style,
                &mut self.energy,
                &mut self.pixels,
            );
            self.frame_done = true;
        }
    }

    fn sound(&mut self) {
        let [a, b, c] = self.ay.output();
        let ay = (i32::from(a) + i32::from(b) + i32::from(c)) * 2 / 3;
        let level = ay - 10_000 + i32::from(self.analog.sound) * 64;
        if let Some(s) = self.audio.push(level) {
            // A first-order high-pass (the amplifier's AC coupling).
            let x = i32::from(s) << 16;
            let y = x - self.dc[0] + ((i64::from(self.dc[1]) * 65_208) >> 16) as i32;
            self.dc = [x, y];
            self.samples
                .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        }
    }

    fn peek(&self, addr: u16, ports: Option<&[Option<Box<dyn PortDevice>>; 2]>) -> u8 {
        match addr {
            0x0000..=0x7fff => self.cart.as_ref().map_or(0xff, |c| c.peek(addr)),
            0xc800..=0xcfff => self.ram[usize::from(addr & 0x3ff)],
            0xd000..=0xdfff => {
                let (pa, pb) = ports.map_or((0xff, 0xff), |p| self.via_pins(p));
                self.via.peek(addr as u8 & 15, pa, pb)
            }
            0xe000..=0xffff => self.rom[usize::from(addr & 0x1fff)],
            _ => 0xff,
        }
    }
}

/// The 6809's bus for one instruction.
struct VxBus<'a> {
    b: &'a mut Board,
    ports: &'a mut [Option<Box<dyn PortDevice>>; 2],
    dbg: Option<&'a mut DebugHooks>,
}

impl M6809Bus for VxBus<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        self.b.tick(self.ports);
        let v = match addr {
            0xd000..=0xdfff => {
                let (pa, pb) = self.b.via_pins(self.ports);
                self.b.via.read(addr as u8 & 15, pa, pb)
            }
            _ => self.b.peek(addr, None),
        };
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let a = u32::from(addr);
                let seen = dbg.on_read(CPU_SPACE, a, Size::Byte, u32::from(v));
                dbg.trace_read(CPU_SPACE, a, Size::Byte, seen, beam(self.b));
                seen as u8
            }
            _ => v,
        }
    }

    fn write(&mut self, addr: u16, v: u8) {
        self.b.tick(self.ports);
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                let a = u32::from(addr);
                dbg.trace_write(CPU_SPACE, a, Size::Byte, u32::from(v), beam(self.b));
                if !dbg.on_write(CPU_SPACE, a, Size::Byte, u32::from(v)) {
                    return;
                }
            }
        }
        match addr {
            0xc800..=0xcfff => self.b.ram[usize::from(addr & 0x3ff)] = v,
            0xd000..=0xd7ff => self.b.via.write(addr as u8 & 15, v),
            0xd800..=0xdfff => {
                self.b.via.write(addr as u8 & 15, v);
                self.b.ram[usize::from(addr & 0x3ff)] = v;
            }
            _ => {}
        }
    }

    fn idle(&mut self) {
        self.b.tick(self.ports);
    }

    fn peek(&self, addr: u16) -> u8 {
        self.b.peek(addr, Some(self.ports))
    }
}

struct PeekBus<'a>(&'a Board);

impl M6809Bus for PeekBus<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        self.0.peek(addr, None)
    }
    fn write(&mut self, _addr: u16, _v: u8) {}
    fn idle(&mut self) {}
    fn peek(&self, addr: u16) -> u8 {
        self.0.peek(addr, None)
    }
}

/// The time in the current frame, for the debugger's trace (the Vectrex
/// has no raster: line 0, cycle = cycles into the frame).
fn beam(b: &Board) -> BeamPos {
    BeamPos {
        frame: b.frames,
        line: 0,
        cycle: (b.cycle - b.frame_start).min(u64::from(u16::MAX)) as u16,
        hpos: 0,
    }
}

pub struct Vectrex {
    cpu: M6809,
    board: Board,
    ports: [Option<Box<dyn PortDevice>>; 2],
    cart: CartSlot<VectrexCart>,
    input: InputQueue,
    frame_done_pending: bool,
}

impl Vectrex {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        match config.model.as_deref() {
            None | Some("vectrex") => {}
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        }
        let rom = match config.options.get("roms").map(String::as_str) {
            Some("stub") => stub::rom(),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => {
                let r = config
                    .roms
                    .get("vectrex")
                    .ok_or_else(|| ConfigError::MissingRom("vectrex".into()))?;
                match r.len() {
                    0x2000 => r.clone(),
                    0x1000 => [r.as_slice(), r.as_slice()].concat(),
                    n => {
                        return Err(ConfigError::Invalid(format!(
                            "vectrex system ROM must be 8192 (or 4096) bytes, got {n}"
                        )))
                    }
                }
            }
        };
        let ports = [
            port_option(config, "port1", false)?,
            port_option(config, "port2", true)?,
        ];
        let style = match config.options.get("lines").map(String::as_str) {
            None | Some("fine") => render::Style::Fine,
            Some("soft") => render::Style::Soft,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "lines option '{other}' (fine or soft)"
                )))
            }
        };
        let eye = match config.options.get("eye").map(String::as_str) {
            None | Some("left") => Eye::Left,
            Some("right") => Eye::Right,
            Some("both") => Eye::Both,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "eye option '{other}' (left, right or both)"
                )))
            }
        };
        let n = (render::WIDTH * render::HEIGHT) as usize;
        let mut m = Self {
            cpu: M6809::new(),
            board: Board {
                rom,
                ram: vec![0; 0x400],
                via: Via::new(),
                ay: Ay::new(),
                cart: None,
                analog: Analog::default(),
                ay_bus: (0, 0),
                cycle: 0,
                frame_start: 0,
                frames: 0,
                frame_done: false,
                ca1: true,
                pin2: false,
                eye,
                style,
                audio: Decimator::new(CPU_HZ, emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                dc: [0; 2],
                samples: Vec::new(),
                pixels: vec![Rgb8::default(); n],
                energy: vec![[0.0; 3]; n],
            },
            ports,
            cart: devices_cartridge::vectrex::slot(),
            input: InputQueue::new(),
            frame_done_pending: false,
        };
        m.power_on();
        Ok(m)
    }

    fn power_on(&mut self) {
        let b = &mut self.board;
        b.ram.fill(0);
        b.via.reset();
        b.ay.reset();
        b.cart = self.cart.cart().cloned();
        if let Some(c) = &mut b.cart {
            c.reset();
        }
        let a = std::mem::take(&mut b.analog);
        b.analog = Analog {
            done: a.done,
            ..Analog::default()
        };
        b.ay_bus = (0, 0);
        self.cpu = M6809::new();
        self.cpu.reset(&mut PeekBus(&self.board));
    }

    /// One instruction. Returns `Some` if emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(Cycle(self.board.cycle)) {
            self.apply_input(ev);
        }
        {
            let mut bus = VxBus {
                b: &mut self.board,
                ports: &mut self.ports,
                dbg: dbg.as_deref_mut(),
            };
            self.cpu.step(&mut bus);
        }
        self.cpu.irq = self.board.via.irq();
        let frame = std::mem::take(&mut self.board.frame_done);
        if let Some(dbg) = dbg {
            let next_pc = u32::from(self.cpu.pc);
            if dbg.trace_instr_armed() {
                dbg.trace_instr(CPU, next_pc, beam(&self.board));
            }
            if dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.exec_armed(CPU_SPACE) {
                dbg.check_exec(CPU_SPACE, next_pc);
            }
            if let Some(stop) = dbg.take_stop() {
                self.frame_done_pending |= frame;
                return Some(stop);
            }
        }
        frame.then_some(StopReason::FrameDone)
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
                    let (port, k) = match machine_key.0 {
                        k @ 16..=23 => (0, k - keys::PORT1),
                        k @ 32..=39 => (1, k - keys::PORT2),
                        _ => return,
                    };
                    if let Some(dev) = &mut self.ports[port] {
                        dev.input(&InputEvent::Key {
                            machine_key: MachineKey(k),
                            down,
                        });
                    }
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

    /// The analog state (tests and tools).
    pub fn analog(&self) -> &Analog {
        &self.board.analog
    }

    /// Read a byte as the CPU sees it (tests and tools).
    pub fn peek_cpu(&self, addr: u16) -> u8 {
        self.board.peek(addr, Some(&self.ports))
    }

    pub fn cpu(&self) -> &M6809 {
        &self.cpu
    }

    fn signal(&self) -> SignalInfo {
        // The raster picture is a virtual display: one picture per frame
        // of FRAME_CYCLES (50 Hz), which frontends derive from these.
        SignalInfo {
            pixel_clock_hz: render::WIDTH * render::HEIGHT * (CPU_HZ / FRAME_CYCLES as u32),
            active_pixels: render::WIDTH,
            total_pixels: render::WIDTH,
            lines_per_field: render::HEIGHT,
            standard: VideoStandard::Pal,
            output: OutputType::Monochrome,
            field: FieldParity::Progressive,
        }
    }
}

impl Machine for Vectrex {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.frame_done_pending) {
            return StopReason::FrameDone;
        }
        for f in dbg.per_frame_freezes() {
            if f.space == CPU_SPACE {
                for i in 0..f.size.bytes() {
                    let a = (f.addr + i) as u16;
                    if (0xc800..0xd000).contains(&a) {
                        self.board.ram[usize::from(a & 0x3ff)] = (f.value >> (8 * i)) as u8;
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

    /// Hard: power cycle. Soft: the reset button (CPU, VIA and AY; RAM
    /// stays, so the executive skips its title).
    fn reset(&mut self, kind: ResetKind) {
        if kind == ResetKind::Hard {
            self.power_on();
        } else {
            self.board.via.reset();
            self.board.ay.reset();
            self.cpu.reset(&mut PeekBus(&self.board));
        }
        self.input.clear();
        self.frame_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: render::WIDTH,
            height: render::HEIGHT,
            pixels: PixelData::Rgb(&self.board.pixels),
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn vectors(&self) -> Option<VectorFrame<'_>> {
        Some(VectorFrame {
            width: render::WIDTH * UNIT as u32,
            height: render::HEIGHT * UNIT as u32,
            segments: &self.board.analog.done,
            phosphor: PHOSPHOR,
            cycle_hz: CPU_HZ,
            frame: self.board.frames,
        })
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        None
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 16,
            big_endian: true,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let v = &self.board.via;
        const VIA: [&str; 16] = [
            "ORB", "ORA", "DDRB", "DDRA", "T1CL", "T1CH", "T1LL", "T1LH", "T2CL", "T2CH", "SR",
            "ACR", "PCR", "IFR", "IER", "ORA_NH",
        ];
        let via_regs = VIA
            .iter()
            .enumerate()
            .map(|(i, n)| ChipReg {
                name: n,
                addr: Some(0xd000 + i as u32),
                value: u32::from(v.peek(i as u8, 0xff, 0xff)),
                bits: 8,
            })
            .collect();
        const AY: [&str; 16] = [
            "AFINE", "ACOARSE", "BFINE", "BCOARSE", "CFINE", "CCOARSE", "NOISE", "MIXER", "AVOL",
            "BVOL", "CVOL", "EFINE", "ECOARSE", "ESHAPE", "PORTA", "PORTB",
        ];
        let ay_regs = AY
            .iter()
            .zip(self.board.ay.regs)
            .map(|(n, val)| reg(*n, u32::from(val), 8))
            .collect();
        let a = &self.board.analog;
        vec![
            ChipInfo {
                name: "via",
                regs: via_regs,
            },
            ChipInfo {
                name: "ay",
                regs: ay_regs,
            },
            ChipInfo {
                name: "analog",
                regs: vec![
                    reg("X", a.x as u32, 32),
                    reg("Y", a.y as u32, 32),
                    reg("YSH", u32::from(a.ysh as u8), 8),
                    reg("ZERO_REF", u32::from(a.zero_ref as u8), 8),
                    reg("Z", u32::from(a.z as u8), 8),
                    reg("LIT", u32::from(a.lit), 1),
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
        Cycle(self.board.cycle)
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
                kind: PortKind::VectrexControl,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::VectrexControl {
            return Err(PortError::Incompatible {
                port: PortKind::VectrexControl,
                device: dev.kind(),
            });
        }
        if slot.is_some() {
            return Err(PortError::Occupied(port));
        }
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
        let a = &b.analog;
        w.chunk(*b"VECX", 1, |w| {
            w.bool(self.frame_done_pending);
            w.bytes(&b.ram);
            w.u8(b.ay_bus.0);
            w.u8(b.ay_bus.1);
            for v in [b.cycle, b.frame_start, b.frames] {
                w.u64(v);
            }
            w.bool(b.frame_done);
            w.bool(b.ca1);
            w.bool(b.pin2);
            b.audio.save(w);
            w.u32(b.dc[0] as u32);
            w.u32(b.dc[1] as u32);
            w.bool(b.cart.is_some());
            w.u32(a.x as u32);
            w.u32(a.y as u32);
            for v in [a.ysh, a.zero_ref, a.z, a.sound] {
                w.u8(v as u8);
            }
            w.bool(a.lit);
            match a.open {
                Some(o) => {
                    w.bool(true);
                    for v in [o.x0, o.y0, o.vx, o.vy] {
                        w.u32(v as u32);
                    }
                    w.u8(o.z);
                    for v in [o.color.r, o.color.g, o.color.b] {
                        w.u8(v);
                    }
                    w.u32(o.cycles);
                }
                None => w.bool(false),
            }
            for list in [&a.segments, &a.done] {
                w.u32(list.len() as u32);
                for s in list.iter() {
                    for v in [s.x0, s.y0, s.x1, s.y1] {
                        w.u32(v as u32);
                    }
                    w.u8(s.intensity);
                    for v in [s.color.r, s.color.g, s.color.b] {
                        w.u8(v);
                    }
                    w.u32(s.cycles);
                }
            }
        });
        self.cpu.save(&mut w);
        b.via.save(&mut w);
        b.ay.save(&mut w);
        if let Some(c) = &b.cart {
            c.save(&mut w);
        }
        self.input.save(&mut w);
        for (i, tag) in [*b"VXP1", *b"VXP2"].into_iter().enumerate() {
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
        let (_, mut c) = r.chunk(*b"VECX", 1)?;
        let frame_done_pending = c.bool()?;
        let mut b = self.board.clone();
        let ram = c.bytes()?;
        if ram.len() != b.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        b.ram = ram.to_vec();
        b.ay_bus = (c.u8()?, c.u8()?);
        b.cycle = c.u64()?;
        b.frame_start = c.u64()?;
        b.frames = c.u64()?;
        if b.frame_start > b.cycle {
            return Err(StateError::Invalid("frame start"));
        }
        b.frame_done = c.bool()?;
        b.ca1 = c.bool()?;
        b.pin2 = c.bool()?;
        b.audio.load(&mut c)?;
        b.dc = [c.u32()? as i32, c.u32()? as i32];
        if c.bool()? != b.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        let a = &mut b.analog;
        a.x = c.u32()? as i32;
        a.y = c.u32()? as i32;
        a.ysh = c.u8()? as i8;
        a.zero_ref = c.u8()? as i8;
        a.z = c.u8()? as i8;
        a.sound = c.u8()? as i8;
        a.lit = c.bool()?;
        a.open = if c.bool()? {
            Some(Open {
                x0: c.u32()? as i32,
                y0: c.u32()? as i32,
                vx: c.u32()? as i32,
                vy: c.u32()? as i32,
                z: c.u8()?,
                color: Rgb8 {
                    r: c.u8()?,
                    g: c.u8()?,
                    b: c.u8()?,
                },
                cycles: c.u32()?,
            })
        } else {
            None
        };
        for n in 0..2 {
            let len = c.u32()? as usize;
            if len > MAX_SEGMENTS {
                return Err(StateError::Invalid("segment count"));
            }
            let mut list = Vec::with_capacity(len);
            for _ in 0..len {
                list.push(Segment {
                    x0: c.u32()? as i32,
                    y0: c.u32()? as i32,
                    x1: c.u32()? as i32,
                    y1: c.u32()? as i32,
                    intensity: c.u8()?,
                    color: Rgb8 {
                        r: c.u8()?,
                        g: c.u8()?,
                        b: c.u8()?,
                    },
                    cycles: c.u32()?,
                });
            }
            if n == 0 {
                a.segments = list;
            } else {
                a.done = list;
            }
        }
        b.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        b.via.load(&mut r)?;
        b.ay.load(&mut r)?;
        if let Some(cart) = &mut b.cart {
            cart.load(&mut r)?;
        }
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"VXP1", *b"VXP2"] {
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
        render::draw(&b.analog.done, UNIT, b.style, &mut b.energy, &mut b.pixels);
        self.cpu = cpu;
        self.board = b;
        self.input = input;
        self.frame_done_pending = frame_done_pending;
        Ok(())
    }
}

impl DebugTarget for Vectrex {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    /// Big-endian like the 6809.
    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != CPU_SPACE {
            return None;
        }
        Some((0..size.bytes()).fold(0, |v, i| {
            v << 8
                | u32::from(
                    self.board
                        .peek(addr.wrapping_add(i) as u16, Some(&self.ports)),
                )
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        let n = size.bytes();
        for i in 0..n {
            let a = addr.wrapping_add(i) as u16;
            let byte = (v >> (8 * (n - 1 - i))) as u8;
            match a {
                0xc800..=0xcfff => self.board.ram[usize::from(a & 0x3ff)] = byte,
                0xd000..=0xdfff => self.board.via.write(a as u8 & 15, byte),
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        cpu_6809::disassemble(&PeekBus(&self.board), addr as u16)
    }
}

#[cfg(test)]
mod tests;
