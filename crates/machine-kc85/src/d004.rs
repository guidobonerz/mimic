//! D004 "Floppy Disk Basis": the KC85's floppy unit, a second computer on
//! top of the base unit.
//!
//! Inside: a U880 (Z80) at 4 MHz with 64 KB RAM, a U857 (CTC), a U8272
//! (µPD765A at 8 MHz, [`chips_amstrad::fdc`]) for up to four 5.25" drives
//! (two here, [`drives_other::cpc_drive::CpcDrive::kc85`]), and the
//! Koppel-RAM: 1 KB both computers reach. The KC side sees an 8 KB ROM
//! in module slot `$FC` (structure byte `$A7`, [`crate::modules`]); CAOS'
//! `JUMP FC` starts its boot loader.
//!
//! **KC side** (only while control bit 2 of slot `$FC` is set):
//!
//! | Port | |
//! | --- | --- |
//! | `$F0–$F3` | Koppel-RAM: the port's low two bits are address bits 9–8, the upper port byte (B in `OUT (C),r`) bits 7–0 |
//! | `$F4` (write) | bit 0: run (RESET released); bit 1: RAM `$0000–$FBFF` cleared and RESET held; bit 2: RESET held; bit 3: NMI |
//!
//! **D004 side** (the lower port byte):
//!
//! | Port | |
//! | --- | --- |
//! | `$F0`, `$F1` | FDC main status, data |
//! | `$F2`, `$F3` | FDC data through DACK (the DMA path the software polls) |
//! | `$F4` (read) | bit 7 /DRQ, bit 6 /INT of the FDC, bit 5 the selected drive is ready, bit 4 index, bits 3–0 high |
//! | `$F6`, `$F7` (write) | drive select, bit n = drive n; its motor runs |
//! | `$F8`, `$F9` (write) | TC to the FDC |
//! | `$FC–$FF` | CTC channels 0–3 (each channel's ZC/TO clocks the next) |
//!
//! The Koppel-RAM is the D004's `$FC00–$FFFF`. How the boot works: the
//! KC clears the D004's RAM (`$F4` bit 1), puts the boot loader into the
//! Koppel-RAM and releases RESET; the D004's CPU runs from `$0000` over
//! the zeros (NOPs) into `$FC00`. A reset with bit 2 keeps the RAM: the
//! CPU starts the program at `$0000` (MicroDOS' warm start).
//!
//! Both computers run in step: after every instruction of the KC, the
//! D004 runs the same time at its own clock. The arbitration of the
//! Koppel-RAM (the manual: "the different clocks are balanced for both
//! sides") costs nothing here.
//!
//! Sources: "KC85 D004 Manual" and "Handbuch für den Bediener" (VEB
//! Mikroelektronik Mühlhausen: slots, structure byte, Koppel-RAM at
//! `$FC00`, 4 MHz, U8272, drives); the user's D004 ROM 2.0 (`floppy20`,
//! disassembled: the KC side's port use, the D004 side's boot loader with
//! ports `$F0`–`$F8` and the polarity of the `$F4` bits); MAME's
//! `src/devices/bus/kc/d004.cpp` (BSD-3-Clause, read as a description, no
//! code taken) for the bits of the KC's port `$F4`, the select port and
//! the CTC. Not checked against hardware: the bit 1 clearing (the real
//! circuit may instead hold the data bus at zero until `$FC00`), the
//! index bit's polarity, what RESET does to the FDC and CTC (both reset
//! here).

use chips_amstrad::fdc::{Fdc, FloppyDrive};
use chips_zilog::Ctc;
use cpu_z80::{IO_BASE, Z80};
use drives_other::cpc_drive::CpcDrive;
use mimic_core::{
    Bus, Cpu, DebugHooks, DebugTarget, DeviceId, Instr, InterruptKind, MemSpaceId, RegisterSet,
    Savestate, Size, StateError, StateReader, StateWriter,
};

pub const D004_DEVICE: DeviceId = DeviceId("d004");
/// The D004 CPU's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const D004_SPACE: MemSpaceId = MemSpaceId(1);

pub const D004_HZ: u32 = 4_000_000;
/// The Koppel-RAM in the D004's address space.
pub const KOPPEL: usize = 0xfc00;

/// KC port `$F4` bits.
pub const RUN: u8 = 0x01;
pub const CLEAR: u8 = 0x02;
pub const HOLD: u8 = 0x04;
pub const NMI: u8 = 0x08;

/// The unit without its drives (which hold media and do not clone).
#[derive(Clone)]
struct Unit {
    cpu: Z80,
    /// 64 KB; `$FC00–$FFFF` is the Koppel-RAM.
    ram: Vec<u8>,
    ctc: Ctc,
    fdc: Fdc,
    /// Port `$F6`.
    select: u8,
    /// RESET held.
    reset: bool,
    /// D004 clocks owed to the KC's time, in units of 1 / (KC Hz × D004
    /// Hz) seconds: positive means the D004 is behind.
    owed: i64,
    /// D004 clocks into the FDC's current microsecond.
    us_phase: u32,
}

impl Unit {
    fn new() -> Self {
        Self {
            cpu: Z80::new(),
            ram: vec![0; 0x1_0000],
            ctc: Ctc::new(),
            fdc: Fdc::new_8mhz(),
            select: 0,
            reset: true,
            owed: 0,
            us_phase: 0,
        }
    }
}

pub struct D004 {
    unit: Unit,
    pub drives: [CpcDrive; 2],
}

impl Default for D004 {
    fn default() -> Self {
        Self::new()
    }
}

/// Port `$F4` as the D004 reads it. No side effects.
fn status(fdc: &Fdc, drives: &[CpcDrive; 2], select: u8) -> u8 {
    let mut v = 0x0f;
    if !fdc.drq() {
        v |= 0x80;
    }
    if !fdc.int() {
        v |= 0x40;
    }
    let ready = (0..4)
        .find(|&i| select & 1 << i != 0)
        .and_then(|i| drives.get(i))
        .is_some_and(|d| d.ready());
    if ready {
        v |= 0x20;
    }
    if fdc.index() {
        v |= 0x10;
    }
    v
}

fn port_peek(u: &Unit, drives: &[CpcDrive; 2], port: u8) -> u8 {
    match port {
        0xf0 => u.fdc.status(),
        0xf1..=0xf3 => u.fdc.peek_data(),
        0xf4 => status(&u.fdc, drives, u.select),
        p @ 0xfc..=0xff => u.ctc.read(usize::from(p & 3)),
        _ => 0xff,
    }
}

fn peek(u: &Unit, drives: &[CpcDrive; 2], addr: u32) -> u8 {
    if addr < IO_BASE {
        u.ram[addr as usize & 0xffff]
    } else {
        port_peek(u, drives, addr as u8)
    }
}

/// The D004 CPU's bus for one instruction.
struct D004Bus<'a> {
    unit: &'a mut Unit,
    drives: &'a mut [CpcDrive; 2],
    dbg: Option<&'a mut DebugHooks>,
}

impl Bus for D004Bus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = u32::from(match addr {
            a if a < IO_BASE => self.unit.ram[a as usize & 0xffff],
            a => match a as u8 {
                0xf1..=0xf3 => self.unit.fdc.read_data(),
                p => port_peek(self.unit, self.drives, p),
            },
        });
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(D004_SPACE) => dbg.on_read(D004_SPACE, addr, size, v),
            _ => v,
        }
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.unit, self.drives, addr))
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(D004_SPACE) && !dbg.on_write(D004_SPACE, addr, size, v & 0xff) {
                return;
            }
        }
        let v = v as u8;
        let u = &mut *self.unit;
        if addr < IO_BASE {
            u.ram[addr as usize & 0xffff] = v;
            return;
        }
        match addr as u8 {
            0xf1..=0xf3 => u.fdc.write_data(v, self.drives),
            0xf6 | 0xf7 => {
                u.select = v;
                for (i, d) in self.drives.iter_mut().enumerate() {
                    d.set_motor(v & 1 << i != 0);
                }
                u.fdc.motor = self.drives.iter().any(CpcDrive::motor);
            }
            0xf8 | 0xf9 => u.fdc.terminal_count(),
            p @ 0xfc..=0xff => u.ctc.write(usize::from(p & 3), v),
            _ => {}
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

/// A side-effect-free view for the debugger.
struct PeekBus<'a>(&'a Unit, &'a [CpcDrive; 2]);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.0, self.1, addr))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl D004 {
    /// Power-on: RESET held, RAM cleared, no discs.
    pub fn new() -> Self {
        Self {
            unit: Unit::new(),
            drives: [CpcDrive::kc85(), CpcDrive::kc85()],
        }
    }

    /// The D004's CPU runs (RESET released).
    pub fn running(&self) -> bool {
        !self.unit.reset
    }

    pub fn fdc(&self) -> &Fdc {
        &self.unit.fdc
    }

    pub fn ctc(&self) -> &Ctc {
        &self.unit.ctc
    }

    pub fn select(&self) -> u8 {
        self.unit.select
    }

    /// The D004's RAM (`$FC00–$FFFF`: the Koppel-RAM).
    pub fn ram(&self) -> &[u8] {
        &self.unit.ram
    }

    /// Port `$F4` as the D004 reads it now.
    pub fn status(&self) -> u8 {
        status(&self.unit.fdc, &self.drives, self.unit.select)
    }

    fn koppel(port: u16) -> usize {
        KOPPEL | usize::from(port & 3) << 8 | usize::from(port >> 8)
    }

    /// A KC read of ports `$F0–$F3`: the Koppel-RAM. No side effects.
    pub fn kc_read(&self, port: u16) -> Option<u8> {
        match port as u8 {
            0xf0..=0xf3 => Some(self.unit.ram[Self::koppel(port)]),
            _ => None,
        }
    }

    /// A KC write of ports `$F0–$F4`.
    pub fn kc_write(&mut self, port: u16, v: u8) {
        match port as u8 {
            0xf0..=0xf3 => self.unit.ram[Self::koppel(port)] = v,
            0xf4 => {
                if v & CLEAR != 0 {
                    self.unit.ram[..KOPPEL].fill(0);
                    self.hold_reset();
                }
                if v & HOLD != 0 {
                    self.hold_reset();
                }
                if v & RUN != 0 {
                    self.unit.reset = false;
                }
                if v & NMI != 0 {
                    self.unit.cpu.set_nmi(true);
                    self.unit.cpu.set_nmi(false);
                }
            }
            _ => {}
        }
    }

    fn hold_reset(&mut self) {
        let u = &mut self.unit;
        u.reset = true;
        u.cpu.reset();
        u.ctc.reset();
        u.fdc.reset();
        u.select = 0;
        for d in &mut self.drives {
            d.set_motor(false);
        }
    }

    /// Power-on of the unit (the discs stay in their drives).
    pub fn power_on(&mut self) {
        for d in &mut self.drives {
            d.set_motor(false);
        }
        self.unit = Unit::new();
    }

    /// Catch up with `kc_clocks` of the KC at `kc_hz`.
    pub fn run(&mut self, kc_clocks: u32, kc_hz: u32, mut dbg: Option<&mut DebugHooks>) {
        self.unit.owed += i64::from(kc_clocks) * i64::from(D004_HZ);
        let unit = i64::from(kc_hz);
        while self.unit.owed > 0 {
            let t = if self.unit.reset {
                // Only the devices run: the clocks owed, in one go.
                ((self.unit.owed + unit - 1) / unit) as u32
            } else {
                self.instruction(dbg.as_deref_mut())
            };
            self.unit.owed -= i64::from(t) * unit;
            self.devices(t);
        }
    }

    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> u32 {
        let u = &mut self.unit;
        let vector = u.ctc.vector();
        u.cpu.set_int(vector.is_some());
        u.cpu.set_int_vector(vector.unwrap_or(0xff));
        // The CTC watches the bus for RETI.
        let pc = usize::from(u.cpu.pc);
        let reti = !u.cpu.halted && u.ram[pc] == 0xed && u.ram[(pc + 1) & 0xffff] == 0x4d;
        let t = {
            let mut bus = D004Bus {
                unit: &mut *u,
                drives: &mut self.drives,
                dbg: dbg.as_deref_mut(),
            };
            // The bus borrows the unit; the CPU is part of it.
            let mut cpu = std::mem::take(&mut bus.unit.cpu);
            let t = cpu.step(&mut bus);
            self.unit.cpu = cpu;
            t
        };
        let u = &mut self.unit;
        match u.cpu.take_interrupt() {
            Some(InterruptKind::Irq) => {
                u.ctc.chain.acknowledge();
            }
            None if reti => {
                u.ctc.chain.reti();
            }
            _ => {}
        }
        if let Some(dbg) = dbg {
            if dbg.exec_armed(D004_SPACE) {
                dbg.check_exec(D004_SPACE, u32::from(u.cpu.pc));
            }
        }
        t
    }

    /// `t` clocks of the CTC and the FDC.
    fn devices(&mut self, t: u32) {
        let u = &mut self.unit;
        let mut pulses = [0u32; 4];
        u.ctc.advance(t, |ch, _| pulses[ch] += 1);
        // ZC/TO 0–2 clock the next channel.
        for ch in 0..3 {
            for _ in 0..pulses[ch] {
                if u.ctc.trigger(ch + 1) {
                    pulses[ch + 1] += 1;
                }
            }
        }
        u.us_phase += t;
        let per_us = D004_HZ / 1_000_000;
        while u.us_phase >= per_us {
            u.us_phase -= per_us;
            u.fdc.tick(&mut self.drives);
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        let u = &self.unit;
        w.chunk(*b"D004", 1, |w| {
            w.raw(&u.ram);
            w.u8(u.select);
            w.bool(u.reset);
            w.u64(u.owed as u64);
            w.u32(u.us_phase);
            for d in &self.drives {
                d.save(w);
            }
        });
        u.cpu.save(w);
        u.ctc.save(w);
        u.fdc.save(w);
    }

    /// Atomic: the unit loads into a copy, the drives' heads (checked
    /// first) only when everything did.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"D004", 1)?;
        let mut n = self.unit.clone();
        c.raw_into(&mut n.ram)?;
        n.select = c.u8()?;
        n.reset = c.bool()?;
        n.owed = c.u64()? as i64;
        n.us_phase = c.u32()?;
        let mut heads = c.clone();
        for _ in 0..2 {
            CpcDrive::kc85().load(&mut c)?;
        }
        n.cpu.load(r)?;
        n.ctc.load(r)?;
        n.fdc.load(r)?;
        if n.us_phase >= D004_HZ / 1_000_000 || n.owed.unsigned_abs() > 1 << 40 {
            return Err(StateError::Invalid("D004 clock"));
        }
        for d in &mut self.drives {
            d.load(&mut heads)?;
        }
        self.unit = n;
        Ok(())
    }
}

/// The D004's CPU as a debug target.
impl DebugTarget for D004 {
    fn regs(&self) -> RegisterSet {
        self.unit.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.unit.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != D004_SPACE {
            return None;
        }
        Some((0..size.bytes()).fold(0, |v, i| {
            v | u32::from(peek(&self.unit, &self.drives, addr.wrapping_add(i))) << (8 * i)
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != D004_SPACE || addr >= IO_BASE {
            return false;
        }
        for i in 0..size.bytes() {
            self.unit.ram[addr.wrapping_add(i) as usize & 0xffff] = (v >> (8 * i)) as u8;
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.unit
            .cpu
            .disassemble(&PeekBus(&self.unit, &self.drives), addr)
    }
}
