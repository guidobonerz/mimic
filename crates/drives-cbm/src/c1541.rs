//! Commodore 1541 disk drive, true drive emulation, read-only.
//!
//! Its own NMOS 6502 at 1 MHz, 2 KB RAM, 16 KB DOS ROM and two VIAs.
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
//! Sources: 1541 service manual schematic (251854); "Inside the 1541".
//! The disk spins at the bit rate of the track's own speed zone (26–32 µs
//! per byte); writing is not supported yet (the disk reads as protected).

use chips_cbm::via::Via;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{
    Bus, Cpu, CycleResult, DebugHooks, DebugTarget, HookedBus, Instr, MemSpaceId, RegisterSet,
    Savestate, Size, StateError, StateReader, StateWriter,
};
use media::{d64::speed_zone, gcr, D64};

use crate::iec::IecLines;

pub const ROM_SIZE: usize = 0x4000;
const HALF_TRACKS: u8 = 84;

/// Drive cycles per GCR byte for speed zones 0..=3.
const CYCLES_PER_BYTE: [u16; 4] = [32, 30, 28, 26];

struct Disk {
    media: Media,
    /// GCR stream per half track (only full tracks carry data).
    tracks: Vec<Option<Vec<u8>>>,
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

pub struct Drive1541 {
    cpu: Cpu6502,
    ram: Box<[u8; 0x800]>,
    rom: Box<[u8; ROM_SIZE]>,
    pub via1: Via,
    pub via2: Via,
    mech: Mechanism,
    disk: Option<Disk>,
    /// Device number (8–11), set by the PB5/6 jumpers.
    device: u8,
    space: MemSpaceId,
    open_bus: u8,
}

/// The drive CPU's bus.
struct DriveBus<'a> {
    ram: &'a mut [u8; 0x800],
    rom: &'a [u8; ROM_SIZE],
    via1: &'a mut Via,
    via2: &'a mut Via,
    via1_pins: (u8, u8),
    via2_pins: (u8, u8),
    open_bus: &'a mut u8,
}

impl DriveBus<'_> {
    fn peek8(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x17ff => self.ram[usize::from(addr & 0x7ff)],
            0x1800..=0x1bff => self
                .via1
                .peek(addr as u8, self.via1_pins.0, self.via1_pins.1),
            0x1c00..=0x1fff => self
                .via2
                .peek(addr as u8, self.via2_pins.0, self.via2_pins.1),
            0x8000..=0xffff => self.rom[usize::from(addr & 0x3fff)],
            _ => *self.open_bus,
        }
    }
}

impl Bus for DriveBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        let v = match a {
            0x1800..=0x1bff => self.via1.read(a as u8, self.via1_pins.0, self.via1_pins.1),
            0x1c00..=0x1fff => self.via2.read(a as u8, self.via2_pins.0, self.via2_pins.1),
            _ => self.peek8(a),
        };
        *self.open_bus = v;
        u32::from(v)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.peek8(addr as u16))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let (a, v) = (addr as u16, v as u8);
        *self.open_bus = v;
        match a {
            0x0000..=0x17ff => self.ram[usize::from(a & 0x7ff)] = v,
            0x1800..=0x1bff => self.via1.write(a as u8, v),
            0x1c00..=0x1fff => self.via2.write(a as u8, v),
            _ => {}
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl Drive1541 {
    /// A drive with device number 8–11 whose CPU memory is debug space `space`.
    pub fn new(rom: Box<[u8; ROM_SIZE]>, device: u8, space: MemSpaceId) -> Self {
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Nmos6502,
            ..CpuConfig::default()
        });
        cpu.reset();
        Self {
            cpu,
            ram: Box::new([0; 0x800]),
            rom,
            via1: Via::new(),
            via2: Via::new(),
            mech: Mechanism {
                half_track: 34, // track 18
                ..Mechanism::default()
            },
            disk: None,
            device: device.clamp(8, 11),
            space,
            open_bus: 0,
        }
    }

    pub fn space(&self) -> MemSpaceId {
        self.space
    }

    /// RESET line (from the serial bus): CPU and both VIAs restart; `hard`
    /// (power cycle) also clears the RAM. Disk and head position stay.
    pub fn reset(&mut self, hard: bool) {
        self.cpu.reset();
        self.via1.reset();
        self.via2.reset();
        if hard {
            self.ram.fill(0);
        }
    }

    /// Track under the head (1-based), as a half-track value / 2 + 1.
    pub fn track(&self) -> f32 {
        f32::from(self.mech.half_track) / 2.0 + 1.0
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

    /// Lines this drive pulls on the IEC bus, given the bus ATN level.
    pub fn iec_out(&self, atn_low: bool) -> IecLines {
        let pb = self.via1.port_b_out();
        let data_out = pb & 0x02 != 0;
        let atna = pb & 0x10 != 0;
        IecLines {
            atn: false,
            clk: pb & 0x08 != 0,
            data: data_out || (atn_low != atna),
        }
    }

    fn via1_pins(&self, bus: IecLines) -> (u8, u8) {
        let jumpers = (self.device - 8) << 5;
        let pb = 0x1a
            | jumpers
            | u8::from(bus.data)
            | (u8::from(bus.clk) << 2)
            | (u8::from(bus.atn) << 7);
        (0xff, pb)
    }

    fn via2_pins(&self) -> (u8, u8) {
        let protected = self.disk.is_some();
        let pb = 0x6f | if protected { 0 } else { 0x10 } | if self.mech.sync { 0 } else { 0x80 };
        (self.mech.last_byte, pb)
    }

    /// Advance the disk under the head by one drive cycle.
    fn rotate(&mut self) {
        if !self.motor_on() {
            return;
        }
        let track_no = self.mech.half_track / 2 + 1;
        let zone = usize::from(speed_zone(track_no));
        self.mech.byte_cycles += 1;
        if self.mech.byte_cycles < CYCLES_PER_BYTE[zone] {
            return;
        }
        self.mech.byte_cycles = 0;
        let data = self
            .disk
            .as_ref()
            .and_then(|d| d.tracks.get(usize::from(self.mech.half_track & !1)))
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
        // Byte ready: SO pin of the CPU (if enabled by CA2) and CA1 pulse.
        if self.via2.ca2_out() {
            self.cpu.set_overflow();
        }
        let pa = self.mech.last_byte;
        self.via2.set_ca1(false, pa);
        self.via2.set_ca1(true, pa);
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

    /// One drive cycle with the IEC bus levels as they are now.
    pub fn tick(&mut self, bus: IecLines, dbg: Option<&mut DebugHooks>) -> CycleResult {
        self.via1.tick();
        self.via2.tick();
        let pins1 = self.via1_pins(bus);
        self.via1.set_ca1(bus.atn, pins1.0);
        self.rotate();
        self.cpu.set_irq(self.via1.irq() || self.via2.irq());

        let pins2 = self.via2_pins();
        let space = self.space;
        let mut b = DriveBus {
            ram: &mut self.ram,
            rom: &self.rom,
            via1: &mut self.via1,
            via2: &mut self.via2,
            via1_pins: pins1,
            via2_pins: pins2,
            open_bus: &mut self.open_bus,
        };
        let mut hooked = HookedBus {
            inner: &mut b,
            dbg,
            space,
            beam: emu_core::BeamPos::default(),
        };
        let r = self.cpu.tick(&mut hooked);
        // Interrupt breakpoints are for the main CPU only.
        let _ = self.cpu.take_interrupt();
        self.step_head();
        r
    }

    pub fn pc(&self) -> u16 {
        self.cpu.pc()
    }

    fn peek_bus(&self) -> PeekBus<'_> {
        PeekBus(self)
    }
}

struct PeekBus<'a>(&'a Drive1541);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        let d = self.0;
        let a = addr as u16;
        u32::from(match a {
            0x0000..=0x17ff => d.ram[usize::from(a & 0x7ff)],
            0x1800..=0x1bff => {
                let (pa, pb) = d.via1_pins(IecLines::default());
                d.via1.peek(a as u8, pa, pb)
            }
            0x1c00..=0x1fff => {
                let (pa, pb) = d.via2_pins();
                d.via2.peek(a as u8, pa, pb)
            }
            0x8000..=0xffff => d.rom[usize::from(a & 0x3fff)],
            _ => d.open_bus,
        })
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl DebugTarget for Drive1541 {
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
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&self.peek_bus(), addr)
    }
}

impl MediaSlot for Drive1541 {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let image = D64::parse(&bytes)?;
        let mut tracks = vec![None; usize::from(HALF_TRACKS)];
        for t in 1..=image.tracks.min(HALF_TRACKS / 2) {
            tracks[usize::from(t - 1) * 2] = Some(gcr::encode_track(&image, t));
        }
        self.disk = Some(Disk { media, tracks });
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.mech.sync = false;
        self.disk.take().map(|d| d.media)
    }

    /// Writing is not supported yet: the disk always reads as protected.
    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

const TAG: [u8; 4] = *b"1541";
const VERSION: u16 = 1;

impl Savestate for Drive1541 {
    /// The disk itself is media, not state: a savestate restores the drive
    /// with whatever disk is inserted.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.ram[..]);
            w.u8(self.mech.half_track);
            w.u8(self.mech.phase);
            w.u32(self.mech.position as u32);
            w.u16(self.mech.byte_cycles);
            w.u8(self.mech.last_byte);
            w.bool(self.mech.sync);
            w.u8(self.open_bus);
        });
        self.cpu.save(w);
        self.via1.save(w);
        self.via2.save(w);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
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
            return Err(StateError::Invalid("1541 mechanism"));
        }
        let open_bus = c.u8()?;
        let mut cpu = self.cpu.clone();
        let mut via1 = self.via1.clone();
        let mut via2 = self.via2.clone();
        cpu.load(r)?;
        via1.load(r)?;
        via2.load(r)?;
        self.ram = ram;
        self.mech = mech;
        self.open_bus = open_bus;
        self.cpu = cpu;
        self.via1 = via1;
        self.via2 = via2;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
