//! Commodore 1541 and 1571 disk drives, true drive emulation.
//!
//! 1541: its own NMOS 6502 at 1 MHz, 2 KB RAM, 16 KB DOS ROM and two VIAs.
//! Memory map: `$0000–$07FF` RAM (mirrored up to `$17FF`), `$1800` VIA1
//! (IEC), `$1C00` VIA2 (drive mechanics), `$C000–$FFFF` ROM (mirrored at
//! `$8000`).
//!
//! VIA1 port B: PB0 DATA in, PB1 DATA out, PB2 CLK in, PB3 CLK out, PB4 ATN
//! acknowledge, PB5/6 device number jumpers, PB7 ATN in; CA1 = ATN in. The
//! drive pulls DATA by hardware while ATN and ATNA differ (7486 XOR).
//!
//! VIA2 port B: PB0/1 head stepper phase, PB2 motor, PB3 LED, PB4 write
//! protect sense (0 = protected), PB5/6 bit-rate zone, PB7 SYNC (0 = in a
//! sync mark); port A = byte from the read head; CA2 = byte-ready enable
//! (byte ready drives the CPU's SO pin), CB2 = read/write mode.
//!
//! 1571: the same plus a second head, a 32 KB ROM at `$8000–$FFFF`, a
//! WD1770 at `$2000` (MFM disks; not emulated, reads 0) and a 6526 CIA at
//! `$4000` for fast serial transfers. VIA1 port A: PA0 track-0 sensor
//! (0 = head on track 1), PA1 FSDIR (1 = the drive sends), PA2 side
//! select (1 = side 1), PA5 clock (1 = 2 MHz), PA7 BYTE READY (0 = a byte
//! is latched, cleared by accessing VIA2 port A). At 2 MHz the CPU, VIAs
//! and CIA run two cycles per microsecond; the disk keeps its speed. Fast
//! serial: the CIA's SP drives DATA and CNT drives SRQ while FSDIR is
//! set, and reads them otherwise. The CIA's IRQ goes to the CPU.
//!
//! Sources: 1541 service manual schematic (251854); "Inside the 1541";
//! 1571 User's Guide; the port bits as the 1571 DOS (310654-05) uses them.
//! The disk spins at the bit rate of the track's own speed zone (26–32 µs
//! per byte). In write mode (CB2 low) the head writes the byte on port A
//! each byte time; written tracks are decoded back into the D64/D71 image
//! when the motor stops, on [`MediaSlot::flush`] and on eject, and written
//! to the media backend (a copy-on-write overlay for user images: the
//! original file changes only when the overlay is committed). Side 1 of a
//! single-sided image (D64) reads as unformatted and keeps no writes.

use chips_cbm::cia::{Cia, CiaModel};
use chips_cbm::via::Via;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{
    Bus, Cpu, CycleResult, DebugHooks, DebugTarget, HookedBus, Instr, MemSpaceId, RegisterSet,
    Savestate, Size, StateError, StateReader, StateWriter,
};
use media::gcr::TRACK_BYTES;
use media::{d64::speed_zone, gcr, D64};

use crate::iec::IecLines;

/// Size of the 1541 DOS ROM.
pub const ROM_SIZE: usize = 0x4000;
const HALF_TRACKS: u8 = 84;
/// Tracks per side of a double-sided image.
const SIDE_TRACKS: u8 = 35;

/// Drive cycles per GCR byte for speed zones 0..=3.
const CYCLES_PER_BYTE: [u16; 4] = [32, 30, 28, 26];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveModel {
    C1541,
    C1571,
    /// 3.5" drive with its own implementation in [`crate::c1581`].
    C1581,
}

impl DriveModel {
    pub fn rom_size(self) -> usize {
        match self {
            Self::C1541 => ROM_SIZE,
            Self::C1571 | Self::C1581 => 0x8000,
        }
    }

    /// The ROM role in a machine configuration.
    pub fn rom_role(self) -> &'static str {
        match self {
            Self::C1541 => "dos1541",
            Self::C1571 => "dos1571",
            Self::C1581 => "dos1581",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::C1541 => "1541",
            Self::C1571 => "1571",
            Self::C1581 => "1581",
        }
    }

    fn sides(self) -> u8 {
        match self {
            Self::C1541 => 1,
            Self::C1571 | Self::C1581 => 2,
        }
    }
}

struct Disk {
    media: Media,
    image: D64,
    /// GCR stream per side and half track (only full tracks carry data).
    tracks: Vec<Option<Vec<u8>>>,
    /// Half tracks written since the last flush.
    dirty: Vec<bool>,
    protected: bool,
}

/// Index into [`Disk::tracks`].
fn track_index(side: u8, half_track: u8) -> usize {
    usize::from(side) * usize::from(HALF_TRACKS) + usize::from(half_track & !1)
}

impl Disk {
    /// The image track a head position reads, if the image has it.
    fn image_track(&self, side: u8, half_track: u8) -> Option<u8> {
        let t = half_track / 2 + 1;
        match side {
            0 if self.image.double_sided() && t > SIDE_TRACKS => None,
            0 => Some(t),
            _ if self.image.double_sided() && t <= SIDE_TRACKS => Some(t + SIDE_TRACKS),
            _ => None,
        }
        .filter(|&t| t <= self.image.tracks)
    }

    /// Decode the written tracks back into the image and write the image
    /// to the media backend.
    fn flush(&mut self) -> Result<(), MediaError> {
        if !self.dirty.iter().any(|&d| d) {
            return Ok(());
        }
        for i in 0..self.dirty.len() {
            if !std::mem::take(&mut self.dirty[i]) {
                continue;
            }
            let side = (i / usize::from(HALF_TRACKS)) as u8;
            let half = (i % usize::from(HALF_TRACKS)) as u8;
            let (Some(track), Some(gcr)) = (self.image_track(side, half), &self.tracks[i]) else {
                continue;
            };
            for s in gcr::decode_track(gcr) {
                if s.track != track {
                    continue;
                }
                if let Some(sector) = self.image.sector_mut(track, s.sector) {
                    sector.copy_from_slice(&s.data);
                }
            }
        }
        let bytes = self.image.to_bytes();
        self.media.backend.write(0, &bytes)
    }
}

/// Mechanism state: head, rotation, byte latch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mechanism {
    half_track: u8,
    phase: u8,
    /// Byte index on the current track.
    position: usize,
    byte_cycles: u16,
    last_byte: u8,
    sync: bool,
}

pub struct CbmDrive {
    model: DriveModel,
    cpu: Cpu6502,
    ram: Box<[u8; 0x800]>,
    rom: Box<[u8]>,
    pub via1: Via,
    pub via2: Via,
    /// Fast serial port (1571 only).
    pub cia: Cia,
    mech: Mechanism,
    /// The 1571's BYTE READY latch (VIA1 PA7).
    byte_ready: bool,
    disk: Option<Disk>,
    /// Device number (8–11), set by the PB5/6 jumpers.
    device: u8,
    /// The motor ran in the previous cycle (a stop flushes written tracks).
    motor_was_on: bool,
    space: MemSpaceId,
    open_bus: u8,
}

/// Input levels of the chips' ports for one cycle.
#[derive(Clone, Copy)]
struct Pins {
    via1: (u8, u8),
    via2: (u8, u8),
}

/// The drive CPU's bus.
struct DriveBus<'a> {
    model: DriveModel,
    ram: &'a mut [u8; 0x800],
    rom: &'a [u8],
    via1: &'a mut Via,
    via2: &'a mut Via,
    cia: &'a mut Cia,
    pins: Pins,
    byte_ready: &'a mut bool,
    open_bus: &'a mut u8,
}

/// A side-effect-free read of the drive's address space.
fn peek8(
    model: DriveModel,
    ram: &[u8; 0x800],
    rom: &[u8],
    (via1, via2, cia): (&Via, &Via, &Cia),
    pins: Pins,
    open_bus: u8,
    addr: u16,
) -> u8 {
    let c1571 = model == DriveModel::C1571;
    match addr {
        0x0000..=0x17ff => ram[usize::from(addr & 0x7ff)],
        0x1800..=0x1bff => via1.peek(addr as u8, pins.via1.0, pins.via1.1),
        0x1c00..=0x1fff => via2.peek(addr as u8, pins.via2.0, pins.via2.1),
        0x2000..=0x3fff if c1571 => 0,
        0x4000..=0x7fff if c1571 => cia.peek(addr as u8 & 0x0f, 0xff, 0xff),
        0x8000..=0xffff => rom
            .get(usize::from(addr) & (model.rom_size() - 1))
            .copied()
            .unwrap_or(0xff),
        _ => open_bus,
    }
}

impl Bus for DriveBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        let c1571 = self.model == DriveModel::C1571;
        let v = match a {
            0x1800..=0x1bff => self.via1.read(a as u8, self.pins.via1.0, self.pins.via1.1),
            0x1c00..=0x1fff => {
                if matches!(a & 0x0f, 0x01 | 0x0f) {
                    *self.byte_ready = false;
                }
                self.via2.read(a as u8, self.pins.via2.0, self.pins.via2.1)
            }
            0x4000..=0x7fff if c1571 => self.cia.read(a as u8 & 0x0f, 0xff, 0xff),
            _ => self.peek(addr, Size::Byte) as u8,
        };
        *self.open_bus = v;
        u32::from(v)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek8(
            self.model,
            self.ram,
            self.rom,
            (self.via1, self.via2, self.cia),
            self.pins,
            *self.open_bus,
            addr as u16,
        ))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let (a, v) = (addr as u16, v as u8);
        *self.open_bus = v;
        match a {
            0x0000..=0x17ff => self.ram[usize::from(a & 0x7ff)] = v,
            0x1800..=0x1bff => self.via1.write(a as u8, v),
            0x1c00..=0x1fff => {
                if matches!(a & 0x0f, 0x01 | 0x0f) {
                    *self.byte_ready = false;
                }
                self.via2.write(a as u8, v);
            }
            0x4000..=0x7fff if self.model == DriveModel::C1571 => {
                self.cia.write(a as u8 & 0x0f, v);
            }
            _ => {}
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl CbmDrive {
    /// A drive with device number 8–11 whose CPU memory is debug space
    /// `space`. `rom` should be [`DriveModel::rom_size`] bytes; missing
    /// bytes read as `$FF`.
    pub fn new(model: DriveModel, rom: Box<[u8]>, device: u8, space: MemSpaceId) -> Self {
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Nmos6502,
            ..CpuConfig::default()
        });
        cpu.reset();
        Self {
            model,
            cpu,
            ram: Box::new([0; 0x800]),
            rom,
            via1: Via::new(),
            via2: Via::new(),
            cia: Cia::new(CiaModel::Mos6526),
            mech: Mechanism {
                half_track: 34, // track 18
                ..Mechanism::default()
            },
            byte_ready: false,
            disk: None,
            device: device.clamp(8, 11),
            motor_was_on: false,
            space,
            open_bus: 0,
        }
    }

    pub fn model(&self) -> DriveModel {
        self.model
    }

    pub fn space(&self) -> MemSpaceId {
        self.space
    }

    /// RESET line (from the serial bus): CPU and the I/O chips restart;
    /// `hard` (power cycle) also clears the RAM. Disk and head position stay.
    pub fn reset(&mut self, hard: bool) {
        self.cpu.reset();
        self.via1.reset();
        self.via2.reset();
        self.cia.reset();
        if hard {
            self.ram.fill(0);
        }
    }

    /// Track under the head (1-based), as a half-track value / 2 + 1.
    pub fn track(&self) -> f32 {
        f32::from(self.mech.half_track) / 2.0 + 1.0
    }

    /// The head in use (1571: from VIA1 PA2; the 1541 has only side 0).
    pub fn side(&self) -> u8 {
        u8::from(self.model == DriveModel::C1571 && self.via1.port_a_out() & 0x04 != 0)
    }

    /// The 1571 runs at 2 MHz (VIA1 PA5).
    pub fn fast_clock(&self) -> bool {
        self.model == DriveModel::C1571 && self.via1.port_a_out() & 0x20 != 0
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    pub fn motor_on(&self) -> bool {
        self.via2.port_b_out() & 0x04 != 0
    }

    pub fn led_on(&self) -> bool {
        self.via2.port_b_out() & 0x08 != 0
    }

    /// The 1571 sends on the fast serial lines (FSDIR set, CIA shifting out).
    fn fast_serial_out(&self) -> bool {
        self.model == DriveModel::C1571
            && self.via1.port_a_out() & 0x02 != 0
            && self.cia.ta.cr & 0x40 != 0
    }

    /// Lines this drive pulls on the IEC bus, given the bus ATN level.
    pub fn iec_out(&self, atn_low: bool) -> IecLines {
        let pb = self.via1.port_b_out();
        let data_out = pb & 0x02 != 0;
        let atna = pb & 0x10 != 0;
        let (cnt, sp) = if self.fast_serial_out() {
            self.cia.serial_out()
        } else {
            (true, true)
        };
        IecLines {
            atn: false,
            clk: pb & 0x08 != 0,
            data: data_out || (atn_low != atna) || !sp,
            srq: !cnt,
        }
    }

    fn via1_pins(&self, bus: IecLines) -> (u8, u8) {
        let jumpers = (self.device - 8) << 5;
        let pb = 0x1a
            | jumpers
            | u8::from(bus.data)
            | (u8::from(bus.clk) << 2)
            | (u8::from(bus.atn) << 7);
        let pa = match self.model {
            DriveModel::C1541 | DriveModel::C1581 => 0xff,
            // Port A reads the pins: outputs as driven, PA0/PA7 inputs.
            DriveModel::C1571 => {
                self.via1.port_a_out()
                    & !u8::from(self.mech.half_track == 0)
                    & !(u8::from(self.byte_ready) << 7)
            }
        };
        (pa, pb)
    }

    fn via2_pins(&self) -> (u8, u8) {
        // No disk lets the light through the write-protect notch sensor.
        let protected = self.disk.as_ref().is_some_and(|d| d.protected);
        let pb = 0x6f | if protected { 0 } else { 0x10 } | if self.mech.sync { 0 } else { 0x80 };
        (self.mech.last_byte, pb)
    }

    fn pins(&self, bus: IecLines) -> Pins {
        Pins {
            via1: self.via1_pins(bus),
            via2: self.via2_pins(),
        }
    }

    /// A byte passed the head: SO pin of the CPU (if enabled by CA2), CA1
    /// pulse and the 1571's BYTE READY latch.
    fn byte_ready_pulse(&mut self, byte: u8) {
        if self.via2.ca2_out() {
            self.cpu.set_overflow();
        }
        self.via2.set_ca1(false, byte);
        self.via2.set_ca1(true, byte);
        self.byte_ready = true;
    }

    /// Advance the disk under the head by one microsecond.
    fn rotate(&mut self) {
        let motor = self.motor_on();
        if self.motor_was_on && !motor {
            // Errors writing back reach the user on the next flush.
            if let Some(d) = &mut self.disk {
                let _ = d.flush();
            }
        }
        self.motor_was_on = motor;
        if !motor {
            return;
        }
        let track_no = self.mech.half_track / 2 + 1;
        let zone = usize::from(speed_zone(track_no));
        self.mech.byte_cycles += 1;
        if self.mech.byte_cycles < CYCLES_PER_BYTE[zone] {
            return;
        }
        self.mech.byte_cycles = 0;
        // CB2 low: write mode.
        if !self.via2.cb2_out() {
            self.write_byte(zone);
            return;
        }
        let index = track_index(self.side(), self.mech.half_track);
        let data = self
            .disk
            .as_ref()
            .and_then(|d| d.tracks.get(index))
            .and_then(Option::as_ref);
        let Some(data) = data else {
            self.mech.sync = false;
            return;
        };
        let pos = self.mech.position % data.len();
        let byte = data[pos];
        let prev = data[(pos + data.len() - 1) % data.len()];
        self.mech.position = pos + 1;
        // Ten or more 1 bits form a SYNC mark; no byte is delivered in it.
        self.mech.sync = byte == 0xff && prev == 0xff;
        if self.mech.sync {
            return;
        }
        self.mech.last_byte = byte;
        self.byte_ready_pulse(byte);
    }

    /// Write mode: the byte on port A goes onto the track under the head.
    fn write_byte(&mut self, zone: usize) {
        self.mech.sync = false;
        let side = self.side();
        let half = self.mech.half_track;
        let byte = self.via2.port_a_out();
        let disk = self
            .disk
            .as_mut()
            .filter(|d| !d.protected && d.image_track(side, half).is_some());
        if let Some(d) = disk {
            let index = track_index(side, half);
            let track = d.tracks[index].get_or_insert_with(|| vec![0x55; TRACK_BYTES[zone]]);
            let pos = self.mech.position % track.len();
            track[pos] = byte;
            self.mech.position = pos + 1;
            d.dirty[index] = true;
        } else {
            self.mech.position += 1;
        }
        // Byte ready asks for the next byte as when reading.
        self.byte_ready_pulse(byte);
    }

    fn step_head(&mut self) {
        let phase = self.via2.port_b_out() & 3;
        if phase == (self.mech.phase + 1) & 3 && self.mech.half_track < HALF_TRACKS - 1 {
            self.mech.half_track += 1;
        } else if phase == (self.mech.phase + 3) & 3 && self.mech.half_track > 0 {
            self.mech.half_track -= 1;
        }
        self.mech.phase = phase;
    }

    /// One CPU cycle; the disk turns when `rotate` is set.
    fn cycle(
        &mut self,
        host: IecLines,
        rotate: bool,
        mut dbg: Option<&mut DebugHooks>,
    ) -> CycleResult {
        let bus = IecLines::combine(&[host, self.iec_out(host.atn)]);
        self.via1.tick();
        self.via2.tick();
        if self.model == DriveModel::C1571 {
            // FSDIR clear: the CIA listens to the bus.
            let listening = self.via1.port_a_out() & 0x02 == 0;
            self.cia.set_sp(!(listening && bus.data));
            self.cia.set_cnt(!(listening && bus.srq));
            self.cia.tick();
        }
        let pins = self.pins(bus);
        self.via1.set_ca1(bus.atn, pins.via1.0);
        if rotate {
            self.rotate();
        }
        let cia_irq = self.model == DriveModel::C1571 && self.cia.irq();
        self.cpu
            .set_irq(self.via1.irq() || self.via2.irq() || cia_irq);

        let pins = Pins {
            via2: self.via2_pins(),
            ..pins
        };
        let space = self.space;
        let mut b = DriveBus {
            model: self.model,
            ram: &mut self.ram,
            rom: &self.rom,
            via1: &mut self.via1,
            via2: &mut self.via2,
            cia: &mut self.cia,
            pins,
            byte_ready: &mut self.byte_ready,
            open_bus: &mut self.open_bus,
        };
        let mut hooked = HookedBus {
            inner: &mut b,
            dbg: dbg.as_deref_mut(),
            space,
            beam: emu_core::BeamPos::default(),
        };
        let r = self.cpu.tick(&mut hooked);
        // Interrupt breakpoints are for the main CPU only.
        let _ = self.cpu.take_interrupt();
        if let (CycleResult::InstrDone { next_pc }, Some(dbg)) = (r, dbg) {
            if dbg.exec_armed(space) {
                dbg.check_exec(space, next_pc);
            }
        }
        self.step_head();
        r
    }

    /// One microsecond (two CPU cycles on a 1571 at 2 MHz) with the lines
    /// the host pulls. Exec breakpoints are checked here; the result is
    /// that of the last CPU cycle, or `Jammed`.
    pub fn tick(&mut self, host: IecLines, mut dbg: Option<&mut DebugHooks>) -> CycleResult {
        let fast = self.fast_clock();
        let r = self.cycle(host, true, dbg.as_deref_mut());
        if !fast || matches!(r, CycleResult::Jammed) {
            return r;
        }
        self.cycle(host, false, dbg)
    }

    pub fn pc(&self) -> u16 {
        self.cpu.pc()
    }

    fn peek_bus(&self) -> PeekBus<'_> {
        PeekBus(self)
    }
}

struct PeekBus<'a>(&'a CbmDrive);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        let d = self.0;
        u32::from(peek8(
            d.model,
            &d.ram,
            &d.rom,
            (&d.via1, &d.via2, &d.cia),
            d.pins(IecLines::default()),
            d.open_bus,
            addr as u16,
        ))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl DebugTarget for CbmDrive {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != self.space {
            return None;
        }
        let bus = self.peek_bus();
        let mut v = 0;
        for i in 0..size.bytes() {
            v |= bus.peek(addr.wrapping_add(i) & 0xffff, Size::Byte) << (8 * i);
        }
        Some(v)
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != self.space {
            return false;
        }
        for i in 0..size.bytes() {
            let a = (addr.wrapping_add(i) & 0xffff) as u16;
            let b = (v >> (8 * i)) as u8;
            match a {
                0x0000..=0x17ff => self.ram[usize::from(a & 0x7ff)] = b,
                0x1800..=0x1bff => self.via1.write(a as u8, b),
                0x1c00..=0x1fff => self.via2.write(a as u8, b),
                0x4000..=0x7fff if self.model == DriveModel::C1571 => {
                    self.cia.write(a as u8 & 0x0f, b);
                }
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&self.peek_bus(), addr)
    }
}

impl MediaSlot for CbmDrive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let image = D64::parse(&bytes)?;
        if !matches!(image.format, media::Format::D64 | media::Format::D71) {
            return Err(MediaError::Format(format!(
                "a {:?} disk does not fit a {}",
                image.format,
                self.model.name()
            )));
        }
        let slots = usize::from(self.model.sides()) * usize::from(HALF_TRACKS);
        let mut disk = Disk {
            media,
            image,
            tracks: vec![None; slots],
            dirty: vec![false; slots],
            protected: false,
        };
        for side in 0..self.model.sides() {
            for half in (0..HALF_TRACKS).step_by(2) {
                if let Some(t) = disk.image_track(side, half) {
                    disk.tracks[track_index(side, half)] = Some(gcr::encode_track(&disk.image, t));
                }
            }
        }
        self.disk = Some(disk);
        Ok(())
    }

    /// Writes back what was written, then hands the media out.
    fn eject(&mut self) -> Option<Media> {
        self.mech.sync = false;
        let mut d = self.disk.take()?;
        let _ = d.flush();
        Some(d.media)
    }

    /// The write-protect notch: covered (`on`) the drive refuses to write.
    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disk {
            d.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.disk {
            Some(d) => d.flush(),
            None => Ok(()),
        }
    }

    /// Active: motor or LED on (VIA2 PB2, PB3). Error: the LED lit with the
    /// motor off, as the DOS flashes it after an error (the frontend
    /// latches the blinking). Track: the whole track under the head
    /// (1-based); on side 1 of a 1571 counted on from 36, as its DOS does.
    fn status(&self) -> DriveStatus {
        let (motor, led) = (self.motor_on(), self.led_on());
        let track = self.mech.half_track / 2 + 1 + self.side() * 35;
        DriveStatus {
            active: motor || led,
            error: led && !motor,
            track: Some(track),
        }
    }
}

const VERSION: u16 = 2;

impl Savestate for CbmDrive {
    /// The disk itself is media, not state: a savestate restores the drive
    /// with whatever disk is inserted.
    fn save(&self, w: &mut StateWriter) {
        let tag = match self.model {
            DriveModel::C1541 => *b"1541",
            DriveModel::C1571 => *b"1571",
            DriveModel::C1581 => *b"1581",
        };
        w.chunk(tag, VERSION, |w| {
            w.raw(&self.ram[..]);
            w.u8(self.mech.half_track);
            w.u8(self.mech.phase);
            w.u32(self.mech.position as u32);
            w.u16(self.mech.byte_cycles);
            w.u8(self.mech.last_byte);
            w.bool(self.mech.sync);
            w.u8(self.open_bus);
            w.bool(self.motor_was_on);
            if self.model == DriveModel::C1571 {
                w.bool(self.byte_ready);
            }
        });
        self.cpu.save(w);
        self.via1.save(w);
        self.via2.save(w);
        if self.model == DriveModel::C1571 {
            self.cia.save(w);
        }
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let c1571 = self.model == DriveModel::C1571;
        let tag = if c1571 { *b"1571" } else { *b"1541" };
        let (version, mut c) = r.chunk(tag, VERSION)?;
        let mut ram = Box::new([0u8; 0x800]);
        c.raw_into(&mut ram[..])?;
        let mech = Mechanism {
            half_track: c.u8()?,
            phase: c.u8()? & 3,
            position: c.u32()? as usize,
            byte_cycles: c.u16()?,
            last_byte: c.u8()?,
            sync: c.bool()?,
        };
        if mech.half_track >= HALF_TRACKS || mech.byte_cycles > 32 {
            return Err(StateError::Invalid("drive mechanism"));
        }
        let open_bus = c.u8()?;
        // Version 1 states (1541 only) predate writing.
        let motor_was_on = version >= 2 && c.bool()?;
        let byte_ready = c1571 && c.bool()?;
        let mut cpu = self.cpu.clone();
        let mut via1 = self.via1.clone();
        let mut via2 = self.via2.clone();
        let mut cia = self.cia.clone();
        cpu.load(r)?;
        via1.load(r)?;
        via2.load(r)?;
        if c1571 {
            cia.load(r)?;
        }
        self.ram = ram;
        self.mech = mech;
        self.open_bus = open_bus;
        self.motor_was_on = motor_was_on;
        self.byte_ready = byte_ready;
        self.cpu = cpu;
        self.via1 = via1;
        self.via2 = via2;
        self.cia = cia;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
