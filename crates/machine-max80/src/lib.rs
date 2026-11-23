//! Lobo MAX-80 (Lobo Systems, 1982): a Z80 computer that runs the TRS-80's
//! LDOS and CP/M, software-compatible with the TRS-80 Model III but with
//! its own hardware.
//!
//! Z80B at 15.2 MHz / 3 = 5.07 MHz. 128 KB RAM (64 KB with `ram=64k`) in
//! four chunks of 32 KB, A1, A2, B1 and B2: bits 5–4 of the PIO's port A
//! pick the chunk at `$0000–$7FFF`, bits 7–6 the one at `$8000–$FFFF`.
//! Everything else sits in one 4 KB block that VLATCH's high nibble places
//! at any 4 KB page (the boot ROM runs with it at `$0000`, CP/M moves it
//! to `$F000`, LDOS to `$3000`, where the keyboard and the screen then lie
//! as on a TRS-80):
//!
//! | Block offset | What |
//! | --- | --- |
//! | `$000–$3FF` | by VLATCH bits 2–0: 0 the 512-byte boot ROM (twice), 1 RAM, 2–3 the second kilobyte of video RAM, 4–5 the first kilobyte of font RAM, 6–7 the second |
//! | `$400–$7CF` | RAM |
//! | `$7D0–$7FF` | the device registers ([`io`]); a write lands in the RAM below too |
//! | `$800–$8FF` | the keyboard ([`keyboard`]); writes go to RAM |
//! | `$900–$BFF` | RAM |
//! | `$C00–$FFF` | the first kilobyte of video RAM |
//!
//! The block's RAM is always chunk A1, at the address without its top
//! bit. Wait states: two on every device register but USTAT and on the RAM
//! at `$700–$7CF`, one on video and font RAM. IN and OUT go to the
//! expansion bus only (nothing there: reads give `$FF`).
//!
//! The boot ROM (512 bytes) reads the DIP switches for the boot device,
//! loads one sector from it to `$0400` and runs it; the operating systems
//! call its sector read at `$0003` while they load. No disk in the drive:
//! it waits. The picture ([`video`]) shows nothing readable until the
//! system has loaded its font.
//!
//! Media slots: "drive 0" to "drive 3" (kind Disk): ImageDisk (IMD), DMK,
//! JV1 or JV3 images; a drive is on the 8-inch connector when its image
//! has 8-inch tracks.
//!
//! Options:
//! - `ram`: "128k" (default) or "64k".
//! - `boot`: the boot device on the DIP switches: "auto" (default: by the
//!   disk in drive 0, an 8-inch or a 5.25-inch floppy), "5", "8", "none"
//!   (the ROM beeps and waits).
//! - `roms`: "stub" runs a small built-in program ([`stub`]).
//!
//! Panel controls: RESET and POWER.
//!
//! ROM user-supplied, never in the repository: "boot" (512 bytes, Lobo's
//! boot ROM, `max80.e12`).
//!
//! Sources: see [`io`]. The implementation is this project's own.

#![forbid(unsafe_code)]

/// The floppy disk controller (the MB8876 takes the FD1793's commands on
/// an inverted data bus).
pub use drives_other::wd17xx as fdc;
pub mod io;
pub mod keyboard;
pub mod stub;
pub mod video;

use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use io::{Io, CPU_HZ, DOT_HZ, DRIVES};
use mimic_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use mimic_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use mimic_core::machine::ChipReg;
use mimic_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use mimic_core::port::{PortDevice, PortError, PortId, PortInfo};
use mimic_core::registry::{ConfigError, MachineConfig, MachineEntry};
use mimic_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The RAM as it is fitted: A1, A2, B1, B2 in turn.
pub const RAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(2);
pub const FONT_SPACE: MemSpaceId = MemSpaceId(3);
pub const SLOT_DRIVE0: SlotId = SlotId(0);

pub const ROM_SIZE: usize = 512;
const CHUNK: usize = 0x8000;

/// Panel controls.
pub const CONTROL_RESET: u16 = 0;
pub const CONTROL_POWER: u16 = 1;

/// The interrupt acknowledge finds this on the data bus.
const INT_VECTOR: u8 = 0xfe;
/// The beep's level in the sound.
const BEEP_LEVEL: i16 = 8000;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "max80",
        name: "Lobo MAX-80",
        models: &["max80"],
        video_formats: &[],
        create: |config| Ok(Box::new(Max80::new(config)?)),
    }
}

/// What an address reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Rom(usize),
    Ram(usize),
    /// A chunk that is not fitted.
    Missing,
    Vram(usize),
    Font(usize),
    /// The keyboard at this block offset, over RAM.
    Keys(u16, usize),
    /// A write-only latch over RAM.
    Latch(u16, usize),
    /// A device register over RAM.
    Device(u16, usize),
}

/// Memory, keyboard and devices: everything on the processor's bus.
struct Board {
    /// Four chunks of 32 KB.
    ram: Vec<u8>,
    /// Chunks B1 and B2 are fitted.
    bank_b: bool,
    rom: Vec<u8>,
    vram: Vec<u8>,
    font: Vec<u8>,
    /// Keyboard rows (1 = pressed).
    keys: [u8; 8],
    io: Io,
}

impl Board {
    fn place(&self, addr: u16) -> Place {
        if addr >> 12 != u16::from(self.io.vlatch >> 4) {
            let pa = self.io.pio_a;
            let chunk = usize::from(if addr & 0x8000 != 0 { pa >> 6 } else { pa >> 4 } & 3);
            return if chunk < 2 || self.bank_b {
                Place::Ram(chunk * CHUNK + usize::from(addr & 0x7fff))
            } else {
                Place::Missing
            };
        }
        let off = addr & 0xfff;
        let ram = usize::from(addr & 0x7fff);
        match off {
            0..=0x3ff => match self.io.vlatch & 7 {
                0 => Place::Rom(usize::from(off) & (ROM_SIZE - 1)),
                1 => Place::Ram(ram),
                2 | 3 => Place::Vram(0x400 + usize::from(off)),
                4 | 5 => Place::Font(usize::from(off)),
                _ => Place::Font(0x400 + usize::from(off)),
            },
            0x7d0..=0x7df => Place::Latch(off, ram),
            0x7e0..=0x7ff => Place::Device(off, ram),
            0x800..=0x8ff => Place::Keys(off, ram),
            0xc00..=0xfff => Place::Vram(usize::from(off) - 0xc00),
            _ => Place::Ram(ram),
        }
    }

    /// The wait states of an access to `addr`.
    fn waits(&self, addr: u16) -> u32 {
        match self.place(addr) {
            Place::Device(off, _) if off & !3 != 0x7f4 => 2,
            Place::Latch(..) => 2,
            Place::Vram(_) | Place::Font(_) => 1,
            Place::Ram(_)
                if addr >> 12 == u16::from(self.io.vlatch >> 4)
                    && (0x700..0x7d0).contains(&(addr & 0xfff)) =>
            {
                2
            }
            _ => 0,
        }
    }

    fn peek(&self, addr: u32, now: u64) -> u8 {
        if addr >= IO_BASE {
            return 0xff;
        }
        match self.place(addr as u16) {
            Place::Rom(i) => self.rom[i],
            Place::Ram(i) | Place::Latch(_, i) => self.ram[i],
            Place::Missing => 0xff,
            Place::Vram(i) => self.vram[i],
            Place::Font(i) => self.font[i],
            Place::Keys(off, _) => keyboard::read(&self.keys, off),
            Place::Device(off, _) => self.io.peek(off, now),
        }
    }

    fn read(&mut self, addr: u32, now: u64) -> u8 {
        if addr < IO_BASE {
            if let Place::Device(off, _) = self.place(addr as u16) {
                return self.io.read(off, now);
            }
        }
        self.peek(addr, now)
    }

    fn write(&mut self, addr: u32, v: u8, now: u64) {
        if addr >= IO_BASE {
            return;
        }
        match self.place(addr as u16) {
            Place::Rom(_) | Place::Missing => {}
            Place::Ram(i) | Place::Keys(_, i) => self.ram[i] = v,
            Place::Vram(i) => self.vram[i] = v,
            Place::Font(i) => self.font[i] = v,
            Place::Latch(off, i) | Place::Device(off, i) => {
                self.ram[i] = v;
                self.io.write(off, v, now);
            }
        }
    }

    /// A write that changes memory only (the debugger's poke).
    fn poke(&mut self, addr: u16, v: u8) -> bool {
        match self.place(addr) {
            Place::Ram(i) => self.ram[i] = v,
            Place::Vram(i) => self.vram[i] = v,
            Place::Font(i) => self.font[i] = v,
            _ => return false,
        }
        true
    }
}

/// The Z80's bus for one instruction.
struct Max80Bus<'a> {
    board: &'a mut Board,
    /// Clock of the instruction's start and of the coming access.
    start: u64,
    at: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus for Max80Bus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = u32::from(self.board.read(addr, self.at));
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
        u32::from(self.board.peek(addr, self.at))
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
        self.board.write(addr, v as u8, self.at);
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for Max80Bus<'_> {
    fn wait_states(&mut self, _access: Access, t: u32) -> u32 {
        self.at = self.start + u64::from(t);
        0
    }

    fn wait_states_at(&mut self, access: Access, t: u32, addr: u16) -> u32 {
        let waits = match access {
            Access::Fetch | Access::Read | Access::Write => self.board.waits(addr),
            _ => 0,
        };
        self.at = self.start + u64::from(t + waits);
        waits
    }
}

struct PeekBus<'a>(&'a Board, u64);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.0.peek(addr, self.1))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

pub struct Max80 {
    cpu: Z80,
    board: Board,
    /// CPU clocks since power-on.
    sched: Scheduler,
    input: InputQueue,
    /// Clock of the next field's start and the clocks of the running one.
    next_field: u64,
    field_clocks: u64,
    fields: u64,
    pixels: Vec<u8>,
    palette: [Rgb8; 2],
    field_done_pending: bool,
    /// Sound samples made so far.
    samples: u64,
}

fn new_cpu() -> Z80 {
    let mut cpu = Z80::new();
    cpu.reset();
    cpu.set_int_vector(INT_VECTOR);
    cpu
}

impl Max80 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        match config.model.as_deref() {
            None | Some("max80") => {}
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        }
        let opt = |k: &str| config.options.get(k).map(String::as_str);
        let bank_b = match opt("ram") {
            None | Some("128k") => true,
            Some("64k") => false,
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "ram option '{o}' (64k, 128k)"
                )))
            }
        };
        let dip = match opt("boot") {
            None | Some("auto") => None,
            Some("5") => Some(1),
            Some("8") => Some(2),
            Some("none") => Some(0),
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "boot option '{o}' (auto, 5, 8, none)"
                )))
            }
        };
        let rom = match opt("roms") {
            Some("stub") => stub::rom(),
            Some(o) => return Err(ConfigError::Invalid(format!("roms option '{o}'"))),
            None => {
                let r = config
                    .roms
                    .get("boot")
                    .ok_or_else(|| ConfigError::MissingRom("boot".into()))?;
                if r.len() != ROM_SIZE {
                    return Err(ConfigError::Invalid(format!(
                        "boot ROM must be {ROM_SIZE} bytes, got {}",
                        r.len()
                    )));
                }
                r.clone()
            }
        };
        let field_clocks = u64::from(CPU_HZ / 60);
        Ok(Self {
            cpu: new_cpu(),
            board: Board {
                ram: vec![0; 4 * CHUNK],
                bank_b,
                rom,
                vram: vec![0; video::VRAM],
                font: vec![0; video::FONT],
                keys: [0; 8],
                io: Io::new(dip),
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            next_field: field_clocks,
            field_clocks,
            fields: 0,
            pixels: vec![0; (video::WIDTH * video::HEIGHT) as usize],
            // A green P31 phosphor.
            palette: [
                Rgb8 { r: 0, g: 0, b: 0 },
                Rgb8 {
                    r: 0x40,
                    g: 0xf0,
                    b: 0x60,
                },
            ],
            field_done_pending: false,
            samples: 0,
        })
    }

    fn beam_pos(&self) -> BeamPos {
        let into = self.sched.now().0 + self.field_clocks - self.next_field;
        // Clocks a line: 8 dots a character, 3 dots a clock.
        let htotal = self
            .board
            .io
            .geometry()
            .map_or(120, |g| u64::from(g.htotal));
        let dots = into * 3;
        let line_dots = htotal * 8;
        BeamPos {
            frame: self.fields,
            line: (dots / line_dots) as u16,
            cycle: (dots % line_dots / 3) as u16,
            hpos: (dots % line_dots) as u16,
        }
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let beam = self.beam_pos();
        let start = self.sched.now().0;
        let t = {
            let mut bus = Max80Bus {
                board: &mut self.board,
                start,
                at: start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            u64::from(self.cpu.step_timed(&mut bus))
        };
        self.sched.advance(t);
        let now = self.sched.now().0;
        self.board.io.tick(now);
        self.cpu.set_int(self.board.io.int());
        let entered = self.cpu.take_interrupt();
        let field = now >= self.next_field;
        if field {
            let io = &mut self.board.io;
            video::render(
                io.geometry(),
                io.wide(),
                &self.board.vram,
                &self.board.font,
                &mut self.pixels,
            );
            self.fields += 1;
            io.field_start = self.next_field;
            self.field_clocks = io
                .geometry()
                .map_or(u64::from(CPU_HZ / 60), |g| g.field_clocks());
            self.next_field += self.field_clocks;
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

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                let (row, bit) = (usize::from(machine_key.0 / 8), machine_key.0 % 8);
                if let Some(r) = self.board.keys.get_mut(row) {
                    if down {
                        *r |= 1 << bit;
                    } else {
                        *r &= !(1 << bit);
                    }
                }
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
        let g = self.board.io.geometry();
        SignalInfo {
            pixel_clock_hz: DOT_HZ as u32,
            active_pixels: video::WIDTH,
            total_pixels: g.map_or(960, |g| g.htotal * 8),
            lines_per_field: g.map_or(264, |g| g.lines),
            standard: VideoStandard::Ntsc,
            output: OutputType::Monochrome,
            field: FieldParity::Progressive,
        }
    }

    /// How long a typed key is held and released: three ticks each.
    fn key_clocks() -> u64 {
        io::tick_cycle(3)
    }

    /// Queue key presses that type `text` (tests and tools).
    pub fn type_text(&mut self, text: &str) {
        let hold = Self::key_clocks();
        let (events, _) = keyboard::typed_events(text, self.sched.now().0, hold, hold);
        for e in events {
            self.input.push(e);
        }
    }

    /// The character codes on the screen, a row for each displayed one.
    pub fn screen_codes(&self) -> Vec<Vec<u8>> {
        let Some(g) = self.board.io.geometry() else {
            return Vec::new();
        };
        (0..g.rows)
            .map(|row| {
                (0..g.cols)
                    .map(|col| {
                        let at = (u32::from(g.start) + row * g.cols + col) as usize;
                        self.board.vram[at & (video::VRAM - 1)]
                    })
                    .collect()
            })
            .collect()
    }

    /// The screen as text, a line for each displayed row (codes outside
    /// ASCII as `.`: what they show is the loaded font's choice), for
    /// tests.
    pub fn screen_text(&self) -> String {
        let mut s = String::new();
        for row in self.screen_codes() {
            s.extend(row.iter().map(|&c| match c {
                0x20..=0x7e => c as char,
                _ => '.',
            }));
            s.push('\n');
        }
        s
    }

    /// A byte as the CPU reads it now, without side effects (tests and
    /// tools).
    pub fn peek_cpu(&self, addr: u16) -> u8 {
        self.board.peek(u32::from(addr), self.sched.now().0)
    }

    /// The device registers (tests and tools).
    pub fn io(&self) -> &Io {
        &self.board.io
    }
}

impl Machine for Max80 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        for f in dbg.per_frame_freezes() {
            if f.space == CPU_SPACE && f.addr < IO_BASE {
                for i in 0..f.size.bytes() {
                    self.board
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

    /// Soft: the RESET button (the processor and the latches; memory
    /// stays). Hard: power off and on.
    fn reset(&mut self, kind: ResetKind) {
        self.cpu = new_cpu();
        self.board.io.reset();
        if kind != ResetKind::Soft {
            self.board.ram.fill(0);
            self.board.vram.fill(0);
            self.board.font.fill(0);
            self.board.keys = [0; 8];
            self.board.io.beeps.clear();
            self.input.clear();
            self.field_done_pending = false;
        }
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: video::WIDTH,
            height: video::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.pixels,
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    /// The beeper: a 1953.125 Hz square wave while a beep lasts.
    fn audio(&mut self, out: &mut Vec<i16>) -> mimic_core::AudioFormat {
        let rate = u128::from(mimic_core::SAMPLE_RATE);
        let clock = |sample: u64| (u128::from(sample) * u128::from(DOT_HZ) / (3 * rate)) as u64;
        let now = self.sched.now().0;
        // More than a second behind (nobody listened): skip ahead.
        if clock(self.samples) + u64::from(CPU_HZ) < now {
            self.samples = (u128::from(now) * 3 * rate / u128::from(DOT_HZ)) as u64;
        }
        let beeps = &mut self.board.io.beeps;
        loop {
            let at = clock(self.samples);
            if at >= now {
                break;
            }
            let on = beeps.iter().any(|&(a, b)| (a..b).contains(&at));
            // Half periods of 15625 / 8 Hz since power-on.
            let high = (u128::from(at) * 15_625 * 6 / (8 * u128::from(DOT_HZ))) & 1 != 0;
            out.push(match (on, high) {
                (false, _) => 0,
                (true, true) => BEEP_LEVEL,
                (true, false) => -BEEP_LEVEL,
            });
            self.samples += 1;
        }
        let done = clock(self.samples);
        beeps.retain(|&(_, b)| b > done);
        mimic_core::AudioFormat {
            sample_rate: mimic_core::SAMPLE_RATE,
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
            space(VRAM_SPACE, "video", 11),
            space(FONT_SPACE, "font", 11),
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        const CRTC: [&str; 18] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14", "R15", "R16", "R17",
        ];
        let io = &self.board.io;
        let base = u32::from(io.vlatch >> 4) << 12;
        let now = self.sched.now().0;
        let reg = |name, off: u32, value: u8| ChipReg {
            name,
            addr: Some(base + off),
            value: u32::from(value),
            bits: 8,
        };
        vec![
            ChipInfo {
                name: "io",
                regs: vec![
                    reg("MLATCH", 0x7d8, io.mlatch),
                    reg("VLATCH", 0x7dc, io.vlatch),
                    reg("SYSFLG", 0x7e0, io.peek(0x7e0, now)),
                    reg("USTAT", 0x7f4, io.peek(0x7f4, now)),
                    reg("DIPSW", 0x7f8, io.dip_switches()),
                    reg("PIO_A", 0x7fc, io.pio_a),
                    reg("PIO_B", 0x7fe, io.pio_b),
                    reg("BAUD_A", 0x7d0, io.baud[0]),
                    reg("BAUD_B", 0x7d4, io.baud[1]),
                ],
            },
            ChipInfo {
                name: "crtc",
                regs: CRTC
                    .iter()
                    .zip(io.crtc)
                    .map(|(&name, v)| ChipReg {
                        name,
                        addr: None,
                        value: u32::from(v),
                        bits: 8,
                    })
                    .collect(),
            },
            ChipInfo {
                name: "fdc",
                regs: vec![
                    reg("STATUS", 0x7ec, io.fdc.peek_status(now, &io.drives)),
                    reg("TRACK", 0x7ed, io.fdc.track),
                    reg("SECTOR", 0x7ee, io.fdc.sector),
                    reg("DATA", 0x7ef, io.fdc.data),
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

    /// Text as key presses, with a pause behind ENTER for the system to
    /// take the line in.
    fn typed_keys(&self, text: &str, start: Cycle) -> Option<(Vec<TimedInput>, Cycle)> {
        let hold = Self::key_clocks();
        Some(mimic_core::input::typed_lines(
            text,
            start,
            hold * 4,
            |line, from| keyboard::typed_events(line, from, hold, hold),
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
        const NAMES: [&str; DRIVES] = ["drive 0", "drive 1", "drive 2", "drive 3"];
        NAMES
            .iter()
            .enumerate()
            .map(|(d, name)| MediaSlotInfo {
                id: SlotId(SLOT_DRIVE0.0 + d as u8),
                name,
                kind: MediaKind::Disk,
                inserted: self.board.io.drives[d].image.is_some(),
            })
            .collect()
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        let d = usize::from(slot.0.checked_sub(SLOT_DRIVE0.0)?);
        self.board
            .io
            .drives
            .get_mut(d)
            .map(|d| d as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"MX80", 1, |w| {
            w.bool(self.board.bank_b);
            w.raw(&self.board.keys);
            w.u64(self.next_field);
            w.u64(self.field_clocks);
            w.u64(self.fields);
            w.bool(self.field_done_pending);
            w.u64(self.samples);
            w.raw(&self.pixels);
            w.raw(&self.board.ram);
            w.raw(&self.board.vram);
            w.raw(&self.board.font);
            self.board.io.save(w);
        });
        self.cpu.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"MX80", 1)?;
        if c.bool()? != self.board.bank_b {
            return Err(StateError::Invalid("RAM size differs"));
        }
        let mut keys = [0u8; 8];
        c.raw_into(&mut keys)?;
        let next_field = c.u64()?;
        let field_clocks = c.u64()?;
        let fields = c.u64()?;
        let field_done_pending = c.bool()?;
        let samples = c.u64()?;
        let mut pixels = vec![0u8; self.pixels.len()];
        c.raw_into(&mut pixels)?;
        let mut ram = vec![0u8; 4 * CHUNK];
        c.raw_into(&mut ram)?;
        let mut vram = vec![0u8; video::VRAM];
        c.raw_into(&mut vram)?;
        let mut font = vec![0u8; video::FONT];
        c.raw_into(&mut font)?;
        let mut io = Io::new(self.board.io.dip);
        io.load(&mut c)?;
        if pixels.iter().any(|&p| p > 1) {
            return Err(StateError::Invalid("MAX-80 pixels"));
        }
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let hz = u64::from(CPU_HZ);
        if !(hz / 100..=hz / 30).contains(&field_clocks)
            || next_field > sched.now().0 + field_clocks
            || next_field < sched.now().0
        {
            return Err(StateError::Invalid("MAX-80 field clock"));
        }
        self.cpu = cpu;
        self.cpu.set_int_vector(INT_VECTOR);
        self.sched = sched;
        self.input = input;
        self.board.keys = keys;
        self.next_field = next_field;
        self.field_clocks = field_clocks;
        self.fields = fields;
        self.field_done_pending = field_done_pending;
        self.samples = samples;
        self.pixels = pixels;
        self.board.ram = ram;
        self.board.vram = vram;
        self.board.font = font;
        self.board.io.restore(io);
        Ok(())
    }
}

impl DebugTarget for Max80 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u8> {
            match space {
                CPU_SPACE => Some(self.board.peek(a & 0x1_ffff, self.sched.now().0)),
                RAM_SPACE => self.board.ram.get(a as usize).copied(),
                VRAM_SPACE => self.board.vram.get(a as usize).copied(),
                FONT_SPACE => self.board.font.get(a as usize).copied(),
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0, |v, i| {
            Some(v | u32::from(byte(addr.wrapping_add(i))?) << (8 * i))
        })
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let (a, b) = (addr.wrapping_add(i), (v >> (8 * i)) as u8);
            let cell = match space {
                CPU_SPACE if a < IO_BASE => {
                    if !self.board.poke(a as u16, b) {
                        return false;
                    }
                    continue;
                }
                RAM_SPACE => self.board.ram.get_mut(a as usize),
                VRAM_SPACE => self.board.vram.get_mut(a as usize),
                FONT_SPACE => self.board.font.get_mut(a as usize),
                _ => None,
            };
            match cell {
                Some(c) => *c = b,
                None => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu
            .disassemble(&PeekBus(&self.board, self.sched.now().0), addr)
    }
}

#[cfg(test)]
mod tests;
