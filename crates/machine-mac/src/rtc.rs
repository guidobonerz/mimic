//! The real-time clock (Apple 343-0040; 343-0042 with 256 bytes of
//! parameter RAM in the Plus and SE): a seconds counter, parameter RAM
//! and a one-second output, on a three-wire serial interface from the
//! VIA (PB0 data, PB1 clock, PB2 /enable).
//!
//! **Protocol** (Inside Macintosh III, "The Real-Time Clock"; Linux/m68k
//! Mac hardware notes): with /enable low the 68000 sends a command byte,
//! bit 7 first, the clock taking each bit on a rising edge of the clock
//! line. Bit 7 of the command is 1 for a read. A write sends the data byte
//! next; a read turns the data line round and the clock chip puts each
//! bit on it after a falling edge (it stays while the clock is high).
//! Raising /enable ends the transfer.
//!
//! | Command (z = read) | Register                                    |
//! | ------------------ | ------------------------------------------- |
//! | z00xaa01           | seconds byte aa (0 = lowest)                |
//! | 00110001           | test register (write)                       |
//! | 00110101           | write-protect register (write; bit 7 = on)  |
//! | z010aa01           | parameter RAM $10–$13                       |
//! | z1aaaa01           | parameter RAM $00–$0F                       |
//! | z0111aaa, 0bbbbb00 | extended parameter RAM aaabbbbb (343-0042)  |
//!
//! In the 256-byte parameter RAM the classic bytes $00–$0F are at
//! $10–$1F and $10–$13 at $08–$0B. The clock starts on 1 January 1990 at
//! power-on (machines never read the host's clock, golden rule 3); the
//! parameter RAM starts cleared, so the ROM writes its defaults.

use emu_core::{StateError, StateReader, StateWriter};

/// Seconds from 1904-01-01 to 1990-01-01.
pub const START_TIME: u32 = 2_713_996_800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Receiving the command byte (or the second command byte).
    Command,
    /// Receiving the data byte of a write.
    Data,
    /// Sending the data byte of a read.
    Send,
    /// Transfer finished; waiting for /enable to rise.
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rtc {
    pub seconds: u32,
    pub pram: Vec<u8>,
    pub write_protect: bool,
    extended: bool,
    enabled: bool,
    clock: bool,
    phase: Phase,
    shift: u8,
    bits: u8,
    /// Command bytes received.
    cmd: [u8; 2],
    cmd_len: u8,
    /// The bit the chip drives in the send phase.
    pub data_out: bool,
}

impl Rtc {
    pub fn new(extended: bool) -> Self {
        Self {
            seconds: START_TIME,
            pram: vec![0; if extended { 256 } else { 20 }],
            write_protect: false,
            extended,
            enabled: false,
            clock: true,
            phase: Phase::Command,
            shift: 0,
            bits: 0,
            cmd: [0; 2],
            cmd_len: 0,
            data_out: true,
        }
    }

    /// The chip is driving the data line (a read is going on).
    pub fn driving(&self) -> bool {
        self.enabled && self.phase == Phase::Send
    }

    /// One second has passed.
    pub fn tick_second(&mut self) {
        self.seconds = self.seconds.wrapping_add(1);
    }

    /// Classic address → index into `pram`.
    fn classic(&self, a: u8) -> usize {
        let a = usize::from(a);
        match (self.extended, a) {
            (false, a) => a,
            (true, a) if a >= 0x10 => a - 0x10 + 0x08,
            (true, a) => a + 0x10,
        }
    }

    /// What a command addresses.
    fn target(&self) -> Target {
        let c = self.cmd[0] & 0x7f;
        if self.cmd_len == 2 {
            let addr = (c & 7) << 5 | (self.cmd[1] >> 2 & 0x1f);
            return Target::Pram(usize::from(addr));
        }
        match c {
            0x31 => Target::Test,
            0x35 => Target::WriteProtect,
            _ if c & 0x63 == 0x01 => Target::Seconds(usize::from(c >> 2 & 3)),
            _ if c & 0x73 == 0x21 => Target::Pram(self.classic(0x10 + (c >> 2 & 3))),
            _ if c & 0x43 == 0x41 => Target::Pram(self.classic(c >> 2 & 0x0f)),
            _ => Target::None,
        }
    }

    fn read_target(&self) -> u8 {
        match self.target() {
            Target::Seconds(n) => (self.seconds >> (8 * n)) as u8,
            Target::Pram(i) => self.pram.get(i).copied().unwrap_or(0),
            _ => 0,
        }
    }

    fn write_target(&mut self, v: u8) {
        let target = self.target();
        if let Target::WriteProtect = target {
            self.write_protect = v & 0x80 != 0;
            return;
        }
        if self.write_protect {
            return;
        }
        match target {
            Target::Seconds(n) => {
                let shift = 8 * n;
                self.seconds = (self.seconds & !(0xff << shift)) | u32::from(v) << shift;
            }
            Target::Pram(i) => {
                if let Some(b) = self.pram.get_mut(i) {
                    *b = v;
                }
            }
            _ => {}
        }
    }

    /// The VIA's port B lines: `enable` = PB2 low, `clock` = PB1, `data`
    /// = PB0 as the VIA drives it.
    pub fn lines(&mut self, enable: bool, clock: bool, data: bool) {
        if !enable {
            self.enabled = false;
            self.phase = Phase::Command;
            self.bits = 0;
            self.cmd_len = 0;
            self.clock = clock;
            return;
        }
        if !self.enabled {
            self.enabled = true;
            self.phase = Phase::Command;
            self.bits = 0;
            self.cmd_len = 0;
        }
        let rising = clock && !self.clock;
        let falling = !clock && self.clock;
        self.clock = clock;
        match self.phase {
            Phase::Command | Phase::Data if rising => {
                self.shift = self.shift << 1 | u8::from(data);
                self.bits += 1;
                if self.bits < 8 {
                    return;
                }
                self.bits = 0;
                if self.phase == Phase::Data {
                    self.write_target(self.shift);
                    self.phase = Phase::Done;
                    return;
                }
                self.cmd[usize::from(self.cmd_len)] = self.shift;
                self.cmd_len += 1;
                // z0111aaa: a second command byte follows (343-0042).
                if self.cmd_len == 1 && self.extended && self.shift & 0x78 == 0x38 {
                    return;
                }
                if self.cmd[0] & 0x80 != 0 {
                    self.phase = Phase::Send;
                    self.shift = self.read_target();
                } else {
                    self.phase = Phase::Data;
                }
            }
            Phase::Send if falling => {
                if self.bits < 8 {
                    self.data_out = self.shift & 0x80 != 0;
                    self.shift <<= 1;
                    self.bits += 1;
                } else {
                    self.phase = Phase::Done;
                }
            }
            _ => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"MRTC", 1, |w| {
            w.u32(self.seconds);
            w.bytes(&self.pram);
            w.bool(self.write_protect);
            w.bool(self.enabled);
            w.bool(self.clock);
            w.u8(match self.phase {
                Phase::Command => 0,
                Phase::Data => 1,
                Phase::Send => 2,
                Phase::Done => 3,
            });
            w.u8(self.shift);
            w.u8(self.bits);
            w.u8(self.cmd[0]);
            w.u8(self.cmd[1]);
            w.u8(self.cmd_len);
            w.bool(self.data_out);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"MRTC", 1)?;
        let mut n = self.clone();
        n.seconds = c.u32()?;
        let pram = c.bytes()?;
        if pram.len() != n.pram.len() {
            return Err(StateError::Invalid("rtc parameter RAM size differs"));
        }
        n.pram = pram.to_vec();
        n.write_protect = c.bool()?;
        n.enabled = c.bool()?;
        n.clock = c.bool()?;
        n.phase = match c.u8()? {
            0 => Phase::Command,
            1 => Phase::Data,
            2 => Phase::Send,
            3 => Phase::Done,
            _ => return Err(StateError::Invalid("rtc phase")),
        };
        n.shift = c.u8()?;
        n.bits = c.u8()?.min(8);
        n.cmd = [c.u8()?, c.u8()?];
        n.cmd_len = c.u8()?.min(2);
        n.data_out = c.bool()?;
        *self = n;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Seconds(usize),
    Test,
    WriteProtect,
    Pram(usize),
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(rtc: &mut Rtc, byte: u8) {
        for k in (0..8).rev() {
            let bit = byte >> k & 1 != 0;
            rtc.lines(true, false, bit);
            rtc.lines(true, true, bit);
        }
    }

    fn receive(rtc: &mut Rtc) -> u8 {
        let mut v = 0;
        for _ in 0..8 {
            rtc.lines(true, false, true);
            rtc.lines(true, true, true);
            v = v << 1 | u8::from(rtc.data_out);
        }
        v
    }

    fn end(rtc: &mut Rtc) {
        rtc.lines(false, true, true);
    }

    #[test]
    fn seconds_and_pram() {
        let mut rtc = Rtc::new(false);
        rtc.lines(false, true, true);
        send(&mut rtc, 0x81);
        assert_eq!(receive(&mut rtc), START_TIME as u8);
        end(&mut rtc);
        send(&mut rtc, 0x8d);
        assert_eq!(receive(&mut rtc), (START_TIME >> 24) as u8);
        end(&mut rtc);
        // Write PRAM $10 (z0100001), read it back.
        send(&mut rtc, 0x21);
        send(&mut rtc, 0xa8);
        end(&mut rtc);
        send(&mut rtc, 0xa1);
        assert_eq!(receive(&mut rtc), 0xa8);
        end(&mut rtc);
        // PRAM $05 (z1010101 = z1aaaa01 with aaaa = 5).
        send(&mut rtc, 0x55);
        send(&mut rtc, 0x42);
        end(&mut rtc);
        assert_eq!(rtc.pram[5], 0x42);
        // Write protect blocks writes.
        send(&mut rtc, 0x35);
        send(&mut rtc, 0x80);
        end(&mut rtc);
        send(&mut rtc, 0x55);
        send(&mut rtc, 0x00);
        end(&mut rtc);
        assert_eq!(rtc.pram[5], 0x42);
    }

    #[test]
    fn extended_pram_and_classic_mapping() {
        let mut rtc = Rtc::new(true);
        rtc.lines(false, true, true);
        // Classic $10 lands at $08.
        send(&mut rtc, 0x21);
        send(&mut rtc, 0xa8);
        end(&mut rtc);
        assert_eq!(rtc.pram[0x08], 0xa8);
        // Extended write $E5: z0111111, 0 00101 00.
        send(&mut rtc, 0x3f);
        send(&mut rtc, 0x14);
        send(&mut rtc, 0x99);
        end(&mut rtc);
        assert_eq!(rtc.pram[0xe5], 0x99);
        send(&mut rtc, 0xbf);
        send(&mut rtc, 0x14);
        assert_eq!(receive(&mut rtc), 0x99);
        end(&mut rtc);
        let mut w = StateWriter::new();
        rtc.save(&mut w);
        let b = w.into_bytes();
        let mut other = Rtc::new(true);
        other.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(other, rtc);
        assert!(Rtc::new(false).load(&mut StateReader::new(&b)).is_err());
    }
}
