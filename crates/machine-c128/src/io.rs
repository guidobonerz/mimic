//! The I/O area `$D000–$DFFF`.
//!
//! | Range         | Chip                                                     |
//! | ------------- | -------------------------------------------------------- |
//! | `$D000–$D3FF` | VIC-IIe (8566 PAL / 8564 NTSC), 64 registers mirrored    |
//! | `$D400–$D4FF` | SID                                                      |
//! | `$D500–$D5FF` | MMU (C128 mode only)                                     |
//! | `$D600–$D6FF` | VDC: even addresses status/register select, odd data     |
//! | `$D800–$DBFF` | colour RAM, 4 bits, two banks                            |
//! | `$DC00–$DCFF` | CIA1: keyboard columns 0–7, joysticks                    |
//! | `$DD00–$DDFF` | CIA2: VIC bank, serial bus, user port                    |
//!
//! The VIC-IIe adds `$D02F` (bits 0–2 drive keyboard columns K0–K2) and
//! `$D030` (bit 0 2 MHz, bit 1 test); they are latches handled here, the
//! VIC-II core is the C64's. Wiring as on the C64 (CIA2 → /NMI with
//! RESTORE; VIC and CIA1 → /IRQ). Source: Commodore 128 Programmer's
//! Reference Guide; C128 schematic 310378.

use chips_cbm::cia::{Cia, CiaModel};
use chips_cbm::sid::{Sid, SidModel};
use chips_cbm::vdc::Vdc;
use chips_cbm::vic2::{self, Vic2};
use drives_cbm::IecLines;
use emu_core::frame::VideoStandard;
use emu_core::{
    ClockDivider, Decimator, Savestate, StateError, StateReader, StateWriter, SAMPLE_RATE,
};

use crate::keyboard::Keyboard;
use crate::memory::Memory;

#[derive(Clone, Debug, PartialEq)]
pub struct Io {
    pub vic: Vic2,
    pub cia1: Cia,
    pub cia2: Cia,
    pub sid: Sid,
    pub vdc: Vdc,
    /// VIC-IIe `$D02F` (K0–K2 out) and `$D030` (2 MHz, test).
    pub d02f: u8,
    pub d030: u8,
    resampler: Decimator,
    /// Audio produced but not yet taken (output, not machine state).
    pub samples: Vec<i16>,
    tod: ClockDivider,
    /// VDC character clocks per CPU cycle.
    vdc_clock: ClockDivider,
    pub keyboard: Keyboard,
    /// Pins pulled low by the devices in control ports 1 and 2.
    pub port_lines: [u32; 2],
    /// POTX/POTY resistances of the devices in control ports 1 and 2
    /// (65535 = open).
    pub port_analog: [[u16; 2]; 2],
    /// Serial bus levels after the last drive update.
    pub iec: IecLines,
}

impl Io {
    pub fn new(vic: vic2::Model, sid: SidModel, vdc_kb: usize) -> Self {
        let cpu_hz = vic.timing().cpu_hz();
        let (standard, mains) = match vic {
            vic2::Model::Mos6569 => (VideoStandard::Pal, 50),
            vic2::Model::Mos6567R8 => (VideoStandard::Ntsc, 60),
        };
        Self {
            vic: Vic2::new(vic),
            cia1: Cia::new(CiaModel::Mos6526A),
            cia2: Cia::new(CiaModel::Mos6526A),
            sid: Sid::new(sid, cpu_hz),
            vdc: Vdc::new(vdc_kb, standard),
            d02f: 0xff,
            d030: 0,
            resampler: Decimator::new(cpu_hz, SAMPLE_RATE).expect("valid rates"),
            samples: Vec::new(),
            tod: ClockDivider::new(mains, cpu_hz).expect("CPU clock is non-zero"),
            vdc_clock: ClockDivider::new(chips_cbm::vdc::CHAR_CLOCK_HZ, cpu_hz)
                .expect("CPU clock is non-zero"),
            keyboard: Keyboard::default(),
            port_lines: [0; 2],
            port_analog: [[u16::MAX; 2]; 2],
            iec: IecLines::default(),
        }
    }

    /// RESET: CIAs, SID and the VIC-IIe latches restart; VIC-II and VDC
    /// start over only on power-up.
    pub fn reset(&mut self, hard: bool) {
        self.cia1.reset();
        self.cia2.reset();
        self.sid = Sid::new(self.sid.model(), self.vic.timing().cpu_hz());
        self.d02f = 0xff;
        self.d030 = 0;
        if hard {
            self.vic = Vic2::new(self.vic.model());
            let kb = self.vdc.ram().len() / 1024;
            let standard = self.vdc.signal().standard;
            self.vdc = Vdc::new(kb, standard);
        }
    }

    /// 2 MHz mode (`$D030` bit 0).
    pub fn fast(&self) -> bool {
        self.d030 & 1 != 0
    }

    /// VIC bank (16 KB) selected by CIA2 PA0/PA1 (active low).
    pub fn vic_base(&self) -> u16 {
        u16::from(!self.cia2.port_a_out() & 3) << 14
    }

    pub fn open_bus(&self) -> u8 {
        self.vic.last_bus()
    }

    /// SID, CIAs (with TOD) and the VDC for one CPU cycle.
    pub fn tick_chips(&mut self) -> bool {
        self.cia1.tick();
        self.cia2.tick();
        for _ in 0..self.tod.tick() {
            self.cia1.tod_tick();
            self.cia2.tod_tick();
        }
        // CIA1 PA6/PA7 switch the pots of control port 1/2 to the SID.
        self.sid
            .set_pots(machine_c64_pots(self.cia1.port_a_out(), self.port_analog));
        let s = self.sid.clock();
        if let Some(out) = self.resampler.push(s) {
            if self.samples.len() < SAMPLE_RATE as usize * 4 {
                self.samples.push(out);
            }
        }
        let mut vdc_frame = false;
        for _ in 0..self.vdc_clock.tick() {
            vdc_frame |= self.vdc.tick();
        }
        vdc_frame
    }

    /// CIA1 port levels: joysticks (port 2 on PA, port 1 on PB), then the
    /// keyboard with K0–K2 from `$D02F`.
    fn cia1_ext(&self) -> (u8, u8) {
        let joy = |lines: u32| -> u8 { !((lines & 0x0f) as u8 | ((lines >> 1) & 0x10) as u8) };
        let a = self.cia1.port_a_out() & joy(self.port_lines[1]);
        let b = self.cia1.port_b_out() & joy(self.port_lines[0]);
        let cols = u16::from(a) | (u16::from(self.d02f & 7) << 8);
        let (cols, rows) = self.keyboard.resolve(cols, b);
        (cols as u8, rows)
    }

    /// CIA2: PA6 CLK in, PA7 DATA in read the serial line levels.
    fn cia2_ext(&self) -> (u8, u8) {
        let pa = 0x3f | if self.iec.clk { 0 } else { 0x40 } | if self.iec.data { 0 } else { 0x80 };
        (pa, 0xff)
    }

    /// Side-effect-free read of an I/O address.
    pub fn peek(&self, mem: &Memory, addr: u16) -> u8 {
        let reg = addr as u8;
        match addr {
            0xd000..=0xd3ff => match reg & 0x3f {
                0x2f => self.d02f | 0xf8,
                0x30 => self.d030 | 0xfc,
                r => self.vic.peek(r),
            },
            0xd400..=0xd4ff => self.sid.peek(reg),
            0xd500..=0xd5ff if !mem.mmu.c64_mode() => {
                mem.mmu.peek(reg, self.keyboard.display_80, false, false)
            }
            0xd600..=0xd6ff => self.vdc.peek(reg),
            0xd800..=0xdbff => (self.open_bus() & 0xf0) | mem.color[mem.color_index(addr)],
            0xdc00..=0xdcff => {
                let (a, b) = self.cia1_ext();
                self.cia1.peek(reg & 0x0f, a, b)
            }
            0xdd00..=0xddff => {
                let (a, b) = self.cia2_ext();
                self.cia2.peek(reg & 0x0f, a, b)
            }
            _ => self.open_bus(),
        }
    }

    pub fn read(&mut self, mem: &Memory, addr: u16) -> u8 {
        let reg = addr as u8;
        match addr {
            0xd000..=0xd3ff if !matches!(reg & 0x3f, 0x2f | 0x30) => self.vic.read(reg),
            0xd600..=0xd6ff => self.vdc.read(reg),
            0xdc00..=0xdcff => {
                let (a, b) = self.cia1_ext();
                self.cia1.read(reg & 0x0f, a, b)
            }
            0xdd00..=0xddff => {
                let (a, b) = self.cia2_ext();
                self.cia2.read(reg & 0x0f, a, b)
            }
            _ => self.peek(mem, addr),
        }
    }

    pub fn write(&mut self, mem: &mut Memory, addr: u16, v: u8) {
        let reg = addr as u8;
        match addr {
            0xd000..=0xd3ff => match reg & 0x3f {
                0x2f => self.d02f = v | 0xf8,
                0x30 => self.d030 = v & 3,
                r => self.vic.write(r, v),
            },
            0xd400..=0xd4ff => self.sid.write(reg, v),
            0xd500..=0xd5ff if !mem.mmu.c64_mode() => mem.mmu.write(reg, v),
            0xd600..=0xd6ff => self.vdc.write(reg, v),
            0xd800..=0xdbff => {
                let i = mem.color_index(addr);
                mem.color[i] = v & 0x0f;
            }
            0xdc00..=0xdcff => self.cia1.write(reg & 0x0f, v),
            0xdd00..=0xddff => self.cia2.write(reg & 0x0f, v),
            _ => {}
        }
    }
}

impl Savestate for Io {
    fn save(&self, w: &mut StateWriter) {
        self.vic.save(w);
        self.cia1.save(w);
        self.cia2.save(w);
        self.sid.save(w);
        self.vdc.save(w);
        w.chunk(*b"AUDR", 1, |w| self.resampler.save(w));
        self.tod.save(w);
        self.vdc_clock.save(w);
        w.chunk(*b"C8IO", 3, |w| {
            w.u8(self.d02f);
            w.u8(self.d030);
            w.raw(&self.keyboard.columns);
            w.bool(self.keyboard.restore);
            w.bool(self.keyboard.display_80);
            w.bool(self.keyboard.caps_lock);
            w.u32(self.port_lines[0]);
            w.u32(self.port_lines[1]);
            w.bool(self.iec.atn);
            w.bool(self.iec.clk);
            w.bool(self.iec.data);
            w.bool(self.iec.srq);
            for pots in self.port_analog {
                w.u16(pots[0]);
                w.u16(pots[1]);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.vic.load(r)?;
        self.cia1.load(r)?;
        self.cia2.load(r)?;
        self.sid.load(r)?;
        self.vdc.load(r)?;
        let (_, mut c) = r.chunk(*b"AUDR", 1)?;
        self.resampler.load(&mut c)?;
        self.tod.load(r)?;
        self.vdc_clock.load(r)?;
        let (version, mut c) = r.chunk(*b"C8IO", 3)?;
        self.d02f = c.u8()? | 0xf8;
        self.d030 = c.u8()? & 3;
        c.raw_into(&mut self.keyboard.columns)?;
        self.keyboard.restore = c.bool()?;
        self.keyboard.display_80 = c.bool()?;
        self.keyboard.caps_lock = c.bool()?;
        self.port_lines = [c.u32()?, c.u32()?];
        self.iec = IecLines {
            atn: c.bool()?,
            clk: c.bool()?,
            data: c.bool()?,
            // Version 1 predates fast serial.
            srq: version >= 2 && c.bool()?,
        };
        // Version 3 adds the analog lines.
        self.port_analog = if version >= 3 {
            [[c.u16()?, c.u16()?], [c.u16()?, c.u16()?]]
        } else {
            [[u16::MAX; 2]; 2]
        };
        Ok(())
    }
}

/// The pot lines the SID sees: CIA1 PA6 switches control port 1's POTX/POTY
/// to it, PA7 port 2's (both together: in parallel), as on the C64.
fn machine_c64_pots(cia1_pa: u8, ports: [[u16; 2]; 2]) -> [u16; 2] {
    let mut pots = [u16::MAX; 2];
    for (bit, port) in [(0x40, ports[0]), (0x80, ports[1])] {
        if cia1_pa & bit != 0 {
            for (p, r) in pots.iter_mut().zip(port) {
                *p = (*p).min(r);
            }
        }
    }
    pots
}
