//! The I/O area `$D000–$DFFF` and the chips behind it.
//!
//! | Range         | Chip                                   |
//! | ------------- | -------------------------------------- |
//! | `$D000–$D3FF` | VIC-II, 64 registers mirrored          |
//! | `$D400–$D7FF` | SID, 32 registers mirrored             |
//! | `$D800–$DBFF` | colour RAM, 4 bits                     |
//! | `$DC00–$DCFF` | CIA1, 16 registers mirrored            |
//! | `$DD00–$DDFF` | CIA2, 16 registers mirrored            |
//! | `$DE00–$DFFF` | I/O1, I/O2 (cartridge port)            |
//!
//! Wiring (C64 schematic 251469): VIC-II and CIA1 IRQ → CPU /IRQ, CIA2 IRQ
//! → CPU /NMI, VIC-II BA → CPU RDY. CIA1 ports scan the keyboard and read
//! the joysticks; CIA2 port A bits 0–1 select the VIC bank (inverted) and
//! drive the IEC bus. Both TOD inputs get the 50/60 Hz power-line signal.

use chips_cbm::cia::{Cia, CiaModel};
use chips_cbm::sid::{Sid, SidModel};
use chips_cbm::vic2::{self, Vic2, VicMemory};
use devices_cartridge::ExpansionCart;
use emu_core::{
    ClockDivider, Decimator, Savestate, StateError, StateReader, StateWriter, SAMPLE_RATE,
};

use drives_cbm::IecLines;

use crate::keyboard::Keyboard;
use crate::memory::Memory;

/// Mains frequency for the TOD input: 50 Hz in PAL countries, 60 Hz in
/// NTSC countries.
fn mains_hz(vic: vic2::Model) -> u32 {
    match vic {
        vic2::Model::Mos6569 => 50,
        vic2::Model::Mos6567R8 => 60,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Io {
    pub vic: Vic2,
    pub cia1: Cia,
    pub cia2: Cia,
    pub sid: Sid,
    resampler: Decimator,
    /// Audio produced but not yet taken by `Machine::audio` (output, not
    /// machine state: not part of savestates).
    pub samples: Vec<i16>,
    tod: ClockDivider,
    pub keyboard: Keyboard,
    /// Pins pulled low by the devices in control ports 1 and 2
    /// (`PortLines::pulled_low`), refreshed when their input changes.
    pub port_lines: [u32; 2],
    /// POTX/POTY resistances of the devices in control ports 1 and 2
    /// (65535 = open).
    pub port_analog: [[u16; 2]; 2],
    /// Serial bus levels after the last drive update.
    pub iec: IecLines,
}

/// The VIC-II's view of memory: one 16 KB bank of RAM, with the character
/// ROM at `$1000–$1FFF` in banks 0 and 2, plus colour RAM.
pub struct VicView<'a> {
    pub mem: &'a Memory,
    /// Bank base address: `$0000`, `$4000`, `$8000` or `$C000`.
    pub bank: u16,
    /// The cartridge: in Ultimax mode the VIC-II sees its ROMH at `$3000`
    /// of every bank.
    pub cart: Option<&'a dyn ExpansionCart>,
}

impl VicMemory for VicView<'_> {
    fn read(&self, addr: u16) -> u8 {
        let addr = addr & 0x3fff;
        if self.mem.ultimax() && addr & 0x3000 == 0x3000 {
            return self
                .cart
                .and_then(|c| c.peek_romh(0xf000 | (addr & 0x0fff)))
                .unwrap_or(0xff);
        }
        if self.bank & 0x4000 == 0 && addr & 0x3000 == 0x1000 {
            self.mem.roms.chargen[usize::from(addr & 0x0fff)]
        } else {
            self.mem.ram[usize::from(self.bank | addr)]
        }
    }

    fn color(&self, index: u16) -> u8 {
        self.mem.color[usize::from(index & 0x3ff)]
    }
}

impl Io {
    /// The CPU clock comes from the VIC-II's dot clock.
    pub fn new(vic: vic2::Model, model: CiaModel, sid: SidModel) -> Self {
        let cpu_hz = vic.timing().cpu_hz();
        Self {
            vic: Vic2::new(vic),
            cia1: Cia::new(model),
            cia2: Cia::new(model),
            sid: Sid::new(sid, cpu_hz),
            resampler: Decimator::new(cpu_hz, SAMPLE_RATE).expect("valid rates"),
            samples: Vec::new(),
            tod: ClockDivider::new(mains_hz(vic), cpu_hz).expect("CPU clock is non-zero"),
            keyboard: Keyboard::default(),
            port_lines: [0; 2],
            port_analog: [[u16::MAX; 2]; 2],
            iec: IecLines::default(),
        }
    }

    /// RESET line: both CIAs and the SID restart. The VIC-II has no reset
    /// pin and only starts over on power-up (`hard`).
    pub fn reset(&mut self, hard: bool) {
        self.cia1.reset();
        self.cia2.reset();
        self.sid = Sid::new(self.sid.model(), self.vic.timing().cpu_hz());
        if hard {
            self.vic = Vic2::new(self.vic.model());
        }
    }

    /// VIC bank selected by CIA2 PA0/PA1 (active low).
    pub fn vic_bank(&self) -> u16 {
        u16::from(!self.cia2.port_a_out() & 3) << 14
    }

    /// The byte on the data bus when nothing else drives it: the VIC-II's
    /// last fetch.
    pub fn open_bus(&self) -> u8 {
        self.vic.last_bus()
    }

    /// SID clock for one φ2 cycle; collects the resampled output.
    pub fn tick_sid(&mut self) {
        self.sid
            .set_pots(pot_lines(self.cia1.port_a_out(), self.port_analog));
        let s = self.sid.clock();
        if let Some(out) = self.resampler.push(s) {
            // Bound the backlog if nobody drains it (headless without --wav).
            if self.samples.len() < SAMPLE_RATE as usize * 4 {
                self.samples.push(out);
            }
        }
    }

    /// CIA clocks for one φ2 cycle, before the CPU's access.
    pub fn tick_cias(&mut self) {
        self.cia1.tick();
        self.cia2.tick();
        for _ in 0..self.tod.tick() {
            self.cia1.tod_tick();
            self.cia2.tod_tick();
        }
    }

    /// Levels on CIA1 ports A/B: control port 2 on PA0–4, control port 1 on
    /// PB0–4 (up, down, left, right, fire), then the keyboard matrix.
    fn cia1_ext(&self) -> (u8, u8) {
        let joy = |lines: u32| -> u8 { !((lines & 0x0f) as u8 | ((lines >> 1) & 0x10) as u8) };
        let a = self.cia1.port_a_out() & joy(self.port_lines[1]);
        let b = self.cia1.port_b_out() & joy(self.port_lines[0]);
        self.keyboard.resolve(a, b)
    }

    /// External levels on CIA2 ports: PA6 CLK in and PA7 DATA in read the
    /// serial bus line level (1 = released); the user port is open.
    fn cia2_ext(&self) -> (u8, u8) {
        let pa = 0x3f | if self.iec.clk { 0 } else { 0x40 } | if self.iec.data { 0 } else { 0x80 };
        (pa, 0xff)
    }

    pub fn read(&mut self, mem: &Memory, addr: u16) -> u8 {
        let reg = addr as u8 & 0x0f;
        match addr {
            0xd000..=0xd3ff => self.vic.read(addr as u8),
            0xdc00..=0xdcff => {
                let (a, b) = self.cia1_ext();
                self.cia1.read(reg, a, b)
            }
            0xdd00..=0xddff => {
                let (a, b) = self.cia2_ext();
                self.cia2.read(reg, a, b)
            }
            _ => self.peek(mem, addr),
        }
    }

    /// Side-effect-free read of an I/O address.
    pub fn peek(&self, mem: &Memory, addr: u16) -> u8 {
        let reg = addr as u8 & 0x0f;
        match addr {
            0xd000..=0xd3ff => self.vic.peek(addr as u8),
            0xd400..=0xd7ff => self.sid.peek(addr as u8),
            // Colour RAM: only 4 bits exist, the upper nibble is open bus.
            0xd800..=0xdbff => (self.open_bus() & 0xf0) | mem.color[usize::from(addr & 0x3ff)],
            0xdc00..=0xdcff => {
                let (a, b) = self.cia1_ext();
                self.cia1.peek(reg, a, b)
            }
            0xdd00..=0xddff => {
                let (a, b) = self.cia2_ext();
                self.cia2.peek(reg, a, b)
            }
            // I/O1, I/O2: no cartridge.
            _ => self.open_bus(),
        }
    }

    pub fn write(&mut self, mem: &mut Memory, addr: u16, v: u8) {
        let reg = addr as u8 & 0x0f;
        match addr {
            0xd000..=0xd3ff => self.vic.write(addr as u8, v),
            0xd400..=0xd7ff => self.sid.write(addr as u8, v),
            0xd800..=0xdbff => mem.color[usize::from(addr & 0x3ff)] = v & 0x0f,
            0xdc00..=0xdcff => self.cia1.write(reg, v),
            0xdd00..=0xddff => self.cia2.write(reg, v),
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
        w.chunk(*b"AUDR", 1, |w| self.resampler.save(w));
        self.tod.save(w);
        w.chunk(*b"C64K", 2, |w| {
            w.u64(self.keyboard.matrix);
            w.bool(self.keyboard.restore);
            w.u32(self.port_lines[0]);
            w.u32(self.port_lines[1]);
            w.bool(self.iec.atn);
            w.bool(self.iec.clk);
            w.bool(self.iec.data);
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
        let (_, mut c) = r.chunk(*b"AUDR", 1)?;
        self.resampler.load(&mut c)?;
        self.tod.load(r)?;
        let (version, mut c) = r.chunk(*b"C64K", 2)?;
        self.keyboard.matrix = c.u64()?;
        self.keyboard.restore = c.bool()?;
        self.port_lines = [c.u32()?, c.u32()?];
        self.iec = IecLines {
            atn: c.bool()?,
            clk: c.bool()?,
            data: c.bool()?,
            srq: false,
        };
        // Version 1 predates the analog lines.
        self.port_analog = if version >= 2 {
            [[c.u16()?, c.u16()?], [c.u16()?, c.u16()?]]
        } else {
            [[u16::MAX; 2]; 2]
        };
        Ok(())
    }
}

/// The pot lines the SID sees: CIA1 PA6 switches control port 1's POTX/POTY
/// to it, PA7 port 2's (4066 analog switch; both together: in parallel).
/// Source: Commodore 64 Programmer's Reference Guide, paddles.
pub fn pot_lines(cia1_pa: u8, ports: [[u16; 2]; 2]) -> [u16; 2] {
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
