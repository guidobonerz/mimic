//! MOS 6520 / Motorola 6820 Peripheral Interface Adapter (PET; also the
//! Atari 800XL's PIA, wired in `machine-atari8`).
//!
//! | Reg | CRx bit 2 = 0     | CRx bit 2 = 1                       |
//! | --- | ----------------- | ----------------------------------- |
//! | 0   | DDRA              | port A (read clears CRA bits 6/7)   |
//! | 1   | CRA               | CRA                                 |
//! | 2   | DDRB              | port B (read clears CRB bits 6/7)   |
//! | 3   | CRB               | CRB                                 |
//!
//! Control register: 7 IRQ1 flag (Cx1 edge), 6 IRQ2 flag (Cx2 edge in
//! input mode), 5 Cx2 output, 4/3 Cx2 mode (input: 4 edge, 3 IRQ enable;
//! output: 4 = 1 manual with level in 3, 4 = 0 handshake), 2 port/DDR
//! select, 1 Cx1 edge (1 = rising), 0 Cx1 IRQ enable. Bits 6 and 7 are read
//! only. Source: MOS 6520 data sheet; Motorola MC6820 data sheet.
//!
//! Port A reads the pin levels; port B reads the output register for
//! output bits. Handshake modes are reduced to their idle level (high).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PiaPort {
    pub or: u8,
    pub ddr: u8,
    pub cr: u8,
    /// Last level seen on Cx1 / Cx2 (for edge detection).
    c1: bool,
    c2: bool,
}

impl PiaPort {
    fn new() -> Self {
        Self {
            c1: true,
            c2: true,
            ..Self::default()
        }
    }

    /// Pin levels driven by the port (inputs pulled up).
    pub fn out(&self) -> u8 {
        self.or | !self.ddr
    }

    fn irq(&self) -> bool {
        let irq1 = self.cr & 0x81 == 0x81;
        let irq2 = self.cr & 0x20 == 0 && self.cr & 0x48 == 0x48;
        irq1 || irq2
    }

    /// Cx2 output level: manual mode drives bit 3; input and handshake
    /// modes are reported high.
    pub fn c2_out(&self) -> bool {
        self.cr & 0x38 != 0x30
    }

    fn set_c1(&mut self, level: bool) {
        let rising = self.cr & 0x02 != 0;
        if level != self.c1 && level == rising {
            self.cr |= 0x80;
        }
        self.c1 = level;
    }

    fn set_c2(&mut self, level: bool) {
        // Only an input in modes with bit 5 clear.
        if self.cr & 0x20 == 0 {
            let rising = self.cr & 0x10 != 0;
            if level != self.c2 && level == rising {
                self.cr |= 0x40;
            }
        }
        self.c2 = level;
    }

    fn write_cr(&mut self, v: u8) {
        self.cr = (self.cr & 0xc0) | (v & 0x3f);
        // Turning Cx2 into an output clears its flag.
        if v & 0x20 != 0 {
            self.cr &= !0x40;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pia {
    pub a: PiaPort,
    pub b: PiaPort,
}

impl Pia {
    pub fn new() -> Self {
        Self {
            a: PiaPort::new(),
            b: PiaPort::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// IRQA or IRQB active (both outputs are wired together on the PET).
    pub fn irq(&self) -> bool {
        self.a.irq() || self.b.irq()
    }

    pub fn irq_a(&self) -> bool {
        self.a.irq()
    }

    pub fn irq_b(&self) -> bool {
        self.b.irq()
    }

    pub fn port_a_out(&self) -> u8 {
        self.a.out()
    }

    pub fn port_b_out(&self) -> u8 {
        self.b.out()
    }

    pub fn ca2_out(&self) -> bool {
        self.a.c2_out()
    }

    pub fn cb2_out(&self) -> bool {
        self.b.c2_out()
    }

    pub fn set_ca1(&mut self, level: bool) {
        self.a.set_c1(level);
    }

    pub fn set_cb1(&mut self, level: bool) {
        self.b.set_c1(level);
    }

    pub fn set_ca2(&mut self, level: bool) {
        self.a.set_c2(level);
    }

    pub fn set_cb2(&mut self, level: bool) {
        self.b.set_c2(level);
    }

    /// Side-effect-free read; `pa_pins` / `pb_pins` are the external levels.
    pub fn peek(&self, reg: u8, pa_pins: u8, pb_pins: u8) -> u8 {
        match reg & 3 {
            0 if self.a.cr & 0x04 != 0 => pa_pins,
            0 => self.a.ddr,
            1 => self.a.cr,
            2 if self.b.cr & 0x04 != 0 => (self.b.or & self.b.ddr) | (pb_pins & !self.b.ddr),
            2 => self.b.ddr,
            _ => self.b.cr,
        }
    }

    pub fn read(&mut self, reg: u8, pa_pins: u8, pb_pins: u8) -> u8 {
        let v = self.peek(reg, pa_pins, pb_pins);
        match reg & 3 {
            0 if self.a.cr & 0x04 != 0 => self.a.cr &= 0x3f,
            2 if self.b.cr & 0x04 != 0 => self.b.cr &= 0x3f,
            _ => {}
        }
        v
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 3 {
            0 if self.a.cr & 0x04 != 0 => self.a.or = v,
            0 => self.a.ddr = v,
            1 => self.a.write_cr(v),
            2 if self.b.cr & 0x04 != 0 => self.b.or = v,
            2 => self.b.ddr = v,
            _ => self.b.write_cr(v),
        }
    }
}

const TAG: [u8; 4] = *b"6520";
const VERSION: u16 = 1;

impl Savestate for Pia {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for p in [&self.a, &self.b] {
                w.u8(p.or);
                w.u8(p.ddr);
                w.u8(p.cr);
                w.bool(p.c1);
                w.bool(p.c2);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut port = || -> Result<PiaPort, StateError> {
            Ok(PiaPort {
                or: c.u8()?,
                ddr: c.u8()?,
                cr: c.u8()?,
                c1: c.bool()?,
                c2: c.bool()?,
            })
        };
        let a = port()?;
        let b = port()?;
        *self = Pia { a, b };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddr_and_port_share_an_address() {
        let mut p = Pia::new();
        p.write(0, 0x0f); // DDRA (CRA bit 2 clear)
        p.write(1, 0x04);
        p.write(0, 0x05); // ORA
        assert_eq!(p.a.ddr, 0x0f);
        assert_eq!(p.port_a_out(), 0xf5);
        assert_eq!(p.peek(0, 0x3c, 0xff), 0x3c, "port A reads the pins");
        p.write(1, 0x00);
        assert_eq!(p.peek(0, 0, 0), 0x0f, "DDR again");
    }

    #[test]
    fn port_b_reads_outputs_from_the_register() {
        let mut p = Pia::new();
        p.write(2, 0xf0);
        p.write(3, 0x04);
        p.write(2, 0xa0);
        assert_eq!(p.peek(2, 0xff, 0x05), 0xa5);
    }

    #[test]
    fn cb1_edge_interrupt_and_acknowledge() {
        let mut p = Pia::new();
        p.write(3, 0x05); // port B, CB1 IRQ on falling edge
        p.set_cb1(true);
        assert!(!p.irq());
        p.set_cb1(false);
        assert!(p.irq());
        assert_eq!(p.peek(3, 0, 0) & 0x80, 0x80);
        p.write(3, 0x05);
        assert!(p.irq(), "writing CR keeps the flag");
        p.read(2, 0xff, 0xff);
        assert!(!p.irq(), "reading the port clears it");
        p.set_cb1(true);
        assert!(!p.irq(), "wrong edge");
    }

    #[test]
    fn ca1_flag_without_enable_does_not_interrupt() {
        let mut p = Pia::new();
        p.write(1, 0x06); // rising edge, IRQ off
        p.set_ca1(false);
        p.set_ca1(true);
        assert_eq!(p.a.cr & 0x80, 0x80);
        assert!(!p.irq());
    }

    #[test]
    fn manual_c2_output() {
        let mut p = Pia::new();
        p.write(1, 0x34);
        assert!(!p.ca2_out());
        p.write(1, 0x3c);
        assert!(p.ca2_out());
        p.write(3, 0x34);
        assert!(!p.cb2_out());
    }

    #[test]
    fn savestate_round_trip() {
        let mut p = Pia::new();
        p.write(1, 0x3d);
        p.write(2, 0x55);
        p.set_ca1(false);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let data = w.into_bytes();
        let mut q = Pia::new();
        q.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(p, q);
        assert!(q
            .load(&mut StateReader::new(&data[..data.len() - 1]))
            .is_err());
    }
}
