//! Z80 PIO: two eight-bit ports, A and B, each with a data and a control
//! port.
//!
//! A write to a **control** port is, in this order:
//! - the I/O select byte after a mode 3 word (1 = the line is an input);
//! - the interrupt mask after an interrupt control word that announced
//!   one (0 = the line is watched);
//! - `xxxx1111`: the **mode**, bits 7–6: 0 output, 1 input, 2
//!   bidirectional (port A), 3 bit control;
//! - `xxxx0111`: **interrupt control**: bit 7 enable, bit 6 AND (else OR),
//!   bit 5 active high, bit 4 a mask follows (mode 3's line logic);
//! - `xxxx0011`: bit 7 alone, the interrupt enable;
//! - bit 0 clear: the **interrupt vector**.
//!
//! Interrupts: in modes 0 to 2 the peripheral's strobe ([`Pio::strobe`],
//! the rising edge of /STB) requests one; in mode 3 the watched input
//! lines do when their AND/OR condition becomes true. Port A has the
//! higher priority.
//!
//! The lines as the outside sees them: [`Pio::pins`] (outputs driven,
//! inputs as the caller's pull-ups leave them); what the outside drives:
//! [`Pio::set_input`].
//!
//! Source: Zilog, "Z80 PIO Parallel I/O" product specification; Z80
//! Family CPU Peripherals User Manual (UM0081).

use mimic_core::{StateError, StateReader, StateWriter};

use crate::Chain;

pub const A: usize = 0;
pub const B: usize = 1;

pub const MODE_OUTPUT: u8 = 0;
pub const MODE_INPUT: u8 = 1;
pub const MODE_BIDIRECTIONAL: u8 = 2;
pub const MODE_BITS: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Port {
    mode: u8,
    /// Output register.
    output: u8,
    /// What the outside drives on the lines.
    input: u8,
    /// Mode 3: 1 = the line is an input.
    io_select: u8,
    /// Interrupt control word (bits 7–5).
    int_control: u8,
    /// 0 = the line takes part in mode 3's interrupt condition.
    mask: u8,
    vector: u8,
    io_follows: bool,
    mask_follows: bool,
    /// Mode 3: the condition held at the last look.
    matched: bool,
}

impl Default for Port {
    fn default() -> Self {
        Self {
            mode: MODE_INPUT,
            output: 0,
            input: 0xff,
            io_select: 0xff,
            int_control: 0,
            mask: 0xff,
            vector: 0,
            io_follows: false,
            mask_follows: false,
            matched: false,
        }
    }
}

impl Port {
    fn enabled(&self) -> bool {
        self.int_control & 0x80 != 0
    }

    /// The lines that are inputs.
    fn inputs(&self) -> u8 {
        match self.mode {
            MODE_OUTPUT => 0,
            MODE_BITS => self.io_select,
            _ => 0xff,
        }
    }

    fn data(&self) -> u8 {
        let i = self.inputs();
        (self.output & !i) | (self.input & i)
    }

    /// Mode 3's interrupt condition on the watched lines.
    fn condition(&self) -> bool {
        let watched = !self.mask;
        if self.mode != MODE_BITS || watched == 0 {
            return false;
        }
        let v = self.data();
        let active = if self.int_control & 0x20 != 0 { v } else { !v } & watched;
        if self.int_control & 0x40 != 0 {
            active == watched
        } else {
            active != 0
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pio {
    port: [Port; 2],
    pub chain: Chain<2>,
}

impl Pio {
    pub fn new() -> Self {
        Self::default()
    }

    /// /RESET: both ports become inputs, interrupts off, output registers
    /// cleared. The vectors stay.
    pub fn reset(&mut self) {
        for p in &mut self.port {
            *p = Port {
                vector: p.vector,
                input: p.input,
                ..Port::default()
            };
        }
        self.chain = Chain::default();
    }

    pub fn write_control(&mut self, n: usize, v: u8) {
        let p = &mut self.port[n & 1];
        if p.io_follows {
            p.io_follows = false;
            p.io_select = v;
        } else if p.mask_follows {
            p.mask_follows = false;
            p.mask = v;
        } else if v & 0x0f == 0x0f {
            p.mode = v >> 6;
            p.io_follows = p.mode == MODE_BITS;
        } else if v & 0x0f == 0x07 {
            p.int_control = v & 0xe0;
            if v & 0x10 != 0 {
                p.mask_follows = true;
                // A new mask drops a request that waits.
                self.chain.request[n & 1] = false;
            }
        } else if v & 0x0f == 0x03 {
            p.int_control = (p.int_control & 0x60) | (v & 0x80);
        } else if v & 1 == 0 {
            p.vector = v;
        }
        let p = &mut self.port[n & 1];
        p.matched = p.condition();
    }

    pub fn write_data(&mut self, n: usize, v: u8) {
        self.port[n & 1].output = v;
        self.look(n & 1);
    }

    /// The data port as the CPU reads it. No side effects (the handshake
    /// lines are not modelled).
    pub fn read_data(&self, n: usize) -> u8 {
        self.port[n & 1].data()
    }

    /// The outside drives the port's lines to `v` (seen where they are
    /// inputs).
    pub fn set_input(&mut self, n: usize, v: u8) {
        self.port[n & 1].input = v;
        self.look(n & 1);
    }

    /// Mode 3: request an interrupt when the condition becomes true.
    fn look(&mut self, n: usize) {
        let p = &mut self.port[n];
        let now = p.condition();
        if now && !p.matched && p.enabled() {
            self.chain.request[n] = true;
        }
        p.matched = now;
    }

    /// The rising edge of the port's /STB: the peripheral took the output
    /// or delivered an input.
    pub fn strobe(&mut self, n: usize) {
        let p = &self.port[n & 1];
        if p.mode != MODE_BITS && p.enabled() {
            self.chain.request[n & 1] = true;
        }
    }

    /// The port's lines: outputs as driven, the others from `pull`.
    pub fn pins(&self, n: usize, pull: u8) -> u8 {
        let p = &self.port[n & 1];
        let i = p.inputs();
        (p.output & !i) | (pull & i)
    }

    /// The vector of the pending interrupt (given IEI high).
    pub fn vector(&self) -> Option<u8> {
        self.chain.pending().map(|n| self.port[n].vector)
    }

    /// Register values for debugger views: mode, output register,
    /// interrupt control, vector.
    pub fn registers(&self, n: usize) -> (u8, u8, u8, u8) {
        let p = &self.port[n & 1];
        (p.mode, p.output, p.int_control, p.vector)
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"ZPIO", 1, |w| {
            self.chain.save(w);
            for p in &self.port {
                for b in [
                    p.mode,
                    p.output,
                    p.input,
                    p.io_select,
                    p.int_control,
                    p.mask,
                    p.vector,
                ] {
                    w.u8(b);
                }
                w.bool(p.io_follows);
                w.bool(p.mask_follows);
                w.bool(p.matched);
            }
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"ZPIO", 1)?;
        let mut n = Self::default();
        n.chain.load(&mut r)?;
        for p in &mut n.port {
            p.mode = r.u8()?;
            if p.mode > 3 {
                return Err(StateError::Invalid("PIO mode"));
            }
            p.output = r.u8()?;
            p.input = r.u8()?;
            p.io_select = r.u8()?;
            p.int_control = r.u8()? & 0xe0;
            p.mask = r.u8()?;
            p.vector = r.u8()?;
            p.io_follows = r.bool()?;
            p.mask_follows = r.bool()?;
            p.matched = r.bool()?;
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_mode_drives_the_lines_and_a_strobe_interrupts() {
        let mut p = Pio::new();
        assert_eq!(p.pins(A, 0xff), 0xff, "inputs after reset");
        p.write_control(A, 0x0f);
        p.write_data(A, 0x5a);
        assert_eq!(p.pins(A, 0xff), 0x5a);
        assert_eq!(p.read_data(A), 0x5a);
        p.write_control(B, 0xe6);
        p.write_control(B, 0x0f);
        p.strobe(B);
        assert_eq!(p.vector(), None, "not enabled");
        p.write_control(B, 0x83);
        p.strobe(B);
        assert_eq!(p.vector(), Some(0xe6));
        assert_eq!(p.chain.acknowledge(), Some(B));
        p.strobe(A);
        assert_eq!(p.vector(), None);
    }

    #[test]
    fn bit_mode_mixes_inputs_and_outputs() {
        let mut p = Pio::new();
        p.write_control(A, 0xcf);
        p.write_control(A, 0xf0);
        p.write_data(A, 0xa5);
        p.set_input(A, 0x3c);
        assert_eq!(p.read_data(A), 0x35);
        assert_eq!(p.pins(A, 0xff), 0xf5);
    }

    #[test]
    fn bit_mode_interrupts_when_the_condition_becomes_true() {
        let mut p = Pio::new();
        p.write_control(A, 0xcf);
        p.write_control(A, 0xff);
        p.set_input(A, 0xff);
        // Enable, OR, active low, mask follows: watch lines 0 and 1.
        p.write_control(A, 0x97);
        p.write_control(A, 0xfc);
        assert_eq!(p.vector(), None);
        p.set_input(A, 0xfe);
        assert!(p.vector().is_some());
        p.chain.acknowledge();
        p.chain.reti();
        p.set_input(A, 0xfc);
        assert_eq!(p.vector(), None, "the condition was true already");
        p.set_input(A, 0xff);
        p.set_input(A, 0xfd);
        assert!(p.vector().is_some());
    }

    #[test]
    fn savestate_round_trip() {
        let mut p = Pio::new();
        p.write_control(A, 0x0f);
        p.write_data(A, 0x9f);
        p.write_control(B, 0xe6);
        p.write_control(B, 0x83);
        p.write_control(B, 0x0f);
        p.strobe(B);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        let mut q = Pio::new();
        q.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(p, q);
        for cut in 0..bytes.len() {
            let _ = q.load(&mut StateReader::new(&bytes[..cut]));
        }
    }
}
