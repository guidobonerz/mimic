//! Intel 8251 USART: the mode and command words of the IMSAI SIO-2's two
//! channels (control ports $03 and $05) and of the MPU card's own 8251
//! ($13). The data path (the terminal, the paper tape) stays in the
//! machine; this is the part a program sets up.
//!
//! After a reset the 8251 takes the next control write as the mode word;
//! in synchronous mode (baud factor 00) one or two sync characters follow;
//! then every control write is a command word, until one with bit 6
//! (internal reset) brings back the mode word. The command's TxEN lets
//! the transmitter send what is in its buffer, RxE lets the receiver
//! report characters (status RxRDY), ER would clear the error flags (no
//! error is produced here). The mode word gives the character length
//! (data bits are masked to it on both directions), parity and stop bits:
//! a character's time on the line.
//!
//! **[S]**: a character that arrives while RxE is off waits on the line
//! instead of being lost (the terminal's typing is kept); a reset leaves
//! the receiver and transmitter off, as the data sheet has it, so a
//! program has to set the 8251 up, as it must on the machine.
//!
//! Source: Intel 8251A Programmable Communication Interface data sheet
//! (mode instruction, command instruction, status read).

use emu_core::{StateError, StateReader, StateWriter};

/// Command word bits.
/// (DTR, SBRK, ER, RTS and EH have no effect here: no modem lines, no
/// errors, no hunt.)
pub mod cmd {
    pub const TXEN: u8 = 0x01;
    pub const RXE: u8 = 0x04;
    pub const IR: u8 = 0x40;
}

/// What the next control write is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Next {
    #[default]
    Mode,
    /// Synchronous mode: sync characters still to come.
    Sync(u8),
    Command,
}

#[derive(Clone, Debug)]
pub struct Usart {
    next: Next,
    mode: u8,
    command: u8,
    /// A character written while TxEN was off: sent once it is on.
    held: Option<u8>,
}

impl Default for Usart {
    fn default() -> Self {
        Self::new()
    }
}

impl Usart {
    /// After a hardware reset: waiting for the mode word, everything off;
    /// until a mode is written a character keeps its 8 bits (mode $4E).
    pub fn new() -> Self {
        Self {
            next: Next::Mode,
            mode: 0x4e,
            command: 0,
            held: None,
        }
    }

    /// Set up as if a program had written mode `mode` and command
    /// `command` (tests and the machine's own conveniences).
    pub fn with(mode: u8, command: u8) -> Self {
        Self {
            next: Next::Command,
            mode,
            command,
            held: None,
        }
    }

    /// A write to the control port.
    pub fn control(&mut self, v: u8) {
        match self.next {
            Next::Mode => {
                self.mode = v;
                self.next = if v & 3 == 0 {
                    // Synchronous: one sync character (bit 7, SCS) or two.
                    Next::Sync(if v & 0x80 != 0 { 1 } else { 2 })
                } else {
                    Next::Command
                };
            }
            Next::Sync(n) => {
                self.next = if n > 1 {
                    Next::Sync(n - 1)
                } else {
                    Next::Command
                };
            }
            Next::Command => {
                if v & cmd::IR != 0 {
                    self.next = Next::Mode;
                    self.command = 0;
                    self.held = None;
                } else {
                    self.command = v;
                }
            }
        }
    }

    pub fn tx_enabled(&self) -> bool {
        self.command & cmd::TXEN != 0
    }

    pub fn rx_enabled(&self) -> bool {
        self.command & cmd::RXE != 0
    }

    /// Data bits of a character (5–8).
    pub fn bits(&self) -> u8 {
        5 + (self.mode >> 2 & 3)
    }

    /// The mask a character's data bits pass.
    pub fn mask(&self) -> u8 {
        (0xffu16 >> (8 - self.bits())) as u8
    }

    /// A character's length on the line in half bit times: start bit,
    /// data, parity, stop bits (1, 1.5 or 2; synchronous: none).
    pub fn half_bits(&self) -> u32 {
        let parity = u32::from(self.mode & 0x10 != 0);
        let stop = match self.mode >> 6 {
            1 => 2,
            2 => 3,
            3 => 4,
            _ => 2,
        };
        let stop = if self.mode & 3 == 0 { 0 } else { stop };
        2 * (1 + u32::from(self.bits()) + parity) + stop
    }

    /// The baud factor (1, 16 or 64; synchronous: 1).
    pub fn factor(&self) -> u32 {
        match self.mode & 3 {
            2 => 16,
            3 => 64,
            _ => 1,
        }
    }

    /// A character written to the data port: `Some` when it goes out now,
    /// else it waits for TxEN.
    pub fn transmit(&mut self, v: u8) -> Option<u8> {
        let v = v & self.mask();
        if self.tx_enabled() {
            Some(v)
        } else {
            self.held = Some(v);
            None
        }
    }

    /// After a command: the held character, if TxEN now lets it go.
    pub fn release(&mut self) -> Option<u8> {
        if self.tx_enabled() {
            self.held.take()
        } else {
            None
        }
    }

    pub fn holding(&self) -> bool {
        self.held.is_some()
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(match self.next {
            Next::Mode => 0,
            Next::Sync(n) => n,
            Next::Command => 3,
        });
        w.u8(self.mode);
        w.u8(self.command);
        w.bool(self.held.is_some());
        w.u8(self.held.unwrap_or(0));
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let next = match r.u8()? {
            0 => Next::Mode,
            n @ (1 | 2) => Next::Sync(n),
            3 => Next::Command,
            _ => return Err(StateError::Invalid("8251 state")),
        };
        let mode = r.u8()?;
        let command = r.u8()?;
        let held = r.bool()?;
        let v = r.u8()?;
        *self = Self {
            next,
            mode,
            command,
            held: held.then_some(v),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_then_commands_then_internal_reset() {
        let mut u = Usart::new();
        assert!(!u.tx_enabled() && !u.rx_enabled(), "off after reset");
        // The MPU-B's set-up: mode $AE, command $37.
        u.control(0xae);
        u.control(0x37);
        assert!(u.tx_enabled() && u.rx_enabled());
        assert_eq!(u.bits(), 8);
        assert_eq!(u.factor(), 16);
        assert_eq!(u.half_bits(), 2 * 9 + 3, "1.5 stop bits");
        // A command with IR: the next write is a mode word again.
        u.control(0x40);
        assert!(!u.tx_enabled());
        u.control(0x4a); // 7 bits, ×16, 1 stop... (bits 3-2 = 10)
        assert_eq!(u.bits(), 7);
        assert_eq!(u.mask(), 0x7f);
        u.control(cmd::RXE);
        assert!(u.rx_enabled() && !u.tx_enabled());
    }

    #[test]
    fn sync_mode_takes_its_sync_characters_first() {
        let mut u = Usart::new();
        u.control(0x0c); // synchronous, 8 bits, two sync characters
        u.control(0x16);
        u.control(0x16);
        u.control(cmd::TXEN);
        assert!(u.tx_enabled());
        let mut one = Usart::new();
        one.control(0x8c); // single sync character
        one.control(0x16);
        one.control(cmd::TXEN);
        assert!(one.tx_enabled());
    }

    #[test]
    fn a_character_waits_for_txen() {
        let mut u = Usart::new();
        u.control(0x4e);
        u.control(0);
        assert_eq!(u.transmit(b'A'), None);
        assert!(u.holding());
        assert_eq!(u.release(), None);
        u.control(cmd::TXEN);
        assert_eq!(u.release(), Some(b'A'));
        assert_eq!(u.transmit(0xc1), Some(0xc1));
    }

    #[test]
    fn savestate_round_trip() {
        let mut u = Usart::new();
        u.control(0x0c);
        u.control(0x16);
        let mut w = StateWriter::new();
        u.save(&mut w);
        let data = w.into_bytes();
        let mut v = Usart::new();
        v.load(&mut StateReader::new(&data)).unwrap();
        v.control(0x16);
        v.control(cmd::TXEN);
        assert!(v.tx_enabled(), "one sync character was still to come");
    }
}
