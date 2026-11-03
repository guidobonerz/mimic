//! Acorn Archimedes: A305, A310, A440 and A3000 as models.
//!
//! ARM2 at 8 MHz ([`cpu_arm`]), MEMC, VIDC and IOC ([`chips_acorn`]), the
//! keyboard's controller ([`keyboard`]), a 1772 disc controller and a
//! PCF8583 clock with battery-backed RAM.
//!
//! **Time** counts in clocks of the 8 MHz bus: RAM takes 1 for an S and 2
//! for an N cycle, ROM 4, 3 or 2 by MEMC's setting, I/O 4 (the real cycles
//! depend on the IOC's speed bits). Whole instructions run at once; the
//! chips catch up after each. Video DMA costs five clocks per 16 bytes, all
//! taken at the end of its line; screens are drawn a line at a time.
//!
//! **I/O** at $3200000 up: A21 selects IOC, A18–A16 the bank, A20–A19 the
//! speed. Bank 0 is IOC itself; bank 1 the 1772 (A3–A2 the register); bank
//! 5: $3350018 latch B (bit 3 /1772 reset), $3350040 latch A (bits 3–0 /drive
//! select, 4 /side 1, 5 /motor), $3350048 latch C. Serial, Econet and
//! podules are not fitted. IOC's C0 is SDA and C1 SCL of the clock's bus, C2
//! the disc's /READY, bit 7 of its control register shows vertical flyback.
//! A word written to the I/O bus travels on D31–D16, a byte on all lanes;
//! reads come on D15–D0.
//!
//! **Sound.** Sixteen bytes at a time by DMA from MEMC's pointer, one byte
//! per sound period through VIDC's exponential DAC and stereo image, summed
//! to 48 kHz stereo.
//!
//! Models and RAM: A305 512 KB, A310 1 MB, A3000 1 MB, A440 4 MB (`ram=512k|
//! 1m|2m|4m` overrides). ROM: option `os=3.11|3.10|3.00|2.00|1.20`, default
//! the newest found (roles "os311", "os310", "os300": 2 MB; "os200", "os120":
//! 512 KB). `roms=stub`: a loop of its own.
//!
//! Sources: VLSI "Acorn RISC Machine Family Data Manual" (1990); Acorn A500
//! Series Service Manual (I/O map, latches, interrupt wiring); Acorn A3000
//! Technical Reference Manual. The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod keyboard;

use chips_acorn::ioc;
use chips_acorn::vidc::{sound_level, stereo_gain};
use chips_acorn::wd1772::{Floppy, Wd1772};
use chips_acorn::{Ioc, Memc, Pcf8583, Target, Vidc};
use cpu_arm::{Arm2, ArmBus};
use emu_core::audio::Decimator;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, ControlInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use keyboard::Keyboard;

pub const CPU: DeviceId = DeviceId("cpu");
/// The ARM's view, as a supervisor sees it.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// RAM by physical address.
pub const RAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const CLOCK_HZ: u32 = 8_000_000;
pub const DRIVES: usize = 2;
pub const SLOT_DISK0: SlotId = SlotId(0);

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "archimedes",
        name: "Acorn Archimedes",
        models: &["a310", "a305", "a440", "a3000"],
        video_formats: &[],
        create: |config| Ok(Box::new(Archimedes::new(config)?)),
    }
}

/// Everything on the bus but the CPU.
#[derive(Clone)]
struct Board {
    ram: Vec<u8>,
    rom: Vec<u8>,
    memc: Memc,
    ioc: Ioc,
    vidc: Vidc,
    rtc: Pcf8583,
    fdc: Wd1772,
    keyboard: Keyboard,
    latch_a: u8,
    latch_b: u8,
    latch_c: u8,
    /// 8 MHz clocks since power-on.
    t: u64,
    /// `t` when the chips last caught up.
    synced: u64,
    line: u32,
    next_line: u64,
    next_sample: u64,
    next_key: u64,
    flyback: bool,
    frames: u64,
    frame_done: bool,
    sound: [u8; 16],
    sound_pos: u8,
    channel: u8,
    audio: [Decimator; 2],
    samples: Vec<i16>,
}

impl Board {
    fn ram_read(&self, addr: u32) -> u8 {
        self.ram[addr as usize & (self.ram.len() - 1)]
    }

    /// Where a physical address lands in the DRAM. MEMC multiplexes the
    /// address by page size; the 32K setting belongs to 1 Mbit × 4 chips,
    /// and smaller ones ignore the RA9 it drives, which is how RISC OS finds
    /// the memory size: 1 and 2 MB alias on bit 13, 512 KB on bits 12 and 13
    /// with bit 19 still distinct (seen in the ROM's sizing code; MEMC's
    /// table 2 is not legible in the data manual's text).
    fn ram_index(&self, addr: u32) -> usize {
        let len = self.ram.len();
        let a = addr as usize;
        let a = if self.memc.page_shift == 15 {
            match len {
                0x10_0000 | 0x20_0000 => a & !0x2000,
                0x8_0000 => (a & !0x3000 & 0x7_ffff) | (a >> 19 & 1) << 13,
                _ => a,
            }
        } else {
            a
        };
        a & (len - 1)
    }

    fn word_at(bytes: &[u8], addr: u32) -> u32 {
        let a = addr as usize & (bytes.len() - 1) & !3;
        u32::from_le_bytes([bytes[a], bytes[a + 1], bytes[a + 2], bytes[a + 3]])
    }

    /// The input side of IOC's control register.
    fn pins(&self, disks: &[Option<Floppy>]) -> u8 {
        let c = self.ioc.control;
        let sda = c & 1 != 0 && self.rtc.sda_out();
        let drive = self.fdc.drive.and_then(|d| disks.get(d)?.as_ref());
        // C2 high: a disc is in the selected drive (ADFS 3.11 only touches
        // the 1772 then; the service manual does not give the polarity).
        let ready = if drive.is_some() { 4 } else { 0 };
        (c & 0x3a) | u8::from(sda) | (ready & c) | 0x40 | if self.flyback { 0x80 } else { 0 }
    }

    fn io_read(&mut self, addr: u32, disks: &[Option<Floppy>]) -> u32 {
        if addr & 0x20_0000 == 0 {
            return 0xffff;
        }
        match addr >> 16 & 7 {
            0 => {
                let pins = self.pins(disks);
                u32::from(self.ioc.read(addr, pins))
            }
            1 => u32::from(self.fdc.read(addr >> 2 & 3)),
            _ => 0xffff,
        }
    }

    fn io_peek(&self, addr: u32, disks: &[Option<Floppy>]) -> u32 {
        if addr & 0x20_0000 == 0 {
            return 0xffff;
        }
        match addr >> 16 & 7 {
            0 => u32::from(self.ioc.peek(addr, self.pins(disks))),
            1 => u32::from(self.fdc.peek(addr >> 2 & 3)),
            _ => 0xffff,
        }
    }

    fn io_write(&mut self, addr: u32, v: u8, disks: &[Option<Floppy>]) {
        if addr & 0x20_0000 == 0 {
            return;
        }
        match addr >> 16 & 7 {
            0 => {
                self.ioc.write(addr, v);
                if addr & 0x7c == 0 {
                    self.rtc.lines(v & 1 != 0, v & 2 != 0);
                }
            }
            1 => self.fdc.write(addr >> 2 & 3, v, disks),
            5 => match addr & 0x7c {
                0x18 => {
                    if self.latch_b & 8 != 0 && v & 8 == 0 {
                        self.fdc.reset();
                    }
                    self.latch_b = v;
                }
                0x40 => {
                    self.latch_a = v;
                    self.fdc.drive = (0..4).find(|d| v >> d & 1 == 0);
                    self.fdc.side = u32::from(v & 0x10 == 0);
                }
                0x48 => self.latch_c = v,
                _ => {}
            },
            _ => {}
        }
    }

    /// A raster line ended.
    fn end_line(&mut self) {
        let vidc = &mut self.vidc;
        let (ram, memc) = (&self.ram, &mut self.memc);
        let mask = ram.len() - 1;
        let line = self.line;
        // Video runs only with DMA on; the picture stays black otherwise.
        let dma = memc.video_enabled();
        let (first, last) = vidc.display_lines();
        let bytes = vidc.line_bytes();
        if dma || !vidc.programmed() {
            let memc = std::cell::RefCell::new(memc);
            vidc.draw_line(
                line,
                || ram[memc.borrow_mut().next_video_byte() as usize & mask],
                || ram[memc.borrow_mut().next_cursor_byte() as usize & mask],
            );
        }
        if dma && (first..last).contains(&line) {
            // Five clocks per 16 bytes, taken from the processor.
            self.t += u64::from(bytes / 16 * 5);
        }
        self.line += 1;
        let total = self.vidc.total_lines();
        let (start, end) = if self.vidc.programmed() {
            self.vidc.display_lines()
        } else {
            (0, 256)
        };
        if self.line == end {
            self.flyback = true;
            self.ioc.flyback();
            self.memc.flyback();
        }
        if self.line >= total {
            self.line = 0;
            self.frame_done = true;
            self.frames += 1;
            if self.frames.is_multiple_of(50) {
                self.rtc.second();
            }
        }
        if self.line == start {
            self.flyback = false;
            self.memc.frame_start();
            self.vidc.frame_start();
        }
    }

    /// The next sound byte goes out.
    fn sound_tick(&mut self) {
        if self.sound_pos == 0 {
            let a = self.memc.next_sound_block() as usize;
            for (i, b) in self.sound.iter_mut().enumerate() {
                *b = self.ram[(a + i) & (self.ram.len() - 1)];
            }
        }
        let byte = self.sound[usize::from(self.sound_pos)];
        self.sound_pos = (self.sound_pos + 1) & 15;
        let (l, r) = stereo_gain(self.vidc.stereo[usize::from(self.channel)]);
        self.channel = (self.channel + 1) & 7;
        let level = sound_level(byte);
        self.push_sound(level * l / 8, level * r / 8);
    }

    fn push_sound(&mut self, l: i32, r: i32) {
        // One input sample per microsecond's worth of output: the sound
        // period sets how often the level changes.
        let n = self.vidc.sample_clocks() / 8;
        for _ in 0..n {
            let left = self.audio[0].push(l);
            let right = self.audio[1].push(r);
            if let (Some(a), Some(b)) = (left, right) {
                self.samples.push(a);
                self.samples.push(b);
            }
        }
    }

    /// Catch up with the processor's time.
    fn sync(&mut self, disks: &mut [Option<Floppy>]) {
        let dt = (self.t - self.synced) as u32;
        self.synced = self.t;
        self.ioc.tick(dt);
        if let Some(b) = self.ioc.sent.take() {
            self.keyboard.receive(b);
        }
        if self.t >= self.next_key && !self.ioc.rx_full() {
            if let Some(b) = self.keyboard.next_byte() {
                self.ioc.receive(b);
                let t3 = u64::from(self.ioc.timer_latch(3)) + 1;
                self.next_key = self.t + 11 * 16 * 2 * t3 * 4;
            }
        }
        self.fdc.tick(dt, disks);
        let mut lines = 0;
        if self.fdc.drq {
            lines |= ioc::FIQ_FLOPPY_DATA;
        }
        if self.fdc.intrq {
            lines |= ioc::FIQ_FLOPPY_INT;
        }
        self.ioc.lines_fiq = lines;
        self.ioc.lines_b = if self.memc.sirq { ioc::IRQ_B_SOUND } else { 0 };
        while self.t >= self.next_line {
            self.end_line();
            self.next_line += self.vidc.line_clocks();
        }
        while self.t >= self.next_sample {
            let period = u64::from(self.vidc.sample_clocks());
            self.next_sample += period;
            if self.memc.sound_enabled() {
                self.sound_tick();
            } else {
                self.push_sound(0, 0);
            }
        }
    }
}

/// The ARM's bus.
struct ArcBus<'a> {
    b: &'a mut Board,
    disks: &'a [Option<Floppy>],
    dbg: Option<&'a mut DebugHooks>,
}

impl ArcBus<'_> {
    /// Time for an access of this kind.
    fn charge(&mut self, target: Target, seq: bool) {
        self.b.t += match target {
            Target::Ram(_) => 1 + u64::from(!seq),
            Target::Rom(_) => self.b.memc.rom_clocks(true),
            Target::LowRom(_) => self.b.memc.rom_clocks(false),
            Target::Io(_) => 4,
            Target::Vidc | Target::Memc => 2,
            Target::Abort => 1,
        };
    }

    fn word(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32> {
        let target = self.b.memc.decode_access(addr, false, privileged);
        self.charge(target, seq);
        match target {
            Target::Ram(p) => {
                let i = self.b.ram_index(p) as u32;
                Some(Board::word_at(&self.b.ram, i))
            }
            Target::Rom(a) => Some(Board::word_at(&self.b.rom, a)),
            Target::LowRom(_) | Target::Vidc | Target::Memc => Some(0xffff_ffff),
            Target::Io(a) => Some(self.b.io_read(a, self.disks) | 0xffff_0000),
            Target::Abort => None,
        }
    }

    fn watch_read(&mut self, addr: u32, size: Size, v: u32) -> u32 {
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => dbg.on_read(CPU_SPACE, addr, size, v),
            _ => v,
        }
    }

    /// False if a freeze or watchpoint swallows the write.
    fn watch_write(&mut self, addr: u32, size: Size, v: u32) -> bool {
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.write_armed(CPU_SPACE) => dbg.on_write(CPU_SPACE, addr, size, v),
            _ => true,
        }
    }

    fn store(
        &mut self,
        addr: u32,
        word: u32,
        byte: Option<u8>,
        seq: bool,
        privileged: bool,
    ) -> bool {
        let target = self.b.memc.decode_access(addr, true, privileged);
        self.charge(target, seq);
        match target {
            Target::Ram(p) => {
                let i = self.b.ram_index(p);
                match byte {
                    Some(v) => self.b.ram[i] = v,
                    None => {
                        let a = i & !3;
                        self.b.ram[a..a + 4].copy_from_slice(&word.to_le_bytes());
                    }
                }
                true
            }
            Target::Io(a) => {
                // A word's bits 23–16 reach the 8-bit I/O registers.
                let v = byte.unwrap_or((word >> 16) as u8);
                self.b.io_write(a, v, self.disks);
                true
            }
            Target::Vidc => {
                self.b.vidc.write(word);
                true
            }
            Target::Memc => {
                self.b.memc.write(addr & 0x3ff_ffff);
                true
            }
            Target::Abort => false,
            Target::Rom(_) | Target::LowRom(_) => true,
        }
    }
}

impl ArmBus for ArcBus<'_> {
    fn fetch(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32> {
        self.word(addr, seq, privileged)
    }

    fn read_word(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32> {
        let v = self.word(addr, seq, privileged)?;
        Some(self.watch_read(addr, Size::Long, v))
    }

    fn read_byte(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u8> {
        let word = self.word(addr & !3, seq, privileged)?;
        let v = word >> ((addr & 3) * 8) & 0xff;
        Some(self.watch_read(addr, Size::Byte, v) as u8)
    }

    fn write_word(&mut self, addr: u32, v: u32, seq: bool, privileged: bool) -> bool {
        if !self.watch_write(addr, Size::Long, v) {
            return true;
        }
        self.store(addr, v, None, seq, privileged)
    }

    fn write_byte(&mut self, addr: u32, v: u8, seq: bool, privileged: bool) -> bool {
        if !self.watch_write(addr, Size::Byte, u32::from(v)) {
            return true;
        }
        let word = u32::from_le_bytes([v; 4]);
        self.store(addr, word, Some(v), seq, privileged)
    }

    fn idle(&mut self, n: u32) {
        self.b.t += u64::from(n);
    }
}

fn peek_byte(b: &Board, disks: &[Option<Floppy>], addr: u32) -> u8 {
    let word = |a: u32| match b.memc.peek_target(a) {
        Target::Ram(p) => Board::word_at(&b.ram, b.ram_index(p) as u32),
        Target::Rom(r) => Board::word_at(&b.rom, r),
        Target::Io(i) => b.io_peek(i, disks),
        _ => 0xffff_ffff,
    };
    (word(addr & !3) >> ((addr & 3) * 8)) as u8
}

struct PeekBus<'a>(&'a Board, &'a [Option<Floppy>]);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        (0..size.bytes()).fold(0, |v, i| {
            v | u32::from(peek_byte(self.0, self.1, addr.wrapping_add(i))) << (8 * i)
        })
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        26
    }
}

/// A floppy drive's slot.
#[derive(Default)]
struct DriveSlot {
    media: Option<Media>,
}

/// One of the machine's drives seen as a media slot.
struct DiskSlot<'a> {
    slot: &'a mut DriveSlot,
    disk: &'a mut Option<Floppy>,
}

impl MediaSlot for DiskSlot<'_> {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let floppy = Floppy::from_image(bytes)
            .ok_or_else(|| MediaError::Format("not an ADFS disc image".into()))?;
        *self.disk = Some(floppy);
        self.slot.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        *self.disk = None;
        self.slot.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = self.disk {
            d.write_protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        let (Some(d), Some(m)) = (self.disk.as_mut(), self.slot.media.as_mut()) else {
            return Ok(());
        };
        if d.dirty {
            m.backend.write(0, &d.bytes)?;
            m.backend.commit()?;
            d.dirty = false;
        }
        Ok(())
    }
}

/// The clock chip as this machine leaves the factory here: both floppy
/// drives, ADFS the filing system. Addresses as `*CONFIGURE FLOPPIES 2` and
/// `*CONFIGURE FILESYSTEM ADFS` write them under RISC OS 3.11; $3F holds
/// the sum of the settings.
fn default_cmos() -> Pcf8583 {
    let mut rtc = Pcf8583::new();
    rtc.ram[0xc7] = DRIVES as u8;
    rtc.ram[0x45] = 8;
    rtc.ram[0x3f] = rtc.ram[0xc7] + rtc.ram[0x45];
    rtc
}

pub struct Archimedes {
    cpu: Arm2,
    board: Board,
    disks: Vec<Option<Floppy>>,
    drives: [DriveSlot; DRIVES],
    /// Scratch for [`Machine::media_slot`]: a slot borrows two fields.
    slot: Option<usize>,
    input: InputQueue,
}

/// The newest OS at hand, or the one asked for.
fn pick_rom(config: &MachineConfig) -> Result<Vec<u8>, ConfigError> {
    const VERSIONS: [(&str, &str); 5] = [
        ("3.11", "os311"),
        ("3.10", "os310"),
        ("3.00", "os300"),
        ("2.00", "os200"),
        ("1.20", "os120"),
    ];
    let wanted = config.options.get("os").map(String::as_str);
    if let Some(w) = wanted {
        let Some((_, role)) = VERSIONS.iter().find(|(v, _)| *v == w) else {
            return Err(ConfigError::Invalid(format!("os option '{w}'")));
        };
        return config
            .roms
            .get(*role)
            .cloned()
            .ok_or_else(|| ConfigError::MissingRom((*role).into()));
    }
    VERSIONS
        .iter()
        .find_map(|(_, role)| config.roms.get(*role).cloned())
        .ok_or_else(|| ConfigError::MissingRom("os311".into()))
}

impl Archimedes {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let default_ram = match config.model.as_deref() {
            None | Some("a310" | "a3000") => 0x10_0000,
            Some("a305") => 0x8_0000,
            Some("a440") => 0x40_0000,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let ram = match config.options.get("ram").map(String::as_str) {
            None => default_ram,
            Some("512k") => 0x8_0000,
            Some("1m") => 0x10_0000,
            Some("2m") => 0x20_0000,
            Some("4m") => 0x40_0000,
            Some(other) => return Err(ConfigError::Invalid(format!("ram option '{other}'"))),
        };
        let rom = match config.options.get("roms").map(String::as_str) {
            // B . at every address.
            Some("stub") => [0xfe, 0xff, 0xff, 0xea].repeat(0x8000),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => pick_rom(config)?,
        };
        if !rom.len().is_power_of_two() {
            return Err(ConfigError::Invalid("ROM size".into()));
        }
        let audio = || {
            Decimator::new(1_000_000, emu_core::SAMPLE_RATE)
                .ok_or_else(|| ConfigError::Invalid("audio rate".into()))
        };
        let mut m = Self {
            cpu: Arm2::new(),
            board: Board {
                ram: vec![0; ram],
                rom,
                memc: Memc::new(),
                ioc: Ioc::new(),
                vidc: Vidc::new(),
                rtc: default_cmos(),
                fdc: Wd1772::new(),
                keyboard: Keyboard::new(),
                latch_a: 0xff,
                latch_b: 0xff,
                latch_c: 0,
                t: 0,
                synced: 0,
                line: 0,
                next_line: 512,
                next_sample: 8,
                next_key: 0,
                flyback: false,
                frames: 0,
                frame_done: false,
                sound: [0; 16],
                sound_pos: 0,
                channel: 0,
                audio: [audio()?, audio()?],
                samples: Vec::new(),
            },
            disks: vec![None; DRIVES],
            drives: Default::default(),
            slot: None,
            input: InputQueue::new(),
        };
        m.start_cpu();
        Ok(m)
    }

    fn start_cpu(&mut self) {
        let mut bus = ArcBus {
            b: &mut self.board,
            disks: &self.disks,
            dbg: None,
        };
        self.cpu.reset(&mut bus);
    }

    /// The CPU (tests and tools).
    pub fn cpu(&self) -> &Arm2 {
        &self.cpu
    }

    /// Key down or up (tests and tools).
    pub fn key(&mut self, key: u8, down: bool) {
        self.board.keyboard.key(key, down);
    }

    pub fn clocks(&self) -> u64 {
        self.board.t
    }

    /// The clock chip's registers and battery-backed RAM.
    pub fn cmos(&self) -> &[u8] {
        &self.board.rtc.ram
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.board.keyboard.key(machine_key.0 as u8, down);
            }
            (InputTarget::Port(_), InputEvent::MouseRel { dx, dy, buttons }) => {
                self.board
                    .keyboard
                    .mouse(i32::from(dx), i32::from(dy), buttons);
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) => match id.0 {
                0 => self.reset(ResetKind::Hard),
                1 => self.reset(ResetKind::Soft),
                _ => {}
            },
            _ => {}
        }
    }

    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(Cycle(self.board.t)) {
            self.apply_input(ev);
        }
        {
            let mut bus = ArcBus {
                b: &mut self.board,
                disks: &self.disks,
                dbg: dbg.as_deref_mut(),
            };
            self.cpu.step(&mut bus);
        }
        self.board.sync(&mut self.disks);
        self.cpu.set_irq(self.board.ioc.irq());
        self.cpu.set_firq(self.board.ioc.firq());
        if let Some(dbg) = dbg {
            let next_pc = self.cpu.next_pc();
            if dbg.trace_instr_armed() {
                dbg.trace_instr(CPU, next_pc, BeamPos::default());
            }
            if dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.exec_armed(CPU_SPACE) {
                dbg.check_exec(CPU_SPACE, next_pc);
            }
            if let Some(stop) = dbg.take_stop() {
                return Some(stop);
            }
        }
        None
    }

    fn signal(&self) -> SignalInfo {
        let v = &self.board.vidc;
        let (_, w, _) = v.picture();
        let total = if v.programmed() { 2 * v.hcr + 2 } else { 1024 };
        SignalInfo {
            // Before the OS sets a mode: the 16 MHz of its usual ones.
            pixel_clock_hz: if v.programmed() { v.pixel_mhz() } else { 16 } * 1_000_000,
            active_pixels: w,
            total_pixels: total,
            lines_per_field: v.total_lines(),
            standard: VideoStandard::Pal,
            output: OutputType::Rgb,
            field: FieldParity::Progressive,
        }
    }
}

impl Machine for Archimedes {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        for f in dbg.per_frame_freezes() {
            if f.space == RAM_SPACE {
                let mask = self.board.ram.len() - 1;
                for i in 0..f.size.bytes() {
                    self.board.ram[(f.addr + i) as usize & mask] = (f.value >> (8 * i)) as u8;
                }
            }
        }
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
            if std::mem::take(&mut self.board.frame_done) {
                return StopReason::FrameDone;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.instruction(None);
        StopReason::StepDone
    }

    fn reset(&mut self, kind: ResetKind) {
        let b = &mut self.board;
        b.memc.reset();
        b.ioc.reset();
        b.fdc.reset();
        if kind == ResetKind::Hard {
            b.ram.fill(0);
            b.vidc = Vidc::new();
            b.keyboard = Keyboard::new();
            b.memc = Memc::new();
            b.ioc = Ioc::new();
        }
        self.input.clear();
        self.start_cpu();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        let (pixels, width, height) = self.board.vidc.picture();
        FrameRef {
            width,
            height,
            pixels: PixelData::Rgb(pixels),
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 2,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(BeamPos {
            frame: self.board.frames,
            line: self.board.line as u16,
            cycle: 0,
            hpos: 0,
        })
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![
            MemSpace {
                id: CPU_SPACE,
                name: "cpu",
                device: CPU,
                addr_bits: 26,
                big_endian: false,
            },
            MemSpace {
                id: RAM_SPACE,
                name: "ram",
                device: CPU,
                addr_bits: 22,
                big_endian: false,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let b = &self.board;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        vec![
            ChipInfo {
                name: "memc",
                regs: vec![
                    reg("CONTROL", b.memc.control, 14),
                    reg("VINIT", b.memc.vinit, 19),
                    reg("VSTART", b.memc.vstart, 19),
                    reg("VEND", b.memc.vend, 19),
                    reg("VPTR", b.memc.vptr, 19),
                    reg("SPTR", b.memc.sptr, 19),
                    reg("PAGE0", b.memc.page_entry(0), 32),
                    reg("SHIFT", b.memc.page_shift, 4),
                ],
            },
            ChipInfo {
                name: "ioc",
                regs: vec![
                    reg("CONTROL", u32::from(b.ioc.control), 8),
                    reg("MASKA", u32::from(b.ioc.mask_a), 8),
                    reg("MASKB", u32::from(b.ioc.mask_b), 8),
                    reg("MASKF", u32::from(b.ioc.mask_fiq), 8),
                ],
            },
            ChipInfo {
                name: "vidc",
                regs: vec![
                    reg("CONTROL", b.vidc.control, 16),
                    reg("HCR", b.vidc.hcr, 10),
                    reg("VCR", b.vidc.vcr, 10),
                    reg("SFR", b.vidc.sfr, 8),
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
        Cycle(self.board.t)
    }

    fn controls(&self) -> Vec<ControlInfo> {
        vec![
            ControlInfo::button(0, "POWER", false),
            ControlInfo::button(1, "RESET", false),
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
        const NAMES: [&str; DRIVES] = [":0", ":1"];
        (0..DRIVES)
            .map(|i| MediaSlotInfo {
                id: SlotId(i as u8),
                name: NAMES[i],
                kind: MediaKind::Disk,
                inserted: self.disks[i].is_some(),
            })
            .collect()
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        let i = usize::from(slot.0);
        (i < DRIVES).then(|| {
            self.slot = Some(i);
            self as &mut dyn MediaSlot
        })
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"ARCH", 1, |w| {
            w.bytes(&b.ram);
            w.u8(b.latch_a);
            w.u8(b.latch_b);
            w.u8(b.latch_c);
            for v in [
                b.t,
                b.synced,
                b.next_line,
                b.next_sample,
                b.next_key,
                b.frames,
            ] {
                w.u64(v);
            }
            w.u32(b.line);
            w.bool(b.flyback);
            w.bool(b.frame_done);
            w.raw(&b.sound);
            w.u8(b.sound_pos);
            w.u8(b.channel);
            b.audio[0].save(w);
            b.audio[1].save(w);
        });
        self.cpu.save(&mut w);
        b.memc.save(&mut w);
        b.ioc.save(&mut w);
        b.vidc.save(&mut w);
        b.rtc.save(&mut w);
        b.fdc.save(&mut w);
        b.keyboard.save(&mut w);
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first. Disc contents stay as
    /// they are in the drives.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"ARCH", 1)?;
        let mut b = self.board.clone();
        let ram = c.bytes()?;
        if ram.len() != b.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        b.ram = ram.to_vec();
        b.latch_a = c.u8()?;
        b.latch_b = c.u8()?;
        b.latch_c = c.u8()?;
        b.t = c.u64()?;
        b.synced = c.u64()?;
        b.next_line = c.u64()?;
        b.next_sample = c.u64()?;
        b.next_key = c.u64()?;
        b.frames = c.u64()?;
        b.line = c.u32()?;
        b.flyback = c.bool()?;
        b.frame_done = c.bool()?;
        c.raw_into(&mut b.sound)?;
        b.sound_pos = c.u8()? & 15;
        b.channel = c.u8()? & 7;
        b.audio[0].load(&mut c)?;
        b.audio[1].load(&mut c)?;
        b.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        b.memc.load(&mut r)?;
        b.ioc.load(&mut r)?;
        b.vidc.load(&mut r)?;
        b.rtc.load(&mut r)?;
        b.fdc.load(&mut r)?;
        b.keyboard.load(&mut r)?;
        input.load(&mut r)?;
        self.cpu = cpu;
        self.input = input;
        self.board = b;
        Ok(())
    }
}

/// Reached through [`Machine::media_slot`]: acts on the drive it chose.
impl MediaSlot for Archimedes {
    fn dirty(&self) -> bool {
        let i = self.slot.unwrap_or(0);
        self.disks[i].as_ref().is_some_and(|d| d.dirty)
            || self.drives[i]
                .media
                .as_ref()
                .is_some_and(|m| m.backend.dirty())
    }

    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        let i = self.slot.unwrap_or(0);
        DiskSlot {
            slot: &mut self.drives[i],
            disk: &mut self.disks[i],
        }
        .insert(media)
    }

    fn eject(&mut self) -> Option<Media> {
        let i = self.slot.unwrap_or(0);
        DiskSlot {
            slot: &mut self.drives[i],
            disk: &mut self.disks[i],
        }
        .eject()
    }

    fn write_protect(&mut self, on: bool) {
        let i = self.slot.unwrap_or(0);
        DiskSlot {
            slot: &mut self.drives[i],
            disk: &mut self.disks[i],
        }
        .write_protect(on);
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        let i = self.slot.unwrap_or(0);
        DiskSlot {
            slot: &mut self.drives[i],
            disk: &mut self.disks[i],
        }
        .flush()
    }
}

impl DebugTarget for Archimedes {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| match space {
            CPU_SPACE => Some(u32::from(peek_byte(&self.board, &self.disks, a))),
            RAM_SPACE => Some(u32::from(self.board.ram_read(a))),
            _ => None,
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        let mask = self.board.ram.len() - 1;
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            let physical = match space {
                RAM_SPACE => Some(a),
                CPU_SPACE => match self.board.memc.peek_target(a) {
                    Target::Ram(p) => Some(p),
                    _ => None,
                },
                _ => None,
            };
            let Some(p) = physical else {
                return false;
            };
            self.board.ram[p as usize & mask] = byte;
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu
            .disassemble(&PeekBus(&self.board, &self.disks), addr)
    }
}

#[cfg(test)]
mod tests;
