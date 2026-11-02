//! IOC (VL86C410): the Archimedes' I/O controller. Interrupts, four timers
//! at 2 MHz, the keyboard's serial link (KART) and six control lines.
//!
//! Registers (byte, A6–A2, bank 0): $00 control, $04 serial data, $10/$14/
//! $18 IRQ A status / request (write: clear) / mask, $20/$24/$28 IRQ B,
//! $30/$34/$38 FIRQ, $40–$7C the timers: count low, high (write: latch),
//! go, latch command.
//!
//! IRQ A: 0 printer busy, 1 ring, 2 printer ack, 3 vertical flyback, 4 power
//! on, 5 and 6 timers 0 and 1, 7 force. IRQ B: 0 podule FIQ, 1 sound buffer
//! swap, 2 serial, 3 hard disc, 4 disc changed, 5 podule, 6 keyboard Tx
//! empty, 7 keyboard Rx full. FIRQ: 0 floppy data, 1 floppy interrupt, 2
//! Econet, 3–5 C3–C5, 6 podule, 7 force.
//!
//! Source: "Acorn RISC Machine Family Data Manual", VL86C410 chapter; Acorn
//! A500 Series Service Manual, part 1 (the interrupt wiring).

use emu_core::{StateError, StateReader, StateWriter};

pub const IRQ_A_FLYBACK: u8 = 0x08;
pub const IRQ_A_POWER_ON: u8 = 0x10;
pub const IRQ_A_TIMER0: u8 = 0x20;
pub const IRQ_A_TIMER1: u8 = 0x40;
pub const IRQ_A_FORCE: u8 = 0x80;
pub const IRQ_B_SOUND: u8 = 0x02;
pub const IRQ_B_TX: u8 = 0x40;
pub const IRQ_B_RX: u8 = 0x80;
pub const FIQ_FLOPPY_DATA: u8 = 0x01;
pub const FIQ_FLOPPY_INT: u8 = 0x02;
pub const FIQ_FORCE: u8 = 0x80;

#[derive(Clone, Copy, Debug, Default)]
struct Timer {
    latch: u16,
    count: u16,
    /// What a read returns, from the last latch command.
    read: u16,
}

#[derive(Clone, Debug)]
pub struct Ioc {
    /// C5–C0 as written (a high bit leaves the pin to others).
    pub control: u8,
    irq_a: u8,
    pub mask_a: u8,
    pub mask_b: u8,
    pub mask_fiq: u8,
    /// IRQ B lines from outside (bits 0–5).
    pub lines_b: u8,
    /// FIRQ lines from outside (bits 0–6).
    pub lines_fiq: u8,
    timers: [Timer; 4],
    /// 8 MHz clocks not yet counted by the timers.
    rest: u32,
    rx_data: u8,
    rx_full: bool,
    tx_empty: bool,
    /// The byte going out and the clocks until it has left.
    tx: Option<(u8, u32)>,
    /// A byte finished sending, for the keyboard.
    pub sent: Option<u8>,
}

impl Default for Ioc {
    fn default() -> Self {
        Self::new()
    }
}

impl Ioc {
    pub fn new() -> Self {
        Self {
            control: 0xff,
            irq_a: IRQ_A_POWER_ON,
            mask_a: 0,
            mask_b: 0,
            mask_fiq: 0,
            lines_b: 0,
            lines_fiq: 0,
            timers: [Timer::default(); 4],
            rest: 0,
            rx_data: 0,
            rx_full: false,
            tx_empty: true,
            tx: None,
            sent: None,
        }
    }

    /// /POR-less reset: the control lines go high.
    pub fn reset(&mut self) {
        self.control = 0xff;
        self.mask_a = 0;
        self.mask_b = 0;
        self.mask_fiq = 0;
    }

    fn status_a(&self) -> u8 {
        self.irq_a | IRQ_A_FORCE
    }

    fn status_b(&self) -> u8 {
        let tx = if self.tx_empty { IRQ_B_TX } else { 0 };
        let rx = if self.rx_full { IRQ_B_RX } else { 0 };
        self.lines_b | tx | rx
    }

    fn status_fiq(&self) -> u8 {
        self.lines_fiq | FIQ_FORCE
    }

    pub fn irq(&self) -> bool {
        self.status_a() & self.mask_a != 0 || self.status_b() & self.mask_b != 0
    }

    pub fn firq(&self) -> bool {
        self.status_fiq() & self.mask_fiq != 0
    }

    /// Vertical flyback began (rising edge on IR).
    pub fn flyback(&mut self) {
        self.irq_a |= IRQ_A_FLYBACK;
    }

    /// Register read at `addr & 0x7c`. `pins` is the value of the control
    /// register's input side.
    pub fn read(&mut self, addr: u32, pins: u8) -> u8 {
        let v = self.peek(addr, pins);
        if addr & 0x7c == 0x04 {
            self.rx_full = false;
        }
        v
    }

    pub fn peek(&self, addr: u32, pins: u8) -> u8 {
        let timer = |n: usize, high: bool| {
            let v = self.timers[n].read;
            if high {
                (v >> 8) as u8
            } else {
                v as u8
            }
        };
        match addr & 0x7c {
            0x00 => pins,
            0x04 => self.rx_data,
            0x10 => self.status_a(),
            0x14 => self.status_a() & self.mask_a,
            0x18 => self.mask_a,
            0x20 => self.status_b(),
            0x24 => self.status_b() & self.mask_b,
            0x28 => self.mask_b,
            0x30 => self.status_fiq(),
            0x34 => self.status_fiq() & self.mask_fiq,
            0x38 => self.mask_fiq,
            r @ (0x40 | 0x50 | 0x60 | 0x70) => timer((r as usize - 0x40) / 16, false),
            r @ (0x44 | 0x54 | 0x64 | 0x74) => timer((r as usize - 0x44) / 16, true),
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u32, v: u8) {
        let reg = addr & 0x7c;
        match reg {
            0x00 => self.control = v | 0xc0,
            0x04 => {
                let t3 = u32::from(self.timers[3].latch) + 1;
                // 11 bits at sixteen clocks of timer 3's output; timer 3
                // counts 2 MHz and toggles: 8 MHz clocks per byte.
                self.tx = Some((v, 11 * 16 * 2 * t3 * 4));
                self.tx_empty = false;
            }
            0x14 => self.irq_a &= !(v & 0x7c),
            0x18 => self.mask_a = v,
            0x28 => self.mask_b = v,
            0x38 => self.mask_fiq = v,
            0x40..=0x7c => {
                let n = ((reg - 0x40) / 16) as usize;
                let t = &mut self.timers[n];
                match reg & 0x0c {
                    0 => t.latch = (t.latch & 0xff00) | u16::from(v),
                    4 => t.latch = (t.latch & 0xff) | u16::from(v) << 8,
                    8 => t.count = t.latch,
                    _ => t.read = t.count,
                }
            }
            _ => {}
        }
    }

    /// A byte from the keyboard arrives.
    pub fn receive(&mut self, v: u8) {
        self.rx_data = v;
        self.rx_full = true;
    }

    pub fn rx_full(&self) -> bool {
        self.rx_full
    }

    pub fn timer_latch(&self, n: usize) -> u16 {
        self.timers[n].latch
    }

    /// `clocks` of the 8 MHz bus went by.
    pub fn tick(&mut self, clocks: u32) {
        if let Some((v, left)) = self.tx {
            if left <= clocks {
                self.tx = None;
                self.tx_empty = true;
                self.sent = Some(v);
            } else {
                self.tx = Some((v, left - clocks));
            }
        }
        self.rest += clocks;
        let ticks = self.rest / 4;
        self.rest %= 4;
        for n in 0..4 {
            let t = &mut self.timers[n];
            let mut left = ticks;
            while left > 0 {
                let c = u32::from(t.count);
                if left <= c {
                    t.count = (c - left) as u16;
                    break;
                }
                left -= c + 1;
                t.count = t.latch;
                if n == 0 {
                    self.irq_a |= IRQ_A_TIMER0;
                } else if n == 1 {
                    self.irq_a |= IRQ_A_TIMER1;
                }
                if t.latch == 0 {
                    break;
                }
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"IOC ", 1, |w| {
            for v in [
                self.control,
                self.irq_a,
                self.mask_a,
                self.mask_b,
                self.mask_fiq,
                self.lines_b,
                self.lines_fiq,
                self.rx_data,
            ] {
                w.u8(v);
            }
            for t in self.timers {
                w.u16(t.latch);
                w.u16(t.count);
                w.u16(t.read);
            }
            w.u32(self.rest);
            w.bool(self.rx_full);
            w.bool(self.tx_empty);
            w.bool(self.tx.is_some());
            let (v, left) = self.tx.unwrap_or((0, 0));
            w.u8(v);
            w.u32(left);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"IOC ", 1)?;
        self.control = c.u8()?;
        self.irq_a = c.u8()?;
        self.mask_a = c.u8()?;
        self.mask_b = c.u8()?;
        self.mask_fiq = c.u8()?;
        self.lines_b = c.u8()?;
        self.lines_fiq = c.u8()?;
        self.rx_data = c.u8()?;
        for t in &mut self.timers {
            t.latch = c.u16()?;
            t.count = c.u16()?;
            t.read = c.u16()?;
        }
        self.rest = c.u32()?;
        self.rx_full = c.bool()?;
        self.tx_empty = c.bool()?;
        let busy = c.bool()?;
        let v = c.u8()?;
        let left = c.u32()?;
        self.tx = busy.then_some((v, left));
        self.sent = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_reloads_and_interrupts() {
        let mut i = Ioc::new();
        i.write(0x40, 9);
        i.write(0x44, 0);
        i.write(0x48, 0);
        i.tick(4 * 5);
        i.write(0x4c, 0);
        assert_eq!(i.read(0x40, 0), 4);
        assert_eq!(i.read(0x10, 0) & IRQ_A_TIMER0, 0);
        i.tick(4 * 5);
        assert_ne!(i.read(0x10, 0) & IRQ_A_TIMER0, 0);
        i.write(0x18, IRQ_A_TIMER0);
        assert!(i.irq());
        i.write(0x14, IRQ_A_TIMER0);
        assert!(!i.irq());
    }

    #[test]
    fn force_bits_and_power_on() {
        let mut i = Ioc::new();
        assert_ne!(i.read(0x10, 0) & (IRQ_A_FORCE | IRQ_A_POWER_ON), 0);
        i.write(0x18, IRQ_A_FORCE);
        assert!(i.irq());
        i.write(0x38, FIQ_FORCE);
        assert!(i.firq());
    }

    #[test]
    fn kart_sends_and_receives() {
        let mut i = Ioc::new();
        i.write(0x70, 1);
        i.write(0x04, 0xff);
        assert_eq!(i.read(0x20, 0) & IRQ_B_TX, 0);
        i.tick(1_000_000);
        assert_ne!(i.read(0x20, 0) & IRQ_B_TX, 0);
        assert_eq!(i.sent, Some(0xff));
        i.receive(0x42);
        assert_ne!(i.read(0x20, 0) & IRQ_B_RX, 0);
        assert_eq!(i.read(0x04, 0), 0x42);
        assert_eq!(i.read(0x20, 0) & IRQ_B_RX, 0);
    }

    #[test]
    fn savestate_round_trip() {
        let mut i = Ioc::new();
        i.write(0x18, 0x55);
        let mut w = StateWriter::new();
        i.save(&mut w);
        let b = w.into_bytes();
        let mut j = Ioc::new();
        j.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(j.mask_a, 0x55);
    }
}
