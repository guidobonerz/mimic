//! Nintendo Game Boy (DMG-01) and Game Boy Pocket (MGB-001).
//!
//! A Sharp LR35902 (`cpu-sm83`) at 4.194304 MHz, 8 KB of work RAM, the
//! picture unit with its 8 KB of video RAM (`chips-nintendo::gbppu`), the
//! timer (`gbtimer`), a serial port, eight buttons and the cartridge with
//! its bank controller (`devices-cartridge::gameboy`). The machine's time
//! is counted in clocks; the processor works in machine cycles of four,
//! and every chip is advanced four clocks before each of the processor's
//! bus accesses, so an access sees the machine as it is at that cycle.
//!
//! **Memory map.**
//!
//! | Addresses | What |
//! | --- | --- |
//! | `$0000`–`$7FFF` | the cartridge's ROM (the boot ROM over `$0000`–`$00FF` until `$FF50` is written) |
//! | `$8000`–`$9FFF` | video RAM (not in mode 3) |
//! | `$A000`–`$BFFF` | the cartridge's RAM |
//! | `$C000`–`$DFFF` | work RAM, again at `$E000`–`$FDFF` |
//! | `$FE00`–`$FE9F` | the object table (not in modes 2 and 3, not during a DMA) |
//! | `$FF00`–`$FF7F` | the registers below |
//! | `$FF80`–`$FFFE` | high RAM |
//! | `$FFFF` | IE |
//!
//! **Registers.** `$FF00` P1 (bits 5 and 4 select the buttons A, B,
//! Select, Start or the directions Right, Left, Up, Down, low when
//! selected; bits 3–0 read them, low when pressed), `$FF01` SB and
//! `$FF02` SC (the serial port), `$FF04`–`$FF07` the timer, `$FF0F` IF
//! (bit 0 vertical blank, 1 STAT, 2 timer, 3 serial, 4 joypad),
//! `$FF10`–`$FF3F` the sound unit (`gbapu`; its two outputs are the
//! stereo sound), `$FF40`–`$FF4B` the picture unit, `$FF46` DMA (a
//! write copies 160 bytes from that page to the object table, a byte a
//! machine cycle), `$FF50` switches the boot ROM off.
//!
//! **Serial port.** Without a second machine: a transfer with the
//! internal clock (SC = `$81`) takes 1024 machine cycles, shifts in ones
//! and requests its interrupt. The bytes sent are kept
//! ([`GameBoy::take_serial`]): test programs report through them.
//!
//! **Boot ROM.** Optional (role `gameboy-boot`, 256 bytes; user-supplied).
//! Without it the machine starts as the boot ROM leaves it: PC `$0100`,
//! SP `$FFFE`, AF `$01B0`, BC `$0013`, DE `$00D8`, HL `$014D`, the
//! display on, the divider at `$ABCC`.
//!
//! **Input.** The buttons are part of the machine: a digital controller's
//! events on port 0 (directions; buttons bit 0 A, 1 B, 2 Select, 3 Start)
//! or the keyboard's machine keys 0–3 (A, B, Select, Start; [`keys`]).
//!
//! Sources: the Pan Docs (<https://gbdev.io/pandocs/>: memory map, joypad,
//! serial, OAM DMA, interrupts, power-up sequence); Joonas Javanainen,
//! *Game Boy: Complete Technical Reference*. The implementation is this
//! project's own.

#![forbid(unsafe_code)]

pub mod demo;

use chips_nintendo::gbapu::GbApu;
use chips_nintendo::gbppu::{self, GbPpu};
use chips_nintendo::gbtimer::{GbTimer, INT_TIMER};
use cpu_sm83::{MBus, Sm83};
use devices_cartridge::gameboy::GbCart;
use devices_cartridge::slot::BatterySlot;
use devices_cartridge::CartSlot;
use mimic_core::audio::Decimator;
use mimic_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use mimic_core::input::{Dirs, InputEvent, InputQueue, InputTarget, TimedInput};
use mimic_core::machine::ChipReg;
use mimic_core::media::{Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use mimic_core::port::{PortDevice, PortError, PortId, PortInfo};
use mimic_core::registry::{ConfigError, MachineConfig, MachineEntry};
use mimic_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
/// The processor's view, `$0000`–`$FFFF`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The 8 KB of video RAM.
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(1);
/// The object table, 160 bytes.
pub const OAM_SPACE: MemSpaceId = MemSpaceId(2);
/// The cartridge's whole ROM.
pub const ROM_SPACE: MemSpaceId = MemSpaceId(3);
/// The cartridge's whole RAM.
pub const CART_RAM_SPACE: MemSpaceId = MemSpaceId(4);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);
/// The cartridge's battery RAM (a `.srm` file beside the cartridge).
pub const SLOT_BATTERY: SlotId = SlotId(1);
/// The buttons, as a controller port for the input mapper.
pub const PORT_BUTTONS: PortId = PortId(0);

/// The machine's clock.
pub const CLOCK_HZ: u32 = 4_194_304;

/// Machine keys: the four buttons beside the cross.
pub mod keys {
    pub const A: u16 = 0;
    pub const B: u16 = 1;
    pub const SELECT: u16 = 2;
    pub const START: u16 = 3;
}

/// The buttons' bits in the machine's state.
mod button {
    pub const RIGHT: u8 = 0x01;
    pub const LEFT: u8 = 0x02;
    pub const UP: u8 = 0x04;
    pub const DOWN: u8 = 0x08;
    pub const A: u8 = 0x10;
    pub const B: u8 = 0x20;
    pub const SELECT: u8 = 0x40;
    pub const START: u8 = 0x80;
}

const INT_SERIAL: u8 = 0x08;
const INT_JOYPAD: u8 = 0x10;
/// Bits of a serial transfer.
const SERIAL_BITS: u16 = 8;
/// The most bytes kept of what the serial port sent.
const SERIAL_KEPT: usize = 65_536;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "gameboy",
        name: "Nintendo Game Boy",
        models: &["dmg", "pocket"],
        video_formats: &[],
        create: |config| Ok(Box::new(GameBoy::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Dmg,
    Pocket,
}

/// The cartridge port's two media: the cartridge and its battery RAM.
struct GbSlot {
    inner: CartSlot<GbCart>,
    battery: BatterySlot,
}

impl MediaSlot for GbSlot {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        self.inner.insert(media)?;
        let len = self
            .inner
            .cart()
            .and_then(|c| c.battery_ram().map(<[u8]>::len));
        self.battery.cart_changed(len);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.battery.cart_changed(None);
        self.inner.eject()
    }

    fn write_protect(&mut self, on: bool) {
        self.inner.write_protect(on);
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.inner.flush()
    }
}

/// Everything on the bus except the processor.
#[derive(Clone, Debug)]
struct Board {
    boot: Option<Vec<u8>>,
    /// The boot ROM lies over `$0000`–`$00FF`.
    boot_on: bool,
    /// The working copy of the inserted cartridge.
    cart: Option<GbCart>,
    wram: Vec<u8>,
    hram: [u8; 127],
    ppu: GbPpu,
    timer: GbTimer,
    /// IF and IE.
    requested: u8,
    enabled: u8,
    /// P1's bits 5 and 4.
    select: u8,
    /// The buttons held (see [`button`]).
    buttons: u8,
    sb: u8,
    sc: u8,
    /// Machine cycles left of the serial transfer under way.
    serial_left: u16,
    serial_out: Vec<u8>,
    /// The DMA register, the next byte to copy, and the cycles until the
    /// copy starts.
    dma: u8,
    dma_at: Option<u8>,
    dma_delay: u8,
    apu: GbApu,
    /// The two sides' resamplers and the samples made (left, right).
    audio: [Decimator; 2],
    samples: Vec<i16>,
    /// Clocks since power-on, and of the second running (MBC3's clock).
    t: u64,
    second: u32,
    frame_done: bool,
    /// The cartridge's RAM was written.
    ram_dirty: bool,
}

impl Board {
    fn request(&mut self, bits: u8) {
        self.requested |= bits;
    }

    /// The low four bits of P1 as the selected buttons give them.
    fn joypad_lines(&self) -> u8 {
        let mut low = 0x0f;
        if self.select & 0x10 == 0 {
            low &= !(self.buttons & 0x0f);
        }
        if self.select & 0x20 == 0 {
            low &= !(self.buttons >> 4);
        }
        low
    }

    /// Change what gives the joypad's lines; a line going low requests the
    /// interrupt.
    fn joypad(&mut self, change: impl FnOnce(&mut Self)) {
        let before = self.joypad_lines();
        change(self);
        if before & !self.joypad_lines() != 0 {
            self.request(INT_JOYPAD);
        }
    }

    /// The serial port's clock is bit 8 of the timer's counter (8192 Hz):
    /// a bit is shifted on its falling edges, wherever in that clock's
    /// period the transfer was started. The edge is taken one machine
    /// cycle ahead of the counter as DIV reads it, because the processor
    /// samples interrupts later in a cycle than it reads (mooneye's
    /// `boot_sclk_align`).
    fn serial_clock(&mut self, before: u16, after: u16) {
        let bit = |c: u16| c.wrapping_add(4) & 0x100 != 0;
        if self.serial_left > 0 && bit(before) && !bit(after) {
            self.serial_left -= 1;
            if self.serial_left == 0 {
                if self.serial_out.len() < SERIAL_KEPT {
                    self.serial_out.push(self.sb);
                }
                // Nobody at the other end: ones come in.
                self.sb = 0xff;
                self.sc &= 0x7f;
                self.request(INT_SERIAL);
            }
        }
    }

    /// One machine cycle of everything but the processor.
    fn tick(&mut self) {
        let before = self.timer.counter;
        let (irq, frame_sequencer) = self.timer.tick();
        self.serial_clock(before, self.timer.counter);
        if irq {
            self.request(INT_TIMER);
        }
        if frame_sequencer {
            self.apu.frame_step();
        }
        let (left, right) = self.apu.tick();
        let left = self.audio[0].push(i32::from(left));
        let right = self.audio[1].push(i32::from(right));
        if let (Some(l), Some(r)) = (left, right) {
            self.samples.extend([l, r]);
        }
        for _ in 0..4 {
            self.frame_done |= self.ppu.tick();
        }
        let irq = self.ppu.take_irq();
        self.request(irq);
        if self.dma_delay > 0 {
            self.dma_delay -= 1;
            if self.dma_delay == 0 {
                self.dma_at = Some(0);
            }
        } else if let Some(at) = self.dma_at {
            // The pages from $E000 on read the work RAM.
            let page = if self.dma >= 0xe0 {
                self.dma - 0x20
            } else {
                self.dma
            };
            let v = self.read_raw(u16::from(page) << 8 | u16::from(at));
            self.ppu.oam[usize::from(at)] = v;
            self.dma_at = (at < 159).then_some(at + 1);
        }
        self.t += 4;
        self.second += 4;
        if self.second >= CLOCK_HZ {
            self.second -= CLOCK_HZ;
            if let Some(c) = &mut self.cart {
                c.clock_second();
            }
        }
    }

    /// Memory below `$FE00` without the picture unit's locks (the DMA's
    /// source, the debugger's view).
    fn read_raw(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x00ff if self.boot_on => {
                self.boot.as_ref().map_or(0xff, |b| b[addr as usize])
            }
            0x0000..=0x7fff => self.cart.as_ref().map_or(0xff, |c| c.read(addr)),
            0x8000..=0x9fff => self.ppu.vram[usize::from(addr & 0x1fff)],
            0xa000..=0xbfff => self.cart.as_ref().map_or(0xff, |c| c.ram_read(addr)),
            _ => self.wram[usize::from(addr & 0x1fff)],
        }
    }

    fn io_read(&self, addr: u16) -> u8 {
        match addr & 0xff {
            0x00 => 0xc0 | self.select | self.joypad_lines(),
            0x01 => self.sb,
            0x02 => self.sc | 0x7e,
            0x04..=0x07 => self.timer.read(addr),
            0x0f => self.requested | 0xe0,
            0x10..=0x3f => self.apu.read(addr),
            0x46 => self.dma,
            0x40..=0x4b => self.ppu.read(addr),
            _ => 0xff,
        }
    }

    /// A read as the processor sees it. `locks`: the picture unit keeps
    /// its memory to itself in its modes (not for the debugger).
    fn read(&self, addr: u16, locks: bool) -> u8 {
        match addr {
            0x8000..=0x9fff if locks && !self.ppu.vram_free() => 0xff,
            0x0000..=0xfdff => self.read_raw(addr),
            0xfe00..=0xfe9f => {
                if locks && (!self.ppu.oam_free() || self.dma_at.is_some()) {
                    0xff
                } else {
                    self.ppu.oam[usize::from(addr - 0xfe00)]
                }
            }
            0xfea0..=0xfeff => 0x00,
            0xff00..=0xff7f => self.io_read(addr),
            0xff80..=0xfffe => self.hram[usize::from(addr - 0xff80)],
            0xffff => self.enabled,
        }
    }

    fn io_write(&mut self, addr: u16, v: u8) {
        match addr & 0xff {
            0x00 => self.joypad(|b| b.select = v & 0x30),
            0x01 => self.sb = v,
            0x02 => {
                self.sc = v & 0x81;
                self.serial_left = if v & 0x81 == 0x81 { SERIAL_BITS } else { 0 };
            }
            0x04..=0x07 => {
                // Clearing the divider can clock the frame sequencer.
                let before = self.timer.counter;
                if self.timer.write(addr, v) {
                    self.apu.frame_step();
                }
                self.serial_clock(before, self.timer.counter);
            }
            0x0f => self.requested = v & 0x1f,
            0x10..=0x3f => self.apu.write(addr, v),
            0x46 => {
                self.dma = v;
                self.dma_delay = 2;
            }
            0x40..=0x4b => {
                self.ppu.write(addr, v);
                let irq = self.ppu.take_irq();
                self.request(irq);
            }
            0x50 if v != 0 => self.boot_on = false,
            _ => {}
        }
    }

    fn write(&mut self, addr: u16, v: u8, locks: bool) {
        match addr {
            0x0000..=0x7fff => {
                if let Some(c) = &mut self.cart {
                    c.write(addr, v);
                }
            }
            0x8000..=0x9fff => {
                if !locks || self.ppu.vram_writable() {
                    self.ppu.vram[usize::from(addr & 0x1fff)] = v;
                }
            }
            0xa000..=0xbfff => {
                if let Some(c) = &mut self.cart {
                    c.ram_write(addr, v);
                    self.ram_dirty = true;
                }
            }
            0xc000..=0xfdff => self.wram[usize::from(addr & 0x1fff)] = v,
            0xfe00..=0xfe9f => {
                if !locks || (self.ppu.oam_writable() && self.dma_at.is_none()) {
                    self.ppu.oam[usize::from(addr - 0xfe00)] = v;
                }
            }
            0xfea0..=0xfeff => {}
            0xff00..=0xff7f => self.io_write(addr, v),
            0xff80..=0xfffe => self.hram[usize::from(addr - 0xff80)] = v,
            0xffff => self.enabled = v,
        }
    }
}

/// The processor's bus: every access first runs the machine one cycle.
struct GbBus<'a> {
    b: &'a mut Board,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl MBus for GbBus<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        self.b.tick();
        let v = self.b.read(addr, true);
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let a = u32::from(addr);
                let seen = dbg.on_read(CPU_SPACE, a, Size::Byte, u32::from(v));
                dbg.trace_read(CPU_SPACE, a, Size::Byte, seen, self.beam);
                seen as u8
            }
            _ => v,
        }
    }

    fn write(&mut self, addr: u16, v: u8) {
        self.b.tick();
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                let a = u32::from(addr);
                dbg.trace_write(CPU_SPACE, a, Size::Byte, u32::from(v), self.beam);
                if !dbg.on_write(CPU_SPACE, a, Size::Byte, u32::from(v)) {
                    return;
                }
            }
        }
        self.b.write(addr, v, true);
    }

    fn idle(&mut self) {
        self.b.tick();
    }

    fn pending(&self) -> u8 {
        self.b.requested & self.b.enabled
    }

    fn acknowledge(&mut self, bit: u8) {
        self.b.requested &= !bit;
    }
}

/// The debugger's view of memory: no side effects, no locks.
struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.0.read(addr as u16, false))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

pub struct GameBoy {
    model: Model,
    cpu: Sm83,
    board: Board,
    slot: GbSlot,
    /// Clocks since power-on.
    sched: Scheduler,
    input: InputQueue,
    frame_done_pending: bool,
}

impl GameBoy {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("dmg") => Model::Dmg,
            Some("pocket") => Model::Pocket,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        // `roms=stub` is what tools pass to every machine to run without
        // ROM files: the Game Boy needs none.
        let unknown = config
            .options
            .iter()
            .find(|(k, v)| (k.as_str(), v.as_str()) != ("roms", "stub"));
        if let Some((key, _)) = unknown {
            return Err(ConfigError::Invalid(format!(
                "the Game Boy has no option '{key}'"
            )));
        }
        let boot = match config.roms.get("gameboy-boot") {
            Some(r) if r.len() == 256 => Some(r.clone()),
            Some(r) => {
                return Err(ConfigError::Invalid(format!(
                    "the Game Boy's boot ROM must be 256 bytes, got {}",
                    r.len()
                )))
            }
            None => None,
        };
        let mut m = Self {
            model,
            cpu: Sm83::new(),
            board: Board {
                boot,
                boot_on: false,
                cart: None,
                wram: vec![0; 0x2000],
                hram: [0; 127],
                ppu: GbPpu::new(),
                timer: GbTimer::new(),
                requested: 0,
                enabled: 0,
                select: 0x30,
                buttons: 0,
                sb: 0,
                sc: 0,
                serial_left: 0,
                serial_out: Vec::new(),
                dma: 0xff,
                dma_at: None,
                dma_delay: 0,
                apu: GbApu::new(),
                audio: [Decimator::new(CLOCK_HZ / 4, mimic_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?;
                    2],
                samples: Vec::new(),
                t: 0,
                second: 0,
                frame_done: false,
                ram_dirty: false,
            },
            slot: GbSlot {
                inner: devices_cartridge::gameboy::slot(),
                battery: BatterySlot::default(),
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            frame_done_pending: false,
        };
        m.power_on();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    /// The bytes the serial port sent since the last call.
    pub fn take_serial(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.board.serial_out)
    }

    /// Power-on: with the boot ROM at its start, without it in the state
    /// the boot ROM leaves.
    fn power_on(&mut self) {
        let b = &mut self.board;
        b.cart = self.slot.inner.cart().cloned();
        if let Some(c) = &mut b.cart {
            c.reset();
            // What the battery kept.
            if let Some(saved) = &self.slot.battery.contents {
                c.set_battery_ram(saved);
            }
        }
        self.slot.battery.loaded = false;
        b.wram.fill(0);
        b.hram.fill(0);
        b.ppu = GbPpu::new();
        b.timer = GbTimer::new();
        b.requested = 0;
        b.enabled = 0;
        b.select = 0x00;
        b.sb = 0;
        b.sc = 0;
        b.serial_left = 0;
        b.dma = 0xff;
        b.dma_at = None;
        b.dma_delay = 0;
        b.apu = GbApu::new();
        b.samples.clear();
        b.frame_done = false;
        b.ram_dirty = false;
        self.cpu = Sm83::new();
        b.boot_on = b.boot.is_some();
        if !b.boot_on {
            // The Pocket's boot ROM leaves $FF in A, the DMG's $01.
            self.cpu.set_af(match self.model {
                Model::Pocket => 0xffb0,
                _ => 0x01b0,
            });
            self.cpu.set_bc(0x0013);
            self.cpu.set_de(0x00d8);
            self.cpu.set_hl(0x014d);
            self.cpu.sp = 0xfffe;
            self.cpu.pc = 0x0100;
            b.timer.counter = 0xabc8;
            b.requested = 0x01;
            b.ppu.bgp = 0xfc;
            b.ppu.obp0 = 0xff;
            b.ppu.obp1 = 0xff;
            b.ppu.write(0xff40, 0x91);
            b.ppu.take_irq();
            b.apu.after_boot();
        }
        self.input.clear();
        self.frame_done_pending = false;
    }

    fn beam_pos(&self) -> BeamPos {
        let p = &self.board.ppu;
        BeamPos {
            frame: p.fields,
            line: u16::from(p.ly),
            cycle: p.dot / 4,
            // Pixel 0 leaves the pipeline at dot 92.
            hpos: p.dot.saturating_sub(92).min(gbppu::WIDTH as u16 - 1),
        }
    }

    /// The battery file in, or the game's writes out.
    fn sync_battery(&mut self) {
        let battery = &mut self.slot.battery;
        if std::mem::take(&mut battery.loaded) {
            if let (Some(file), Some(c)) = (&battery.contents, &mut self.board.cart) {
                c.set_battery_ram(file);
            }
        }
        if std::mem::take(&mut self.board.ram_dirty) {
            if let Some(ram) = self.board.cart.as_ref().and_then(GbCart::battery_ram) {
                battery.update(ram);
            }
        }
    }

    /// One instruction (or interrupt dispatch, or idle cycle). Returns
    /// `Some` if emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.slot.inner.take_changed() {
            self.power_on();
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let beam = self.beam_pos();
        let m = {
            let mut bus = GbBus {
                b: &mut self.board,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step(&mut bus)
        };
        self.sched.advance(u64::from(m) * 4);
        let frame = std::mem::take(&mut self.board.frame_done);
        if frame {
            self.sync_battery();
        }
        if let Some(dbg) = dbg {
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
                self.frame_done_pending |= frame;
                return Some(stop);
            }
        }
        frame.then_some(StopReason::FrameDone)
    }

    fn set_button(&mut self, bit: u8, down: bool) {
        self.board.joypad(|b| {
            if down {
                b.buttons |= bit;
            } else {
                b.buttons &= !bit;
            }
        });
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Port(PORT_BUTTONS), InputEvent::Digital { dirs, buttons }) => {
                let mut held = 0;
                for (dir, bit) in [
                    (Dirs::RIGHT, button::RIGHT),
                    (Dirs::LEFT, button::LEFT),
                    (Dirs::UP, button::UP),
                    (Dirs::DOWN, button::DOWN),
                ] {
                    if dirs.contains(dir) {
                        held |= bit;
                    }
                }
                held |= (buttons & 0x0f) << 4;
                // The keyboard's buttons stay as they are.
                self.board.joypad(|b| b.buttons = held);
            }
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                let bit = match machine_key.0 {
                    keys::A => button::A,
                    keys::B => button::B,
                    keys::SELECT => button::SELECT,
                    keys::START => button::START,
                    _ => return,
                };
                self.set_button(bit, down);
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) if id.0 == 0 => {
                self.reset(ResetKind::Hard);
            }
            _ => {}
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: CLOCK_HZ,
            active_pixels: gbppu::WIDTH as u32,
            total_pixels: u32::from(gbppu::LINE_DOTS),
            lines_per_field: u32::from(gbppu::LINES),
            // An LCD: neither; the frame rate (59.73 Hz) is NTSC's.
            standard: VideoStandard::Ntsc,
            output: OutputType::Rgb,
            field: FieldParity::Progressive,
        }
    }
}

/// Read a byte as the debugger sees it (tests and tools).
pub fn peek_cpu(m: &GameBoy, addr: u16) -> u8 {
    m.board.read(addr, false)
}

/// The picture unit (tests and tools).
pub fn ppu(m: &GameBoy) -> &GbPpu {
    &m.board.ppu
}

impl Machine for GameBoy {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.frame_done_pending) {
            return StopReason::FrameDone;
        }
        if self.board.ppu.ly == 144 && self.board.ppu.dot < 24 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE {
                    for i in 0..f.size.bytes() {
                        self.board
                            .write((f.addr + i) as u16, (f.value >> (8 * i)) as u8, false);
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

    /// The Game Boy has no reset button: both kinds are a power cycle
    /// (the cartridge's battery RAM is kept).
    fn reset(&mut self, _kind: ResetKind) {
        if let Some(ram) = self.board.cart.as_ref().and_then(GbCart::battery_ram) {
            self.slot.battery.update(ram);
        }
        self.power_on();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: gbppu::WIDTH as u32,
            height: gbppu::HEIGHT as u32,
            pixels: PixelData::Indexed {
                pixels: self.board.ppu.frame(),
                palette: match self.model {
                    Model::Dmg => &gbppu::PALETTE_DMG,
                    Model::Pocket => &gbppu::PALETTE_POCKET,
                },
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    /// Row 0 is line 0; the vertical blank's lines have no row.
    fn frame_row(&self, line: u16) -> Option<u32> {
        (usize::from(line) < gbppu::HEIGHT).then_some(u32::from(line))
    }

    /// Stereo: the sound unit's two outputs.
    fn audio(&mut self, out: &mut Vec<i16>) -> mimic_core::AudioFormat {
        out.append(&mut self.board.samples);
        mimic_core::AudioFormat {
            sample_rate: mimic_core::SAMPLE_RATE,
            channels: 2,
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
            space(CPU_SPACE, "cpu", 16),
            space(VRAM_SPACE, "vram", 13),
            space(OAM_SPACE, "oam", 8),
            space(ROM_SPACE, "rom", 23),
            space(CART_RAM_SPACE, "cartram", 17),
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let b = &self.board;
        let reg = |name, addr: u16, value: u8| ChipReg {
            name,
            addr: Some(u32::from(addr)),
            value: u32::from(value),
            bits: 8,
        };
        let p = &b.ppu;
        vec![
            ChipInfo {
                name: "ppu",
                regs: vec![
                    reg("LCDC", 0xff40, p.read(0xff40)),
                    reg("STAT", 0xff41, p.read(0xff41)),
                    reg("SCY", 0xff42, p.scy),
                    reg("SCX", 0xff43, p.scx),
                    reg("LY", 0xff44, p.read(0xff44)),
                    reg("LYC", 0xff45, p.lyc),
                    reg("DMA", 0xff46, b.dma),
                    reg("BGP", 0xff47, p.bgp),
                    reg("OBP0", 0xff48, p.obp0),
                    reg("OBP1", 0xff49, p.obp1),
                    reg("WY", 0xff4a, p.wy),
                    reg("WX", 0xff4b, p.wx),
                ],
            },
            ChipInfo {
                name: "timer",
                regs: vec![
                    reg("DIV", 0xff04, b.timer.read(0xff04)),
                    reg("TIMA", 0xff05, b.timer.tima),
                    reg("TMA", 0xff06, b.timer.tma),
                    reg("TAC", 0xff07, b.timer.read(0xff07)),
                ],
            },
            ChipInfo {
                name: "system",
                regs: vec![
                    reg("P1", 0xff00, b.io_read(0xff00)),
                    reg("SB", 0xff01, b.sb),
                    reg("SC", 0xff02, b.io_read(0xff02)),
                    reg("IF", 0xff0f, b.io_read(0xff0f)),
                    reg("IE", 0xffff, b.enabled),
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

    fn controls(&self) -> Vec<mimic_core::ControlInfo> {
        vec![mimic_core::ControlInfo::button(0, "POWER", false)]
    }

    /// The buttons are built in: nothing to plug.
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
        vec![
            MediaSlotInfo {
                id: SLOT_CARTRIDGE,
                name: "cartridge",
                kind: MediaKind::Cartridge,
                inserted: self.slot.inner.is_inserted(),
            },
            MediaSlotInfo {
                id: SLOT_BATTERY,
                name: "battery RAM",
                kind: MediaKind::BatteryRam,
                inserted: self.slot.battery.is_inserted(),
            },
        ]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_CARTRIDGE => Some(&mut self.slot as &mut dyn MediaSlot),
            SLOT_BATTERY => Some(&mut self.slot.battery as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"GBOY", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.frame_done_pending);
            w.bool(b.boot_on);
            w.bytes(&b.wram);
            w.bytes(&b.hram);
            for v in [
                b.requested,
                b.enabled,
                b.select,
                b.buttons,
                b.sb,
                b.sc,
                b.dma,
                b.dma_at.unwrap_or(0xff),
                b.dma_delay,
            ] {
                w.u8(v);
            }
            w.u16(b.serial_left);
            b.audio[0].save(w);
            b.audio[1].save(w);
            w.u64(b.t);
            w.u32(b.second);
            w.bool(b.frame_done);
            w.bool(b.cart.is_some());
        });
        self.cpu.save(&mut w);
        b.ppu.save(&mut w);
        b.timer.save(&mut w);
        b.apu.save(&mut w);
        if let Some(c) = &b.cart {
            c.save(&mut w);
        }
        self.sched.save(&mut w);
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        // A cartridge that went in since the last instruction counts.
        if self.slot.inner.take_changed() {
            self.power_on();
        }
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"GBOY", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let frame_done_pending = c.bool()?;
        let mut board = self.board.clone();
        board.boot_on = c.bool()?;
        if board.boot_on && board.boot.is_none() {
            return Err(StateError::Invalid("the state needs the boot ROM"));
        }
        let (wram, hram) = (c.bytes()?, c.bytes()?);
        if wram.len() != board.wram.len() || hram.len() != board.hram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        board.wram.copy_from_slice(wram);
        board.hram.copy_from_slice(hram);
        board.requested = c.u8()? & 0x1f;
        board.enabled = c.u8()?;
        board.select = c.u8()? & 0x30;
        board.buttons = c.u8()?;
        board.sb = c.u8()?;
        board.sc = c.u8()? & 0x81;
        board.dma = c.u8()?;
        board.dma_at = Some(c.u8()?).filter(|at| *at < 160);
        board.dma_delay = c.u8()?.min(2);
        board.serial_left = c.u16()?.min(SERIAL_BITS);
        board.audio[0].load(&mut c)?;
        board.audio[1].load(&mut c)?;
        board.samples.clear();
        board.t = c.u64()?;
        board.second = c.u32()? % CLOCK_HZ;
        board.frame_done = c.bool()?;
        if c.bool()? != board.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        board.serial_out.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        board.ppu.load(&mut r)?;
        board.timer.load(&mut r)?;
        board.apu.load(&mut r)?;
        if let Some(cart) = &mut board.cart {
            cart.load(&mut r)?;
        }
        sched.load(&mut r)?;
        input.load(&mut r)?;
        self.cpu = cpu;
        self.board = board;
        self.sched = sched;
        self.input = input;
        self.frame_done_pending = frame_done_pending;
        Ok(())
    }
}

impl DebugTarget for GameBoy {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let b = &self.board;
        let byte = |a: u32| -> Option<u32> {
            let v = match space {
                CPU_SPACE => b.read(a as u16, false),
                VRAM_SPACE => b.ppu.vram[(a & 0x1fff) as usize],
                OAM_SPACE => *b.ppu.oam.get(a as usize)?,
                ROM_SPACE => *b.cart.as_ref()?.rom().get(a as usize)?,
                CART_RAM_SPACE => *b.cart.as_ref()?.ram().get(a as usize)?,
                _ => return None,
            };
            Some(u32::from(v))
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        let b = &mut self.board;
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            match space {
                // RAM and registers; the ROM stays as it is.
                CPU_SPACE if (0x8000..=0xffff).contains(&a) => b.write(a as u16, byte, false),
                VRAM_SPACE => b.ppu.vram[(a & 0x1fff) as usize] = byte,
                OAM_SPACE if a < 160 => b.ppu.oam[a as usize] = byte,
                CART_RAM_SPACE => {
                    let Some(slot) = b
                        .cart
                        .as_mut()
                        .and_then(|c| c.ram_mut().get_mut(a as usize))
                    else {
                        return false;
                    };
                    *slot = byte;
                    b.ram_dirty = true;
                }
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
