//! The keyboard's own controller at the other end of the IOC's serial link.
//!
//! Half duplex, a byte at a time. Reset: the keyboard sends HRST and on
//! each of the computer's HRST, RAK1 and RAK2 echoes it. Key changes go out as two bytes, KDDA or KUDA with the row and with
//! the column, the first answered by BACK, the last by NACK, SACK, MACK or
//! SMAK, which also say whether keys and mouse may be sent. Mouse motion is
//! two bytes of seven-bit counts.
//!
//! Machine keys: row × 16 + column, the matrix codes of the keyboard. The
//! mouse buttons are keys too (row 7).
//!
//! Source: Acorn A3000 Technical Reference Manual, "The keyboard and mouse"
//! (protocol); code values and key numbers as documented in the RISC OS
//! Programmer's Reference Manual.

use std::collections::VecDeque;

use emu_core::{StateError, StateReader, StateWriter};

pub const HRST: u8 = 0xff;
pub const RAK1: u8 = 0xfe;
pub const RAK2: u8 = 0xfd;
pub const BACK: u8 = 0x3f;
pub const NACK: u8 = 0x30;
pub const SMAK: u8 = 0x33;
pub const RQID: u8 = 0x20;
pub const PRST: u8 = 0x21;
pub const RQMP: u8 = 0x22;
pub const KBID: u8 = 0x80;
pub const KDDA: u8 = 0xc0;
pub const KUDA: u8 = 0xd0;

/// Mouse buttons as keys: Select, Menu, Adjust.
pub const MOUSE_KEYS: [u8; 3] = [0x70, 0x71, 0x72];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// Nothing outstanding.
    Free,
    /// HRST sent, HRST awaited.
    Hrst,
    /// RAK1 sent.
    Rak1,
    /// RAK2 sent.
    Rak2,
    /// BACK before the given second byte.
    Back(u8),
    /// The closing acknowledge.
    Ack,
}

#[derive(Clone, Debug)]
pub struct Keyboard {
    wait: Wait,
    /// Bytes ready to go to the computer.
    out: VecDeque<u8>,
    scan: bool,
    mouse: bool,
    keys: VecDeque<(u8, bool)>,
    dx: i32,
    dy: i32,
    buttons: u8,
    pub leds: u8,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Keyboard {
    /// At power-on it announces itself with HRST.
    pub fn new() -> Self {
        Self {
            wait: Wait::Hrst,
            out: VecDeque::from([HRST]),
            scan: false,
            mouse: false,
            keys: VecDeque::new(),
            dx: 0,
            dy: 0,
            buttons: 0,
            leds: 0,
        }
    }

    pub fn key(&mut self, key: u8, down: bool) {
        if self.scan {
            self.keys.push_back((key, down));
        }
    }

    /// Host mouse motion; `buttons` bit 0 left (Select), 1 right (Menu), 2
    /// middle (Adjust).
    pub fn mouse(&mut self, dx: i32, dy: i32, buttons: u8) {
        self.dx += dx;
        self.dy -= dy;
        let changed = self.buttons ^ buttons;
        self.buttons = buttons;
        for (bit, key) in MOUSE_KEYS.into_iter().enumerate() {
            if changed >> bit & 1 != 0 {
                self.key(key, buttons >> bit & 1 != 0);
            }
        }
    }

    /// A byte from the computer.
    pub fn receive(&mut self, b: u8) {
        match b {
            HRST => {
                self.out.clear();
                self.out.push_back(HRST);
                self.wait = Wait::Rak1;
                self.scan = false;
                self.mouse = false;
                self.keys.clear();
            }
            RAK1 if matches!(self.wait, Wait::Hrst | Wait::Rak1) => {
                self.out.push_back(RAK1);
                self.wait = Wait::Rak2;
            }
            RAK2 if self.wait == Wait::Rak2 => {
                self.out.push_back(RAK2);
                self.wait = Wait::Hrst;
            }
            BACK => {
                if let Wait::Back(second) = self.wait {
                    self.out.push_back(second);
                    self.wait = Wait::Ack;
                }
            }
            NACK..=SMAK => {
                self.scan = b & 1 != 0;
                self.mouse = b & 2 != 0;
                self.wait = Wait::Free;
            }
            RQID => self.out.push_back(KBID | 1),
            PRST => {}
            RQMP => {
                let (x, y) = self.take_motion();
                self.out.push_back(x);
                self.wait = Wait::Back(y);
            }
            0x40..=0x4f => self.out.push_back(0xe0 | (b & 15)),
            0x00..=0x07 => self.leds = b,
            _ => {
                if self.wait != Wait::Hrst {
                    self.out.clear();
                    self.out.push_back(HRST);
                    self.wait = Wait::Hrst;
                }
            }
        }
    }

    fn take_motion(&mut self) -> (u8, u8) {
        let x = self.dx.clamp(-64, 63);
        let y = self.dy.clamp(-64, 63);
        self.dx -= x;
        self.dy -= y;
        ((x & 0x7f) as u8, (y & 0x7f) as u8)
    }

    /// The next byte for the computer, if one may go.
    pub fn next_byte(&mut self) -> Option<u8> {
        if let Some(b) = self.out.pop_front() {
            return Some(b);
        }
        if self.wait != Wait::Free {
            return None;
        }
        if let Some((key, down)) = self.keys.pop_front() {
            let code = if down { KDDA } else { KUDA };
            self.wait = Wait::Back(code | (key & 15));
            return Some(code | (key >> 4));
        }
        if self.mouse && (self.dx != 0 || self.dy != 0) {
            let (x, y) = self.take_motion();
            self.wait = Wait::Back(y);
            return Some(x);
        }
        None
    }

    pub fn save(&self, w: &mut StateWriter) {
        let (kind, arg) = match self.wait {
            Wait::Free => (0, 0),
            Wait::Hrst => (1, 0),
            Wait::Rak1 => (2, 0),
            Wait::Rak2 => (3, 0),
            Wait::Back(b) => (4, b),
            Wait::Ack => (5, 0),
        };
        w.chunk(*b"AKBD", 1, |w| {
            w.u8(kind);
            w.u8(arg);
            w.bytes(&self.out.iter().copied().collect::<Vec<u8>>());
            w.bool(self.scan);
            w.bool(self.mouse);
            w.u32(self.keys.len() as u32);
            for (k, d) in &self.keys {
                w.u8(*k);
                w.bool(*d);
            }
            w.u32(self.dx as u32);
            w.u32(self.dy as u32);
            w.u8(self.buttons);
            w.u8(self.leds);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"AKBD", 1)?;
        let kind = c.u8()?;
        let arg = c.u8()?;
        self.wait = match kind {
            0 => Wait::Free,
            1 => Wait::Hrst,
            2 => Wait::Rak1,
            3 => Wait::Rak2,
            4 => Wait::Back(arg),
            _ => Wait::Ack,
        };
        self.out = c.bytes()?.iter().copied().collect();
        self.scan = c.bool()?;
        self.mouse = c.bool()?;
        let n = c.u32()?;
        self.keys.clear();
        for _ in 0..n {
            let k = c.u8()?;
            let d = c.bool()?;
            self.keys.push_back((k, d));
        }
        self.dx = c.u32()? as i32;
        self.dy = c.u32()? as i32;
        self.buttons = c.u8()?;
        self.leds = c.u8()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Keyboard {
        let mut k = Keyboard::new();
        assert_eq!(k.next_byte(), Some(HRST));
        k.receive(RAK1);
        assert_eq!(k.next_byte(), Some(RAK1));
        k.receive(RAK2);
        assert_eq!(k.next_byte(), Some(RAK2));
        k.receive(SMAK);
        k
    }

    #[test]
    fn a_key_goes_as_row_and_column() {
        let mut k = ready();
        k.key(0x27, true); // Q
        assert_eq!(k.next_byte(), Some(KDDA | 2));
        assert_eq!(k.next_byte(), None, "waits for BACK");
        k.receive(BACK);
        assert_eq!(k.next_byte(), Some(KDDA | 7));
        k.receive(SMAK);
        k.key(0x27, false);
        assert_eq!(k.next_byte(), Some(KUDA | 2));
    }

    #[test]
    fn mouse_counts_and_buttons() {
        let mut k = ready();
        k.mouse(5, 3, 0);
        assert_eq!(k.next_byte(), Some(5));
        k.receive(BACK);
        assert_eq!(k.next_byte(), Some(0x7d), "-3 in seven bits: Y up");
        k.receive(SMAK);
        k.mouse(0, 0, 1);
        assert_eq!(k.next_byte(), Some(KDDA | 7));
    }

    #[test]
    fn scanning_off_drops_keys_and_rqid_answers() {
        let mut k = ready();
        k.receive(NACK);
        k.key(0x10, true);
        assert_eq!(k.next_byte(), None);
        k.receive(RQID);
        assert_eq!(k.next_byte(), Some(KBID | 1));
    }

    #[test]
    fn savestate_round_trip() {
        let mut k = ready();
        k.key(0x10, true);
        let mut w = StateWriter::new();
        k.save(&mut w);
        let b = w.into_bytes();
        let mut n = Keyboard::new();
        n.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(n.next_byte(), Some(KDDA | 1));
    }
}
