//! The I/O block `$9000–$93FF` and the chips behind it.
//!
//! | Range         | Chip                                              |
//! | ------------- | ------------------------------------------------- |
//! | `$9000–$90FF` | VIC, 16 registers mirrored                        |
//! | `$9100–$93FF` | VIA1 where A4 = 1 (`$9110`), VIA2 where A5 = 1 (`$9120`) |
//!
//! Wiring (VIC-20 Programmer's Reference Guide, VIA tables):
//!
//! - VIA1 (`$9110`): PA0 serial CLK in, PA1 serial DATA in, PA2 joystick
//!   up, PA3 down, PA4 left, PA5 fire, PA6 cassette sense, PA7 serial ATN
//!   out; port B user port; CA1 RESTORE key; CA2 cassette motor (low =
//!   on). Its IRQ output drives /NMI.
//! - VIA2 (`$9120`): port A keyboard rows (in), port B keyboard columns
//!   (out) with PB7 also joystick right and PB3 also cassette write; CA2
//!   serial CLK out, CB2 serial DATA out; CA1 cassette read, CB1 serial SRQ
//!   in. Its IRQ output drives /IRQ.
//!
//! Serial outputs go through 7406 inverters (1 = line pulled low); the
//! inputs read the line level (1 = released).

use chips_cbm::via::Via;
use chips_cbm::vic::{self, Vic};
use drives_cbm::IecLines;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

use crate::keyboard::Keyboard;

/// Joystick pins in `PortLines::pulled_low` (see `devices_input::joystick`).
const JOY_UP: u32 = 1 << 0;
const JOY_DOWN: u32 = 1 << 1;
const JOY_LEFT: u32 = 1 << 2;
const JOY_RIGHT: u32 = 1 << 3;
const JOY_FIRE: u32 = 1 << 5;

#[derive(Clone, Debug, PartialEq)]
pub struct Io {
    pub vic: Vic,
    pub via1: Via,
    pub via2: Via,
    pub keyboard: Keyboard,
    /// Pins pulled low by the device in the control port.
    pub port_lines: u32,
    /// Serial bus levels after the last drive update.
    pub iec: IecLines,
    /// A datasette key is down (VIA1 PA6 low).
    pub cassette_sense: bool,
}

impl Io {
    pub fn new(vic: vic::Model) -> Self {
        Self {
            vic: Vic::new(vic),
            via1: Via::new(),
            via2: Via::new(),
            keyboard: Keyboard::default(),
            port_lines: 0,
            iec: IecLines::default(),
            cassette_sense: false,
        }
    }

    /// RESET line: both VIAs restart. The VIC has no reset pin and only
    /// starts over on power-up (`hard`).
    pub fn reset(&mut self, hard: bool) {
        self.via1.reset();
        self.via2.reset();
        if hard {
            self.vic = Vic::new(self.vic.model());
        }
    }

    /// The byte on the data bus when nothing drives it: the VIC's last fetch.
    pub fn open_bus(&self) -> u8 {
        self.vic.last_bus()
    }

    /// VIA1 port A pin levels: what the VIA drives, pulled low by the
    /// serial bus and the joystick. Port B (user port) is open.
    pub fn via1_pins(&self) -> (u8, u8) {
        let mut ext = 0xff;
        for (low, bit) in [
            (self.iec.clk, 0x01),
            (self.iec.data, 0x02),
            (self.port_lines & JOY_UP != 0, 0x04),
            (self.port_lines & JOY_DOWN != 0, 0x08),
            (self.port_lines & JOY_LEFT != 0, 0x10),
            (self.port_lines & JOY_FIRE != 0, 0x20),
            (self.cassette_sense, 0x40),
        ] {
            if low {
                ext &= !bit;
            }
        }
        (self.via1.port_a_out() & ext, 0xff)
    }

    /// VIA2 pin levels: the keyboard matrix between port B (columns, PB7
    /// also pulled by joystick right) and port A (rows).
    pub fn via2_pins(&self) -> (u8, u8) {
        let right = if self.port_lines & JOY_RIGHT != 0 {
            0x7f
        } else {
            0xff
        };
        let (cols, rows) = self
            .keyboard
            .resolve(self.via2.port_b_out() & right, self.via2.port_a_out());
        (rows, cols)
    }

    /// What the VIC-20 pulls on the serial bus.
    pub fn iec_out(&self) -> IecLines {
        IecLines {
            atn: self.via1.port_a_out() & 0x80 != 0,
            clk: self.via2.ca2_out(),
            data: self.via2.cb2_out(),
            srq: false,
        }
    }

    /// VIA clocks for one φ2 cycle, before the CPU's access; RESTORE on CA1.
    pub fn tick_vias(&mut self) {
        self.via1.tick();
        self.via2.tick();
        let (pa, _) = self.via1_pins();
        self.via1.set_ca1(!self.keyboard.restore, pa);
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        let reg = addr as u8 & 0x0f;
        match addr {
            0x9000..=0x90ff => self.vic.read(reg),
            _ => {
                let mut v = 0xff;
                let mut selected = false;
                if addr & 0x10 != 0 {
                    let (a, b) = self.via1_pins();
                    v &= self.via1.read(reg, a, b);
                    selected = true;
                }
                if addr & 0x20 != 0 {
                    let (a, b) = self.via2_pins();
                    v &= self.via2.read(reg, a, b);
                    selected = true;
                }
                if selected {
                    v
                } else {
                    self.open_bus()
                }
            }
        }
    }

    /// Side-effect-free read of an I/O address.
    pub fn peek(&self, addr: u16) -> u8 {
        let reg = addr as u8 & 0x0f;
        match addr {
            0x9000..=0x90ff => self.vic.peek(reg),
            _ => {
                let mut v = 0xff;
                let mut selected = false;
                if addr & 0x10 != 0 {
                    let (a, b) = self.via1_pins();
                    v &= self.via1.peek(reg, a, b);
                    selected = true;
                }
                if addr & 0x20 != 0 {
                    let (a, b) = self.via2_pins();
                    v &= self.via2.peek(reg, a, b);
                    selected = true;
                }
                if selected {
                    v
                } else {
                    self.open_bus()
                }
            }
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        let reg = addr as u8 & 0x0f;
        match addr {
            0x9000..=0x90ff => self.vic.write(reg, v),
            _ => {
                if addr & 0x10 != 0 {
                    self.via1.write(reg, v);
                }
                if addr & 0x20 != 0 {
                    self.via2.write(reg, v);
                }
            }
        }
    }
}

impl Savestate for Io {
    fn save(&self, w: &mut StateWriter) {
        self.vic.save(w);
        self.via1.save(w);
        self.via2.save(w);
        w.chunk(*b"V20K", 1, |w| {
            w.u64(self.keyboard.matrix);
            w.bool(self.keyboard.restore);
            w.u32(self.port_lines);
            w.bool(self.iec.atn);
            w.bool(self.iec.clk);
            w.bool(self.iec.data);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.vic.load(r)?;
        self.via1.load(r)?;
        self.via2.load(r)?;
        let (_, mut c) = r.chunk(*b"V20K", 1)?;
        self.keyboard.matrix = c.u64()?;
        self.keyboard.restore = c.bool()?;
        self.port_lines = c.u32()?;
        self.iec = IecLines {
            atn: c.bool()?,
            clk: c.bool()?,
            data: c.bool()?,
            srq: false,
        };
        Ok(())
    }
}
