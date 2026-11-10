//! A tiny built-in MOS for tests and for running without the Acorn ROMs
//! (`--option roms=stub`): it programs the 6845 and the Video ULA for
//! MODE 4 (screen at `&5800`), sets the palette to black and white, paints
//! the top half of the screen white and waits. No BASIC.
//!
//! Hand-assembled (opcodes from the MOS MCS6500 programming manual) with
//! the small assembler below.

/// Bytes, labels and branches/jumps to labels.
struct Asm {
    code: Vec<u8>,
    origin: u16,
    labels: Vec<(&'static str, u16)>,
    /// (position, label, relative)
    fixups: Vec<(usize, &'static str, bool)>,
}

impl Asm {
    fn new(origin: u16) -> Self {
        Self {
            code: Vec::new(),
            origin,
            labels: Vec::new(),
            fixups: Vec::new(),
        }
    }

    fn b(&mut self, bytes: &[u8]) -> &mut Self {
        self.code.extend_from_slice(bytes);
        self
    }

    fn label(&mut self, name: &'static str) -> &mut Self {
        let at = self.origin + self.code.len() as u16;
        self.labels.push((name, at));
        self
    }

    /// A branch opcode to a label.
    fn br(&mut self, op: u8, to: &'static str) -> &mut Self {
        self.code.push(op);
        self.fixups.push((self.code.len(), to, true));
        self.code.push(0);
        self
    }

    /// An instruction with an absolute label operand.
    fn abs(&mut self, op: u8, to: &'static str) -> &mut Self {
        self.code.push(op);
        self.fixups.push((self.code.len(), to, false));
        self.code.extend_from_slice(&[0, 0]);
        self
    }

    fn finish(mut self) -> (u16, Vec<u8>) {
        for (pos, to, rel) in std::mem::take(&mut self.fixups) {
            let target = self
                .labels
                .iter()
                .find(|(n, _)| *n == to)
                .map(|(_, a)| *a)
                .expect("label");
            if rel {
                let next = self.origin + pos as u16 + 1;
                let d = target.wrapping_sub(next) as i16;
                assert!((-128..=127).contains(&d), "branch too far");
                self.code[pos] = d as u8;
            } else {
                self.code[pos..pos + 2].copy_from_slice(&target.to_le_bytes());
            }
        }
        (self.origin, self.code)
    }
}

/// MODE 4's 6845 registers R0–R13 (cursor off).
const CRTC: [u8; 14] = [63, 40, 49, 0x24, 38, 0, 32, 34, 0, 7, 0x20, 0, 0x0b, 0x00];

fn code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0xc000);
    a.label("reset")
        .b(&[0x78]) // SEI
        .b(&[0xa2, 0xff, 0x9a]) // LDX #$FF; TXS
        .b(&[0xa2, 0x00]) // LDX #0
        .label("crtc")
        .b(&[0x8e, 0x00, 0xfe]) // STX &FE00
        .abs(0xbd, "table") // LDA table,X
        .b(&[0x8d, 0x01, 0xfe]) // STA &FE01
        .b(&[0xe8, 0xe0, 14]) // INX; CPX #14
        .br(0xd0, "crtc") // BNE
        .b(&[0xa9, 0x88, 0x8d, 0x20, 0xfe]) // LDA #&88; STA &FE20
        .b(&[0xa2, 0x00]) // LDX #0
        .label("pal")
        .b(&[0x8a, 0x0a, 0x0a, 0x0a, 0x0a]) // TXA; ASL ×4
        .b(&[0xe0, 0x08]) // CPX #8
        .br(0xb0, "white") // BCS white
        .b(&[0x09, 0x07]) // ORA #7 (black)
        .label("white")
        .b(&[0x8d, 0x21, 0xfe]) // STA &FE21
        .b(&[0xe8, 0xe0, 16]) // INX; CPX #16
        .br(0xd0, "pal") // BNE
        .b(&[0xa9, 0x58, 0x85, 0x71, 0xa9, 0x00, 0x85, 0x70]) // &70 = &5800
        .b(&[0xa0, 0x00, 0xa2, 20]) // LDY #0; LDX #20
        .label("fill")
        .b(&[0xa9, 0xff, 0x91, 0x70, 0xc8]) // LDA #&FF; STA (&70),Y; INY
        .br(0xd0, "fill")
        .b(&[0xe6, 0x71, 0xca]) // INC &71; DEX
        .br(0xd0, "fill")
        .b(&[0xa2, 20]) // LDX #20
        .label("zero")
        .b(&[0xa9, 0x00, 0x91, 0x70, 0xc8]) // LDA #0; STA (&70),Y; INY
        .br(0xd0, "zero")
        .b(&[0xe6, 0x71, 0xca]) // INC &71; DEX
        .br(0xd0, "zero")
        .label("done")
        .abs(0x4c, "done") // JMP done
        .label("rti")
        .b(&[0x40]) // RTI
        .label("table")
        .b(&CRTC);
    a.finish()
}

/// The 16 KB stub MOS for `&C000`–`&FFFF`.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0xffu8; 0x4000];
    let (org, code) = code();
    let o = usize::from(org - 0xc000);
    rom[o..o + code.len()].copy_from_slice(&code);
    let rti = org + code.len() as u16 - CRTC.len() as u16 - 1;
    for (v, at) in [(rti, 0x3ffa), (org, 0x3ffc), (rti, 0x3ffe)] {
        rom[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    rom
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_vectors_point_into_the_code() {
        let rom = super::rom();
        assert_eq!(&rom[0x3ffc..0x3ffe], &[0x00, 0xc0]);
        let irq = u16::from_le_bytes([rom[0x3ffe], rom[0x3fff]]);
        assert_eq!(rom[usize::from(irq - 0xc000)], 0x40, "RTI");
    }
}
