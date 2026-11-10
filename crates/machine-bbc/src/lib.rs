//! Acorn BBC Microcomputer Model B.
//!
//! 6502 at 2 MHz, 32 KB RAM, the 16 KB MOS ROM, up to 16 sideways ROMs at
//! `&8000`, the 6845 CRTC (HD6845S, `chips-amstrad` type 0) with the Video
//! ULA (`chips_acorn::VideoUla`), the system and user 6522 VIAs
//! (`chips-cbm`), the SN76489 sound generator (`chips-ti`) at 4 MHz.
//!
//! **Memory.** `&0000`–`&7FFF` RAM; `&8000`–`&BFFF` the sideways ROM ROMSEL
//! (`&FE30`) selects (BASIC in slot 15; an empty slot reads `&FF`);
//! `&C000`–`&FBFF` and `&FF00`–`&FFFF` the MOS; `&FC00`–`&FEFF` I/O
//! (FRED, JIM, SHEILA): `&FE00` 6845 address, `&FE01` data; `&FE08` the
//! 6850 ACIA (for now its status at rest); `&FE20` ULA
//! control, `&FE21` palette; `&FE30` ROMSEL; `&FE40` system VIA; `&FE60`
//! user VIA. Unfitted I/O reads `&FE`.
//!
//! **The 1 MHz bus.** FRED, JIM and the 6845, ACIA, serial ULA, VIAs and
//! ADC run at 1 MHz: an access to them stretches the CPU's cycle by one or
//! two 2 MHz cycles to the next 1 MHz edge (the CPU waits after the access
//! here). The VIAs count at 1 MHz.
//!
//! **System VIA.** Port A: the keyboard ([`keyboard`]) and the sound
//! chip's data. Port B: PB0–PB2 the address and PB3 the data of the
//! addressable latch IC32: bit 0 sound write (the SN76489 takes port A
//! when it goes low), 1–2 speech, 3 keyboard (low: read by PA, high: it
//! scans itself), 4–5 the screen size for the hardware wrap-around, 6–7
//! the CAPS and SHIFT LOCK lights; PB4–PB5 the joystick buttons (high: not
//! pressed). CA1 is the 6845's vertical sync, CA2 the keyboard.
//!
//! **Screen.** Graphics modes address `(MA × 8 + RA)`; with MA bit 12 set
//! the wrap-around subtracts the screen size (latch bits 4–5: 16, 8, 20 or
//! 10 KB). Teletext (MA bit 13) reads `&7C00 + (MA & &3FF)`; with the ULA's
//! teletext bit the bytes go to the SAA5050 (`chips_acorn::Saa5050`, its
//! 12 half dots spread over the character's 16 pixels; the scan line in
//! the row is RA, counted in half lines when the 6845 interlaces video).
//! Scan lines with RA bit 3 set are blank in the
//! graphics modes. The cursor: the 6845's (R10, R11, R14, R15, blink by
//! R10 bits 5–6) over the ULA's cursor segments.
//!
//! **Picture.** 16 MHz pixels: 768 × 288 from 176 pixels after the
//! horizontal sync and 24 lines after the vertical sync (the MOS's modes
//! sit in the middle). A field ends at the 6845's vertical sync.
//!
//! Options: `roms`: "stub" ([`stub`]). No models yet (the Model B; B+ and
//! Master later).
//!
//! ROMs (user-supplied): role "bbc-mos" (MOS 1.20, 16 KB), "bbc-basic"
//! (BASIC II, 16 KB).
//!
//! Sources: Acorn, *BBC Microcomputer Advanced User Guide* (1983): the
//! memory map, SHEILA, the 1 MHz bus, the system VIA and addressable
//! latch, the keyboard, the screen wrap-around, the video ULA; *BBC
//! Microcomputer Service Manual*; the 6845, 6522 and SN76489 data (see the
//! chip crates). The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod keyboard;
pub mod serial;
pub mod stub;

use chips_acorn::{Saa5050, VideoUla};
use chips_amstrad::crtc::{Crtc, CrtcOut, CrtcType};
use chips_cbm::via::Via;
use chips_ti::Sn76489;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use emu_core::audio::Decimator;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::hooked::HookedBus;
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use keyboard::Keyboard;
use serial::{Acia, Cassette};

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
pub const CPU_HZ: u32 = 2_000_000;
/// The SN76489's input clock.
const PSG_HZ: u32 = 4_000_000;

/// The picture: 16 MHz pixels.
pub const WIDTH: u32 = 768;
pub const HEIGHT: u32 = 288;
const WIN_X: u32 = 176;
const WIN_Y: u32 = 24;
const LINE_PIXELS: u32 = 1024;

/// The eight physical colours: bit 0 red, 1 green, 2 blue.
pub const PALETTE: [Rgb8; 8] = [
    Rgb8 { r: 0, g: 0, b: 0 },
    Rgb8 { r: 255, g: 0, b: 0 },
    Rgb8 { r: 0, g: 255, b: 0 },
    Rgb8 {
        r: 255,
        g: 255,
        b: 0,
    },
    Rgb8 { r: 0, g: 0, b: 255 },
    Rgb8 {
        r: 255,
        g: 0,
        b: 255,
    },
    Rgb8 {
        r: 0,
        g: 255,
        b: 255,
    },
    Rgb8 {
        r: 255,
        g: 255,
        b: 255,
    },
];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "bbc",
        name: "BBC Micro",
        models: &[],
        video_formats: &[],
        create: |config| Ok(Box::new(Bbc::new(config)?)),
    }
}

/// Where the raster stands and the picture.
#[derive(Clone, Debug)]
struct Video {
    fb: Vec<u8>,
    /// 16 MHz pixels since the horizontal sync began; lines since the
    /// vertical sync began.
    x: u32,
    y: u32,
    hsync: bool,
    vsync: bool,
    fields: u64,
    /// Cursor characters still to draw on this line.
    cursor_left: u8,
}

impl Video {
    fn new() -> Self {
        Self {
            fb: vec![0; (WIDTH * HEIGHT) as usize],
            x: 0,
            y: 0,
            hsync: false,
            vsync: false,
            fields: 0,
            cursor_left: 0,
        }
    }

    fn put(&mut self, px: &[u8]) {
        if self.y >= WIN_Y && self.y < WIN_Y + HEIGHT {
            let row = ((self.y - WIN_Y) * WIDTH) as usize;
            for (i, &c) in px.iter().enumerate() {
                let x = self.x + i as u32;
                if (WIN_X..WIN_X + WIDTH).contains(&x) {
                    self.fb[row + (x - WIN_X) as usize] = c;
                }
            }
        }
        self.x = (self.x + px.len() as u32).min(LINE_PIXELS * 2);
    }
}

/// Everything on the bus but the CPU.
#[derive(Clone, Debug)]
struct Board {
    ram: Vec<u8>,
    mos: Vec<u8>,
    /// The sideways ROMs (empty: an empty socket).
    roms: [Vec<u8>; 16],
    romsel: u8,
    crtc: Crtc,
    ula: VideoUla,
    saa: Saa5050,
    /// The SAA5050 has seen this scan line's start.
    saa_line: bool,
    sysvia: Via,
    uservia: Via,
    /// The addressable latch IC32.
    latch: u8,
    acia: Acia,
    /// The serial ULA's control register (`&FE10`).
    serproc: u8,
    psg: Sn76489,
    keyboard: Keyboard,
    video: Video,
    /// 2 MHz cycles since power-on.
    cycles: u64,
    field_done: bool,
    audio: Decimator,
    samples: Vec<i16>,
}

impl Board {
    /// The screen address of a character.
    fn screen_addr(&self, ma: u16, ra: u8) -> u16 {
        if ma & 0x2000 != 0 {
            return 0x7c00 | (ma & 0x3ff);
        }
        let mut a = ((ma & 0x0fff) << 3) | u16::from(ra & 7);
        if ma & 0x1000 != 0 {
            let size = match self.latch >> 4 & 3 {
                0 => 0x4000,
                1 => 0x2000,
                2 => 0x5000,
                _ => 0x2800,
            };
            a = a.wrapping_sub(size);
        }
        a & 0x7fff
    }

    /// The 6845's cursor is on this character.
    fn cursor_here(&self, out: &CrtcOut) -> bool {
        let r = &self.crtc.regs;
        let blink = match r[10] >> 5 & 3 {
            0 => true,
            1 => false,
            2 => self.video.fields & 8 != 0,
            _ => self.video.fields & 16 != 0,
        };
        let addr = (u16::from(r[14]) << 8 | u16::from(r[15])) & 0x3fff;
        blink
            && out.disp
            && out.ma == addr
            && (r[10] & 0x1f..=r[11] & 0x1f).contains(&out.ra)
            && r[8] & 0xc0 != 0xc0
    }

    /// One character from the 6845.
    fn character(&mut self) {
        let out = self.crtc.tick();
        let fast = self.ula.fast_clock();
        let n = if fast { 8 } else { 16 };
        if out.hsync && !self.video.hsync {
            self.video.x = 0;
            self.video.y += 1;
            self.saa_line = false;
        }
        if out.vsync && !self.video.vsync {
            self.video.y = 0;
            self.video.fields += 1;
            self.field_done = true;
            self.saa.new_field(self.video.fields);
        }
        self.video.hsync = out.hsync;
        self.video.vsync = out.vsync;
        let mut px = [0u8; 16];
        if out.disp && self.ula.teletext() {
            if !self.saa_line {
                self.saa_line = true;
                let interlaced = self.crtc.regs[8] & 3 == 3;
                self.saa
                    .new_line(if interlaced { out.ra } else { out.ra * 2 });
            }
            let byte = self.ram[usize::from(self.screen_addr(out.ma, out.ra))];
            let mut half = [0u8; 12];
            self.saa.character(byte, &mut half);
            for (i, p) in px[..n].iter_mut().enumerate() {
                *p = half[i * 12 / n];
            }
        } else if out.disp && out.ra & 8 == 0 {
            let byte = self.ram[usize::from(self.screen_addr(out.ma, out.ra))];
            self.ula.pixels(byte, &mut px[..n]);
        }
        if self.cursor_here(&out) {
            self.video.cursor_left = self.ula.cursor_width();
        }
        if self.video.cursor_left > 0 {
            self.video.cursor_left -= 1;
            for p in &mut px[..n] {
                *p ^= 7;
            }
        }
        self.video.put(&px[..n]);
    }

    /// One 2 MHz cycle of everything but the CPU.
    fn tick(&mut self) {
        if self.ula.fast_clock() || self.cycles & 1 == 0 {
            self.character();
        }
        if self.cycles & 1 == 1 {
            self.sysvia.tick();
            self.uservia.tick();
        }
        // CA1: vertical sync; CA2: the keyboard.
        let pa = self.sysvia.port_a_out();
        self.sysvia.set_ca1(self.video.vsync, pa);
        let scanning = self.latch & 0x08 != 0;
        let any = self
            .keyboard
            .any(if scanning { None } else { Some(pa & 15) });
        self.sysvia.set_ca2(any);
        for _ in 0..PSG_HZ / CPU_HZ {
            let level = self.psg.tick();
            if let Some(s) = self.audio.push(level - 16_382) {
                if self.samples.len() < emu_core::SAMPLE_RATE as usize * 4 {
                    self.samples.push(s);
                }
            }
        }
        self.cycles += 1;
    }

    /// The system VIA's port A as the pins carry it.
    fn sysvia_pa(&self) -> u8 {
        let out = self.sysvia.port_a_out();
        if self.latch & 0x08 == 0 && self.keyboard.down(out & 0x7f) {
            out | 0x80
        } else {
            out & 0x7f
        }
    }

    /// A write to the system VIA's port B: the addressable latch.
    fn latch_update(&mut self) {
        let pb = self.sysvia.port_b_out();
        let bit = pb & 7;
        let was = self.latch;
        if pb & 8 != 0 {
            self.latch |= 1 << bit;
        } else {
            self.latch &= !(1 << bit);
        }
        if was & 1 != 0 && self.latch & 1 == 0 {
            self.psg.write(self.sysvia.port_a_out());
        }
    }

    fn io_read(&mut self, addr: u16) -> u8 {
        match addr {
            0xfe00..=0xfe07 if addr & 1 == 1 => self.crtc.read(),
            0xfe40..=0xfe5f => {
                let pa = self.sysvia_pa();
                self.sysvia.read(
                    (addr & 15) as u8,
                    pa,
                    0xf0 | (self.sysvia.port_b_out() & 0x0f),
                )
            }
            0xfe60..=0xfe7f => self.uservia.read((addr & 15) as u8, 0xff, 0xff),
            0xfe08..=0xfe0f => self.acia.read(addr),
            _ => self.io_peek(addr),
        }
    }

    fn io_peek(&self, addr: u16) -> u8 {
        match addr {
            0xfe00..=0xfe07 if addr & 1 == 1 => self.crtc.read(),
            0xfe40..=0xfe5f => self.sysvia.peek(
                (addr & 15) as u8,
                self.sysvia_pa(),
                0xf0 | (self.sysvia.port_b_out() & 0x0f),
            ),
            0xfe60..=0xfe7f => self.uservia.peek((addr & 15) as u8, 0xff, 0xff),
            0xfe08..=0xfe0f => self.acia.peek(addr),
            0xfc00..=0xfdff => 0xff,
            _ => 0xfe,
        }
    }

    fn io_write(&mut self, addr: u16, v: u8) {
        match addr {
            0xfe00..=0xfe07 => {
                if addr & 1 == 0 {
                    self.crtc.select(v);
                } else {
                    self.crtc.write(v);
                }
            }
            0xfe08..=0xfe0f => self.acia.write(addr, v),
            0xfe10..=0xfe1f => self.serproc = v,
            0xfe20..=0xfe2f => self.ula.write(addr, v),
            0xfe30..=0xfe3f => self.romsel = v & 15,
            0xfe40..=0xfe5f => {
                let reg = (addr & 15) as u8;
                self.sysvia.write(reg, v);
                if matches!(reg, 0 | 2) {
                    self.latch_update();
                }
            }
            0xfe60..=0xfe7f => self.uservia.write((addr & 15) as u8, v),
            _ => {}
        }
    }

    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0xfc00..=0xfeff => self.io_read(addr),
            _ => self.peek(addr),
        }
    }

    fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7fff => self.ram[usize::from(addr)],
            0x8000..=0xbfff => self.roms[usize::from(self.romsel)]
                .get(usize::from(addr - 0x8000))
                .copied()
                .unwrap_or(0xff),
            0xfc00..=0xfeff => self.io_peek(addr),
            _ => self.mos[usize::from(addr - 0xc000)],
        }
    }

    fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0x0000..=0x7fff => self.ram[usize::from(addr)] = v,
            0xfc00..=0xfeff => self.io_write(addr, v),
            _ => {}
        }
    }
}

/// The 1 MHz devices.
fn slow(addr: u16) -> bool {
    matches!(addr, 0xfc00..=0xfdff | 0xfe00..=0xfe1f | 0xfe40..=0xfe7f | 0xfec0..=0xfedf)
}

/// The CPU's bus for one cycle: notes a 1 MHz access.
struct BbcBus<'a> {
    b: &'a mut Board,
    slow: bool,
}

impl Bus for BbcBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        self.slow |= slow(a);
        u32::from(self.b.read(a))
    }
    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.b.peek(addr as u16))
    }
    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        self.slow |= slow(a);
        self.b.write(a, v as u8);
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

pub struct Bbc {
    cpu: Cpu6502,
    board: Board,
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
    cassette: Cassette,
}

/// The cassette's media slot.
pub const SLOT_CASSETTE: SlotId = SlotId(0);

fn rom(config: &MachineConfig, role: &str) -> Result<Vec<u8>, ConfigError> {
    let r = config
        .roms
        .get(role)
        .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
    if r.len() != 0x4000 {
        return Err(ConfigError::Invalid(format!(
            "{role} must be 16384 bytes, got {}",
            r.len()
        )));
    }
    Ok(r.clone())
}

impl Bbc {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        if let Some(other) = config.model.as_deref() {
            return Err(ConfigError::UnknownModel(other.into()));
        }
        let mut roms: [Vec<u8>; 16] = Default::default();
        let mos = match config.options.get("roms").map(String::as_str) {
            Some("stub") => stub::rom(),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => {
                roms[15] = rom(config, "bbc-basic")?;
                rom(config, "bbc-mos")?
            }
        };
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Nmos6502,
            ..CpuConfig::default()
        });
        cpu.reset();
        Ok(Self {
            cpu,
            board: Board {
                ram: vec![0; 0x8000],
                mos,
                roms,
                romsel: 0,
                crtc: Crtc::new(CrtcType::T0),
                ula: VideoUla::new(),
                saa: Saa5050::new(),
                saa_line: false,
                sysvia: Via::new(),
                uservia: Via::new(),
                latch: 0,
                acia: Acia::new(),
                serproc: 0,
                psg: Sn76489::new(),
                keyboard: Keyboard::default(),
                video: Video::new(),
                cycles: 0,
                field_done: false,
                audio: Decimator::new(PSG_HZ, emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                samples: Vec::new(),
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
            cassette: Cassette::new(),
        })
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.video;
        BeamPos {
            frame: v.fields,
            line: v.y as u16,
            cycle: (v.x / 8) as u16,
            hpos: v.x.saturating_sub(WIN_X).min(WIDTH - 1) as u16,
        }
    }

    /// One 2 MHz cycle of the board and the cassette (motor: the serial
    /// ULA's bit 7; RS423 selected disconnects the tape from the ACIA).
    fn board_tick(&mut self) {
        self.board.tick();
        let b = &mut self.board;
        if b.serproc & 0x40 == 0 {
            self.cassette.tick(b.serproc & 0x80 != 0, &mut b.acia);
        } else {
            b.acia.set_dcd(false);
        }
    }

    /// One CPU cycle (and the wait of a 1 MHz access).
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        self.board_tick();
        let b = &self.board;
        self.cpu
            .set_irq(b.sysvia.irq() || b.uservia.irq() || b.acia.irq());
        let beam = self.beam_pos();
        let (result, slow_access) = {
            let mut bus = BbcBus {
                b: &mut self.board,
                slow: false,
            };
            let r = {
                let mut hooked = HookedBus {
                    inner: &mut bus,
                    dbg: dbg.as_deref_mut(),
                    space: CPU_SPACE,
                    beam,
                };
                self.cpu.tick(&mut hooked)
            };
            (r, bus.slow)
        };
        let entered = self.cpu.take_interrupt();
        self.sched.tick();
        if slow_access {
            // Stretched to the next 1 MHz edge.
            for _ in 0..1 + (self.board.cycles & 1) {
                self.board_tick();
                self.sched.tick();
            }
        }
        let field = std::mem::take(&mut self.board.field_done);
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
                self.field_done_pending |= field;
                return Some(stop);
            }
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
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.board.keyboard.key(machine_key.0, down);
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) => match id.0 {
                0 => self.reset(ResetKind::Hard),
                keyboard::BREAK => self.reset(ResetKind::Soft),
                _ => {}
            },
            _ => {}
        }
    }
}

/// A byte as the CPU sees it (tests and tools).
pub fn peek_cpu(m: &Bbc, addr: u16) -> u8 {
    m.board.peek(addr)
}

impl Machine for Bbc {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
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

    /// Hard: power on (RAM cleared, the VIAs reset: the MOS sees a
    /// power-up). Soft: BREAK, the CPU only.
    fn reset(&mut self, kind: ResetKind) {
        if kind == ResetKind::Hard {
            let b = &mut self.board;
            b.ram.fill(0);
            b.sysvia.reset();
            b.uservia.reset();
            b.crtc = Crtc::new(CrtcType::T0);
            b.ula = VideoUla::new();
            b.saa = Saa5050::new();
            b.psg = Sn76489::new();
            b.latch = 0;
            b.romsel = 0;
            b.acia = Acia::new();
            b.serproc = 0;
        }
        self.cpu.reset();
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: WIDTH,
            height: HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.board.video.fb,
                palette: &PALETTE,
            },
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 16_000_000,
                active_pixels: WIDTH,
                total_pixels: LINE_PIXELS,
                lines_per_field: 312,
                standard: VideoStandard::Pal,
                output: OutputType::Rgb,
                field: FieldParity::Progressive,
            },
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
        vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 16,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let b = &self.board;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        const R: [&str; 18] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14", "R15", "R16", "R17",
        ];
        let crtc = R
            .iter()
            .zip(b.crtc.regs)
            .map(|(n, v)| reg(*n, u32::from(v), 8))
            .collect();
        vec![
            ChipInfo {
                name: "crtc",
                regs: crtc,
            },
            ChipInfo {
                name: "ula",
                regs: vec![
                    reg("CTRL", u32::from(b.ula.ctrl), 8),
                    reg("ROMSEL", u32::from(b.romsel), 4),
                    reg("LATCH", u32::from(b.latch), 8),
                ],
            },
            ChipInfo {
                name: "acia",
                regs: vec![
                    reg("CONTROL", u32::from(b.acia.control), 8),
                    reg("STATUS", u32::from(b.acia.status()), 8),
                    reg("SERPROC", u32::from(b.serproc), 8),
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
            emu_core::ControlInfo::button(keyboard::BREAK, "BREAK", false),
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
        vec![MediaSlotInfo {
            id: SLOT_CASSETTE,
            name: "cassette",
            kind: MediaKind::Tape,
            inserted: self.cassette.has_tape(),
        }]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        (slot == SLOT_CASSETTE).then_some(&mut self.cassette as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"BBCB", 2, |w| {
            w.bool(self.field_done_pending);
            w.bytes(&b.ram);
            w.u8(b.romsel);
            w.u8(b.latch);
            b.acia.save(w);
            w.u8(b.serproc);
            self.cassette.save(w);
            b.ula.save(w);
            b.saa.save(w);
            w.bool(b.saa_line);
            b.keyboard.save(w);
            let v = &b.video;
            w.u32(v.x);
            w.u32(v.y);
            w.bool(v.hsync);
            w.bool(v.vsync);
            w.u64(v.fields);
            w.u8(v.cursor_left);
            w.u64(b.cycles);
            w.bool(b.field_done);
            b.audio.save(w);
        });
        self.cpu.save(&mut w);
        b.crtc.save(&mut w);
        b.sysvia.save(&mut w);
        b.uservia.save(&mut w);
        b.psg.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"BBCB", 2)?;
        let field_done_pending = c.bool()?;
        let mut b = self.board.clone();
        let ram = c.bytes()?;
        if ram.len() != b.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        b.ram = ram.to_vec();
        b.romsel = c.u8()? & 15;
        b.latch = c.u8()?;
        b.acia.load(&mut c)?;
        b.serproc = c.u8()?;
        let mut tape = Cassette::new();
        tape.load(&mut c)?;
        b.ula.load(&mut c)?;
        b.saa.load(&mut c)?;
        b.saa_line = c.bool()?;
        b.keyboard.load(&mut c)?;
        b.video.x = c.u32()?.min(LINE_PIXELS * 2);
        b.video.y = c.u32()?;
        b.video.hsync = c.bool()?;
        b.video.vsync = c.bool()?;
        b.video.fields = c.u64()?;
        b.video.cursor_left = c.u8()?.min(4);
        b.cycles = c.u64()?;
        b.field_done = c.bool()?;
        b.audio.load(&mut c)?;
        b.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        b.crtc.load(&mut r)?;
        b.sysvia.load(&mut r)?;
        b.uservia.load(&mut r)?;
        b.psg.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        self.cpu = cpu;
        self.board = b;
        self.cassette.adopt(tape);
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for Bbc {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        (space == CPU_SPACE).then(|| {
            (0..size.bytes()).fold(0, |v, i| {
                v | u32::from(self.board.peek(addr.wrapping_add(i) as u16)) << (8 * i)
            })
        })
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i) as u16;
            if a >= 0x8000 {
                return false;
            }
            self.board.ram[usize::from(a)] = (v >> (8 * i)) as u8;
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.board), addr)
    }
}

#[cfg(test)]
mod tests;
