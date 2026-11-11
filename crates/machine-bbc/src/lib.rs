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
//! Models ([`Model`]): "b" (default), "bplus", "bplus128", "master".
//! Options: `roms`: "stub" ([`stub`]); Master: `rtc` (the clock's start,
//! `YYYY-MM-DD[THH:MM[:SS]]`), `cmos` ("configured", the default, or
//! "blank").
//!
//! ROMs (user-supplied): role "bbc-mos" (MOS 1.20, 16 KB), "bbc-basic"
//! (BASIC II, 16 KB, sideways slot 15), optional "bbc-dfs" (DFS 1.20,
//! slot 14): with it the 8271 disc interface and its two drives
//! ([`disc`]).
//!
//! Sources: Acorn, *BBC Microcomputer Advanced User Guide* (1983): the
//! memory map, SHEILA, the 1 MHz bus, the system VIA and addressable
//! latch, the keyboard, the screen wrap-around, the video ULA; *BBC
//! Microcomputer Service Manual*; the 6845, 6522 and SN76489 data (see the
//! chip crates). The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod disc;
pub mod keyboard;
pub mod serial;
pub mod stub;

use chips_acorn::{Saa5050, VideoUla};
use chips_amstrad::crtc::{Crtc, CrtcOut, CrtcType};
use chips_cbm::via::Via;
use chips_ti::Sn76489;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use disc::Disc;
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
/// B+: the shadow RAM (addresses `&3000`–`&7FFF` as the CPU sees them) and
/// the private RAM (`&8000`–`&AFFF`).
pub const SHADOW_SPACE: MemSpaceId = MemSpaceId(1);
pub const PRIVATE_SPACE: MemSpaceId = MemSpaceId(2);
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

/// The models (`--model`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// Model B (default, "b"): 32 KB, the 8271.
    B,
    /// Model B+ ("bplus"): 64 KB (20 KB shadow, 12 KB private RAM), the
    /// 1770.
    BPlus,
    /// Model B+ 128 ("bplus128"): the B+ with 64 KB sideways RAM in slots
    /// 0, 1, 12 and 13.
    BPlus128,
    /// Master 128 ("master"): 65SC12, MOS 3.20 (BASIC IV, DFS, ADFS, View,
    /// ViewSheet, Edit, Terminal in banks 9–15), LYNNE/HAZEL/ANDY, sideways
    /// RAM in banks 4–7, the CMOS RAM and clock, the 1770 at `&FE24`.
    Master,
}

impl Model {
    fn parse(name: Option<&str>) -> Result<Self, ConfigError> {
        Ok(match name {
            None | Some("b") => Model::B,
            Some("bplus" | "b+") => Model::BPlus,
            Some("bplus128" | "b+128") => Model::BPlus128,
            Some("master" | "master128") => Model::Master,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        })
    }

    /// The B+ family (shadow and private RAM after the B+'s rules).
    fn plus(self) -> bool {
        matches!(self, Model::BPlus | Model::BPlus128)
    }

    fn master(self) -> bool {
        self == Model::Master
    }
}

/// The clock's registers and the CMOS RAM for the debugger (0–13, then
/// RAM bytes by their CMOS offset, 0 = address 14).
const RTC_REGS: [&str; 64] = [
    "SEC", "SECALM", "MIN", "MINALM", "HOUR", "HOURALM", "WDAY", "DATE", "MONTH", "YEAR", "A", "B",
    "C", "D", "CMOS0", "CMOS1", "CMOS2", "CMOS3", "CMOS4", "CMOS5", "CMOS6", "CMOS7", "CMOS8",
    "CMOS9", "CMOS10", "CMOS11", "CMOS12", "CMOS13", "CMOS14", "CMOS15", "CMOS16", "CMOS17",
    "CMOS18", "CMOS19", "CMOS20", "CMOS21", "CMOS22", "CMOS23", "CMOS24", "CMOS25", "CMOS26",
    "CMOS27", "CMOS28", "CMOS29", "CMOS30", "CMOS31", "CMOS32", "CMOS33", "CMOS34", "CMOS35",
    "CMOS36", "CMOS37", "CMOS38", "CMOS39", "CMOS40", "CMOS41", "CMOS42", "CMOS43", "CMOS44",
    "CMOS45", "CMOS46", "CMOS47", "CMOS48", "CMOS49",
];

/// The Master's sideways RAM banks.
const MASTER_SWRAM: [usize; 4] = [4, 5, 6, 7];
/// The Master's ROM banks from MOS 3.20 (role, bank).
const MASTER_BANKS: [(&str, usize); 7] = [
    ("master-dfs", 9),
    ("master-viewsheet", 10),
    ("master-edit", 11),
    ("master-basic", 12),
    ("master-adfs", 13),
    ("master-view", 14),
    ("master-terminal", 15),
];
/// The CMOS RAM of a new Master (bytes 0–49, RTC addresses 14–63): what
/// MOS 3.20's own `*CONFIGURE` writes for LANG 12 (BASIC), FILE 9 (DFS),
/// MODE 7, DELAY 50, REPEAT 8, CAPS, LOUD, NOBOOT, SCROLL, NODIR, FLOPPY,
/// FDRIVE 0, BAUD 7, DATA 4, PRINT 1, NOTUBE and TV 0,1, starting from the
/// blank RAM its "R power-on" leaves (all 0, the ROM frugal bits 6–7
/// &FF). Acorn shipped Masters configured; the MOS 3.20 R power-on does
/// not restore settings (MOS 3.50's does).
const DEFAULT_CMOS: [u8; 50] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0xc9, 0xff, 0xff, 0x00, 0x00, 0x17, 0xe0, 0x32, 0x08, 0x00, 0x38,
    0x82, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00,
];

/// The sideways RAM banks of the B+ 128.
const SWRAM_SLOTS: [usize; 4] = [0, 1, 12, 13];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "bbc",
        name: "BBC Micro",
        models: &["b", "bplus", "bplus128", "master"],
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
    model: Model,
    ram: Vec<u8>,
    /// B+: 20 KB shadow RAM behind `&3000`–`&7FFF` and 12 KB private RAM
    /// at `&8000`–`&AFFF` (empty on the Model B).
    shadow: Vec<u8>,
    private: Vec<u8>,
    /// ROMSEL bit 7 (B+): the private RAM is paged in.
    private_on: bool,
    /// ACCCON at `&FE34`. B+: bit 7 shadow RAM on (display and the VDU
    /// driver's accesses). Master: IRR TST IFJ ITU Y X E D (bit 7 to 0):
    /// IRR pulls IRQ, Y pages HAZEL at `&C000`–`&DFFF`, X sends every
    /// access to `&3000`–`&7FFF` to LYNNE, E only the VDU driver's, D
    /// displays LYNNE.
    acccon: u8,
    /// Master: 8 KB HAZEL (filing system workspace over the VDU driver).
    hazel: Vec<u8>,
    /// Master: the clock with the CMOS RAM.
    rtc: Option<chips_acorn::Mc146818>,

    /// The instruction running was fetched from the VDU driver's area
    /// (`&C000`–`&DFFF`, or `&A000`–`&AFFF` with the private RAM in).
    vdu: bool,
    mos: Vec<u8>,
    /// The sideways ROMs (empty: an empty socket).
    roms: [Vec<u8>; 16],
    /// Slots that are RAM (B+ 128).
    swram: [bool; 16],
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

    /// A byte of screen memory: from the shadow RAM while it is on.
    fn screen_byte(&self, a: u16) -> u8 {
        let on = if self.model.master() {
            self.acccon & 0x01 != 0
        } else {
            self.acccon & 0x80 != 0
        };
        if on && a >= 0x3000 && !self.shadow.is_empty() {
            self.shadow[usize::from(a - 0x3000)]
        } else {
            self.ram[usize::from(a)]
        }
    }

    /// The CPU reaches the shadow RAM at `&3000`–`&7FFF` now.
    fn cpu_shadow(&self) -> bool {
        if self.model.master() {
            self.acccon & 0x04 != 0 || (self.vdu && self.acccon & 0x02 != 0)
        } else {
            self.vdu && self.acccon & 0x80 != 0 && !self.shadow.is_empty()
        }
    }

    /// Master: HAZEL is paged in.
    fn hazel_on(&self) -> bool {
        self.model.master() && self.acccon & 0x08 != 0
    }

    /// The Master's IRR bit pulls the IRQ line.
    fn irr(&self) -> bool {
        self.model.master() && self.acccon & 0x80 != 0
    }

    /// The Master's clock sees the slow data bus: CE PB6, AS PB7, R/W
    /// latch bit 1, DS latch bit 2, the data on port A.
    fn rtc_bus(&mut self) {
        let pb = self.sysvia.port_b_out();
        let pa = self.sysvia.port_a_out();
        let (latch, rtc) = (self.latch, &mut self.rtc);
        if let Some(rtc) = rtc {
            rtc.bus(
                pb & 0x40 != 0,
                pb & 0x80 != 0,
                latch & 4 != 0,
                latch & 2 != 0,
                pa,
            );
        }
    }

    /// The byte the clock drives onto port A now.
    fn rtc_driving(&self) -> Option<u8> {
        let pb = self.sysvia.port_b_out();
        self.rtc
            .as_ref()
            .and_then(|r| r.driving(pb & 0x40 != 0, self.latch & 4 != 0, self.latch & 2 != 0))
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
            let byte = self.screen_byte(self.screen_addr(out.ma, out.ra));
            let mut half = [0u8; 12];
            self.saa.character(byte, &mut half);
            for (i, p) in px[..n].iter_mut().enumerate() {
                *p = half[i * 12 / n];
            }
        } else if out.disp && out.ra & 8 == 0 {
            let byte = self.screen_byte(self.screen_addr(out.ma, out.ra));
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
        // CA1: vertical sync; CA2: the keyboard. Autoscanning (latch bit 3
        // high) a counter steps through the 16 columns at 1 MHz and CA2 is
        // high while it is on a column with a key down: a key held from
        // power-on still makes edges after the MOS has cleared the flags.
        let pa = self.sysvia.port_a_out();
        self.sysvia.set_ca1(self.video.vsync, pa);
        let scanning = self.latch & 0x08 != 0;
        let column = if scanning {
            (self.cycles >> 1) as u8 & 15
        } else {
            pa & 15
        };
        let any = self.keyboard.any(Some(column));
        self.sysvia.set_ca2(any);
        for _ in 0..PSG_HZ / CPU_HZ {
            let level = self.psg.tick();
            if let Some(s) = self.audio.push(level - 16_382) {
                if self.samples.len() < emu_core::SAMPLE_RATE as usize * 4 {
                    self.samples.push(s);
                }
            }
        }
        if let Some(rtc) = &mut self.rtc {
            rtc.tick();
        }

        self.cycles += 1;
    }

    /// The system VIA's port A as the pins carry it.
    fn sysvia_pa(&self) -> u8 {
        if let Some(v) = self.rtc_driving() {
            return v;
        }
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
        self.rtc_bus();
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
            // The Master's ACCCON reads back (MOS 3.20 changes it with TSB
            // and TRB).
            0xfe34..=0xfe37 if self.model.master() => self.acccon,
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
            // Master: the video ULA at &FE20–&FE23 only (the drive control
            // and the 1770 follow, decoded with the disc interface).
            0xfe20..=0xfe23 if self.model.master() => self.ula.write(addr, v),
            0xfe24..=0xfe2f if self.model.master() => {}
            0xfe20..=0xfe2f => self.ula.write(addr, v),
            0xfe30..=0xfe3f if self.model.master() && addr & 0x0c == 0x04 => self.acccon = v,
            0xfe38..=0xfe3f if self.model.master() => {}
            // B+: ACCCON at &FE34–&FE37, ROMSEL (bit 7 the private RAM)
            // at &FE30–&FE33 and the mirrors.
            0xfe30..=0xfe3f if self.model.plus() && addr & 0x0c == 0x04 => self.acccon = v & 0x80,
            0xfe30..=0xfe3f => {
                self.romsel = v & 15;
                self.private_on = self.model != Model::B && v & 0x80 != 0;
            }
            0xfe40..=0xfe5f => {
                let reg = (addr & 15) as u8;
                self.sysvia.write(reg, v);
                if matches!(reg, 0 | 2) {
                    self.latch_update();
                } else {
                    self.rtc_bus();
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
            0x3000..=0x7fff if self.cpu_shadow() => self.shadow[usize::from(addr - 0x3000)],
            0x0000..=0x7fff => self.ram[usize::from(addr)],
            0x8000..=0xafff
                if self.private_on && usize::from(addr - 0x8000) < self.private.len() =>
            {
                self.private[usize::from(addr - 0x8000)]
            }
            0x8000..=0xbfff => self.roms[usize::from(self.romsel)]
                .get(usize::from(addr - 0x8000))
                .copied()
                .unwrap_or(0xff),
            0xc000..=0xdfff if self.hazel_on() => self.hazel[usize::from(addr - 0xc000)],
            0xfc00..=0xfeff => self.io_peek(addr),
            _ => self.mos[usize::from(addr - 0xc000)],
        }
    }

    fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0x3000..=0x7fff if self.cpu_shadow() => self.shadow[usize::from(addr - 0x3000)] = v,
            0x0000..=0x7fff => self.ram[usize::from(addr)] = v,
            0x8000..=0xafff
                if self.private_on && usize::from(addr - 0x8000) < self.private.len() =>
            {
                self.private[usize::from(addr - 0x8000)] = v;
            }
            0x8000..=0xbfff if self.swram[usize::from(self.romsel)] => {
                self.roms[usize::from(self.romsel)][usize::from(addr - 0x8000)] = v;
            }
            0xc000..=0xdfff if self.hazel_on() => self.hazel[usize::from(addr - 0xc000)] = v,
            0xfc00..=0xfeff => self.io_write(addr, v),
            _ => {}
        }
    }
}

/// The 1 MHz devices (the Master: the 1 MHz bus and the system VIA).
fn slow(model: Model, addr: u16) -> bool {
    if model.master() {
        matches!(addr, 0xfc00..=0xfdff | 0xfe40..=0xfe5f)
    } else {
        matches!(addr, 0xfc00..=0xfdff | 0xfe00..=0xfe1f | 0xfe40..=0xfe7f | 0xfec0..=0xfedf)
    }
}

/// The disc controller's addresses: the 8271 or the B+'s 1770 at
/// `&FE80`–`&FE9F`, the Master's 1770 at `&FE24`–`&FE2F`.
fn fdc_addr(model: Model, a: u16) -> bool {
    if model.master() {
        (0xfe24..=0xfe2f).contains(&a)
    } else {
        (0xfe80..=0xfe9f).contains(&a)
    }
}

struct BbcBus<'a> {
    b: &'a mut Board,
    disc: Option<&'a mut Disc>,
    slow: bool,
}

impl Bus for BbcBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        self.slow |= slow(self.b.model, a);
        match &mut self.disc {
            Some(d) if fdc_addr(self.b.model, a) => u32::from(d.read(a)),
            _ => u32::from(self.b.read(a)),
        }
    }
    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_byte(self.b, self.disc.as_deref(), addr as u16))
    }
    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        self.slow |= slow(self.b.model, a);
        match &mut self.disc {
            Some(d) if fdc_addr(self.b.model, a) => d.write(a, v as u8),
            _ => self.b.write(a, v as u8),
        }
    }
    fn addr_bits(&self) -> u8 {
        16
    }
}

/// A byte without side effects, the disc interface included.
fn peek_byte(b: &Board, disc: Option<&Disc>, a: u16) -> u8 {
    match disc {
        Some(d) if fdc_addr(b.model, a) => d.peek(a),
        _ => b.peek(a),
    }
}

struct PeekBus<'a>(&'a Board, Option<&'a Disc>);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }
    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek_byte(self.0, self.1, addr as u16))
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
    /// The 8271 and its drives (with the DFS ROM).
    disc: Option<Disc>,
}

/// The cassette's media slot.
pub const SLOT_CASSETTE: SlotId = SlotId(0);
/// The disc drives' media slots (DFS drives 0/2 and 1/3).
pub const SLOT_DRIVE0: SlotId = SlotId(1);
pub const SLOT_DRIVE1: SlotId = SlotId(2);

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
        let model = Model::parse(config.model.as_deref())?;
        let mut roms: [Vec<u8>; 16] = Default::default();
        let (mos_role, dfs_role) = if model.plus() {
            ("bbc-bplus-mos", "bbc-dfs226")
        } else {
            ("bbc-mos", "bbc-dfs")
        };
        let mos = match config.options.get("roms").map(String::as_str) {
            Some("stub") => stub::rom(),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None if model.master() => {
                for (role, bank) in MASTER_BANKS {
                    if config.roms.contains_key(role) {
                        roms[bank] = rom(config, role)?;
                    }
                }
                rom(config, "master-mos")?
            }
            None => {
                roms[15] = rom(config, "bbc-basic")?;
                if config.roms.contains_key(dfs_role) {
                    roms[14] = rom(config, dfs_role)?;
                }
                rom(config, mos_role)?
            }
        };
        let mut swram = [false; 16];
        let slots: &[usize] = match model {
            Model::BPlus128 => &SWRAM_SLOTS,
            Model::Master => &MASTER_SWRAM,
            _ => &[],
        };
        for &s in slots {
            swram[s] = true;
            roms[s] = vec![0; 0x4000];
        }
        // The Model B and B+ have a disc interface with their DFS ROM; the
        // Master always has its 1770.
        let disc = (model.master() || !roms[14].is_empty()).then(|| match model {
            Model::B => Disc::new(),
            Model::Master => Disc::new_master(),
            _ => Disc::new_1770(),
        });
        let (shadow, private, hazel) = match model {
            Model::B => (Vec::new(), Vec::new(), Vec::new()),
            Model::Master => (vec![0; 0x5000], vec![0; 0x1000], vec![0; 0x2000]),
            _ => (vec![0; 0x5000], vec![0; 0x3000], Vec::new()),
        };
        let rtc = if model.master() {
            let start = match config.options.get("rtc") {
                Some(t) => chips_acorn::mc146818::DateTime::parse(t).ok_or_else(|| {
                    ConfigError::Invalid(format!("rtc '{t}' (YYYY-MM-DD[THH:MM[:SS]])"))
                })?,
                None => chips_acorn::mc146818::DateTime::default(),
            };
            let mut rtc = chips_acorn::Mc146818::new(start, CPU_HZ);
            match config.options.get("cmos").map(String::as_str) {
                None | Some("configured") => rtc.regs[14..].copy_from_slice(&DEFAULT_CMOS),
                Some("blank") => {
                    rtc.regs[14 + 6] = 0xff;
                    rtc.regs[14 + 7] = 0xff;
                }
                Some(other) => {
                    return Err(ConfigError::Invalid(format!(
                        "cmos '{other}' (configured or blank)"
                    )))
                }
            }
            Some(rtc)
        } else {
            None
        };
        let keyboard = Keyboard::default();
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: if model.master() {
                Variant::Cmos65C02
            } else {
                Variant::Nmos6502
            },
            ..CpuConfig::default()
        });
        cpu.reset();
        Ok(Self {
            cpu,
            board: Board {
                model,
                ram: vec![0; 0x8000],
                shadow,
                private,
                private_on: false,
                acccon: 0,
                hazel,
                rtc,
                vdu: false,
                mos,
                roms,
                swram,
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
                keyboard,
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
            disc,
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
        if let Some(d) = &mut self.disc {
            d.tick();
        }
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
        // B+: an instruction fetched from the VDU driver's area reaches
        // the shadow RAM.
        if self.board.model != Model::B && self.cpu.at_instruction_boundary() {
            let pc = self.cpu.pc();
            self.board.vdu = (0xc000..=0xdfff).contains(&pc)
                || (self.board.model.plus()
                    && self.board.private_on
                    && (0xa000..=0xafff).contains(&pc));
        }
        self.board_tick();
        let b = &self.board;
        self.cpu
            .set_irq(b.sysvia.irq() || b.uservia.irq() || b.acia.irq() || b.irr());
        // The 8271's INT on NMI (edge-triggered: every data byte raises it
        // anew).
        self.cpu
            .set_nmi(self.disc.as_ref().is_some_and(|d| d.fdc.nmi()));
        let beam = self.beam_pos();
        let (result, slow_access) = {
            let mut bus = BbcBus {
                b: &mut self.board,
                disc: self.disc.as_mut(),
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
    peek_byte(&m.board, m.disc.as_ref(), addr)
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
            b.shadow.fill(0);
            b.private.fill(0);
            b.hazel.fill(0);
            b.private_on = false;
            b.acccon = 0;
            b.vdu = false;
            for s in 0..16 {
                if b.swram[s] {
                    b.roms[s].fill(0);
                }
            }
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
            if let Some(d) = &mut self.disc {
                d.fdc = d.fdc.power_on();
            }
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
        let mut spaces = vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 16,
            big_endian: false,
        }];
        if self.board.model != Model::B {
            for (id, name) in [(SHADOW_SPACE, "shadow"), (PRIVATE_SPACE, "private")] {
                spaces.push(MemSpace {
                    id,
                    name,
                    device: CPU,
                    addr_bits: 16,
                    big_endian: false,
                });
            }
        }
        spaces
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
        let mut chips = vec![
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
        ];
        if let Some(rtc) = &b.rtc {
            chips.push(ChipInfo {
                name: "rtc",
                regs: RTC_REGS
                    .iter()
                    .zip(rtc.regs)
                    .map(|(n, v)| reg(n, u32::from(v), 8))
                    .collect(),
            });
            chips.push(ChipInfo {
                name: "acccon",
                regs: vec![reg("ACCCON", u32::from(b.acccon), 8)],
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
        let mut slots = vec![MediaSlotInfo {
            id: SLOT_CASSETTE,
            name: "cassette",
            kind: MediaKind::Tape,
            inserted: self.cassette.has_tape(),
        }];
        if let Some(d) = &self.disc {
            for (id, name, drive) in [(SLOT_DRIVE0, "drive 0", 0), (SLOT_DRIVE1, "drive 1", 1)] {
                slots.push(MediaSlotInfo {
                    id,
                    name,
                    kind: MediaKind::Disk,
                    inserted: d.inserted(drive),
                });
            }
        }
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_CASSETTE => Some(&mut self.cassette as &mut dyn MediaSlot),
            SLOT_DRIVE0 | SLOT_DRIVE1 => self.disc.as_mut().map(|d| {
                d.selected = usize::from(slot == SLOT_DRIVE1);
                d as &mut dyn MediaSlot
            }),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"BBCB", 4, |w| {
            w.bool(self.field_done_pending);
            w.bytes(&b.ram);
            w.bytes(&b.shadow);
            w.bytes(&b.private);
            w.bool(b.private_on);
            w.u8(b.acccon);
            w.bytes(&b.hazel);

            if let Some(rtc) = &b.rtc {
                rtc.save(w);
            }
            w.bool(b.vdu);
            for s in 0..16 {
                if b.swram[s] {
                    w.bytes(&b.roms[s]);
                }
            }
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
        w.chunk(*b"BBCF", 1, |w| {
            w.bool(self.disc.is_some());
            if let Some(d) = &self.disc {
                d.save(w);
            }
        });
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"BBCB", 4)?;
        let field_done_pending = c.bool()?;
        let mut b = self.board.clone();
        let ram = c.bytes()?;
        if ram.len() != b.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        b.ram = ram.to_vec();
        for (name, mem) in [
            ("shadow RAM size", &mut b.shadow),
            ("private RAM size", &mut b.private),
        ] {
            let bytes = c.bytes()?;
            if bytes.len() != mem.len() {
                return Err(StateError::Invalid(name));
            }
            mem.copy_from_slice(bytes);
        }
        b.private_on = c.bool()?;
        b.acccon = c.u8()?;
        if !b.model.master() {
            b.acccon &= 0x80;
        }
        let hazel = c.bytes()?;
        if hazel.len() != b.hazel.len() {
            return Err(StateError::Invalid("HAZEL size"));
        }
        b.hazel.copy_from_slice(hazel);

        if let Some(rtc) = &mut b.rtc {
            rtc.load(&mut c)?;
        }
        b.vdu = c.bool()?;
        for s in 0..16 {
            if b.swram[s] {
                let bytes = c.bytes()?;
                if bytes.len() != 0x4000 {
                    return Err(StateError::Invalid("sideways RAM size"));
                }
                b.roms[s].copy_from_slice(bytes);
            }
        }
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
        let (_, mut c) = r.chunk(*b"BBCF", 1)?;
        if c.bool()? != self.disc.is_some() {
            return Err(StateError::Invalid("disc interface present or not"));
        }
        let disc = match &self.disc {
            Some(d) => Some(d.load(&mut c)?),
            None => None,
        };
        if let (Some(d), Some(parsed)) = (&mut self.disc, disc) {
            d.commit(parsed);
        }
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
        let b = &self.board;
        let extra = |mem: &[u8], base: u32| {
            (0..size.bytes()).try_fold(0u32, |v, i| {
                let a = addr.wrapping_add(i).checked_sub(base)? as usize;
                Some(v | u32::from(*mem.get(a)?) << (8 * i))
            })
        };
        if b.model != Model::B && space == SHADOW_SPACE {
            return extra(&b.shadow, 0x3000);
        }
        if b.model != Model::B && space == PRIVATE_SPACE {
            return extra(&b.private, 0x8000);
        }
        // The CPU space as code outside the VDU driver sees it: main RAM
        // at &3000–&7FFF whatever runs at the moment (the shadow RAM has
        // its own space).
        (space == CPU_SPACE).then(|| {
            (0..size.bytes()).fold(0, |v, i| {
                let a = addr.wrapping_add(i) as u16;
                let byte = if (0x3000..0x8000).contains(&a) {
                    b.ram[usize::from(a)]
                } else {
                    peek_byte(b, self.disc.as_ref(), a)
                };
                v | u32::from(byte) << (8 * i)
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
        self.cpu
            .disassemble(&PeekBus(&self.board, self.disc.as_ref()), addr)
    }
}

#[cfg(test)]
mod tests;
