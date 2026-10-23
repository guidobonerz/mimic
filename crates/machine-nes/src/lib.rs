//! Nintendo Entertainment System / Famicom (NTSC and PAL).
//!
//! 2A03 (PAL: 2A07) at master clock ÷ 12 (÷ 16): the `cpu-6502` core
//! without decimal mode plus the APU, OAM DMA and the controller strobe;
//! the 2C02 (PAL: 2C07) PPU at master clock ÷ 4 (÷ 5), i.e. three PPU dots
//! a CPU cycle (PAL 3.2); 2 KB work RAM, 2 KB VRAM for the nametables, two
//! controller ports, the cartridge (`devices_cartridge::nes`).
//!
//! **CPU memory.** $0000–$1FFF the 2 KB RAM, repeated; $2000–$3FFF the PPU
//! registers, repeated every 8 bytes; $4000–$4013 and $4015, $4017 the
//! APU; $4014 OAM DMA; $4016 controller strobe (write) and port 1 (read),
//! $4017 port 2 (read); $4018–$401F unused; $4020–$FFFF the cartridge.
//! Reads nothing drives return the last value on the data bus (open bus;
//! a $4015 read does not change it); controller reads drive bits 0–4 only.
//!
//! **PPU memory.** $0000–$1FFF the cartridge's pattern tables, $2000–$2FFF
//! (and $3000–$3EFF) the nametables, placed by the cartridge in the
//! console's 2 KB or its own VRAM.
//!
//! **Clocking.** One CPU cycle: two of the PPU's dots of that cycle, one
//! APU cycle, the CPU's bus access, the remaining dot (PAL: one or two);
//! then the CPU samples /NMI and /IRQ for that cycle's interrupt poll.
//! NMI is the PPU's output; IRQ the APU's (frame counter, DMC) or the
//! cartridge's (MMC3). The cartridge's /IRQ is taken as it is at the bus
//! access: the MMC3 works with the CPU clock (M2), and with a pattern
//! address on the PPU bus a dot before its read this puts the scanline
//! interrupt where blargg's mmc3_test_2 4-scanline_timing wants it.
//!
//! **DMA.** A write to $4014 copies page `v` to OAM: the CPU stops on its
//! next read cycle (the halt cycle), one more cycle if that lands on a
//! put cycle, then 256 reads (get cycles) and writes to $2004 (put
//! cycles): 513 or 514 cycles. The DMC's sample fetches stop the CPU the
//! same way (halt cycle, a dummy cycle, the read on a get cycle): a load
//! fetch (after a $4015 write) halts on a get cycle and takes 3 cycles, a
//! reload fetch (the sample buffer emptied) halts on a put cycle and takes
//! 4 (NESdev wiki, "DMA"). During OAM DMA a DMC read takes a get cycle
//! from it. Get cycles are the even CPU cycles since
//! power-on (this project's choice of the alignment; blargg's
//! sprdma_and_dmc_dma and cpu_interrupts_v2 4-irq_and_dma agree with it).
//!
//! **Controllers** (`devices_input::nes`): writes to $4016 bit 0 drive OUT
//! on both ports; a read of $4016/$4017 returns D0, D3, D4 in bits 0, 3, 4
//! and pulses CLK. A Zapper's light sensor sees light while the beam is
//! up to [`ZAPPER_LINES`] lines below the point it aims at and that point
//! (or a pixel next to it) is bright in the picture just drawn.
//! With `fourscore=on` the Four Score adapter adds ports 3 and 4: $4016
//! reads pad 1, then pad 3, then the signature %00010000 (bit 20 set),
//! $4017 pad 2, pad 4 and %00100000 (bit 19); then 1s.
//!
//! Models: "ntsc" (default) and "pal". Options:
//! - `port1`, `port2`: "controller" (default), "zapper", "arkanoid" or
//!   "none".
//! - `fourscore`: "on" adds ports 3 and 4 with controllers (ports 1 and
//!   2 must hold controllers then).
//!
//! Media slot "cartridge": iNES / NES 2.0 images. Inserting or removing
//! one powers the console up again. Without a cartridge the CPU does not
//! run.
//!
//! **Keyboard** (machine keys, mapped by the host keymap): controller 1's
//! A, B, Select, Start = 0–3, controller 2's = 8–11.
//!
//! No system ROM. Sources: NESdev wiki, "CPU memory map", "PPU memory
//! map", "Cycle reference chart" (clocks), "DMA", "Four Score", "Zapper"
//! (read 2026-10-23). The implementation is this project's own.

#![forbid(unsafe_code)]

use chips_nintendo::ppu::{self, PpuBus};
use chips_nintendo::{Apu, ApuModel, Ppu, PpuModel};
use cpu_6502::{Config, Cpu6502, Variant};
use devices_cartridge::nes::{Nametable, NesCart};
use devices_cartridge::CartSlot;
use devices_input::nes::{pin, NO_AIM};
use devices_input::{NesController, NesKind};
use emu_core::audio::Decimator;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::input::{InputEvent, InputQueue, InputTarget, MachineKey, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The PPU's 14-bit address space (pattern tables, nametables, palette).
pub const PPU_SPACE: MemSpaceId = MemSpaceId(1);
/// The 256 bytes of sprite memory.
pub const OAM_SPACE: MemSpaceId = MemSpaceId(2);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);

pub const MASTER_HZ_NTSC: u32 = 21_477_272;
pub const MASTER_HZ_PAL: u32 = 26_601_712;
/// PPU dots of a CPU cycle that run before the CPU's bus access (the rest
/// run after it). Two, from blargg's ppu_vbl_nmi tests: a PPUSTATUS read
/// up to two dots after the flag rises sees it and suppresses the NMI.
const DOTS_BEFORE_ACCESS: u32 = 2;
/// Lines below its aim during which a Zapper sees a bright point.
pub const ZAPPER_LINES: u16 = 20;

/// Machine keys.
pub mod keys {
    /// Controller 1's A, B, Select, Start.
    pub const PAD1: u16 = 0;
    /// Controller 2's.
    pub const PAD2: u16 = 8;
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "nes",
        name: "Nintendo Entertainment System",
        models: &["ntsc", "pal"],
        create: |config| Ok(Box::new(Nes::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Ntsc,
    Pal,
}

impl Model {
    /// Master clocks per CPU cycle and per PPU dot.
    fn dividers(self) -> (u32, u32) {
        match self {
            Model::Ntsc => (12, 4),
            Model::Pal => (16, 5),
        }
    }

    pub fn cpu_hz(self) -> u32 {
        match self {
            Model::Ntsc => MASTER_HZ_NTSC / 12,
            Model::Pal => MASTER_HZ_PAL / 16,
        }
    }
}

fn port_option(config: &MachineConfig, key: &str) -> Result<Option<NesKind>, ConfigError> {
    Ok(match config.options.get(key).map(String::as_str) {
        None | Some("controller") => Some(NesKind::Standard),
        Some("zapper") => Some(NesKind::Zapper),
        Some("arkanoid") => Some(NesKind::Arkanoid),
        Some("none") => None,
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "{key} option '{other}' (controller, zapper, arkanoid or none)"
            )))
        }
    })
}

/// The DMA unit's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Dma {
    /// OAM DMA: page, and the step (0–511: even = read, odd = write).
    oam: Option<(u8, u16)>,
    /// The byte the last OAM read got.
    oam_byte: u8,
    /// The DMC wants a byte: its halt and dummy cycles passed (0–2).
    dmc: Option<u8>,
    /// The CPU has given up the bus (the halt cycle is over).
    halted: bool,
}

impl Dma {
    fn active(&self) -> bool {
        self.oam.is_some() || self.dmc.is_some()
    }
}

/// Everything on the buses except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    ram: Vec<u8>,
    ciram: Vec<u8>,
    ppu: Ppu,
    apu: Apu,
    cart: Option<NesCart>,
    open_bus: u8,
    strobe: bool,
    /// Four Score: reads of each port since the strobe.
    reads: [u8; 2],
    dma: Dma,
    /// CPU cycles and PPU dots since power-on.
    cycle: u64,
    dots: u64,
    /// Master clocks the PPU is behind the CPU.
    master: u32,
    field_done: bool,
    audio: Decimator,
    /// DC blocker: last input and output (16.16).
    dc: [i32; 2],
    samples: Vec<i16>,
}

/// The PPU's view of the cartridge and the console's VRAM.
struct PpuView<'a> {
    cart: Option<&'a mut NesCart>,
    ciram: &'a mut [u8],
    dots: u64,
}

impl PpuBus for PpuView<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        if let Some(c) = self.cart.as_deref_mut() {
            c.ppu_address(addr, self.dots);
        }
        self.peek(addr)
    }

    fn write(&mut self, addr: u16, v: u8) {
        if let Some(c) = self.cart.as_deref_mut() {
            c.ppu_address(addr, self.dots);
        }
        poke_ppu(self.cart.as_deref_mut(), self.ciram, addr, v);
    }

    fn peek(&self, addr: u16) -> u8 {
        peek_ppu(self.cart.as_deref(), self.ciram, addr)
    }

    fn set_address(&mut self, addr: u16) {
        if let Some(c) = self.cart.as_deref_mut() {
            c.ppu_address(addr, self.dots);
        }
    }
}

/// A write to the PPU address space below $3F00, without A12 tracking.
fn poke_ppu(cart: Option<&mut NesCart>, ciram: &mut [u8], addr: u16, v: u8) {
    let addr = addr & 0x3fff;
    let Some(c) = cart else {
        if addr >= 0x2000 {
            ciram[usize::from(addr & 0x7ff)] = v;
        }
        return;
    };
    if addr < 0x2000 {
        c.chr_write(addr, v);
    } else {
        match c.nametable(0x2000 | (addr & 0x0fff)) {
            Nametable::Console(o) => ciram[usize::from(o)] = v,
            Nametable::Cart(o) => c.vram_write(o, v),
        }
    }
}

/// The PPU address space below $3F00 without side effects.
fn peek_ppu(cart: Option<&NesCart>, ciram: &[u8], addr: u16) -> u8 {
    let addr = addr & 0x3fff;
    match cart {
        None => {
            if addr >= 0x2000 {
                ciram[usize::from(addr & 0x7ff)]
            } else {
                0
            }
        }
        Some(c) if addr < 0x2000 => c.chr_peek(addr),
        Some(c) => match c.nametable(0x2000 | (addr & 0x0fff)) {
            Nametable::Console(o) => ciram[usize::from(o)],
            Nametable::Cart(o) => c.vram_peek(o),
        },
    }
}

impl Board {
    fn ppu_view(&mut self) -> (&mut Ppu, PpuView<'_>) {
        (
            &mut self.ppu,
            PpuView {
                cart: self.cart.as_mut(),
                ciram: &mut self.ciram,
                dots: self.dots,
            },
        )
    }

    /// Run up to `max` of the PPU dots due.
    fn run_ppu(&mut self, model: Model, max: u32) {
        let dot_div = model.dividers().1;
        let mut n = 0;
        while self.master >= dot_div && n < max {
            n += 1;
            self.master -= dot_div;
            let (ppu, mut view) = self.ppu_view();
            self.field_done |= ppu.tick(&mut view);
            self.dots += 1;
        }
    }

    /// Side-effect-free CPU read.
    fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1fff => self.ram[usize::from(addr & 0x7ff)],
            0x2000..=0x3fff => self.ppu.peek(addr as u8 & 7),
            0x4015 => self.apu.peek_status() | (self.open_bus & 0x20),
            0x4000..=0x401f => self.open_bus,
            _ => self
                .cart
                .as_ref()
                .and_then(|c| c.cpu_peek(addr))
                .unwrap_or(self.open_bus),
        }
    }

    fn write(&mut self, addr: u16, v: u8, ports: &mut Ports) {
        self.open_bus = v;
        match addr {
            0x0000..=0x1fff => self.ram[usize::from(addr & 0x7ff)] = v,
            0x2000..=0x3fff => {
                let (ppu, mut view) = self.ppu_view();
                ppu.write(addr as u8 & 7, v, &mut view);
            }
            0x4014 => self.dma.oam = Some((v, 0)),
            0x4016 => {
                self.strobe = v & 1 != 0;
                if self.strobe {
                    self.reads = [0; 2];
                }
                let out = if self.strobe { pin::OUT } else { 0 };
                for d in ports.devices.iter_mut().flatten() {
                    d.write_lines(
                        PortLines {
                            pulled_low: out,
                            analog: [0; 2],
                        },
                        Cycle(self.cycle),
                    );
                }
            }
            0x4000..=0x4017 => self.apu.write(addr, v),
            0x4018..=0x401f => {}
            _ => {
                if let Some(c) = &mut self.cart {
                    c.cpu_write(addr, v, self.cycle);
                }
            }
        }
    }

    fn read(&mut self, addr: u16, ports: &mut Ports) -> u8 {
        let v = match addr {
            0x0000..=0x1fff => self.ram[usize::from(addr & 0x7ff)],
            0x2000..=0x3fff => {
                let (ppu, mut view) = self.ppu_view();
                ppu.read(addr as u8 & 7, &mut view)
            }
            0x4015 => {
                // Not driven onto the external bus: open bus unchanged.
                return self.apu.read_status() | (self.open_bus & 0x20);
            }
            0x4016 | 0x4017 => {
                let n = usize::from(addr & 1);
                (self.open_bus & 0xe0) | self.read_port(n, ports)
            }
            0x4000..=0x401f => self.open_bus,
            _ => self
                .cart
                .as_mut()
                .and_then(|c| c.cpu_read(addr))
                .unwrap_or(self.open_bus),
        };
        self.open_bus = v;
        v
    }

    /// Bits 0, 3, 4 of a controller read, then the CLK pulse.
    fn read_port(&mut self, n: usize, ports: &mut Ports) -> u8 {
        let now = Cycle(self.cycle);
        let dev = if ports.four_score {
            let r = self.reads[n];
            if !self.strobe {
                self.reads[n] = r.saturating_add(1);
            }
            match r {
                0..=7 => n,
                8..=15 => n + 2,
                _ => {
                    let signature = if n == 0 { 19 } else { 18 };
                    return u8::from(r == signature || r >= 24);
                }
            }
        } else {
            n
        };
        let Some(d) = ports.devices[dev].as_mut() else {
            return 0;
        };
        let lines = d.read_lines(now);
        let low = lines.pulled_low;
        let mut v = 0;
        if low & pin::D0 != 0 {
            v |= 1;
        }
        if low & pin::D3 != 0 {
            v |= 8;
        }
        if low & pin::D4 != 0 {
            v |= 0x10;
        }
        if low & pin::PHOTODIODE != 0 && !self.light(lines.analog) {
            v |= 8;
        }
        let out = if self.strobe { pin::OUT } else { 0 };
        for bits in [out | pin::CLK, out] {
            d.write_lines(
                PortLines {
                    pulled_low: bits,
                    analog: [0; 2],
                },
                now,
            );
        }
        v
    }

    /// Does a light sensor aimed at frame pixel (x, y) see light now?
    fn light(&self, aim: [u16; 2]) -> bool {
        let [x, y] = aim;
        if x == NO_AIM || y == NO_AIM || usize::from(y) >= ppu::HEIGHT {
            return false;
        }
        let line = self.ppu.line;
        if line < y || line >= y + ZAPPER_LINES {
            return false;
        }
        let frame = self.ppu.frame();
        let x = i32::from(x);
        (-2..=2).any(|dx| {
            let px = x + dx;
            if px < 0 || px as usize >= ppu::WIDTH {
                return false;
            }
            // On the aim line only pixels the beam has drawn count.
            if line == y && px as usize >= ppu::BORDER + usize::from(self.ppu.dot) {
                return false;
            }
            let c = frame[usize::from(y) * ppu::WIDTH + px as usize];
            u32::from(c.r) + u32::from(c.g) + u32::from(c.b) >= 3 * 0xa0
        })
    }
}

/// The controller ports (ports 3 and 4 only with the Four Score).
struct Ports {
    devices: [Option<Box<dyn PortDevice>>; 4],
    four_score: bool,
}

/// The CPU's bus for one cycle.
struct NesBus<'a> {
    b: &'a mut Board,
    ports: &'a mut Ports,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus for NesBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = u32::from(self.b.read(addr as u16, self.ports));
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
        u32::from(self.b.peek(addr as u16))
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
        self.b.write(addr as u16, v as u8, self.ports);
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.0.peek(addr as u16))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

pub struct Nes {
    model: Model,
    cpu: Cpu6502,
    board: Board,
    ports: Ports,
    cart: CartSlot<NesCart>,
    /// CPU cycles since power-on.
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
}

fn new_cpu() -> Cpu6502 {
    let mut cpu = Cpu6502::new(Config {
        variant: Variant::Rp2A03,
        // The unstable LXA ($AB) as blargg's instr_test-v5 expects of the
        // 2A03: A = X = immediate (A | $FF).
        lxa_magic: 0xff,
        ..Config::default()
    });
    cpu.reset();
    cpu
}

impl Nes {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("ntsc") => Model::Ntsc,
            Some("pal") => Model::Pal,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let four_score = match config.options.get("fourscore").map(String::as_str) {
            None | Some("off") => false,
            Some("on") => true,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "fourscore option '{other}' (on or off)"
                )))
            }
        };
        let kinds = [port_option(config, "port1")?, port_option(config, "port2")?];
        if four_score && kinds.iter().any(|k| *k != Some(NesKind::Standard)) {
            return Err(ConfigError::Invalid(
                "the Four Score takes controllers in ports 1 and 2".into(),
            ));
        }
        let dev = |k: Option<NesKind>| -> Option<Box<dyn PortDevice>> {
            k.map(|k| Box::new(NesController::new(k)) as Box<dyn PortDevice>)
        };
        let extra = four_score.then_some(NesKind::Standard);
        let devices = [dev(kinds[0]), dev(kinds[1]), dev(extra), dev(extra)];
        let (ppu_model, apu_model) = match model {
            Model::Ntsc => (PpuModel::Ntsc, ApuModel::Ntsc),
            Model::Pal => (PpuModel::Pal, ApuModel::Pal),
        };
        Ok(Self {
            model,
            cpu: new_cpu(),
            board: Board {
                ram: vec![0; 0x800],
                ciram: vec![0; 0x800],
                ppu: Ppu::new(ppu_model),
                apu: Apu::new(apu_model),
                cart: None,
                open_bus: 0,
                strobe: false,
                reads: [0; 2],
                dma: Dma::default(),
                cycle: 0,
                dots: 0,
                master: 0,
                field_done: false,
                audio: Decimator::new(model.cpu_hz(), emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                dc: [0; 2],
                samples: Vec::new(),
            },
            ports: Ports {
                devices,
                four_score,
            },
            cart: devices_cartridge::nes::slot(),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
        })
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let p = &self.board.ppu;
        BeamPos {
            frame: p.fields,
            line: p.line,
            cycle: p.dot / 3,
            hpos: p.dot,
        }
    }

    /// One DMA bus cycle while the CPU is stopped.
    ///
    /// The DMC's halt and dummy cycles do not use the bus, so during OAM
    /// DMA they overlap its transfers; its read takes a get cycle, after
    /// which OAM DMA needs a cycle to realign (NESdev wiki, "DMA").
    fn dma_cycle(&mut self) {
        let b = &mut self.board;
        // The first stopped cycle is the halt cycle (the CPU's repeated
        // read): no transfer yet.
        let halt = !b.dma.halted;
        b.dma.halted = true;
        let get = b.cycle & 1 == 0;
        if let Some(stage) = b.dma.dmc {
            if stage < 2 {
                b.dma.dmc = Some(stage + 1);
            } else if get {
                if let Some(addr) = b.apu.dmc_request() {
                    let v = b.read(addr, &mut self.ports);
                    b.apu.dmc_fill(v);
                }
                b.dma.dmc = None;
                return;
            }
        }
        if halt {
            return;
        }
        if let Some((page, step)) = b.dma.oam {
            if step & 1 == 0 {
                if get {
                    let addr = u16::from(page) << 8 | (step >> 1);
                    b.dma.oam_byte = b.read(addr, &mut self.ports);
                    b.dma.oam = Some((page, step + 1));
                }
            } else if !get {
                let v = b.dma.oam_byte;
                b.write(0x2004, v, &mut self.ports);
                b.dma.oam = (step < 511).then_some((page, step + 1));
            }
        }
    }

    /// One CPU cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        self.board.master += self.model.dividers().0;
        self.board.run_ppu(self.model, DOTS_BEFORE_ACCESS);
        self.board.apu.tick();
        // A load fetch halts on a get cycle, a reload fetch on a put cycle.
        let get = self.board.cycle & 1 == 0;
        if self.board.dma.dmc.is_none()
            && get == self.board.apu.dmc_request_is_load()
            && self.board.apu.dmc_request().is_some()
        {
            self.board.dma.dmc = Some(0);
        }
        let cart_irq = self.board.cart.as_ref().is_some_and(NesCart::irq);
        let result = if self.board.cart.is_some() {
            let dma = self.board.dma.active();
            self.cpu.set_rdy(!dma);
            let beam = self.beam_pos();
            let r = {
                let mut bus = NesBus {
                    b: &mut self.board,
                    ports: &mut self.ports,
                    dbg: dbg.as_deref_mut(),
                    beam,
                };
                self.cpu.tick(&mut bus)
            };
            if dma && self.cpu.halted() {
                self.dma_cycle();
            }
            if !self.board.dma.active() {
                self.board.dma.halted = false;
            }
            r
        } else {
            CycleResult::Busy
        };
        self.board.run_ppu(self.model, u32::MAX);
        // The interrupt inputs are sampled at the end of the cycle: a
        // PPUSTATUS read in the cycle the flag rose cancels the NMI.
        self.cpu
            .sample_interrupts(self.board.ppu.nmi(), self.board.apu.irq() || cart_irq);
        self.board.cycle += 1;
        self.sched.tick();
        let level = self.board.apu.output() - 32_768;
        if let Some(s) = self.board.audio.push(level / 2) {
            // A first-order high-pass (the console's AC coupling).
            let b = &mut self.board;
            let x = i32::from(s) << 16;
            let y = x - b.dc[0] + ((i64::from(b.dc[1]) * 65_208) >> 16) as i32;
            b.dc = [x, y];
            b.samples
                .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        }
        let field = std::mem::take(&mut self.board.field_done);
        if let Some(dbg) = dbg {
            if let CycleResult::InstrDone { next_pc } = result {
                if let Some(kind) = self.cpu.take_interrupt() {
                    if dbg.interrupt_armed(kind) {
                        dbg.check_interrupt(kind);
                    }
                }
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
                self.field_done_pending |= field;
                return Some(stop);
            }
        } else if matches!(result, CycleResult::InstrDone { .. }) {
            self.cpu.take_interrupt();
        }
        match result {
            CycleResult::Jammed => Some(StopReason::CpuJam {
                device: CPU,
                pc: u32::from(self.cpu.pc()),
            }),
            _ if field => Some(StopReason::FrameDone),
            _ => None,
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match ev.target {
            InputTarget::Port(p) => {
                if let Some(Some(dev)) = self.ports.devices.get_mut(usize::from(p.0)) {
                    dev.input(&ev.event);
                }
            }
            InputTarget::Keyboard => {
                if let InputEvent::Key { machine_key, down } = ev.event {
                    let (port, k) = match machine_key.0 {
                        k @ 0..=3 => (0, k),
                        k @ 8..=11 => (1, k - keys::PAD2),
                        _ => return,
                    };
                    if let Some(dev) = &mut self.ports.devices[port] {
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

    fn signal(&self) -> SignalInfo {
        let (standard, master) = match self.model {
            Model::Ntsc => (VideoStandard::Ntsc, MASTER_HZ_NTSC),
            Model::Pal => (VideoStandard::Pal, MASTER_HZ_PAL),
        };
        SignalInfo {
            pixel_clock_hz: master / self.model.dividers().1,
            active_pixels: ppu::WIDTH as u32,
            total_pixels: u32::from(ppu::DOTS),
            lines_per_field: u32::from(self.board.ppu.model.lines()),
            standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Nes, addr: u16) -> u8 {
    m.board.peek(addr)
}

/// The PPU (tests and tools).
pub fn ppu(m: &Nes) -> &Ppu {
    &m.board.ppu
}

/// CPU cycles since power-on (tests).
pub fn cpu_cycles(m: &Nes) -> u64 {
    m.board.cycle
}

impl Machine for Nes {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        let b = self.beam_pos();
        if b.line == 241 && b.hpos < 4 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE {
                    for i in 0..f.size.bytes() {
                        let a = (f.addr + i) as u16;
                        if a < 0x2000 {
                            self.board.ram[usize::from(a & 0x7ff)] = (f.value >> (8 * i)) as u8;
                        }
                    }
                }
            }
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

    /// Hard: power cycle (RAM, VRAM, PPU, APU and the mapper start over).
    /// Soft: the RESET button (CPU reset sequence, PPU and APU reset; RAM
    /// and the mapper keep their state).
    fn reset(&mut self, kind: ResetKind) {
        let b = &mut self.board;
        if kind == ResetKind::Hard {
            self.cpu = new_cpu();
            b.ram.fill(0);
            b.ciram.fill(0);
            b.ppu = Ppu::new(b.ppu.model);
            b.apu = Apu::new(b.apu.model);
            b.cart = self.cart.cart().cloned();
            if let Some(c) = &mut b.cart {
                c.reset();
            }
            b.open_bus = 0;
        } else {
            self.cpu.reset();
            b.ppu.reset();
            b.apu.reset();
        }
        b.dma = Dma::default();
        b.strobe = false;
        b.reads = [0; 2];
        self.cpu.set_rdy(true);
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: ppu::WIDTH as u32,
            height: ppu::HEIGHT as u32,
            pixels: PixelData::Rgb(self.board.ppu.frame()),
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
                addr_bits: 16,
                big_endian: false,
            },
            MemSpace {
                id: PPU_SPACE,
                name: "ppu",
                device: CPU,
                addr_bits: 14,
                big_endian: false,
            },
            MemSpace {
                id: OAM_SPACE,
                name: "oam",
                device: CPU,
                addr_bits: 8,
                big_endian: false,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let p = &self.board.ppu;
        let a = &self.board.apu;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let mut chips = vec![
            ChipInfo {
                name: "ppu",
                regs: vec![
                    reg("PPUCTRL", u32::from(p.ctrl), 8),
                    reg("PPUMASK", u32::from(p.mask), 8),
                    reg("PPUSTATUS", u32::from(p.peek(2)), 8),
                    reg("OAMADDR", u32::from(p.oam_addr), 8),
                    reg("V", u32::from(p.v), 15),
                    reg("T", u32::from(p.t), 15),
                    reg("X", u32::from(p.x), 3),
                    reg("W", u32::from(p.w), 1),
                    reg("LINE", u32::from(p.line), 9),
                    reg("DOT", u32::from(p.dot), 9),
                ],
            },
            ChipInfo {
                name: "apu",
                regs: {
                    let (remaining, full, bits, level) = a.dmc_state();
                    vec![
                        reg("STATUS", u32::from(a.peek_status()), 8),
                        reg("FRAME", u32::from(a.frame_mode), 8),
                        reg("DMC_LEFT", u32::from(remaining), 12),
                        reg("DMC_BUF", u32::from(full), 1),
                        reg("DMC_BITS", u32::from(bits), 4),
                        reg("DMC_LEVEL", u32::from(level), 7),
                    ]
                },
            },
        ];
        if let Some(c) = &self.board.cart {
            use devices_cartridge::nes::Mapper;
            let mut regs = vec![reg("MAPPER", u32::from(c.mapper_id), 12)];
            match &c.mapper {
                Mapper::Nrom => {}
                Mapper::Uxrom { bank } | Mapper::Cnrom { bank } => {
                    regs.push(reg("BANK", u32::from(*bank), 8));
                }
                Mapper::Mmc1(m) => regs.extend([
                    reg("CONTROL", u32::from(m.control), 5),
                    reg("CHR0", u32::from(m.chr0), 5),
                    reg("CHR1", u32::from(m.chr1), 5),
                    reg("PRG", u32::from(m.prg), 5),
                ]),
                Mapper::Mmc3(m) => {
                    const R: [&str; 8] = ["R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7"];
                    regs.push(reg("SELECT", u32::from(m.select), 8));
                    regs.extend(R.iter().zip(m.regs).map(|(n, v)| reg(n, u32::from(v), 8)));
                    regs.extend([
                        reg("IRQLATCH", u32::from(m.irq_latch), 8),
                        reg("IRQCOUNT", u32::from(m.irq_counter), 8),
                        reg("IRQEN", u32::from(m.irq_enabled), 1),
                        reg("IRQ", u32::from(m.irq_pending), 1),
                    ]);
                }
            }
            chips.push(ChipInfo {
                name: "mapper",
                regs,
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
        vec![
            emu_core::ControlInfo::button(0, "POWER", false),
            emu_core::ControlInfo::button(1, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        const NAMES: [&str; 4] = [
            "controller 1",
            "controller 2",
            "controller 3",
            "controller 4",
        ];
        let n = if self.ports.four_score { 4 } else { 2 };
        (0..n)
            .map(|i| PortInfo {
                id: PortId(i as u8),
                kind: PortKind::NesControl,
                name: NAMES[i],
                connected: self.ports.devices[i].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, mut dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let n = if self.ports.four_score { 4 } else { 2 };
        let i = usize::from(port.0);
        if i >= n {
            return Err(PortError::NoSuchPort(port));
        }
        if dev.kind() != PortKind::NesControl {
            return Err(PortError::Incompatible {
                port: PortKind::NesControl,
                device: dev.kind(),
            });
        }
        if self.ports.devices[i].is_some() {
            return Err(PortError::Occupied(port));
        }
        dev.write_lines(
            PortLines {
                pulled_low: if self.board.strobe { pin::OUT } else { 0 },
                analog: [0; 2],
            },
            Cycle(self.board.cycle),
        );
        self.ports.devices[i] = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        self.ports.devices.get_mut(usize::from(port.0))?.take()
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
        w.chunk(*b"NESM", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.ports.four_score);
            w.bool(self.field_done_pending);
            w.bytes(&b.ram);
            w.bytes(&b.ciram);
            w.u8(b.open_bus);
            w.bool(b.strobe);
            w.u8(b.reads[0]);
            w.u8(b.reads[1]);
            w.bool(b.dma.oam.is_some());
            let (page, step) = b.dma.oam.unwrap_or((0, 0));
            w.u8(page);
            w.u16(step);
            w.u8(b.dma.oam_byte);
            w.bool(b.dma.dmc.is_some());
            w.u8(b.dma.dmc.unwrap_or(0));
            w.bool(b.dma.halted);
            w.u64(b.cycle);
            w.u64(b.dots);
            w.u32(b.master);
            w.bool(b.field_done);
            b.audio.save(w);
            w.u32(b.dc[0] as u32);
            w.u32(b.dc[1] as u32);
            w.bool(b.cart.is_some());
        });
        self.cpu.save(&mut w);
        b.ppu.save(&mut w);
        b.apu.save(&mut w);
        if let Some(c) = &b.cart {
            c.save(&mut w);
        }
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for d in &self.ports.devices {
            w.chunk(*b"NESD", 1, |w| {
                w.bytes(&d.as_ref().map(|d| d.save_state()).unwrap_or_default());
            });
        }
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"NESM", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        if c.bool()? != self.ports.four_score {
            return Err(StateError::Invalid("Four Score differs"));
        }
        let field_done_pending = c.bool()?;
        let mut b = self.board.clone();
        for (dst, name) in [(&mut b.ram, "RAM size"), (&mut b.ciram, "VRAM size")] {
            let src = c.bytes()?;
            if src.len() != dst.len() {
                return Err(StateError::Invalid(name));
            }
            dst.copy_from_slice(src);
        }
        b.open_bus = c.u8()?;
        b.strobe = c.bool()?;
        b.reads = [c.u8()?, c.u8()?];
        let has_oam = c.bool()?;
        let page = c.u8()?;
        let step = c.u16()?;
        if step > 511 {
            return Err(StateError::Invalid("OAM DMA step"));
        }
        b.dma.oam = has_oam.then_some((page, step));
        b.dma.oam_byte = c.u8()?;
        let has_dmc = c.bool()?;
        let stage = c.u8()?;
        b.dma.dmc = has_dmc.then_some(stage.min(2));
        b.dma.halted = c.bool()?;
        b.cycle = c.u64()?;
        b.dots = c.u64()?;
        b.master = c.u32()?;
        if b.master >= 16 {
            return Err(StateError::Invalid("clock phase"));
        }
        b.field_done = c.bool()?;
        b.audio.load(&mut c)?;
        b.dc = [c.u32()? as i32, c.u32()? as i32];
        if c.bool()? != b.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        b.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        b.ppu.load(&mut r)?;
        b.apu.load(&mut r)?;
        if let Some(cart) = &mut b.cart {
            cart.load(&mut r)?;
        }
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut states = Vec::new();
        for _ in 0..4 {
            let (_, mut c) = r.chunk(*b"NESD", 1)?;
            states.push(c.bytes()?.to_vec());
        }
        for (dev, st) in self.ports.devices.iter().zip(&states) {
            if dev.is_none() != st.is_empty() {
                return Err(StateError::Invalid("controllers differ"));
            }
        }
        for (dev, st) in self.ports.devices.iter_mut().zip(&states) {
            if let Some(d) = dev {
                d.load_state(st)?;
            }
        }
        self.cpu = cpu;
        self.board = b;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for Nes {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let b = &self.board;
        let byte = |a: u32| -> Option<u32> {
            Some(u32::from(match space {
                CPU_SPACE => b.peek(a as u16),
                PPU_SPACE => {
                    let a = (a & 0x3fff) as u16;
                    if a >= 0x3f00 {
                        b.ppu.palette[usize::from(a & 0x1f)]
                    } else {
                        peek_ppu(b.cart.as_ref(), &b.ciram, a)
                    }
                }
                OAM_SPACE => b.ppu.oam[(a & 0xff) as usize],
                _ => return None,
            }))
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            let b = &mut self.board;
            match space {
                CPU_SPACE if a < 0x2000 => b.ram[(a & 0x7ff) as usize] = byte,
                CPU_SPACE if (0x6000..0x8000).contains(&a) => match &mut b.cart {
                    Some(c) => {
                        let n = c.prg_ram.len();
                        c.prg_ram[(a & 0x1fff) as usize % n] = byte;
                    }
                    None => return false,
                },
                PPU_SPACE => {
                    let a = (a & 0x3fff) as u16;
                    if a >= 0x3f00 {
                        b.ppu.palette[usize::from(a & 0x1f)] = byte & 0x3f;
                    } else {
                        poke_ppu(b.cart.as_mut(), &mut b.ciram, a, byte);
                    }
                }
                OAM_SPACE => b.ppu.oam[(a & 0xff) as usize] = byte,
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
