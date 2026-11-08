//! Intel 8253 programmable interval timer, as on the IMSAI MPU-B card:
//! memory-mapped at $D100 (counter 0), $D101 (1), $D102 (2) and $D103
//! (control word), every counter clocked at 2 MHz with its gate high.
//! The MPU-B monitor programs counter 0 in mode 3 as the baud-rate clock
//! of the card's 8251 (count 13 = 9600 baud at a ×16 clock), which fixes
//! the clock at the CPU's 2 MHz (`mpu-b-rom.asm`, baud table #2, in Bruce
//! Hall's listings).
//!
//! The counters are evaluated lazily: a counter keeps the clock it was
//! loaded at, and a read works out its value from the clocks since. The
//! outputs drive nothing on this machine; counter 0's period sets the
//! MPU 8251's character time.
//!
//! **[S]** (this project's choices, the data sheet leaves them open or the
//! gate makes them unreachable): modes 1 and 5 wait for a gate edge that
//! never comes and hold their count; a new count in modes 2 and 3 takes
//! effect at once, not at the end of the running period.
//!
//! Source: Intel 8253/8253-5 Programmable Interval Timer data sheet
//! (control word, read/write order, counter latch, modes 0–5, BCD).

use emu_core::{StateError, StateReader, StateWriter};

/// Read/write order of a counter's bytes (control word bits 5–4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Access {
    #[default]
    Lsb,
    Msb,
    Both,
}

#[derive(Clone, Debug, Default)]
struct Counter {
    mode: u8,
    bcd: bool,
    access: Access,
    /// The count written (binary; 0 means the full range).
    reload: u32,
    /// Counting since this clock; `None` until a count is written.
    loaded_at: Option<u64>,
    /// Both-byte access: the next write is the MSB.
    write_msb: bool,
    /// The LSB written so far.
    lsb: u8,
    /// Both-byte access: the next read is the MSB.
    read_msb: bool,
    /// The value a counter latch command froze, until read.
    latched: Option<u16>,
}

impl Counter {
    /// The counting range of a full (0) count.
    fn range(&self) -> u32 {
        if self.bcd {
            10_000
        } else {
            0x1_0000
        }
    }

    fn period(&self) -> u32 {
        if self.reload == 0 {
            self.range()
        } else {
            self.reload
        }
    }

    /// The counter's value at clock `t` (binary).
    fn value(&self, t: u64) -> u32 {
        let Some(at) = self.loaded_at else {
            return self.reload;
        };
        let n = u64::from(self.period());
        let e = t.saturating_sub(at);
        let range = u64::from(self.range());
        match self.mode {
            // Counting down from N, on through 0 and round the range.
            0 | 4 => ((n + range - e % range) % range) as u32,
            // N … 1, again.
            2 => (n - e % n) as u32,
            // Down by two each clock, reloading every half period.
            3 => {
                let half = n.div_ceil(2);
                let phase = e % n;
                let into = if phase < half { phase } else { phase - half };
                let start = if n % 2 == 1 && phase >= half {
                    n - 1
                } else {
                    n
                };
                (start.saturating_sub(2 * into).max(2) & !1).min(n) as u32
            }
            _ => self.reload,
        }
    }

    /// The output pin at clock `t` (high = true).
    fn out(&self, t: u64) -> bool {
        let Some(at) = self.loaded_at else {
            return self.mode != 0;
        };
        let n = u64::from(self.period());
        let e = t.saturating_sub(at);
        match self.mode {
            0 => e >= n,
            2 => e % n != n - 1,
            3 => e % n < n.div_ceil(2),
            4 => e != n,
            _ => true,
        }
    }

    fn shown(&self, v: u32) -> u16 {
        if !self.bcd {
            return v as u16;
        }
        let v = v % 10_000;
        ((v / 1000) << 12 | (v / 100 % 10) << 8 | (v / 10 % 10) << 4 | (v % 10)) as u16
    }

    fn count_of(&self, v: u16) -> u32 {
        if !self.bcd {
            return u32::from(v);
        }
        let d = |s: u16| u32::from(v >> s & 0xf).min(9);
        d(12) * 1000 + d(8) * 100 + d(4) * 10 + d(0)
    }
}

/// The three counters.
#[derive(Clone, Debug, Default)]
pub struct Pit {
    c: [Counter; 3],
}

impl Pit {
    pub fn new() -> Self {
        Self::default()
    }

    /// A write to address 0–3 (counter 0–2, control word) at clock `t`.
    pub fn write(&mut self, addr: u8, v: u8, t: u64) {
        match addr & 3 {
            3 => {
                let n = usize::from(v >> 6);
                if n == 3 {
                    return; // not on the 8253
                }
                let c = &mut self.c[n];
                let access = v >> 4 & 3;
                if access == 0 {
                    // Counter latch: the value now, until read.
                    if c.latched.is_none() {
                        c.latched = Some(c.shown(c.value(t)));
                    }
                    return;
                }
                c.access = match access {
                    1 => Access::Lsb,
                    2 => Access::Msb,
                    _ => Access::Both,
                };
                c.mode = match v >> 1 & 7 {
                    6 => 2,
                    7 => 3,
                    m => m,
                };
                c.bcd = v & 1 != 0;
                c.loaded_at = None;
                c.write_msb = false;
                c.read_msb = false;
                c.latched = None;
            }
            n => {
                let c = &mut self.c[usize::from(n)];
                let count = match c.access {
                    Access::Lsb => Some(u16::from(v)),
                    Access::Msb => Some(u16::from(v) << 8),
                    Access::Both if !c.write_msb => {
                        c.lsb = v;
                        c.write_msb = true;
                        // Mode 0 stops counting on the first byte.
                        if c.mode == 0 {
                            c.loaded_at = None;
                        }
                        None
                    }
                    Access::Both => {
                        c.write_msb = false;
                        Some(u16::from(v) << 8 | u16::from(c.lsb))
                    }
                };
                if let Some(count) = count {
                    c.reload = c.count_of(count);
                    // Modes 1 and 5 wait for a gate edge (the gate is
                    // tied high): they hold the count.
                    c.loaded_at = (c.mode != 1 && c.mode != 5).then_some(t + 1);
                }
            }
        }
    }

    /// A read of address 0–2 (a counter; the control word reads $FF).
    pub fn read(&mut self, addr: u8, t: u64) -> u8 {
        let n = usize::from(addr & 3);
        if n == 3 {
            return 0xff;
        }
        let v = self.peek(addr, t);
        let c = &mut self.c[n];
        match c.access {
            Access::Both => {
                if c.read_msb {
                    c.latched = None;
                }
                c.read_msb = !c.read_msb;
            }
            _ => c.latched = None,
        }
        v
    }

    /// [`Pit::read`] without its effects.
    pub fn peek(&self, addr: u8, t: u64) -> u8 {
        let n = usize::from(addr & 3);
        if n == 3 {
            return 0xff;
        }
        let c = &self.c[n];
        let v = c.latched.unwrap_or_else(|| c.shown(c.value(t)));
        let msb = match c.access {
            Access::Lsb => false,
            Access::Msb => true,
            Access::Both => c.read_msb,
        };
        if msb {
            (v >> 8) as u8
        } else {
            v as u8
        }
    }

    /// Counter `n`'s output at clock `t`.
    pub fn out(&self, n: usize, t: u64) -> bool {
        self.c[n].out(t)
    }

    /// Counter 0's period in clocks while it runs as a rate or square-wave
    /// generator (the MPU 8251's clock).
    pub fn period0(&self) -> Option<u32> {
        let c = &self.c[0];
        (c.loaded_at.is_some() && matches!(c.mode, 2 | 3)).then(|| c.period())
    }

    pub fn save(&self, w: &mut StateWriter) {
        for c in &self.c {
            w.u8(c.mode);
            w.bool(c.bcd);
            w.u8(match c.access {
                Access::Lsb => 1,
                Access::Msb => 2,
                Access::Both => 3,
            });
            w.u32(c.reload);
            w.bool(c.loaded_at.is_some());
            w.u64(c.loaded_at.unwrap_or(0));
            w.bool(c.write_msb);
            w.u8(c.lsb);
            w.bool(c.read_msb);
            w.bool(c.latched.is_some());
            w.u16(c.latched.unwrap_or(0));
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut p = Pit::new();
        for c in &mut p.c {
            c.mode = r.u8()?;
            if c.mode > 5 {
                return Err(StateError::Invalid("8253 mode"));
            }
            c.bcd = r.bool()?;
            c.access = match r.u8()? {
                1 => Access::Lsb,
                2 => Access::Msb,
                3 => Access::Both,
                _ => return Err(StateError::Invalid("8253 access")),
            };
            c.reload = r.u32()?;
            if c.reload > 0xffff {
                return Err(StateError::Invalid("8253 count"));
            }
            let loaded = r.bool()?;
            let at = r.u64()?;
            c.loaded_at = loaded.then_some(at);
            c.write_msb = r.bool()?;
            c.lsb = r.u8()?;
            c.read_msb = r.bool()?;
            let latched = r.bool()?;
            let v = r.u16()?;
            c.latched = latched.then_some(v);
        }
        *self = p;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a counter's two bytes (LSB then MSB access).
    fn read16(p: &mut Pit, n: u8, t: u64) -> u16 {
        u16::from(p.read(n, t)) | u16::from(p.read(n, t)) << 8
    }

    #[test]
    fn mode_3_as_the_mpu_b_programs_it() {
        // `setT0`: control $36 (counter 0, LSB then MSB, mode 3, binary),
        // count 13 (9600 baud at a ×16 clock).
        let mut p = Pit::new();
        p.write(3, 0x36, 100);
        p.write(0, 13, 101);
        p.write(0, 0, 102);
        assert_eq!(p.period0(), Some(13));
        // Odd count: high for 7 clocks, low for 6.
        let start = 103;
        let highs = (0..13).filter(|k| p.out(0, start + k)).count();
        assert_eq!(highs, 7);
        assert!(p.out(0, start + 13), "the next period starts high");
    }

    #[test]
    fn mode_0_counts_down_and_raises_its_output() {
        let mut p = Pit::new();
        p.write(3, 0x70, 0); // counter 1, both bytes, mode 0
        assert!(!p.out(1, 0), "mode 0 starts low");
        p.write(1, 0x00, 10);
        p.write(1, 0x01, 11); // 256
        assert_eq!(read16(&mut p, 1, 12 + 100), 156);
        assert!(!p.out(1, 12 + 255));
        assert!(p.out(1, 12 + 256), "high at terminal count");
        // A latch freezes the value until both bytes are read.
        p.write(3, 0x40, 12 + 200);
        assert_eq!(read16(&mut p, 1, 12 + 250), 56);
        assert_eq!(read16(&mut p, 1, 12 + 250), 6, "running again");
    }

    #[test]
    fn mode_2_and_bcd() {
        let mut p = Pit::new();
        p.write(3, 0b1001_0101, 0); // counter 2, MSB only, mode 2, BCD
        p.write(2, 0x10, 0); // 1000 in BCD
        assert_eq!(p.peek(2, 1), 0x10);
        assert_eq!(p.peek(2, 1 + 1), 0x09, "999: MSB $09");
        assert!(!p.out(2, 1 + 999), "low for one clock at 1");
        assert!(p.out(2, 1 + 1000));
    }

    #[test]
    fn savestate_round_trip() {
        let mut p = Pit::new();
        p.write(3, 0x36, 0);
        p.write(0, 13, 1);
        p.write(0, 0, 2);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let data = w.into_bytes();
        let mut q = Pit::new();
        q.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(q.period0(), Some(13));
        assert_eq!(q.peek(0, 500), p.peek(0, 500));
        assert!(Pit::new().load(&mut StateReader::new(&data[..5])).is_err());
    }
}
