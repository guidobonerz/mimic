//! A tiny built-in console ROM for tests and for running without the TI
//! ROMs (`--option roms=stub`): it sets the VDP to Graphics I, clears the
//! video RAM, paints the top half of the screen white and the bottom half
//! dark blue, enables the VDP interrupt through the TMS9901 and waits;
//! the interrupt routine counts fields at [`FRAMES`]. No GROMs, no BASIC.
//!
//! Hand-assembled with the small assembler below (opcodes from the TMS
//! 9900 Data Manual); the addresses are the TI-99/4A's (see the crate doc).

/// The field counter (a word in the scratchpad).
pub const FRAMES: u16 = 0x8300;

/// Words, labels and jumps to labels.
struct Asm {
    words: Vec<u16>,
    origin: u16,
    labels: Vec<(&'static str, u16)>,
    /// (word index, label) of jumps.
    jumps: Vec<(usize, &'static str)>,
}

impl Asm {
    fn new(origin: u16) -> Self {
        Self {
            words: Vec::new(),
            origin,
            labels: Vec::new(),
            jumps: Vec::new(),
        }
    }

    fn w(&mut self, words: &[u16]) -> &mut Self {
        self.words.extend_from_slice(words);
        self
    }

    fn label(&mut self, name: &'static str) -> &mut Self {
        let at = self.origin + 2 * self.words.len() as u16;
        self.labels.push((name, at));
        self
    }

    /// A jump opcode (`>1000`–`>1C00`) to a label.
    fn jump(&mut self, op: u16, to: &'static str) -> &mut Self {
        self.jumps.push((self.words.len(), to));
        self.w(&[op])
    }

    /// The VDP address for writing: LI R0,addr|>4000, then both bytes.
    fn vdp_write_addr(&mut self, addr: u16) -> &mut Self {
        self.w(&[0x0200, addr | 0x4000]) // LI R0,addr
            .w(&[0x06c0]) // SWPB R0
            .w(&[0xd800, 0x8c02]) // MOVB R0,@>8C02
            .w(&[0x06c0]) // SWPB R0
            .w(&[0xd800, 0x8c02]) // MOVB R0,@>8C02
    }

    /// `count` bytes of `value` to the VDP.
    fn vdp_fill(&mut self, label: &'static str, count: u16, value: u8) -> &mut Self {
        self.w(&[0x0203, count]) // LI R3,count
            .w(&[0x0204, u16::from(value) << 8]) // LI R4,value
            .label(label)
            .w(&[0xd804, 0x8c00]) // MOVB R4,@>8C00
            .w(&[0x0603]) // DEC R3
            .jump(0x1600, label) // JNE
    }

    fn finish(mut self) -> (u16, Vec<u16>) {
        for (i, to) in std::mem::take(&mut self.jumps) {
            let target = self
                .labels
                .iter()
                .find(|(n, _)| *n == to)
                .map(|(_, a)| *a)
                .expect("label");
            let next = self.origin + 2 * (i as u16 + 1);
            let disp = (target.wrapping_sub(next) as i16) / 2;
            assert!((-128..=127).contains(&disp), "jump too far");
            self.words[i] |= disp as u8 as u16;
        }
        (self.origin, self.words)
    }
}

/// The VDP registers: Graphics I, 16 KB, display on, interrupt on; names
/// at >0000, colours at >0380, patterns at >0800, sprites at >0300.
/// Where the table lies.
const REGS: u16 = 0x0800;
const VDP_REGS: [u8; 8] = [0x00, 0xe0, 0x00, 0x0e, 0x01, 0x06, 0x00, 0x14];

fn main_code() -> (u16, Vec<u16>) {
    let mut a = Asm::new(0x0024);
    a.w(&[0x0300, 0x0000]) // LIMI 0
        .w(&[0x02e0, 0x83e0]) // LWPI >83E0
        .w(&[0x0201, REGS]) // LI R1,REGS
        .w(&[0x0202, 0x8000]) // LI R2,>8000
        .w(&[0x0203, 0x0008]) // LI R3,8
        .label("regs")
        .w(&[0xd831, 0x8c02]) // MOVB *R1+,@>8C02
        .w(&[0xd802, 0x8c02]) // MOVB R2,@>8C02
        .w(&[0x0222, 0x0100]) // AI R2,>0100
        .w(&[0x0603]) // DEC R3
        .jump(0x1600, "regs") // JNE regs
        .vdp_write_addr(0x0000)
        .vdp_fill("clear", 0x4000, 0x00)
        .vdp_write_addr(0x0808)
        .vdp_fill("block", 8, 0xff)
        .vdp_write_addr(0x0380)
        .vdp_fill("colors", 32, 0xf4)
        .vdp_write_addr(0x0000)
        .vdp_fill("names", 384, 0x01)
        .w(&[0x04e0, FRAMES]) // CLR @FRAMES
        // The 9901: I/O mode, VDP interrupt (INT2) enabled.
        .w(&[0x020c, 0x0000]) // LI R12,0
        .w(&[0x1e00]) // SBZ 0
        .w(&[0x1d02]) // SBO 2
        .w(&[0x0300, 0x0002]) // LIMI 2
        .label("idle")
        .jump(0x1000, "idle"); // JMP idle
    a.finish()
}

fn int_code() -> (u16, Vec<u16>) {
    let mut a = Asm::new(0x0900);
    a.w(&[0xd020, 0x8802]) // MOVB @>8802,R0: status, clears the interrupt
        .w(&[0x05a0, FRAMES]) // INC @FRAMES
        .w(&[0x0380]); // RTWP
    a.finish()
}

/// The 8 KB stub ROM.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x2000];
    let mut put = |addr: u16, words: &[u16]| {
        for (i, w) in words.iter().enumerate() {
            let a = usize::from(addr) + 2 * i;
            rom[a..a + 2].copy_from_slice(&w.to_be_bytes());
        }
    };
    put(0x0000, &[0x83e0, 0x0024, 0x83c0, 0x0900]);
    let regs: Vec<u16> = VDP_REGS
        .chunks(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect();
    put(REGS, &regs);
    for (org, words) in [main_code(), int_code()] {
        put(org, &words);
    }
    rom
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pieces_do_not_overlap() {
        let (org, main) = super::main_code();
        assert!(org + 2 * main.len() as u16 <= super::REGS);
        assert_eq!(super::rom().len(), 0x2000);
    }
}
