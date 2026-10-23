//! A small built-in system ROM for tests without the user's ROM: it draws
//! a test figure by driving the VIA and the analog hardware directly,
//! once per 30 000 cycles (timer 2). It is not the Vectrex executive: it
//! has none of its routines, so cartridges do not run with it.
//!
//! The figure, from the centre at intensity $7F: a square of four lines
//! (+Y, +X, −Y, −X, each $30 for $40 cycles: about 3 100 units) and a dot
//! at the centre. This project's own code.

/// 8 KB image for $E000–$FFFF.
pub fn rom() -> Vec<u8> {
    let mut a = Asm::new(0xf000);
    // VIA: port A all outputs (DAC), port B outputs but PB5 (comparator)
    // and PB6 (cartridge); ACR: PB7 from T1 one-shot, SR shift-out at φ2/2.
    a.lds(0xcbea);
    a.lda_sta(0xff, 0xd003);
    a.lda_sta(0x9f, 0xd002);
    a.lda_sta(0x98, 0xd00b);
    // Port B idle: /RAMP high, MUX off (PB0 high), select 00.
    a.lda_sta(0x81, 0xd000);
    let frame = a.here();
    // Timer 2: 30 000 cycles a frame.
    a.lda_sta(0x30, 0xd008);
    a.lda_sta(0x75, 0xd009);
    // /ZERO (CA2 low) and the zero reference: DAC 0 through MUX 01.
    a.lda_sta(0xcc, 0xd00c);
    a.lda_sta(0x00, 0xd001);
    a.lda_sta(0x82, 0xd000);
    a.lda_sta(0x81, 0xd000);
    // Intensity $7F through MUX 10.
    a.lda_sta(0x7f, 0xd001);
    a.lda_sta(0x84, 0xd000);
    a.lda_sta(0x81, 0xd000);
    // Release the integrators.
    a.lda_sta(0xce, 0xd00c);
    // The dot: beam on for 32 cycles at rest.
    a.lda_sta(0xff, 0xd00a);
    for _ in 0..16 {
        a.nop();
    }
    a.clr(0xd00a);
    for (dy, dx) in [(0x30u8, 0x00u8), (0x00, 0x30), (0xd0, 0x00), (0x00, 0xd0)] {
        // Y through MUX 00 into its sample/hold, then X on the DAC.
        a.lda_sta(dy, 0xd001);
        a.lda_sta(0x80, 0xd000);
        a.lda_sta(0x81, 0xd000);
        a.lda_sta(dx, 0xd001);
        // Beam on, ramp for T1 = $40 cycles, wait for the timer, beam off.
        a.lda_sta(0xff, 0xd00a);
        a.lda_sta(0x40, 0xd004);
        a.clr(0xd005);
        let wait = a.here();
        a.lda_ext(0xd00d);
        a.bita(0x40);
        a.beq(wait);
        a.clr(0xd00a);
    }
    // Wait for the end of the frame (T2).
    let wait = a.here();
    a.lda_ext(0xd00d);
    a.bita(0x20);
    a.beq(wait);
    a.jmp(frame);
    let code = a.bytes;
    let mut rom = vec![0xff; 0x2000];
    rom[0x1000..0x1000 + code.len()].copy_from_slice(&code);
    // Reset vector.
    rom[0x1ffe] = 0xf0;
    rom[0x1fff] = 0x00;
    rom
}

/// Just enough of a 6809 assembler for the stub.
struct Asm {
    org: u16,
    bytes: Vec<u8>,
}

impl Asm {
    fn new(org: u16) -> Self {
        Self {
            org,
            bytes: Vec::new(),
        }
    }

    fn here(&self) -> u16 {
        self.org + self.bytes.len() as u16
    }

    fn emit(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }

    fn lds(&mut self, v: u16) {
        let [hi, lo] = v.to_be_bytes();
        self.emit(&[0x10, 0xce, hi, lo]);
    }

    fn lda_sta(&mut self, v: u8, addr: u16) {
        let [hi, lo] = addr.to_be_bytes();
        self.emit(&[0x86, v, 0xb7, hi, lo]);
    }

    fn lda_ext(&mut self, addr: u16) {
        let [hi, lo] = addr.to_be_bytes();
        self.emit(&[0xb6, hi, lo]);
    }

    fn clr(&mut self, addr: u16) {
        let [hi, lo] = addr.to_be_bytes();
        self.emit(&[0x7f, hi, lo]);
    }

    fn bita(&mut self, v: u8) {
        self.emit(&[0x85, v]);
    }

    fn nop(&mut self) {
        self.emit(&[0x12]);
    }

    fn branch(&mut self, op: u8, to: u16) {
        let off = to.wrapping_sub(self.here() + 2) as i16;
        assert!((-128..128).contains(&off), "branch out of range");
        self.emit(&[op, off as u8]);
    }

    fn beq(&mut self, to: u16) {
        self.branch(0x27, to);
    }

    fn jmp(&mut self, to: u16) {
        let [hi, lo] = to.to_be_bytes();
        self.emit(&[0x7e, hi, lo]);
    }
}
