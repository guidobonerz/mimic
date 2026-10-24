//! Apple Desktop Bus on the Mac SE: the ADB transceiver (a microcontroller
//! between the VIA and the bus) with a keyboard at address 2 and a mouse
//! at address 3.
//!
//! **Transceiver interface** (Linux/m68k `via-macii` notes; Apple, "Guide
//! to the Macintosh Family Hardware", ADB chapter): the VIA's PB4/PB5
//! (ST0/ST1) give the state — 0 new command, 1 even byte, 2 odd byte, 3
//! idle — PB3 is the transceiver's /INT, and bytes move through the VIA's
//! shift register clocked by the transceiver (mode 7 out, mode 3 in). In
//! state 0 the Mac writes the command byte; each change to state 1 or 2
//! moves one data byte: in for Talk (the transceiver shifts it into the
//! VIA), out for Listen (the Mac writes it). When a Talk has no (more)
//! data the transceiver shifts in $FF with /INT low. In idle /INT goes
//! low when a device asks for service (it has data and its SRQ is
//! enabled), and the transceiver repeats the last Talk every 11 ms: when
//! the device answers it shifts in the command byte with /INT low and the
//! Mac fetches the data bytes through states 1 and 2.
//!
//! **Commands** (ADB command byte: address in bits 7–4, bits 3–2 the
//! command, bits 1–0 the register): 0000 SendReset, 0001 Flush, 10rr
//! Listen, 11rr Talk.
//!
//! **Devices**: register 3 holds bit 5 of its high byte SRQ enable,
//! bits 3–0 the address, the low byte the handler ID (keyboard 1, mouse
//! 1); a Listen to register 3 with handler $FE moves the device to the
//! new address. The keyboard's register 0 is two key transitions (bit 7
//! up, bits 6–0 the key code; $FF when there is no second); the mouse's
//! is the button (bit 7 of the first byte, 0 = down) with the Y and X
//! movement as 7-bit two's complement numbers. The exact handshake of the
//! transceiver (when /INT falls in the middle of a transfer, auto-polling
//! in idle) is this project's reading of the descriptions above and is
//! checked once an SE ROM runs.

use std::collections::VecDeque;

use chips_cbm::via::Via;
use emu_core::{StateError, StateReader, StateWriter};

/// CPU clocks a byte takes on the bus (about 1 ms).
pub const BYTE_CLOCKS: u64 = 7_834;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Device {
    addr: u8,
    handler: u8,
    srq: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adb {
    pub st: u8,
    /// /INT asserted (PB3 low).
    pub int: bool,
    kbd: Device,
    mouse: Device,
    keys: VecDeque<u8>,
    dx: i32,
    dy: i32,
    button: bool,
    button_sent: bool,
    /// The last command byte.
    cmd: u8,
    reply: VecDeque<u8>,
    listen: Vec<u8>,
    /// A byte the Mac wrote is clocked out at this time.
    out_at: Option<u64>,
    /// A byte to shift in at this time.
    in_at: Option<(u8, bool, u64)>,
    /// Next auto-poll in idle.
    poll_at: u64,
}

/// CPU clocks between auto-polls in idle (about 11 ms).
pub const POLL_CLOCKS: u64 = 86_000;

impl Default for Adb {
    fn default() -> Self {
        Self::new()
    }
}

impl Adb {
    pub fn new() -> Self {
        Self {
            st: 3,
            int: false,
            kbd: Device {
                addr: 2,
                handler: 1,
                srq: true,
            },
            mouse: Device {
                addr: 3,
                handler: 1,
                srq: true,
            },
            keys: VecDeque::new(),
            dx: 0,
            dy: 0,
            button: false,
            button_sent: false,
            cmd: 0,
            reply: VecDeque::new(),
            listen: Vec::new(),
            out_at: None,
            in_at: None,
            poll_at: 0,
        }
    }

    pub fn key(&mut self, key: u8, down: bool) {
        self.keys
            .push_back(key & 0x7f | if down { 0 } else { 0x80 });
    }

    pub fn mouse_move(&mut self, dx: i32, dy: i32, button: bool) {
        self.dx += dx;
        self.dy += dy;
        self.button = button;
    }

    fn mouse_has_data(&self) -> bool {
        self.dx != 0 || self.dy != 0 || self.button != self.button_sent
    }

    fn srq(&self) -> bool {
        (!self.keys.is_empty() && self.kbd.srq) || (self.mouse_has_data() && self.mouse.srq)
    }

    /// PB3 as the VIA sees it (low = asserted).
    pub fn int_line(&self) -> bool {
        !(self.int || (self.st == 3 && self.srq()))
    }

    /// The Mac wrote the shift register in mode 7.
    pub fn byte_written(&mut self, now: u64) {
        self.out_at = Some(now + BYTE_CLOCKS);
    }

    /// The state lines ST1/ST0 (PB5/PB4).
    pub fn set_state(&mut self, st: u8, now: u64) {
        let st = st & 3;
        if st == self.st {
            return;
        }
        let old = self.st;
        self.st = st;
        match st {
            0 => {
                self.int = false;
                self.reply.clear();
                self.listen.clear();
            }
            // Everything but Listen answers with bytes (SendReset and
            // Flush with none: $FF and /INT).
            1 | 2 if self.cmd >> 2 & 3 != 2 => {
                let (b, end) = match self.reply.pop_front() {
                    Some(b) => (b, false),
                    None => (0xff, true),
                };
                self.in_at = Some((b, end, now + BYTE_CLOCKS));
            }
            3 if old != 0 && self.cmd >> 2 & 3 == 2 => {
                let data = std::mem::take(&mut self.listen);
                self.do_listen(&data);
            }
            _ => {}
        }
    }

    pub fn next_event(&self) -> u64 {
        let a = self.out_at.unwrap_or(u64::MAX);
        let b = self.in_at.map_or(u64::MAX, |(_, _, t)| t);
        let c = if self.st == 3 && self.cmd >> 2 & 3 == 3 {
            self.poll_at
        } else {
            u64::MAX
        };
        a.min(b).min(c)
    }

    pub fn run(&mut self, now: u64, via: &mut Via) {
        // Auto-poll: in idle the transceiver repeats the last Talk; when
        // the device answers it shifts in a byte with /INT low, and the
        // Mac fetches the answer through states 1 and 2.
        if self.st == 3
            && self.cmd >> 2 & 3 == 3
            && now >= self.poll_at
            && self.in_at.is_none()
            && self.out_at.is_none()
        {
            self.poll_at = now + POLL_CLOCKS;
            let reply = self.talk(self.cmd >> 4, self.cmd & 3);
            if !reply.is_empty() {
                self.reply = reply.into();
                self.in_at = Some((self.cmd, true, now + BYTE_CLOCKS));
            }
        }
        if let Some(t) = self.out_at {
            if now >= t {
                self.out_at = None;
                let mut v = 0u8;
                for _ in 0..8 {
                    v = v << 1 | u8::from(via.shift_ext(true).unwrap_or(true));
                }
                if self.st == 0 {
                    self.command(v);
                } else {
                    self.listen.push(v);
                }
            }
        }
        if let Some((b, end, t)) = self.in_at {
            if now >= t {
                if via.acr >> 2 & 7 != 3 || via.shift_bits_left() == 0 {
                    self.in_at = Some((b, end, now + 1000));
                    return;
                }
                self.in_at = None;
                self.int = end;
                for k in (0..8).rev() {
                    via.shift_ext(b >> k & 1 != 0);
                }
            }
        }
    }

    fn command(&mut self, cmd: u8) {
        self.cmd = cmd;
        let addr = cmd >> 4;
        let reg = cmd & 3;
        self.reply.clear();
        match cmd >> 2 & 3 {
            0 if cmd & 0x0f == 0 => self.reset_devices(),
            0 if cmd & 3 == 1 => {
                if addr == self.kbd.addr {
                    self.keys.clear();
                }
            }
            3 => self.reply = self.talk(addr, reg).into(),
            _ => {}
        }
    }

    fn reset_devices(&mut self) {
        let fresh = Self::new();
        self.kbd = fresh.kbd;
        self.mouse = fresh.mouse;
        self.keys.clear();
        self.dx = 0;
        self.dy = 0;
    }

    fn reg3(d: &Device) -> Vec<u8> {
        vec![0x40 | u8::from(d.srq) << 5 | d.addr, d.handler]
    }

    fn talk(&mut self, addr: u8, reg: u8) -> Vec<u8> {
        if addr == self.kbd.addr {
            return match reg {
                0 => match self.keys.pop_front() {
                    Some(k1) => vec![k1, self.keys.pop_front().unwrap_or(0xff)],
                    None => Vec::new(),
                },
                2 => vec![0xff, 0xff],
                3 => Self::reg3(&self.kbd),
                _ => Vec::new(),
            };
        }
        if addr == self.mouse.addr {
            return match reg {
                0 if self.mouse_has_data() => {
                    let dx = self.dx.clamp(-63, 63);
                    let dy = self.dy.clamp(-63, 63);
                    self.dx -= dx;
                    self.dy -= dy;
                    self.button_sent = self.button;
                    vec![
                        u8::from(!self.button) << 7 | (dy as u8 & 0x7f),
                        0x80 | (dx as u8 & 0x7f),
                    ]
                }
                3 => Self::reg3(&self.mouse),
                _ => Vec::new(),
            };
        }
        Vec::new()
    }

    fn do_listen(&mut self, data: &[u8]) {
        let addr = self.cmd >> 4;
        if self.cmd & 3 != 3 || data.len() < 2 {
            return;
        }
        let dev = if addr == self.kbd.addr {
            &mut self.kbd
        } else if addr == self.mouse.addr {
            &mut self.mouse
        } else {
            return;
        };
        match data[1] {
            0xfe => dev.addr = data[0] & 0x0f,
            0x00 => {
                dev.addr = data[0] & 0x0f;
                dev.srq = data[0] & 0x20 != 0;
            }
            h @ (1..=4) => dev.handler = h,
            _ => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"MADB", 1, |w| {
            w.u8(self.st);
            w.bool(self.int);
            for d in [&self.kbd, &self.mouse] {
                w.u8(d.addr);
                w.u8(d.handler);
                w.bool(d.srq);
            }
            w.bytes(&self.keys.iter().copied().collect::<Vec<u8>>());
            w.u32(self.dx as u32);
            w.u32(self.dy as u32);
            w.bool(self.button);
            w.bool(self.button_sent);
            w.u8(self.cmd);
            w.bytes(&self.reply.iter().copied().collect::<Vec<u8>>());
            w.bytes(&self.listen);
            w.u64(self.out_at.unwrap_or(u64::MAX));
            let (b, end, t) = self.in_at.unwrap_or((0, false, u64::MAX));
            w.u8(b);
            w.bool(end);
            w.u64(t);
            w.u64(self.poll_at);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"MADB", 1)?;
        let mut n = Self::new();
        n.st = c.u8()? & 3;
        n.int = c.bool()?;
        for d in [&mut n.kbd, &mut n.mouse] {
            d.addr = c.u8()? & 0x0f;
            d.handler = c.u8()?;
            d.srq = c.bool()?;
        }
        n.keys = c.bytes()?.iter().copied().collect();
        n.dx = c.u32()? as i32;
        n.dy = c.u32()? as i32;
        n.button = c.bool()?;
        n.button_sent = c.bool()?;
        n.cmd = c.u8()?;
        n.reply = c.bytes()?.iter().copied().collect();
        n.listen = c.bytes()?.to_vec();
        let out = c.u64()?;
        n.out_at = (out != u64::MAX).then_some(out);
        let (b, end, t) = (c.u8()?, c.bool()?, c.u64()?);
        n.in_at = (t != u64::MAX).then_some((b, end, t));
        n.poll_at = c.u64()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Send a command in state 0; returns the clock after it.
    fn send(adb: &mut Adb, via: &mut Via, cmd: u8, now: u64) -> u64 {
        adb.set_state(0, now);
        via.write(0xb, 0x1c);
        via.write(0xa, cmd);
        adb.byte_written(now);
        let now = now + BYTE_CLOCKS;
        adb.run(now, via);
        now
    }

    /// Read one Talk byte by moving to state `st`.
    fn byte(adb: &mut Adb, via: &mut Via, st: u8, now: u64) -> (u8, bool, u64) {
        via.write(0xb, 0x0c);
        via.read(0xa, 0, 0);
        adb.set_state(st, now);
        let now = now + BYTE_CLOCKS;
        adb.run(now, via);
        (via.read(0xa, 0, 0), adb.int_line(), now)
    }

    #[test]
    fn talk_keyboard_and_mouse() {
        let mut via = Via::new();
        let mut adb = Adb::new();
        assert!(adb.int_line(), "no service request");
        adb.key(0x00, true);
        assert!(!adb.int_line(), "keyboard requests service in idle");
        let t = send(&mut adb, &mut via, 0x2c, 0); // Talk R0, address 2
        let (k1, int1, t) = byte(&mut adb, &mut via, 1, t);
        let (k2, int2, t) = byte(&mut adb, &mut via, 2, t);
        assert_eq!((k1, int1, k2, int2), (0x00, true, 0xff, true));
        let (_, int3, t) = byte(&mut adb, &mut via, 1, t);
        assert!(!int3, "end of data");
        adb.set_state(3, t);
        // Mouse: Talk R0 at address 3.
        adb.mouse_move(5, -2, true);
        let t = send(&mut adb, &mut via, 0x3c, t);
        let (m1, _, t) = byte(&mut adb, &mut via, 1, t);
        let (m2, _, t) = byte(&mut adb, &mut via, 2, t);
        assert_eq!((m1, m2), (0x7e, 0x85));
        // Nobody at address 5: $FF with /INT at once.
        let t = send(&mut adb, &mut via, 0x5c, t);
        let (v, int, _) = byte(&mut adb, &mut via, 1, t);
        assert_eq!((v, int), (0xff, false));
    }

    #[test]
    fn listen_moves_a_device() {
        let mut via = Via::new();
        let mut adb = Adb::new();
        let t = send(&mut adb, &mut via, 0x3b, 0); // Listen R3, address 3
        for (st, b) in [(1, 0x67u8), (2, 0xfe)] {
            adb.set_state(st, t);
            via.write(0xb, 0x1c);
            via.write(0xa, b);
            adb.byte_written(t);
            adb.run(t + BYTE_CLOCKS, &mut via);
        }
        adb.set_state(3, t);
        assert_eq!(adb.mouse.addr, 7);
        let mut w = StateWriter::new();
        adb.save(&mut w);
        let b = w.into_bytes();
        let mut other = Adb::new();
        other.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(other, adb);
    }
}
