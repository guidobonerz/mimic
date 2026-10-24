//! The keyboard: an encoder turns a key press into a 7-bit ASCII code in
//! the keyboard latch with the strobe (bit 7) set; reading $C000 shows the
//! latch, an access to $C010 clears the strobe (Apple II Reference Manual,
//! "The Keyboard"; Apple IIe Technical Reference Manual, chapter 2). On
//! the IIe reading $C010 also tells in bit 7 whether any key is down, and
//! a held key repeats after about half a second, 15 times a second.
//!
//! **Machine keys** ([`keys`]) are key positions named by what the key
//! sends unshifted on the IIe's US keyboard (letters by their lower-case
//! code), plus the modifiers, arrows and the RESET key. The encoder works
//! like the IIe's for every model: SHIFT gives the shifted character of
//! the US layout, CONTROL the control code of letters and @ [ \ ] ^ _;
//! the II and II+ keyboards have no lower case (letters always arrive in
//! upper case) and no up/down arrows or TAB. So a PC keyboard types what
//! its keycaps show; the II+'s own key layout (Shift-2 for ", Shift-P for
//! @) is not reproduced.

use emu_core::{StateError, StateReader, StateWriter};

/// Machine key codes (see the module docs).
pub mod keys {
    pub const RETURN: u16 = 0x0d;
    pub const ESC: u16 = 0x1b;
    pub const TAB: u16 = 0x09;
    pub const DELETE: u16 = 0x7f;
    pub const SPACE: u16 = 0x20;
    pub const LEFT: u16 = 0x08;
    pub const RIGHT: u16 = 0x15;
    pub const UP: u16 = 0x0b;
    pub const DOWN: u16 = 0x0a;
    pub const SHIFT: u16 = 0x100;
    pub const CONTROL: u16 = 0x101;
    pub const CAPS_LOCK: u16 = 0x102;
    pub const RESET: u16 = 0x103;
    /// IIe: the Apple keys, wired to push buttons 0 and 1.
    pub const OPEN_APPLE: u16 = 0x104;
    pub const SOLID_APPLE: u16 = 0x105;
}

/// Cycles before a held key repeats, and between repeats (IIe).
const REPEAT_DELAY: u64 = 545_000;
const REPEAT_RATE: u64 = 68_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    pub iie: bool,
    /// Keyboard latch: ASCII in bits 6–0, strobe in bit 7.
    pub latch: u8,
    shift: bool,
    control: bool,
    /// IIe CAPS LOCK (a locking key; down at power-on).
    pub caps: bool,
    pub open_apple: bool,
    pub solid_apple: bool,
    /// Character keys held, the last one pressed, and when it repeats.
    held: Vec<u16>,
    repeat_at: Option<u64>,
}

/// The US-layout shifted character of an unshifted one.
fn shifted(c: u8) -> u8 {
    match c {
        b'a'..=b'z' => c - 0x20,
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b'\\' => b'|',
        b';' => b':',
        b'\'' => b'"',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        b'`' => b'~',
        _ => c,
    }
}

impl Keyboard {
    pub fn new(iie: bool) -> Self {
        Self {
            iie,
            latch: 0,
            shift: false,
            control: false,
            caps: true,
            open_apple: false,
            solid_apple: false,
            held: Vec::new(),
            repeat_at: None,
        }
    }

    /// The code a character key sends with the current modifiers, or
    /// `None` for keys the model does not have.
    fn encode(&self, key: u16) -> Option<u8> {
        let k = u8::try_from(key).ok()?;
        if !self.iie && matches!(key, keys::UP | keys::DOWN | keys::TAB) {
            return None;
        }
        if !self.iie && key == keys::DELETE {
            return None;
        }
        let mut c = match k {
            0x08 | 0x09 | 0x0a | 0x0b | 0x0d | 0x15 | 0x1b | 0x7f => return Some(k),
            0x20..=0x7e => {
                if self.shift {
                    shifted(k)
                } else {
                    k
                }
            }
            _ => return None,
        };
        if self.control
            && (c.is_ascii_alphabetic() || matches!(c, b'@' | b'[' | b'\\' | b']' | b'^' | b'_'))
        {
            return Some(c.to_ascii_uppercase() & 0x1f);
        }
        if c.is_ascii_lowercase() && (!self.iie || self.caps) {
            c = c.to_ascii_uppercase();
        }
        if !self.iie && matches!(c, b'`' | b'{' | b'|' | b'}' | b'~') {
            // Not on the II/II+ keyboard.
            return None;
        }
        Some(c)
    }

    /// A key went down or up at cycle `now`. Returns true for RESET down.
    pub fn key(&mut self, key: u16, down: bool, now: u64) -> bool {
        match key {
            keys::SHIFT => self.shift = down,
            keys::CONTROL => self.control = down,
            keys::CAPS_LOCK => {
                if down {
                    self.caps = !self.caps;
                }
            }
            keys::OPEN_APPLE => self.open_apple = down,
            keys::SOLID_APPLE => self.solid_apple = down,
            keys::RESET => return down,
            _ => {
                self.held.retain(|&k| k != key);
                if down {
                    if let Some(c) = self.encode(key) {
                        self.latch = c | 0x80;
                        self.held.push(key);
                        self.repeat_at = Some(now + REPEAT_DELAY);
                    }
                } else if self.held.is_empty() {
                    self.repeat_at = None;
                }
            }
        }
        false
    }

    /// CONTROL is held.
    pub fn control(&self) -> bool {
        self.control
    }

    /// A key (other than the modifiers) is down.
    pub fn any_down(&self) -> bool {
        !self.held.is_empty()
    }

    /// Auto-repeat (IIe), called once per CPU cycle.
    pub fn tick(&mut self, now: u64) {
        if !self.iie {
            return;
        }
        if let (Some(at), Some(&key)) = (self.repeat_at, self.held.last()) {
            if now >= at {
                if let Some(c) = self.encode(key) {
                    self.latch = c | 0x80;
                }
                self.repeat_at = Some(now + REPEAT_RATE);
            }
        }
    }

    pub fn clear_strobe(&mut self) {
        self.latch &= 0x7f;
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"A2KB", 1, |w| {
            w.u8(self.latch);
            for b in [
                self.shift,
                self.control,
                self.caps,
                self.open_apple,
                self.solid_apple,
            ] {
                w.bool(b);
            }
            w.u32(self.held.len() as u32);
            for &k in &self.held {
                w.u16(k);
            }
            w.u64(self.repeat_at.unwrap_or(u64::MAX));
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"A2KB", 1)?;
        let mut n = self.clone();
        n.latch = c.u8()?;
        n.shift = c.bool()?;
        n.control = c.bool()?;
        n.caps = c.bool()?;
        n.open_apple = c.bool()?;
        n.solid_apple = c.bool()?;
        let len = c.u32()? as usize;
        if len > 128 {
            return Err(StateError::Invalid("apple2 keys held"));
        }
        n.held = (0..len).map(|_| c.u16()).collect::<Result<_, _>>()?;
        n.repeat_at = Some(c.u64()?).filter(|&v| v != u64::MAX);
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_follows_the_model() {
        let mut ii = Keyboard::new(false);
        ii.key(u16::from(b'a'), true, 0);
        assert_eq!(ii.latch, 0x80 | b'A', "II: upper case only");
        ii.key(keys::SHIFT, true, 0);
        ii.key(u16::from(b'2'), true, 0);
        assert_eq!(ii.latch, 0x80 | b'@');
        ii.key(keys::SHIFT, false, 0);
        ii.key(keys::CONTROL, true, 0);
        ii.key(u16::from(b'c'), true, 0);
        assert_eq!(ii.latch, 0x83, "CTRL-C");
        ii.clear_strobe();
        assert_eq!(ii.latch, 0x03);
        ii.key(keys::CONTROL, false, 0);
        ii.key(keys::UP, true, 0);
        assert_eq!(ii.latch, 0x03, "no up arrow on the II");

        let mut e = Keyboard::new(true);
        e.key(keys::CAPS_LOCK, true, 0);
        e.key(u16::from(b'a'), true, 0);
        assert_eq!(e.latch, 0x80 | b'a', "IIe: caps lock off gives lower case");
        assert!(e.any_down());
    }

    #[test]
    fn iie_repeats_a_held_key() {
        let mut e = Keyboard::new(true);
        e.key(u16::from(b'x'), true, 0);
        e.clear_strobe();
        e.tick(REPEAT_DELAY - 1);
        assert_eq!(e.latch & 0x80, 0);
        e.tick(REPEAT_DELAY);
        assert_eq!(e.latch, 0x80 | b'X');
        e.key(u16::from(b'x'), false, REPEAT_DELAY + 10);
        e.clear_strobe();
        e.tick(REPEAT_DELAY * 4);
        assert_eq!(e.latch & 0x80, 0);
    }
}
