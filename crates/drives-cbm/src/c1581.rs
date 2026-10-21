//! Commodore 1581 3.5" disk drive, true drive emulation.
//!
//! NMOS 6502 at 2 MHz, 8 KB RAM, 32 KB DOS ROM, an 8520 CIA and a WD1772
//! floppy controller. Memory map: `$0000–$1FFF` RAM, `$4000` CIA
//! (mirrored to `$5FFF`), `$6000` WD1772 (mirrored to `$7FFF`),
//! `$8000–$FFFF` ROM.
//!
//! CIA port A: PA0 side select (1 = side 1), PA1 /READY (0 = disk in and
//! up to speed), PA2 /MOTOR (0 = on), PA3/PA4 device number jumpers
//! (device 8 + n), PA5 power LED, PA6 activity LED, PA7 /DISK CHANGE (0 =
//! a disk was changed; a step pulse with a disk inserted clears it).
//! Port B: PB0 DATA in, PB1 DATA out, PB2 CLK in, PB3 CLK out, PB4 ATN
//! acknowledge, PB5 fast serial direction (1 = the drive sends), PB6
//! /WRITE PROTECT (0 = protected), PB7 ATN in. The drive pulls DATA by
//! hardware while ATN is asserted and ATNA is set (the DOS clears ATNA
//! while it serves ATN). The CIA's SP and CNT carry fast serial data on
//! DATA and SRQ; /FLAG sees ATN. Only the CIA interrupts the CPU; the DOS
//! polls the WD1772's BUSY and DRQ.
//!
//! The disk turns at 300 rpm while the motor runs with a disk inserted;
//! tracks are MFM streams ([`media::mfm`]), written back into the D81 when
//! the motor stops, on flush and on eject. Sources: Commodore 1581 User's
//! Guide and service manual (schematic 318044); the port bits as the DOS
//! (318045-02) uses them.

use chips_cbm::cia::{Cia, CiaModel};
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{
    Bus, Cpu, CycleResult, DebugHooks, DebugTarget, HookedBus, Instr, MemSpaceId, RegisterSet,
    Savestate, Size, StateError, StateReader, StateWriter,
};
use media::mfm::{self, TRACK_BYTES};
use media::{Format, D64};

use crate::iec::IecLines;
use crate::wd177x::{FloppyIo, Pulse, Wd177x, BYTE_US};

pub const ROM_SIZE: usize = 0x8000;
const CYLINDERS: u8 = 83;
/// One revolution at 300 rpm.
const REV_US: u32 = TRACK_BYTES as u32 * BYTE_US;
/// Length of the index pulse.
const INDEX_US: u32 = 4_000;
/// The mechanism reports ready this long after the motor starts.
const SPIN_UP_US: u32 = 250_000;

struct Disk {
    media: Media,
    image: D64,
    /// MFM stream per cylinder and side (`cylinder * 2 + side`).
    tracks: Vec<Option<Vec<u16>>>,
    dirty: Vec<bool>,
    protected: bool,
}

impl Disk {
    fn flush(&mut self) -> Result<(), MediaError> {
        if !self.dirty.iter().any(|&d| d) {
            return Ok(());
        }
        for i in 0..self.dirty.len() {
            if std::mem::take(&mut self.dirty[i]) {
                if let Some(t) = &self.tracks[i] {
                    mfm::store_sectors(&mut self.image, &mfm::decode_track(t));
                }
            }
        }
        let bytes = self.image.to_bytes();
        self.media.backend.write(0, &bytes)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mechanism {
    cylinder: u8,
    /// Position in the revolution, µs from the index hole.
    rotation: u32,
    /// µs since the motor started (saturating).
    motor_us: u32,
    /// The disk change latch (/DISK CHANGE low).
    changed: bool,
}

/// The head as the WD1772 sees it.
struct Head<'a> {
    mech: &'a mut Mechanism,
    disk: Option<&'a mut Disk>,
    side: u8,
    spinning: bool,
}

impl Head<'_> {
    fn index(&self) -> usize {
        usize::from(self.mech.cylinder) * 2 + usize::from(self.side)
    }

    fn pos(&self) -> usize {
        (self.mech.rotation / BYTE_US) as usize % TRACK_BYTES
    }
}

impl FloppyIo for Head<'_> {
    fn read(&self) -> Option<u16> {
        if !self.spinning {
            return None;
        }
        let disk = self.disk.as_ref()?;
        disk.tracks
            .get(self.index())?
            .as_ref()
            .map(|t| t[self.pos()])
    }

    fn write(&mut self, byte: u16) {
        let (i, pos) = (self.index(), self.pos());
        if !self.spinning {
            return;
        }
        if let Some(d) = self.disk.as_deref_mut().filter(|d| !d.protected) {
            if let Some(t) = d.tracks.get_mut(i) {
                t.get_or_insert_with(|| vec![0x4e; TRACK_BYTES])[pos] = byte;
                d.dirty[i] = true;
            }
        }
    }

    fn step(&mut self, outward: bool) {
        if outward {
            self.mech.cylinder = self.mech.cylinder.saturating_sub(1);
        } else if self.mech.cylinder < CYLINDERS - 1 {
            self.mech.cylinder += 1;
        }
        if self.disk.is_some() {
            self.mech.changed = false;
        }
    }

    fn track0(&self) -> bool {
        self.mech.cylinder == 0
    }

    fn protected(&self) -> bool {
        self.disk.as_ref().is_some_and(|d| d.protected)
    }
}

/// Track 0 and write protect for register accesses from the CPU.
#[derive(Clone, Copy)]
struct Sense {
    track0: bool,
    protected: bool,
}

impl FloppyIo for Sense {
    fn read(&self) -> Option<u16> {
        None
    }
    fn write(&mut self, _byte: u16) {}
    fn step(&mut self, _outward: bool) {}
    fn track0(&self) -> bool {
        self.track0
    }
    fn protected(&self) -> bool {
        self.protected
    }
}

pub struct Drive1581 {
    cpu: Cpu6502,
    ram: Box<[u8; 0x2000]>,
    rom: Box<[u8]>,
    pub cia: Cia,
    pub wd: Wd177x,
    mech: Mechanism,
    disk: Option<Disk>,
    /// Device number (8–11), set by the PA3/4 jumpers.
    device: u8,
    motor_was_on: bool,
    space: MemSpaceId,
    open_bus: u8,
}

/// The drive CPU's bus.
struct DriveBus<'a> {
    ram: &'a mut [u8; 0x2000],
    rom: &'a [u8],
    cia: &'a mut Cia,
    wd: &'a mut Wd177x,
    pins: (u8, u8),
    sense: Sense,
    open_bus: &'a mut u8,
}

fn peek8(
    ram: &[u8; 0x2000],
    rom: &[u8],
    (cia, wd): (&Cia, &Wd177x),
    pins: (u8, u8),
    sense: Sense,
    open_bus: u8,
    addr: u16,
) -> u8 {
    match addr {
        0x0000..=0x1fff => ram[usize::from(addr)],
        0x4000..=0x5fff => cia.peek(addr as u8 & 0x0f, pins.0, pins.1),
        0x6000..=0x7fff => wd.peek(addr as u8, &sense),
        0x8000..=0xffff => rom
            .get(usize::from(addr) & (ROM_SIZE - 1))
            .copied()
            .unwrap_or(0xff),
        _ => open_bus,
    }
}

impl Bus for DriveBus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        let v = match a {
            0x4000..=0x5fff => self.cia.read(a as u8 & 0x0f, self.pins.0, self.pins.1),
            0x6000..=0x7fff => self.wd.read(a as u8, &self.sense),
            _ => self.peek(addr, Size::Byte) as u8,
        };
        *self.open_bus = v;
        u32::from(v)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek8(
            self.ram,
            self.rom,
            (self.cia, self.wd),
            self.pins,
            self.sense,
            *self.open_bus,
            addr as u16,
        ))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let (a, v) = (addr as u16, v as u8);
        *self.open_bus = v;
        match a {
            0x0000..=0x1fff => self.ram[usize::from(a)] = v,
            0x4000..=0x5fff => self.cia.write(a as u8 & 0x0f, v),
            0x6000..=0x7fff => self.wd.write(a as u8, v),
            _ => {}
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl Drive1581 {
    /// A drive with device number 8–11 whose CPU memory is debug space
    /// `space`. `rom` should be [`ROM_SIZE`] bytes; missing bytes read as
    /// `$FF`.
    pub fn new(rom: Box<[u8]>, device: u8, space: MemSpaceId) -> Self {
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Nmos6502,
            ..CpuConfig::default()
        });
        cpu.reset();
        Self {
            cpu,
            ram: Box::new([0; 0x2000]),
            rom,
            cia: Cia::new(CiaModel::Mos6526A),
            wd: Wd177x::new(),
            mech: Mechanism {
                changed: true,
                ..Mechanism::default()
            },
            disk: None,
            device: device.clamp(8, 11),
            motor_was_on: false,
            space,
            open_bus: 0,
        }
    }

    pub fn space(&self) -> MemSpaceId {
        self.space
    }

    pub fn pc(&self) -> u16 {
        self.cpu.pc()
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    /// RESET from the serial bus: CPU, CIA and WD1772 restart; `hard`
    /// (power cycle) also clears the RAM. Disk and head position stay.
    pub fn reset(&mut self, hard: bool) {
        self.cpu.reset();
        self.cia.reset();
        self.wd.reset();
        if hard {
            self.ram.fill(0);
        }
    }

    pub fn cylinder(&self) -> u8 {
        self.mech.cylinder
    }

    pub fn motor_on(&self) -> bool {
        self.cia.port_a_out() & 0x04 == 0
    }

    /// The activity LED (CIA PA6, 1 = lit).
    pub fn led_on(&self) -> bool {
        self.cia.port_a_out() & 0x40 != 0
    }

    fn side(&self) -> u8 {
        self.cia.port_a_out() & 0x01
    }

    fn spinning(&self) -> bool {
        self.motor_on() && self.disk.is_some()
    }

    fn ready(&self) -> bool {
        self.spinning() && self.mech.motor_us >= SPIN_UP_US
    }

    /// The CIA serial port sends (FSDIR set, serial port in output mode).
    fn fast_serial_out(&self) -> bool {
        self.cia.port_b_out() & 0x20 != 0 && self.cia.ta.cr & 0x40 != 0
    }

    /// Lines this drive pulls on the IEC bus, given the bus ATN level.
    pub fn iec_out(&self, atn_low: bool) -> IecLines {
        let pb = self.cia.port_b_out();
        let atna = pb & 0x10 != 0;
        let (cnt, sp) = if self.fast_serial_out() {
            self.cia.serial_out()
        } else {
            (true, true)
        };
        IecLines {
            atn: false,
            clk: pb & 0x08 != 0,
            data: pb & 0x02 != 0 || (atn_low && atna) || !sp,
            srq: !cnt,
        }
    }

    /// External levels on the CIA ports (1 = not pulled low).
    fn pins(&self, bus: IecLines) -> (u8, u8) {
        let jumpers = (self.device - 8) << 3;
        let pa = 0x65
            | jumpers
            | if self.ready() { 0 } else { 0x02 }
            | if self.mech.changed { 0 } else { 0x80 };
        let protected = self.disk.as_ref().is_some_and(|d| d.protected);
        let pb = 0x3a
            | u8::from(bus.data)
            | (u8::from(bus.clk) << 2)
            | if protected { 0 } else { 0x40 }
            | (u8::from(bus.atn) << 7);
        (pa, pb)
    }

    fn sense(&self) -> Sense {
        Sense {
            track0: self.mech.cylinder == 0,
            protected: self.disk.as_ref().is_some_and(|d| d.protected),
        }
    }

    /// The disk and the WD1772 for one microsecond.
    fn mechanics(&mut self) {
        let motor = self.motor_on();
        if self.motor_was_on && !motor {
            // Errors writing back reach the user on the next flush.
            if let Some(d) = &mut self.disk {
                let _ = d.flush();
            }
        }
        self.motor_was_on = motor;
        self.mech.motor_us = if motor {
            self.mech.motor_us.saturating_add(1)
        } else {
            0
        };
        let spinning = self.spinning();
        let mut pulse = Pulse::default();
        if spinning {
            self.mech.rotation = (self.mech.rotation + 1) % REV_US;
            let r = self.mech.rotation;
            pulse.byte = r.is_multiple_of(BYTE_US);
            pulse.index = r == 0;
            pulse.index_active = r < INDEX_US;
        }
        let side = self.side();
        let mut head = Head {
            mech: &mut self.mech,
            disk: self.disk.as_mut(),
            side,
            spinning,
        };
        self.wd.tick(&mut head, pulse);
    }

    fn cycle(&mut self, host: IecLines, mut dbg: Option<&mut DebugHooks>) -> CycleResult {
        let bus = IecLines::combine(&[host, self.iec_out(host.atn)]);
        // FSDIR clear: the CIA listens to the bus.
        let listening = self.cia.port_b_out() & 0x20 == 0;
        self.cia.set_sp(!(listening && bus.data));
        self.cia.set_cnt(!(listening && bus.srq));
        self.cia.set_flag(!bus.atn);
        self.cia.tick();
        self.cpu.set_irq(self.cia.irq());
        let pins = self.pins(bus);
        let sense = self.sense();
        let space = self.space;
        let mut b = DriveBus {
            ram: &mut self.ram,
            rom: &self.rom,
            cia: &mut self.cia,
            wd: &mut self.wd,
            pins,
            sense,
            open_bus: &mut self.open_bus,
        };
        let mut hooked = HookedBus {
            inner: &mut b,
            dbg: dbg.as_deref_mut(),
            space,
            beam: emu_core::BeamPos::default(),
        };
        let r = self.cpu.tick(&mut hooked);
        let _ = self.cpu.take_interrupt();
        if let (CycleResult::InstrDone { next_pc }, Some(dbg)) = (r, dbg) {
            if dbg.exec_armed(space) {
                dbg.check_exec(space, next_pc);
            }
        }
        r
    }

    /// One microsecond: the mechanics and the WD1772 once, the CPU and the
    /// CIA twice (2 MHz).
    pub fn tick(&mut self, host: IecLines, mut dbg: Option<&mut DebugHooks>) -> CycleResult {
        self.mechanics();
        let r = self.cycle(host, dbg.as_deref_mut());
        if matches!(r, CycleResult::Jammed) {
            return r;
        }
        self.cycle(host, dbg)
    }

    fn peek_byte(&self, addr: u16) -> u8 {
        peek8(
            &self.ram,
            &self.rom,
            (&self.cia, &self.wd),
            self.pins(IecLines::default()),
            self.sense(),
            self.open_bus,
            addr,
        )
    }
}

struct PeekBus<'a>(&'a Drive1581);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.0.peek_byte(addr as u16))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

impl DebugTarget for Drive1581 {
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
        let mut v = 0;
        for i in 0..size.bytes() {
            let a = (addr.wrapping_add(i) & 0xffff) as u16;
            v |= u32::from(self.peek_byte(a)) << (8 * i);
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
                0x0000..=0x1fff => self.ram[usize::from(a)] = b,
                0x4000..=0x5fff => self.cia.write(a as u8 & 0x0f, b),
                0x6000..=0x7fff => self.wd.write(a as u8, b),
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(self), addr)
    }
}

impl MediaSlot for Drive1581 {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let image = D64::parse(&bytes)?;
        if image.format != Format::D81 {
            return Err(MediaError::Format(
                "the 1581 takes D81 images (3.5\" disks)".into(),
            ));
        }
        let slots = usize::from(CYLINDERS) * 2;
        let mut tracks = vec![None; slots];
        for c in 0..80u8 {
            for side in 0..2u8 {
                tracks[usize::from(c) * 2 + usize::from(side)] =
                    Some(mfm::encode_track(&image, c, side));
            }
        }
        self.disk = Some(Disk {
            media,
            image,
            tracks,
            dirty: vec![false; slots],
            protected: false,
        });
        self.mech.changed = true;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let mut d = self.disk.take()?;
        self.mech.changed = true;
        let _ = d.flush();
        Some(d.media)
    }

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

    /// Active: motor or activity LED on. Error: the LED lit with the motor
    /// off, as the DOS flashes it after an error. Track: the cylinder under
    /// the head, 1-based like the DOS's tracks 1–80.
    fn status(&self) -> DriveStatus {
        let (motor, led) = (self.motor_on(), self.led_on());
        DriveStatus {
            active: motor || led,
            error: led && !motor,
            track: Some(self.mech.cylinder.saturating_add(1)),
        }
    }
}

const TAG: [u8; 4] = *b"1581";
const VERSION: u16 = 1;

impl Savestate for Drive1581 {
    /// The disk itself is media, not state.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.ram[..]);
            w.u8(self.mech.cylinder);
            w.u32(self.mech.rotation);
            w.u32(self.mech.motor_us);
            w.bool(self.mech.changed);
            w.u8(self.open_bus);
            w.bool(self.motor_was_on);
        });
        self.cpu.save(w);
        self.cia.save(w);
        self.wd.save(w);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut ram = Box::new([0u8; 0x2000]);
        c.raw_into(&mut ram[..])?;
        let mech = Mechanism {
            cylinder: c.u8()?,
            rotation: c.u32()?,
            motor_us: c.u32()?,
            changed: c.bool()?,
        };
        if mech.cylinder >= CYLINDERS || mech.rotation >= REV_US {
            return Err(StateError::Invalid("1581 mechanism"));
        }
        let open_bus = c.u8()?;
        let motor_was_on = c.bool()?;
        let mut cpu = self.cpu.clone();
        let mut cia = self.cia.clone();
        let mut wd = self.wd.clone();
        cpu.load(r)?;
        cia.load(r)?;
        wd.load(r)?;
        self.ram = ram;
        self.mech = mech;
        self.open_bus = open_bus;
        self.motor_was_on = motor_was_on;
        self.cpu = cpu;
        self.cia = cia;
        self.wd = wd;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
