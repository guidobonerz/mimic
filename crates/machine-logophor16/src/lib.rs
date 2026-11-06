//! The Logophor 16, a fictional steampunk home computer designed for this
//! project: CPU Ada, chips Babbage, Lumière, Calliope and Telegraph, 4 MiB
//! chip RAM and 28 MiB fast RAM, the ROM Foundry (assembled from this
//! crate's `asm/foundry` sources by the build script and built in, so the
//! machine needs no user files), shipped as a standard profile.
//!
//! Specification: `docs/logophor16/SPEC.md`. Built so far (steps 3–8
//! of Phase 31): the CPU, the memory map, Lumière (modes 0, 1 and 4,
//! sprites), Babbage (DMA control, Copper, Blitter, Stereoscope), Calliope
//! (8 sample channels, stereo at 48 kHz), Telegraph's interrupt controller, the
//! debug port (FFFF:43F0: console output kept in the machine, exit code),
//! a program slot that loads a raw binary at 0040:0000 and starts it.
//!
//! Timing: a line is 318 CPU cycles, 848 chip-bus slots and 1060 pixels
//! (40 MHz), a field 628 lines. Each CPU cycle covers 8/3 slots: the fixed
//! DMA (the next line's framebuffer fetch, from slot 640 on, running into
//! the next line if it must) takes its slots, the Copper the free ones it
//! wants, and a CPU instruction that reads or writes chip RAM (or is
//! fetched from it) starts only in a cycle with a free slot left
//! (SPEC.md 5; a simplification: one free slot covers all of the
//! instruction's accesses). Lumière puts the picture out up to the beam
//! before each register write, so writes in a line show where they
//! happen. [`Machine::now`] counts master ticks of 480 MHz (CPU cycle =
//! 40, pixel = 12; SPEC.md 2).
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use chips_logophor::babbage::{self, Babbage, CopperAction};
use chips_logophor::blitter::ChipRamMut;
use chips_logophor::calliope::{self, Calliope};
use chips_logophor::lumiere::{self, Beam, ChipRam, Lumiere};
use chips_logophor::telegraph::{self, Interrupts};
use chips_logophor::telegraph_io::{Input, PortDev, Sd, SdCard, Timers};
use cpu_ada::Ada;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::{ChipReg, DebugTarget};
use emu_core::media::{Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, CycleResult, DebugHooks, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Size, StateError, StateReader,
    StateWriter, StopReason,
};

/// The Foundry ROM, assembled by the build script.
pub static FOUNDRY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/foundry.rom"));

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
const SLOT_PROGRAM: SlotId = SlotId(0);
const SLOT_SD: SlotId = SlotId(1);
/// The clock's start without an `rtc` option: 2026-01-01 00:00 UTC in
/// seconds since 2000-01-01.
pub const RTC_DEFAULT: u32 = 820_454_400;

/// CPU cycles a line, lines a field, visible lines (SPEC.md 2).
pub const LINE_CYCLES: u16 = 318;
pub const LINES: u16 = 628;
pub const VISIBLE_LINES: u16 = 600;
/// Master ticks (480 MHz) per CPU cycle and per line.
const TICKS_PER_CYCLE: u64 = 40;
const TICKS_PER_LINE: u64 = TICKS_PER_CYCLE * LINE_CYCLES as u64;

const RAM_SEGMENTS: u32 = 0x200;
/// The end of chip RAM (segments 0–$3F).
const CHIP_END: u32 = 0x0040_0000;
/// Chip-bus slots a line, and the slot where the fixed DMA starts (the
/// horizontal blank, x = 800).
const LINE_SLOTS: u16 = 848;
const FIXED_START: u16 = 640;
const ROM_FIRST: u32 = 0xfffb;
const ROM_SIZE: usize = 0x4_0000;
/// Where the program slot loads and starts a binary.
pub const PROGRAM_LOAD: u32 = 0x0040_0000;

/// Names of Calliope's channel registers in `chips()`.
const CALLIOPE_CTL: [&str; 8] = [
    "AUD0CTL", "AUD1CTL", "AUD2CTL", "AUD3CTL", "AUD4CTL", "AUD5CTL", "AUD6CTL", "AUD7CTL",
];
const CALLIOPE_PER: [&str; 8] = [
    "AUD0PER", "AUD1PER", "AUD2PER", "AUD3PER", "AUD4PER", "AUD5PER", "AUD6PER", "AUD7PER",
];

/// Debug port (SPEC.md 10.5).
const DBGOUT: u16 = 0x43f0;
const DBGEXIT: u16 = 0x43f2;
const DBGID: u16 = 0x43f4;
/// The console output kept: the last this many bytes.
const CONSOLE_KEEP: usize = 1 << 16;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "logophor16",
        name: "Logophor 16",
        models: &["16"],
        video_formats: &[],
        create: |config| Ok(Box::new(Logophor::new(config)?)),
    }
}

/// Memory and I/O as the CPU sees them.
#[derive(Clone)]
pub struct Board {
    ram: Vec<u8>,
    /// RAM segments written since power-on (the others are all zero): a
    /// savestate skips them without looking.
    touched: [bool; RAM_SEGMENTS as usize],
    rom: Vec<u8>,
    pub lumiere: Lumiere,
    pub babbage: Babbage,
    pub calliope: Calliope,
    pub ints: Interrupts,
    pub timers: Timers,
    pub input: Input,
    pub sd: Sd,
    card: SdSlot,
    /// Debug console output (the last [`CONSOLE_KEEP`] bytes).
    pub console: Vec<u8>,
    /// Last value written to DBGEXIT.
    pub exit: Option<u16>,
    beam: Beam,
}

/// Chip RAM for the Blitter, which marks the segments it writes.
struct ChipMut<'a> {
    ram: &'a mut [u8],
    touched: &'a mut [bool; RAM_SEGMENTS as usize],
}

impl ChipRamMut for ChipMut<'_> {
    fn get(&self, addr: u32) -> u8 {
        if addr < CHIP_END {
            self.ram[addr as usize]
        } else {
            0
        }
    }

    fn set(&mut self, addr: u32, v: u8) {
        if addr < CHIP_END {
            self.ram[addr as usize] = v;
            self.touched[(addr >> 16) as usize] = true;
        }
    }
}

/// The SD card slot: an image of whole 512-byte blocks.
#[derive(Default)]
struct SdSlot {
    media: Option<Media>,
    protected: bool,
}

impl Clone for SdSlot {
    /// A board copy (savestate loading) keeps no card: the card stays in
    /// the machine's slot, not in the state.
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl SdCard for SdSlot {
    fn blocks(&self) -> u32 {
        self.media.as_ref().map_or(0, |m| {
            (m.backend.len() / 512).min(u64::from(u32::MAX)) as u32
        })
    }

    fn protected(&self) -> bool {
        self.protected
    }

    fn read_block(&mut self, block: u32, buf: &mut [u8; 512]) -> bool {
        self.media
            .as_mut()
            .is_some_and(|m| m.backend.read(u64::from(block) * 512, buf).is_ok())
    }

    fn write_block(&mut self, block: u32, buf: &[u8; 512]) -> bool {
        self.media
            .as_mut()
            .is_some_and(|m| m.backend.write(u64::from(block) * 512, buf).is_ok())
    }
}

impl MediaSlot for Board {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        if media.backend.len() < 512 || !media.backend.len().is_multiple_of(512) {
            return Err(MediaError::Format(
                "an SD image is whole 512-byte blocks".into(),
            ));
        }
        self.card.media = Some(media);
        self.sd.card(&self.card);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let m = self.card.media.take();
        self.sd.card(&self.card);
        m
    }

    fn write_protect(&mut self, on: bool) {
        self.card.protected = on;
        self.sd.card(&self.card);
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.card.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }
}

/// Chip RAM for Lumière.
struct ChipView<'a>(&'a [u8]);

impl ChipRam for ChipView<'_> {
    fn chip_byte(&self, addr: u32) -> u8 {
        // Chip RAM is segments 0–$3F; a pointer outside reads 0 (the DMA
        // error comes with Babbage).
        if addr < CHIP_END {
            self.0[addr as usize]
        } else {
            0
        }
    }
}

impl Board {
    fn new() -> Self {
        let mut rom = vec![0xff; ROM_SIZE];
        rom[..FOUNDRY.len().min(ROM_SIZE)].copy_from_slice(&FOUNDRY[..FOUNDRY.len().min(ROM_SIZE)]);
        Self {
            ram: vec![0; (RAM_SEGMENTS as usize) << 16],
            touched: [false; RAM_SEGMENTS as usize],
            rom,
            lumiere: Lumiere::new(),
            babbage: Babbage::new(),
            calliope: Calliope::new(),
            ints: Interrupts::new(),
            timers: Timers::new(RTC_DEFAULT),
            input: Input::new([PortDev::Mouse, PortDev::Joystick]),
            sd: Sd::default(),
            card: SdSlot::default(),
            console: Vec::new(),
            exit: None,
            beam: Beam::default(),
        }
    }

    fn byte(&self, a: u32) -> Option<u8> {
        let seg = a >> 16;
        if seg < RAM_SEGMENTS {
            Some(self.ram[a as usize])
        } else if (ROM_FIRST..ROM_FIRST + 4).contains(&seg) {
            Some(self.rom[(a - (ROM_FIRST << 16)) as usize])
        } else {
            None
        }
    }

    fn io_peek(&self, off: u16) -> u16 {
        match off & !1 {
            0x0000..=0x07ff => self.babbage.peek(off),
            0x0800..=0x11ff | 0x2000..=0x27ff => self.lumiere.peek(off, self.beam),
            0x3000..=0x31ff => self.calliope.peek(off),
            0x4200..=0x422f => self.timers.peek(off),
            0x4240..=0x425f => self.input.peek(off),
            0x4260..=0x427f => self.sd.peek(off),
            telegraph::INTREQ | telegraph::INTENA => self.ints.peek(off),
            DBGID => 0xc016,
            _ => 0,
        }
    }

    fn io_read(&mut self, off: u16) -> u16 {
        match off & !1 {
            0x0000..=0x07ff => self.babbage.read(off),
            0x0800..=0x11ff | 0x2000..=0x27ff => self.lumiere.read(off, self.beam),
            0x3000..=0x31ff => self.calliope.read(off),
            0x4200..=0x422f => self.timers.read(off),
            0x4240..=0x425f => self.input.read(off),
            0x4260..=0x427f => self.sd.read(off),
            _ => self.io_peek(off),
        }
    }

    fn io_write(&mut self, off: u16, v: u16) {
        match off & !1 {
            0x0000..=0x07ff => {
                self.babbage.write(off, v);
                self.lumiere
                    .set_dma(self.babbage.dma(babbage::DMA_FRAMEBUFFER));
                self.lumiere
                    .set_sprite_dma(self.babbage.dma(babbage::DMA_SPRITES));
                if self.babbage.take_error() {
                    self.ints.raise(telegraph::INT_DMAERR);
                }
                if self.babbage.stereo.take_start() {
                    let Board {
                        ram,
                        touched,
                        babbage,
                        ..
                    } = self;
                    let ok = babbage.stereo.run(&mut ChipMut { ram, touched });
                    if !ok {
                        babbage.stereo_error();
                        self.ints.raise(telegraph::INT_DMAERR);
                    }
                }
            }
            0x0800..=0x11ff | 0x2000..=0x27ff => {
                // The picture up to the beam with the old value first.
                let Board {
                    ram, lumiere, beam, ..
                } = self;
                lumiere.draw_to(beam.y, beam.x, &ChipView(&ram[..CHIP_END as usize]));
                lumiere.write(off, v);
            }
            0x3000..=0x31ff => self.calliope.write(off, v),
            0x4200..=0x422f => self.timers.write(off, v),
            0x4260..=0x427f => {
                if self.sd.write(off, v) {
                    self.ints.raise(telegraph::INT_SD);
                }
            }
            telegraph::INTREQ | telegraph::INTENA => self.ints.write(off, v),
            DBGOUT => {
                self.console.push(v as u8);
                if self.console.len() > CONSOLE_KEEP {
                    self.console.drain(..CONSOLE_KEEP / 2);
                }
            }
            DBGEXIT => self.exit = Some(v),
            _ => {}
        }
    }

    fn write_byte(&mut self, a: u32, v: u8) -> bool {
        if a >> 16 < RAM_SEGMENTS {
            self.ram[a as usize] = v;
            self.touched[(a >> 16) as usize] = true;
            true
        } else {
            false
        }
    }
}

impl Bus for Board {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        if addr >> 16 == 0xffff {
            let v = self.io_read(addr as u16);
            return match size {
                Size::Byte => u32::from(v >> (8 * (addr & 1)) & 0xff),
                _ => u32::from(v),
            };
        }
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        if addr >> 16 == 0xffff {
            let v = self.io_peek(addr as u16);
            return match size {
                Size::Byte => u32::from(v >> (8 * (addr & 1)) & 0xff),
                _ => u32::from(v),
            };
        }
        let b = |a: u32| u32::from(self.byte(a).unwrap_or(0xff));
        match size {
            Size::Byte => b(addr),
            _ => b(addr & !1) | b(addr | 1) << 8,
        }
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if addr >> 16 == 0xffff {
            // A byte write gives the register its low byte, high byte 0.
            let v = if size == Size::Byte {
                v & 0xff
            } else {
                v & 0xffff
            };
            self.io_write(addr as u16, v as u16);
            return;
        }
        match size {
            Size::Byte => {
                self.write_byte(addr, v as u8);
            }
            _ => {
                self.write_byte(addr & !1, v as u8);
                self.write_byte(addr | 1, (v >> 8) as u8);
            }
        }
    }

    fn addr_bits(&self) -> u8 {
        32
    }
}

/// The program slot: a raw binary, loaded at 0040:0000 and started there.
#[derive(Default)]
struct ProgramSlot {
    media: Option<Media>,
    bytes: Vec<u8>,
    pending: bool,
}

impl MediaSlot for ProgramSlot {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Program {
            return Err(MediaError::Unsupported(media.kind));
        }
        let len = media.backend.len() as usize;
        if len > (RAM_SEGMENTS as usize - 0x40) << 16 {
            return Err(MediaError::Format("program larger than fast RAM".into()));
        }
        let mut bytes = vec![0; len];
        media.backend.read(0, &mut bytes)?;
        self.bytes = bytes;
        self.pending = true;
        self.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.pending = false;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

pub struct Logophor {
    pub cpu: Ada,
    pub board: Board,
    /// Beam: line (0–627) and CPU cycle in it (0–317).
    line: u16,
    cycle: u16,
    fields: u64,
    field_done: bool,
    /// This line's fixed DMA: slots from FIXED_START, and slots at its
    /// start the last line's ran into.
    fixed: u16,
    carry: u16,
    input: InputQueue,
    program: ProgramSlot,
}

impl Logophor {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        if let Some(m) = config.model.as_deref().filter(|m| *m != "16") {
            return Err(ConfigError::UnknownModel(m.into()));
        }
        // `roms` (e.g. "stub" from the test runners) means nothing here:
        // the ROM is built in.
        let mut board = Board::new();
        let mut ports = [PortDev::Mouse, PortDev::Joystick];
        for (k, v) in &config.options {
            match k.as_str() {
                "roms" => {}
                "rtc" => {
                    let s = v.parse::<u32>().map_err(|_| {
                        ConfigError::Invalid(format!("rtc={v}: seconds since 2000-01-01"))
                    })?;
                    board.timers = Timers::new(s);
                }
                "port0" | "port1" => {
                    ports[usize::from(k == "port1")] = match v.as_str() {
                        "mouse" => PortDev::Mouse,
                        "joystick" => PortDev::Joystick,
                        "none" => PortDev::None,
                        _ => {
                            return Err(ConfigError::Invalid(format!(
                                "{k}={v}: mouse, joystick or none"
                            )))
                        }
                    }
                }
                _ => return Err(ConfigError::Invalid(format!("unknown option '{k}'"))),
            }
        }
        board.input = Input::new(ports);
        Ok(Self {
            cpu: Ada::new(),
            board,
            line: 0,
            cycle: 0,
            fields: 0,
            field_done: false,
            fixed: 0,
            carry: 0,
            input: InputQueue::new(),
            program: ProgramSlot::default(),
        })
    }

    /// The debug console's output so far.
    pub fn console(&self) -> &[u8] {
        &self.board.console
    }

    fn beam_pos(&self) -> BeamPos {
        BeamPos {
            frame: self.fields,
            line: self.line,
            cycle: self.cycle,
            hpos: self.cycle * 10 / 3,
        }
    }

    /// Start a program the slot got: load it at 0040:0000, the CPU there
    /// in supervisor mode with the stack at the top of chip RAM.
    fn service(&mut self) {
        if !self.program.pending {
            return;
        }
        self.program.pending = false;
        let bytes = std::mem::take(&mut self.program.bytes);
        for (i, b) in bytes.iter().enumerate() {
            self.board.write_byte(PROGRAM_LOAD + i as u32, *b);
        }
        self.program.bytes = bytes;
        let mut cpu = Ada::new();
        cpu.brk_stops = self.cpu.brk_stops;
        cpu.s[0] = (PROGRAM_LOAD >> 16) as u16;
        cpu.s[1] = cpu.s[0];
        cpu.s[2] = 0x003f;
        cpu.r[12] = 0xfffe;
        cpu.pc = PROGRAM_LOAD as u16;
        self.cpu = cpu;
    }

    /// The end of a line: put it out, VBlank, field end, the next line's
    /// fixed DMA.
    fn end_line(&mut self) {
        {
            let Board { ram, lumiere, .. } = &mut self.board;
            lumiere.end_line(self.line, &ChipView(&ram[..CHIP_END as usize]));
        }
        self.carry = (FIXED_START + self.fixed).saturating_sub(LINE_SLOTS);
        self.board.calliope.end_line();
        self.line += 1;
        if self.line == VISIBLE_LINES {
            self.board.lumiere.vblank();
            self.board.ints.raise(telegraph::INT_VBLANK);
        }
        if self.line == LINES {
            self.line = 0;
            self.fields += 1;
            self.field_done = true;
            self.board.babbage.field_start();
        }
        // The next line's fetch is set up at slot 640 (`prepare_fetch`).
        self.fixed = 0;
    }

    /// Slot 640: the fixed DMA for the next line, its framebuffer row and
    /// the sprites Lumière chooses for it now.
    fn prepare_fetch(&mut self) {
        let next = if self.line + 1 == LINES {
            0
        } else {
            self.line + 1
        };
        let dma = self.board.babbage.dma(babbage::DMA_CALLIOPE);
        let Board {
            ram,
            lumiere,
            calliope,
            ..
        } = &mut self.board;
        let chip = ChipView(&ram[..CHIP_END as usize]);
        let sprites = lumiere.select_sprites(next, &chip);
        let audio = calliope.fetch_line(dma, &chip);
        self.fixed = lumiere.fetch_slots(next) + sprites + audio;
        let dma = self.board.babbage.dma(babbage::DMA_TELEGRAPH);
        let Board {
            ram,
            touched,
            sd,
            card,
            ints,
            ..
        } = &mut self.board;
        let (slots, done) = sd.line(dma, card, &mut ChipMut { ram, touched });
        self.fixed += slots;
        if done {
            ints.raise(telegraph::INT_SD);
        }
    }

    /// Slot `s` of this line belongs to the fixed DMA.
    fn fixed_slot(&self, s: u16) -> bool {
        s < self.carry || (FIXED_START..FIXED_START + self.fixed).contains(&s)
    }

    /// Carry out what a Copper instruction did at (`y`, `x`).
    fn copper_action(&mut self, action: CopperAction, y: u16, x: u16) {
        match action {
            CopperAction::Move { offset, value } => {
                let cpu_beam = self.board.beam;
                self.board.beam = Beam { x, y };
                self.board.io_write(offset, value);
                self.board.beam = cpu_beam;
            }
            CopperAction::Irq => self.board.ints.raise(telegraph::INT_COPPER),
            CopperAction::Error => self.board.ints.raise(telegraph::INT_DMAERR),
            CopperAction::Blit | CopperAction::None => {}
        }
    }

    /// The chip-bus slots of this CPU cycle: fixed DMA, then the Copper;
    /// returns the slots left free.
    fn slots(&mut self) -> u32 {
        let c = u32::from(self.cycle);
        let (mut free, mut cpu_served) = (0, false);
        for s in (c * 8 / 3)..((c + 1) * 8 / 3) {
            let s = s as u16;
            if s == FIXED_START {
                self.prepare_fetch();
            }
            if self.fixed_slot(s) {
                continue;
            }
            let (y, x) = (self.line, s * 5 / 4);
            let b = &mut self.board.babbage;
            if b.copper_active() && (!b.copper_waiting() || b.copper_compare(y, x)) {
                let action = {
                    let Board { ram, babbage, .. } = &mut self.board;
                    babbage.copper_slot(y, x, &ChipView(&ram[..CHIP_END as usize]))
                };
                self.copper_action(action, y, x);
                continue;
            }
            let (blit, stereo) = (
                self.board.babbage.blitter_wants(),
                self.board.babbage.stereo_wants(),
            );
            if blit || stereo {
                // BUSCON bit 0: a waiting CPU comes first (one slot).
                if self.board.babbage.cpu_first()
                    && !cpu_served
                    && self.cpu.at_boundary()
                    && self.needs_chip()
                {
                    cpu_served = true;
                    free += 1;
                    continue;
                }
                if stereo && (!blit || self.board.babbage.stereo_turn()) {
                    if self.board.babbage.stereo.slot() {
                        self.board.ints.raise(telegraph::INT_STEREOSCOPE);
                    }
                    continue;
                }
                let done = {
                    let Board {
                        ram,
                        touched,
                        babbage,
                        ..
                    } = &mut self.board;
                    babbage.blitter_slot(&mut ChipMut { ram, touched })
                };
                if done {
                    self.board.ints.raise(telegraph::INT_BLITTER);
                }
                continue;
            }
            free += 1;
        }
        free
    }

    /// The next instruction touches chip RAM (fetch or data).
    fn needs_chip(&self) -> bool {
        let (fetch, data) = self.cpu.next_accesses(&self.board);
        fetch < CHIP_END || data.is_some_and(|d| d < CHIP_END)
    }

    /// One CPU cycle; `Some` when emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        let beam = self.beam_pos();
        let free = self.slots();
        if self.board.calliope.cycle() {
            self.board.ints.raise(telegraph::INT_CALLIOPE);
        }
        let t = self.board.timers.cycle();
        if t.timer {
            self.board.ints.raise(telegraph::INT_TIMER);
        }
        if t.rtc {
            self.board.ints.raise(telegraph::INT_RTC);
        }
        if !self.input.is_empty() {
            let now = self.now();
            while let Some(ev) = self.input.pop_due(now) {
                self.apply_input(ev);
            }
        }
        self.cpu.set_irq_level(self.board.ints.level());
        let stall = free == 0 && self.cpu.at_boundary() && self.needs_chip();
        let r = if stall {
            CycleResult::Busy
        } else {
            let mut bus = HookedBus {
                inner: &mut self.board,
                dbg: dbg.as_deref_mut(),
                space: CPU_SPACE,
                beam,
            };
            self.cpu.tick(&mut bus)
        };
        self.cycle += 1;
        if self.cycle == LINE_CYCLES {
            self.cycle = 0;
            self.end_line();
        }
        // The beam for the next cycle's register reads (and the debugger).
        self.board.beam = Beam {
            x: self.cycle * 10 / 3,
            y: self.line,
        };
        let dbg = dbg?;
        if let CycleResult::InstrDone { next_pc } = r {
            if dbg.trace_instr_armed() {
                dbg.trace_instr(CPU, next_pc, beam);
            }
            if dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.exec_armed(CPU_SPACE) {
                dbg.check_exec(CPU_SPACE, next_pc);
            }
        }
        dbg.take_stop()
    }

    /// A host event reaches Telegraph: keys as USB HID usages, the mouse
    /// and the joystick by port.
    fn apply_input(&mut self, ev: TimedInput) {
        let i = &mut self.board.input;
        let raise = match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => i
                .key(machine_key.0 as u8, down)
                .then_some(telegraph::INT_KEYBOARD),
            (InputTarget::Port(p), InputEvent::MouseRel { dx, dy, buttons }) => i
                .mouse(usize::from(p.0), dx, dy, buttons)
                .then_some(telegraph::INT_PORTS),
            (InputTarget::Port(p), InputEvent::Digital { dirs, buttons }) => {
                let bits = u16::from(dirs.0 & 15) | u16::from(buttons & 7) << 4;
                i.joystick(usize::from(p.0), bits)
                    .then_some(telegraph::INT_PORTS)
            }
            _ => None,
        };
        if let Some(bit) = raise {
            self.board.ints.raise(bit);
        }
    }

    fn lumiere_regs(&self) -> Vec<ChipReg> {
        let l = &self.board.lumiere;
        let beam = self.board.beam;
        [
            ("VMODE", lumiere::VMODE),
            ("FBSEG", lumiere::FBSEG),
            ("FBOFS", lumiere::FBOFS),
            ("PITCH", lumiere::PITCH),
            ("FINEX", lumiere::FINEX),
            ("BORDER", lumiere::BORDER),
            ("FGINDEX", lumiere::FGINDEX),
            ("TXTBANK", lumiere::TXTBANK),
            ("FONTSEG", lumiere::FONTSEG),
            ("FONTOFS", lumiere::FONTOFS),
            ("CURSOR", lumiere::CURSOR),
            ("CURSHAPE", lumiere::CURSHAPE),
            ("BEAMY", lumiere::BEAMY),
            ("BEAMX", lumiere::BEAMX),
            ("VSTAT", lumiere::VSTAT),
        ]
        .into_iter()
        .map(|(name, off)| ChipReg {
            name,
            addr: Some(0xffff_0000 | u32::from(off)),
            value: u32::from(l.peek(off, beam)),
            bits: 16,
        })
        .collect()
    }
}

impl Machine for Logophor {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        self.service();
        loop {
            if let Some(stop) = self.cycle(Some(dbg)) {
                return stop;
            }
            if self.cpu.brk_hit {
                self.cpu.brk_hit = false;
                return StopReason::StepDone;
            }
            if self.field_done {
                self.field_done = false;
                return StopReason::FrameDone;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.service();
        self.cycle(None);
        self.field_done = false;
        StopReason::StepDone
    }

    fn reset(&mut self, kind: ResetKind) {
        if kind == ResetKind::Hard {
            self.board.ram.fill(0);
            self.board.touched = [false; RAM_SEGMENTS as usize];
            self.board.console.clear();
        }
        self.board.exit = None;
        self.board.lumiere.reset();
        self.board.babbage.reset();
        self.board.calliope.reset();
        self.board.ints = Interrupts::new();
        let ports = self.board.input.ports;
        self.board.input = Input::new(ports);
        self.board.sd = Sd::default();
        self.board.sd.card(&self.board.card);
        if kind == ResetKind::Hard {
            let seconds = self.board.timers.seconds();
            self.board.timers = Timers::new(seconds);
        }
        self.fixed = 0;
        self.carry = 0;
        let brk_stops = self.cpu.brk_stops;
        self.cpu.reset();
        self.cpu.brk_stops = brk_stops;
        self.input.clear();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: lumiere::WIDTH,
            height: lumiere::HEIGHT,
            pixels: PixelData::Rgb(self.board.lumiere.pixels()),
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 40_000_000,
                active_pixels: lumiere::WIDTH,
                total_pixels: 1060,
                lines_per_field: u32::from(LINES),
                standard: VideoStandard::Ntsc,
                output: OutputType::Rgb,
                field: FieldParity::Progressive,
            },
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        self.board.calliope.take_output(out);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 2,
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
            addr_bits: 32,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, off: u16, value: u16| ChipReg {
            name,
            addr: Some(0xffff_0000 | u32::from(off)),
            value: u32::from(value),
            bits: 16,
        };
        let b = &self.board.babbage;
        let pc = b.copper_pc();
        vec![
            ChipInfo {
                name: "lumiere",
                regs: self.lumiere_regs(),
            },
            ChipInfo {
                name: "babbage",
                regs: [
                    ("DMACON", babbage::DMACON),
                    ("DMAERR", babbage::DMAERR),
                    ("BUSCON", babbage::BUSCON),
                    ("COP1SEG", babbage::COP1SEG),
                    ("COP1OFS", babbage::COP1OFS),
                    ("COP2SEG", babbage::COP2SEG),
                    ("COP2OFS", babbage::COP2OFS),
                    ("COPCON", babbage::COPCON),
                    ("COPIRQ", babbage::COPIRQ),
                ]
                .into_iter()
                .map(|(n, off)| reg(n, off, b.peek(off)))
                .chain([ChipReg {
                    name: "COPPC",
                    addr: None,
                    value: pc,
                    bits: 32,
                }])
                .collect(),
            },
            ChipInfo {
                name: "calliope",
                regs: [
                    ("AUDMASTER", calliope::AUDMASTER),
                    ("AUDIRQ", calliope::AUDIRQ),
                ]
                .into_iter()
                .chain((0..8u16).flat_map(|n| {
                    [
                        (
                            CALLIOPE_CTL[usize::from(n)],
                            calliope::AUD0 + 0x20 * n + calliope::AUDCTL,
                        ),
                        (
                            CALLIOPE_PER[usize::from(n)],
                            calliope::AUD0 + 0x20 * n + calliope::AUDPER,
                        ),
                    ]
                }))
                .map(|(n, off)| reg(n, off, self.board.calliope.peek(off)))
                .collect(),
            },
            ChipInfo {
                name: "telegraph",
                regs: vec![
                    reg(
                        "INTREQ",
                        telegraph::INTREQ,
                        self.board.ints.peek(telegraph::INTREQ),
                    ),
                    reg(
                        "INTENA",
                        telegraph::INTENA,
                        self.board.ints.peek(telegraph::INTENA),
                    ),
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
        Cycle(
            (self.fields * u64::from(LINES) + u64::from(self.line)) * TICKS_PER_LINE
                + u64::from(self.cycle) * TICKS_PER_CYCLE,
        )
    }

    fn ports(&self) -> Vec<PortInfo> {
        [(0u8, "port 0"), (1, "port 1")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id: PortId(id),
                kind: PortKind::LogophorPort,
                name,
                connected: self.board.input.ports[usize::from(id)] != PortDev::None,
            })
            .collect()
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
                id: SLOT_PROGRAM,
                name: "program",
                kind: MediaKind::Program,
                inserted: self.program.media.is_some(),
            },
            MediaSlotInfo {
                id: SLOT_SD,
                name: "SD card",
                kind: MediaKind::Disk,
                inserted: self.board.card.media.is_some(),
            },
        ]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_PROGRAM => Some(&mut self.program as &mut dyn MediaSlot),
            SLOT_SD => Some(&mut self.board as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"C16M", 2, |w| {
            // RAM by segment, all-zero segments as one byte.
            for (seg, touched) in self.board.ram.chunks(1 << 16).zip(self.board.touched) {
                if !touched || seg.iter().all(|b| *b == 0) {
                    w.u8(0);
                } else {
                    w.u8(1);
                    w.raw(seg);
                }
            }
            w.u16(self.line);
            w.u16(self.cycle);
            w.u64(self.fields);
            w.bool(self.board.exit.is_some());
            w.u16(self.board.exit.unwrap_or(0));
            w.u16(self.fixed);
            w.u16(self.carry);
        });
        self.cpu.save(&mut w);
        self.board.lumiere.save(&mut w);
        self.board.babbage.save(&mut w);
        self.board.ints.save(&mut w);
        w.chunk(*b"CALL", 1, |w| self.board.calliope.save(w));
        w.chunk(*b"TGIO", 1, |w| {
            self.board.timers.save(w);
            self.board.input.save(w);
            self.board.sd.save(w);
        });
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"C16M", 2)?;
        let mut ram = vec![0; self.board.ram.len()];
        let mut touched = [false; RAM_SEGMENTS as usize];
        for (seg, t) in ram.chunks_mut(1 << 16).zip(touched.iter_mut()) {
            match c.u8()? {
                0 => {}
                1 => {
                    c.raw_into(seg)?;
                    *t = true;
                }
                _ => return Err(StateError::Invalid("RAM segment flag")),
            }
        }
        let line = c.u16()?;
        let cycle = c.u16()?;
        if line >= LINES || cycle >= LINE_CYCLES {
            return Err(StateError::Invalid("beam position"));
        }
        let fields = c.u64()?;
        let has_exit = c.bool()?;
        let exit = c.u16()?;
        let fixed = c.u16()?.min(LINE_SLOTS);
        let carry = c.u16()?.min(LINE_SLOTS);
        let mut cpu = self.cpu.clone();
        cpu.load(&mut r)?;
        let mut lumiere = self.board.lumiere.clone();
        lumiere.load(&mut r)?;
        let mut babbage = self.board.babbage.clone();
        babbage.load(&mut r)?;
        let mut ints = self.board.ints.clone();
        ints.load(&mut r)?;
        let mut calliope = self.board.calliope.clone();
        {
            let (_, mut c) = r.chunk(*b"CALL", 1)?;
            calliope.load(&mut c)?;
        }
        let (mut timers, mut tinput, mut sd) = (
            self.board.timers.clone(),
            self.board.input.clone(),
            self.board.sd.clone(),
        );
        {
            let (_, mut c) = r.chunk(*b"TGIO", 1)?;
            timers.load(&mut c)?;
            tinput.load(&mut c)?;
            sd.load(&mut c)?;
        }
        let mut input = self.input.clone();
        input.load(&mut r)?;
        self.board.ram = ram;
        self.board.touched = touched;
        self.board.exit = has_exit.then_some(exit);
        self.board.lumiere = lumiere;
        self.board.babbage = babbage;
        self.board.ints = ints;
        self.board.calliope = calliope;
        self.board.timers = timers;
        self.board.input = tinput;
        self.board.sd = sd;
        self.fixed = fixed;
        self.carry = carry;
        self.cpu = cpu;
        self.input = input;
        self.line = line;
        self.cycle = cycle;
        self.fields = fields;
        self.field_done = false;
        Ok(())
    }
}

impl DebugTarget for Logophor {
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
        Some((0..size.bytes()).fold(0, |v, i| {
            v | self.board.peek(addr.wrapping_add(i), Size::Byte) << (8 * i)
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        (0..size.bytes()).all(|i| {
            self.board
                .write_byte(addr.wrapping_add(i), (v >> (8 * i)) as u8)
        })
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&self.board, addr)
    }
}
