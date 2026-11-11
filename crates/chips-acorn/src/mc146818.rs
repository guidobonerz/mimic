//! Motorola MC146818 real-time clock with 50 bytes of battery-backed RAM
//! (the BBC Master's CMOS RAM), on a multiplexed Motorola-style bus.
//!
//! **Registers**: 0 seconds, 1 seconds alarm, 2 minutes, 3 minutes alarm,
//! 4 hours, 5 hours alarm, 6 day of week (1 = Sunday), 7 date, 8 month,
//! 9 year (two digits); `A` (10): bit 7 update in progress, bits 6–4 the
//! divider (`010`: the 32.768 kHz crystal runs the clock), bits 3–0 the
//! periodic rate; `B` (11): bit 7 SET (the update stops), 6 periodic, 5
//! alarm, 4 update-ended interrupt enables, 3 square wave, 2 DM (1 binary,
//! 0 BCD), 1 24/12 (1 = 24 hours), 0 daylight saving; `C` (12, read only,
//! cleared by reading): bit 7 IRQF, 6 PF, 5 AF, 4 UF; `D` (13): bit 7 valid
//! RAM and time. 14–63 RAM.
//!
//! **Bus**: the falling edge of AS latches the address from the data bus;
//! with R/W high the chip drives the data bus while DS is high; with R/W
//! low it takes the data at the falling edge of DS. CE gates both.
//!
//! **Time**: counted in the machine's own cycles from the time the machine
//! was given (no host clock: determinism); an update once a second, with
//! update in progress set for the last 244 µs before it. In 12-hour mode
//! bit 7 of the hours is PM.
//!
//! Source: Motorola, *MC146818 Real-Time Clock Plus RAM (RTC)* data sheet
//! (register map, bus timing, update cycle); Acorn, *Advanced Reference
//! Manual for the BBC Master Series* (the CMOS RAM, the slow data bus).

use emu_core::{StateError, StateReader, StateWriter};

const REG_A: usize = 10;
const REG_B: usize = 11;
const REG_C: usize = 12;
const REG_D: usize = 13;

/// A calendar time to start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// `YYYY-MM-DD` with an optional `THH:MM[:SS]` (or a space).
    pub fn parse(s: &str) -> Option<Self> {
        let (date, time) = match s.split_once(['T', ' ']) {
            Some((d, t)) => (d, t),
            None => (s, "00:00:00"),
        };
        let mut d = date.split('-').map(|p| p.parse::<u16>().ok());
        let (year, month, day) = (d.next()??, d.next()??, d.next()??);
        let mut t = time.split(':').map(|p| p.parse::<u8>().ok());
        let hour = t.next()??;
        let minute = t.next()??;
        let second = t.next().unwrap_or(Some(0))?;
        let dt = Self {
            year,
            month: month as u8,
            day: day as u8,
            hour,
            minute,
            second,
        };
        let ok = (1..=12).contains(&dt.month)
            && dt.day >= 1
            && dt.day <= days_in_month(dt.year, dt.month)
            && dt.hour < 24
            && dt.minute < 60
            && dt.second < 60;
        ok.then_some(dt)
    }
}

impl Default for DateTime {
    /// 2000-01-01 00:00:00 (a Saturday).
    fn default() -> Self {
        Self {
            year: 2000,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }
}

fn leap(year: u16) -> bool {
    // The chip knows only every fourth year (no century rule).
    year.is_multiple_of(4)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Day of the week, 1 = Sunday (Zeller-style, Gregorian).
fn weekday(year: u16, month: u8, day: u8) -> u8 {
    let (y, m) = if month < 3 {
        (i32::from(year) - 1, i32::from(month) + 12)
    } else {
        (i32::from(year), i32::from(month))
    };
    let k = y % 100;
    let j = y / 100;
    let h = (i32::from(day) + 13 * (m + 1) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    // h: 0 = Saturday, 1 = Sunday, …
    (((h + 6) % 7) + 1) as u8
}

fn to_bcd(v: u8) -> u8 {
    ((v / 10) << 4) | (v % 10)
}

fn from_bcd(v: u8) -> u8 {
    (v >> 4) * 10 + (v & 15)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mc146818 {
    pub regs: [u8; 64],
    /// Clock cycles into the current second, and the cycles a second has.
    phase: u32,
    hz: u32,
    address: u8,
    /// The bus lines as last seen.
    as_line: bool,
    ds_line: bool,
}

impl Mc146818 {
    /// A clock at `start`, counting `hz` machine cycles a second; the RAM
    /// cleared, the clock in BCD and 24-hour mode, running.
    pub fn new(start: DateTime, hz: u32) -> Self {
        let mut c = Self {
            regs: [0; 64],
            phase: 0,
            hz: hz.max(1),
            address: 0,
            as_line: false,
            ds_line: false,
        };
        c.regs[REG_A] = 0x20;
        c.regs[REG_B] = 0x02;
        c.regs[REG_D] = 0x80;
        c.set_time(start);
        c
    }

    fn binary(&self) -> bool {
        self.regs[REG_B] & 0x04 != 0
    }

    fn enc(&self, v: u8) -> u8 {
        if self.binary() {
            v
        } else {
            to_bcd(v)
        }
    }

    fn dec(&self, v: u8) -> u8 {
        if self.binary() {
            v
        } else {
            from_bcd(v)
        }
    }

    pub fn set_time(&mut self, t: DateTime) {
        self.regs[0] = self.enc(t.second);
        self.regs[2] = self.enc(t.minute);
        self.regs[4] = self.encode_hour(t.hour);
        self.regs[6] = weekday(t.year, t.month, t.day);
        self.regs[7] = self.enc(t.day);
        self.regs[8] = self.enc(t.month);
        self.regs[9] = self.enc((t.year % 100) as u8);
    }

    fn encode_hour(&self, h: u8) -> u8 {
        if self.regs[REG_B] & 0x02 != 0 {
            self.enc(h)
        } else {
            let pm = h >= 12;
            let h12 = match h % 12 {
                0 => 12,
                n => n,
            };
            self.enc(h12) | u8::from(pm) << 7
        }
    }

    fn decode_hour(&self, v: u8) -> u8 {
        if self.regs[REG_B] & 0x02 != 0 {
            self.dec(v)
        } else {
            let h = self.dec(v & 0x7f) % 12;
            h + if v & 0x80 != 0 { 12 } else { 0 }
        }
    }

    fn running(&self) -> bool {
        self.regs[REG_A] & 0x70 == 0x20 && self.regs[REG_B] & 0x80 == 0
    }

    /// One machine cycle.
    #[inline]
    pub fn tick(&mut self) {
        if !self.running() {
            return;
        }
        self.phase += 1;
        // Update in progress for the last 244 µs.
        let uip_from = self.hz - (self.hz / 4098).max(1);
        if self.phase >= uip_from {
            self.regs[REG_A] |= 0x80;
        }
        if self.phase >= self.hz {
            self.phase = 0;
            self.regs[REG_A] &= 0x7f;
            self.advance();
        }
    }

    fn advance(&mut self) {
        let mut s = self.dec(self.regs[0]) + 1;
        let mut m = self.dec(self.regs[2]);
        let mut h = self.decode_hour(self.regs[4]);
        let mut wd = self.regs[6];
        let mut d = self.dec(self.regs[7]);
        let mut mo = self.dec(self.regs[8]).clamp(1, 12);
        let mut y = self.dec(self.regs[9]);
        if s >= 60 {
            s = 0;
            m += 1;
        }
        if m >= 60 {
            m = 0;
            h += 1;
        }
        if h >= 24 {
            h = 0;
            wd = wd % 7 + 1;
            d += 1;
            if d > days_in_month(2000 + u16::from(y), mo) {
                d = 1;
                mo += 1;
                if mo > 12 {
                    mo = 1;
                    y = (y + 1) % 100;
                }
            }
        }
        self.regs[0] = self.enc(s);
        self.regs[2] = self.enc(m);
        self.regs[4] = self.encode_hour(h);
        self.regs[6] = wd;
        self.regs[7] = self.enc(d);
        self.regs[8] = self.enc(mo);
        self.regs[9] = self.enc(y);
        let mut flags = 0x10;
        let alarm = |at: u8, now: u8| at & 0xc0 == 0xc0 || at == now;
        if alarm(self.regs[1], self.regs[0])
            && alarm(self.regs[3], self.regs[2])
            && alarm(self.regs[5], self.regs[4])
        {
            flags |= 0x20;
        }
        self.regs[REG_C] |= flags;
        if self.regs[REG_C] & self.regs[REG_B] & 0x70 != 0 {
            self.regs[REG_C] |= 0x80;
        }
    }

    fn read_reg(&self, a: usize) -> u8 {
        self.regs[a]
    }

    fn write_reg(&mut self, a: usize, v: u8) {
        match a {
            REG_A => self.regs[a] = (self.regs[a] & 0x80) | (v & 0x7f),
            REG_C | REG_D => {}
            REG_B => {
                self.regs[a] = v;
                if v & 0x80 != 0 {
                    // SET: the update stops and the update-ended flag goes.
                    self.regs[REG_A] &= 0x7f;
                }
            }
            _ => self.regs[a] = v,
        }
    }

    /// The bus lines now: chip enable, address strobe, data strobe, R/W
    /// (true: read) and the data bus as driven by the CPU. Returns the
    /// byte the chip drives onto the bus, if any.
    pub fn bus(&mut self, ce: bool, as_line: bool, ds: bool, read: bool, data: u8) -> Option<u8> {
        if ce && self.as_line && !as_line {
            self.address = data & 0x3f;
        }
        let a = usize::from(self.address);
        if ce && self.ds_line && !ds {
            if read {
                if a == REG_C {
                    self.regs[REG_C] = 0;
                }
            } else {
                self.write_reg(a, data);
            }
        }
        self.as_line = as_line;
        self.ds_line = ds;
        (ce && ds && read).then(|| self.read_reg(a))
    }

    /// The byte the chip drives with these lines, without side effects.
    pub fn driving(&self, ce: bool, ds: bool, read: bool) -> Option<u8> {
        (ce && ds && read).then(|| self.read_reg(usize::from(self.address)))
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.bytes(&self.regs);
        w.u32(self.phase);
        w.u32(self.hz);
        w.u8(self.address);
        w.bool(self.as_line);
        w.bool(self.ds_line);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let regs = r.bytes()?;
        if regs.len() != 64 {
            return Err(StateError::Invalid("RTC registers"));
        }
        let phase = r.u32()?;
        let hz = r.u32()?.max(1);
        let address = r.u8()? & 0x3f;
        let as_line = r.bool()?;
        let ds_line = r.bool()?;
        self.regs.copy_from_slice(regs);
        (self.phase, self.hz, self.address) = (phase % hz, hz, address);
        (self.as_line, self.ds_line) = (as_line, ds_line);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(c: &mut Mc146818, a: u8, v: u8) {
        c.bus(true, true, false, false, a);
        c.bus(true, false, false, false, a);
        c.bus(true, false, true, false, v);
        c.bus(true, false, false, false, v);
    }

    fn read(c: &mut Mc146818, a: u8) -> u8 {
        c.bus(true, true, false, true, a);
        c.bus(true, false, false, true, a);
        let v = c.bus(true, false, true, true, 0).unwrap();
        c.bus(true, false, false, true, 0);
        v
    }

    #[test]
    fn ram_and_registers_through_the_bus() {
        let mut c = Mc146818::new(DateTime::default(), 1000);
        write(&mut c, 30, 0x5a);
        assert_eq!(read(&mut c, 30), 0x5a);
        assert_eq!(read(&mut c, 13), 0x80, "valid RAM and time");
        assert_eq!(read(&mut c, 6), 7, "2000-01-01 was a Saturday");
        assert_eq!(read(&mut c, 9), 0x00);
        // Without chip enable nothing happens.
        c.bus(false, true, false, false, 31);
        c.bus(false, false, false, false, 31);
        assert_eq!(c.bus(false, false, true, true, 0), None);
    }

    #[test]
    fn the_clock_counts_in_bcd_through_midnight_and_the_month() {
        let start = DateTime::parse("2024-02-28T23:59:59").unwrap();
        let mut c = Mc146818::new(start, 100);
        for _ in 0..100 {
            c.tick();
        }
        assert_eq!(&c.regs[0..10], &[0, 0, 0, 0, 0, 0, 5, 0x29, 0x02, 0x24]);
        assert!(c.regs[12] & 0x10 != 0, "update ended");
        assert_eq!(read(&mut c, 12) & 0x10, 0x10);
        assert_eq!(read(&mut c, 12), 0, "cleared by the read");
        // SET stops it.
        write(&mut c, 11, 0x82);
        for _ in 0..1000 {
            c.tick();
        }
        assert_eq!(c.regs[0], 0);
    }

    #[test]
    fn dates_parse_and_check() {
        assert!(DateTime::parse("2026-11-11").is_some());
        assert!(DateTime::parse("2026-02-30").is_none());
        assert_eq!(DateTime::parse("2026-11-11 10:20").unwrap().minute, 20);
        assert_eq!(weekday(2026, 11, 11), 4, "a Wednesday");
    }
}
