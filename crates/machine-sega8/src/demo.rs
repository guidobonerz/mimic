//! A small built-in test cartridge (`--option cart=demo`, and the tests).
//!
//! Graphics I with a striped screen of box patterns, one sprite the pad in
//! port 1 moves, a tone while its left button (TL) is held. The VDP's
//! frame interrupt (Z80 /INT, mode 1) counts fields at [`FRAMES`], reads
//! the pads through the PPI (row 7 selected) and, on an SC-3000, keyboard
//! row 0 into [`ROW0`] (port A) and [`ROW0_B`] (port B). The NMI (PAUSE on
//! the SG-1000, the RESET key on the SC-3000) counts at [`PAUSES`].
//!
//! Hand-assembled with the small assembler below (opcodes from the Zilog
//! Z80 CPU User Manual); ports as in the crate doc.

/// The field counter (16 bits).
pub const FRAMES: u16 = 0xc000;
/// The sprite's X and Y.
pub const SPRITE_X: u16 = 0xc002;
pub const SPRITE_Y: u16 = 0xc003;
/// NMIs counted.
pub const PAUSES: u16 = 0xc004;
/// Keyboard row 0 as read: PPI port A, port B.
pub const ROW0: u16 = 0xc005;
pub const ROW0_B: u16 = 0xc006;
/// The pads as read (row 7): PPI port A, port B.
pub const PADS: u16 = 0xc007;
pub const PADS_B: u16 = 0xc008;

/// A minimal assembler: bytes, labels, relative and absolute jumps.
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

    fn here(&self) -> u16 {
        self.origin + self.code.len() as u16
    }

    fn b(&mut self, bytes: &[u8]) -> &mut Self {
        self.code.extend_from_slice(bytes);
        self
    }

    fn label(&mut self, name: &'static str) -> &mut Self {
        let at = self.here();
        self.labels.push((name, at));
        self
    }

    /// A relative jump opcode (JR, JR cc, DJNZ) to a label.
    fn jr(&mut self, op: u8, to: &'static str) -> &mut Self {
        self.code.push(op);
        self.fixups.push((self.code.len(), to, true));
        self.code.push(0);
        self
    }

    /// An instruction with a 16-bit label operand (JP, LD HL,nn …).
    fn abs(&mut self, op: &[u8], to: &'static str) -> &mut Self {
        self.code.extend_from_slice(op);
        self.fixups.push((self.code.len(), to, false));
        self.code.extend_from_slice(&[0, 0]);
        self
    }

    /// LD A,n ; OUT (port),A
    fn out_n(&mut self, port: u8, n: u8) -> &mut Self {
        self.b(&[0x3e, n, 0xd3, port])
    }

    /// LD A,(nn)
    fn ld_a_mem(&mut self, addr: u16) -> &mut Self {
        self.b(&[0x3a]).b(&addr.to_le_bytes())
    }

    /// LD (nn),A
    fn st_a_mem(&mut self, addr: u16) -> &mut Self {
        self.b(&[0x32]).b(&addr.to_le_bytes())
    }

    /// Set the VDP's VRAM address for writing.
    fn vram_write(&mut self, addr: u16) -> &mut Self {
        self.out_n(0xbf, addr as u8)
            .out_n(0xbf, 0x40 | (addr >> 8) as u8)
    }

    fn finish(mut self) -> (u16, Vec<u8>) {
        for (pos, name, rel) in std::mem::take(&mut self.fixups) {
            let target = self
                .labels
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("label {name}"))
                .1;
            if rel {
                let from = self.origin + pos as u16 + 1;
                let d = i32::from(target) - i32::from(from);
                assert!((-128..=127).contains(&d), "jump to {name} too far");
                self.code[pos] = d as i8 as u8;
            } else {
                self.code[pos..pos + 2].copy_from_slice(&target.to_le_bytes());
            }
        }
        (self.origin, self.code)
    }
}

const JR: u8 = 0x18;
const JR_NZ: u8 = 0x20;
const DJNZ: u8 = 0x10;

/// The VDP set-up: Graphics I, names $1800, colours $2000, patterns $0000,
/// sprite attributes $1B00, sprite patterns $3800, white on dark blue.
const VDP_REGS: [u8; 16] = [
    0x00, 0x80, // R0
    0x80, 0x81, // R1: 16K, display still off
    0x06, 0x82, // R2: names $1800
    0x80, 0x83, // R3: colours $2000
    0x00, 0x84, // R4: patterns $0000
    0x36, 0x85, // R5: sprite attributes $1B00
    0x07, 0x86, // R6: sprite patterns $3800
    0xf4, 0x87, // R7: white on dark blue
];

/// $0000: the start.
fn reset_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0000);
    a.b(&[0xf3]) // DI
        .b(&[0x31, 0x00, 0xc4]) // LD SP,$C400
        .b(&[0xed, 0x56]) // IM 1
        .b(&[0xc3, 0x00, 0x01]); // JP $0100
    a.finish()
}

/// $0100: the set-up and the main loop.
fn main_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0100);
    // The PPI: A and B inputs, C outputs; row 7 (the pads).
    a.out_n(0xdf, 0x92)
        .out_n(0xde, 0x07)
        // Sound off: all four attenuations to 15; tone 0 period $0FE.
        .out_n(0x7f, 0x9f)
        .out_n(0x7f, 0xbf)
        .out_n(0x7f, 0xdf)
        .out_n(0x7f, 0xff)
        .out_n(0x7f, 0x8e)
        .out_n(0x7f, 0x0f)
        .abs(&[0x21], "vdp_regs") // LD HL,VDP_REGS
        .b(&[0x06, 16]); // LD B,16
    a.label("regs")
        .b(&[0x7e, 0xd3, 0xbf, 0x23]) // LD A,(HL) ; OUT ($BF),A ; INC HL
        .jr(DJNZ, "regs")
        // Pattern 1: a box.
        .vram_write(0x0008)
        .abs(&[0x21], "box")
        .b(&[0x06, 8]);
    a.label("box_loop")
        .b(&[0x7e, 0xd3, 0xbe, 0x23])
        .jr(DJNZ, "box_loop")
        // Colour group 0: yellow on dark blue.
        .vram_write(0x2000)
        .out_n(0xbe, 0xa4)
        // Sprite pattern 0: a block.
        .vram_write(0x3800)
        .b(&[0x06, 8]);
    a.label("block")
        .out_n(0xbe, 0xff)
        .jr(DJNZ, "block")
        // Names: stripes of patterns 0 and 1.
        .vram_write(0x1800)
        .b(&[0x01, 0x00, 0x03]) // LD BC,$0300
        .b(&[0x16, 0x00]); // LD D,0
    a.label("names")
        .b(&[0x7a, 0xd3, 0xbe]) // LD A,D ; OUT ($BE),A
        .b(&[0xee, 0x01, 0x57]) // XOR 1 ; LD D,A
        .b(&[0x0b, 0x78, 0xb1]) // DEC BC ; LD A,B ; OR C
        .jr(JR_NZ, "names")
        // The sprite at (120, 96), the counters cleared, the end mark.
        .b(&[0x21, 0x78, 0x60, 0x22]) // LD HL,$6078 ; LD (SPRITE_X),HL
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0x21, 0x00, 0x00, 0x22]) // LD HL,0 ; LD (FRAMES),HL
        .b(&FRAMES.to_le_bytes())
        .b(&[0xaf])
        .st_a_mem(PAUSES)
        .vram_write(0x1b00)
        .out_n(0xbe, 0x60)
        .out_n(0xbe, 0x78)
        .out_n(0xbe, 0x00)
        .out_n(0xbe, 0x0f)
        .out_n(0xbe, 0xd0)
        // Display and frame interrupt on.
        .out_n(0xbf, 0xe0)
        .out_n(0xbf, 0x81)
        .b(&[0xfb]); // EI
    a.label("main").b(&[0x76]).jr(JR, "main"); // HALT ; JR MAIN
    a.label("vdp_regs").b(&VDP_REGS);
    a.label("box")
        .b(&[0xff, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0xff]);
    a.finish()
}

/// $0038: the VDP's frame interrupt (mode 1), to [`INT_HANDLER`].
fn int_vector() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0038);
    a.b(&[0xc3]).b(&INT_HANDLER.to_le_bytes()); // JP INT_HANDLER
    a.finish()
}

/// Where the frame interrupt's handler is.
const INT_HANDLER: u16 = 0x0300;

fn int_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(INT_HANDLER);
    a.b(&[0xf5, 0xe5, 0xc5]) // PUSH AF ; PUSH HL ; PUSH BC
        .b(&[0xdb, 0xbf]) // IN A,($BF): acknowledge
        .b(&[0x2a])
        .b(&FRAMES.to_le_bytes())
        .b(&[0x23, 0x22]) // INC HL ; LD (FRAMES),HL
        .b(&FRAMES.to_le_bytes())
        // Keyboard row 0 (an SC-3000; the SG-1000 reads the pads anyway).
        .out_n(0xde, 0x00)
        .b(&[0xdb, 0xdc])
        .st_a_mem(ROW0)
        .b(&[0xdb, 0xdd])
        .st_a_mem(ROW0_B)
        // Row 7: the pads.
        .out_n(0xde, 0x07)
        .b(&[0xdb, 0xdd])
        .st_a_mem(PADS_B)
        .b(&[0xdb, 0xdc, 0x4f]) // IN A,($DC) ; LD C,A
        .st_a_mem(PADS)
        .b(&[0x21])
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0xcb, 0x51]) // BIT 2,C: left
        .jr(JR_NZ, "no_left")
        .b(&[0x35]); // DEC (HL)
    a.label("no_left")
        .b(&[0xcb, 0x59]) // BIT 3,C: right
        .jr(JR_NZ, "no_right")
        .b(&[0x34]); // INC (HL)
    a.label("no_right")
        .b(&[0x23]) // INC HL: Y
        .b(&[0xcb, 0x41]) // BIT 0,C: up
        .jr(JR_NZ, "no_up")
        .b(&[0x35]);
    a.label("no_up")
        .b(&[0xcb, 0x49]) // BIT 1,C: down
        .jr(JR_NZ, "no_down")
        .b(&[0x34]);
    a.label("no_down")
        .b(&[0x3e, 0x9f]) // LD A,$9F: tone 0 off
        .b(&[0xcb, 0x61]) // BIT 4,C: TL
        .jr(JR_NZ, "silent")
        .b(&[0x3e, 0x90]); // LD A,$90: loud
    a.label("silent")
        .b(&[0xd3, 0x7f])
        .vram_write(0x1b00)
        .ld_a_mem(SPRITE_Y)
        .b(&[0xd3, 0xbe])
        .ld_a_mem(SPRITE_X)
        .b(&[0xd3, 0xbe])
        .b(&[0xc1, 0xe1, 0xf1]) // POP BC ; POP HL ; POP AF
        .b(&[0xfb, 0xed, 0x4d]); // EI ; RETI
    a.finish()
}

/// $0066: PAUSE / the RESET key.
fn nmi_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0066);
    a.b(&[0xf5]) // PUSH AF
        .ld_a_mem(PAUSES)
        .b(&[0x3c]) // INC A
        .st_a_mem(PAUSES)
        .b(&[0xf1, 0xed, 0x45]); // POP AF ; RETN
    a.finish()
}

/// The 8 KB image.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0xffu8; 0x2000];
    for (at, code) in [
        reset_code(),
        int_vector(),
        nmi_code(),
        main_code(),
        int_code(),
    ] {
        let at = usize::from(at);
        rom[at..at + code.len()].copy_from_slice(&code);
    }
    rom
}

// ---- The Master System / Game Gear test cartridge (mode 4) ----------------

/// Mode 4: names $3800, sprite attributes $3F00, sprite patterns from 0,
/// display off; the backdrop colour 16.
const SMS_VDP_REGS: [u8; 22] = [
    0x04, 0x80, // R0: mode 4
    0x00, 0x81, // R1: display off
    0xff, 0x82, // R2: names $3800
    0xff, 0x83, // R3
    0xff, 0x84, // R4
    0xff, 0x85, // R5: sprite attributes $3F00
    0xfb, 0x86, // R6: sprite patterns $0000
    0x00, 0x87, // R7: backdrop colour 16
    0x00, 0x88, // R8: no horizontal scroll
    0x00, 0x89, // R9: no vertical scroll
    0xff, 0x8a, // R10: line counter
];

/// CRAM: background 0 dark blue, 1 yellow; sprites 16 blue, 17 white (SMS
/// `--BBGGRR`; the Game Gear's words are written by the same bytes as
/// `----BBBB GGGGRRRR` pairs, see `sms_main_code`).
const SMS_CRAM: [u8; 4] = [0x20, 0x0f, 0x10, 0x3f];
const GG_CRAM: [[u8; 2]; 4] = [[0x00, 0x08], [0xff, 0x00], [0x00, 0x04], [0xff, 0x0f]];

/// Set the VDP address to CRAM `entry` (SMS bytes; Game Gear words: × 2).
fn cram_write(a: &mut Asm, addr: u8) {
    a.out_n(0xbf, addr).out_n(0xbf, 0xc0);
}

/// $0100 of the mode-4 cartridge: the set-up and the main loop. `gg`:
/// write the Game Gear's 12-bit CRAM words.
fn sms_main_code(gg: bool) -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0100);
    // Sound off.
    a.out_n(0x7f, 0x9f)
        .out_n(0x7f, 0xbf)
        .out_n(0x7f, 0xdf)
        .out_n(0x7f, 0xff)
        .out_n(0x7f, 0x8e)
        .out_n(0x7f, 0x0f)
        .abs(&[0x21], "vdp_regs")
        .b(&[0x06, SMS_VDP_REGS.len() as u8]);
    a.label("regs")
        .b(&[0x7e, 0xd3, 0xbf, 0x23])
        .jr(DJNZ, "regs");
    // CRAM 0, 1 and 16, 17.
    for (i, entry) in [0u8, 1, 16, 17].into_iter().enumerate() {
        if gg {
            cram_write(&mut a, entry * 2);
            a.out_n(0xbe, GG_CRAM[i][0]).out_n(0xbe, GG_CRAM[i][1]);
        } else {
            cram_write(&mut a, entry);
            a.out_n(0xbe, SMS_CRAM[i]);
        }
    }
    // Pattern 1: a box in colour 1 (plane 0); pattern 2: a solid block.
    a.vram_write(0x0020).abs(&[0x21], "box").b(&[0x06, 8]);
    a.label("box_loop")
        .b(&[0x7e, 0xd3, 0xbe, 0x23]) // plane 0
        .b(&[0xaf, 0xd3, 0xbe, 0xd3, 0xbe, 0xd3, 0xbe]) // planes 1-3: 0
        .jr(DJNZ, "box_loop")
        .b(&[0x06, 8]);
    a.label("block")
        .out_n(0xbe, 0xff)
        .b(&[0xaf, 0xd3, 0xbe, 0xd3, 0xbe, 0xd3, 0xbe])
        .jr(DJNZ, "block")
        // Names: stripes of patterns 0 and 1 (low byte, high byte 0).
        .vram_write(0x3800)
        .b(&[0x01, 0x00, 0x03]) // LD BC,$0300 (768 words)
        .b(&[0x16, 0x00]); // LD D,0
    a.label("names")
        .b(&[0x7a, 0xd3, 0xbe]) // LD A,D ; OUT ($BE),A
        .b(&[0xaf, 0xd3, 0xbe]) // XOR A ; OUT ($BE),A
        .b(&[0x7a, 0xee, 0x01, 0x57]) // LD A,D ; XOR 1 ; LD D,A
        .b(&[0x0b, 0x78, 0xb1]) // DEC BC ; LD A,B ; OR C
        .jr(JR_NZ, "names")
        // The sprite at (120, 96), the counters cleared.
        .b(&[0x21, 0x78, 0x60, 0x22]) // LD HL,$6078 ; LD (SPRITE_X),HL
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0x21, 0x00, 0x00, 0x22]) // LD HL,0 ; LD (FRAMES),HL
        .b(&FRAMES.to_le_bytes())
        .b(&[0xaf])
        .st_a_mem(PAUSES)
        .vram_write(0x3f00)
        .out_n(0xbe, 0x60)
        .out_n(0xbe, 0xd0)
        .vram_write(0x3f80)
        .out_n(0xbe, 0x78)
        .out_n(0xbe, 0x02)
        // Display and frame interrupt on.
        .out_n(0xbf, 0x60)
        .out_n(0xbf, 0x81)
        .b(&[0xfb]); // EI
    a.label("main").b(&[0x76]).jr(JR, "main");
    a.label("vdp_regs").b(&SMS_VDP_REGS);
    a.label("box")
        .b(&[0xff, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0xff]);
    a.finish()
}

/// The mode-4 frame interrupt: count, read the pads, move the sprite,
/// sound while TL is held.
fn sms_int_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(INT_HANDLER);
    a.b(&[0xf5, 0xe5, 0xc5])
        .b(&[0xdb, 0xbf]) // acknowledge
        .b(&[0x2a])
        .b(&FRAMES.to_le_bytes())
        .b(&[0x23, 0x22])
        .b(&FRAMES.to_le_bytes())
        .b(&[0xdb, 0xdd])
        .st_a_mem(PADS_B)
        .b(&[0xdb, 0xdc, 0x4f])
        .st_a_mem(PADS)
        .b(&[0x21])
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0xcb, 0x51])
        .jr(JR_NZ, "no_left")
        .b(&[0x35]);
    a.label("no_left")
        .b(&[0xcb, 0x59])
        .jr(JR_NZ, "no_right")
        .b(&[0x34]);
    a.label("no_right")
        .b(&[0x23])
        .b(&[0xcb, 0x41])
        .jr(JR_NZ, "no_up")
        .b(&[0x35]);
    a.label("no_up")
        .b(&[0xcb, 0x49])
        .jr(JR_NZ, "no_down")
        .b(&[0x34]);
    a.label("no_down")
        .b(&[0x3e, 0x9f])
        .b(&[0xcb, 0x61])
        .jr(JR_NZ, "silent")
        .b(&[0x3e, 0x90]);
    a.label("silent")
        .b(&[0xd3, 0x7f])
        .vram_write(0x3f00)
        .ld_a_mem(SPRITE_Y)
        .b(&[0xd3, 0xbe])
        .vram_write(0x3f80)
        .ld_a_mem(SPRITE_X)
        .b(&[0xd3, 0xbe])
        .b(&[0xc1, 0xe1, 0xf1])
        .b(&[0xfb, 0xed, 0x4d]);
    a.finish()
}

/// The 16 KB Master System (`gg` false) or Game Gear test cartridge.
pub fn sms_rom(gg: bool) -> Vec<u8> {
    let mut rom = vec![0xffu8; 0x4000];
    let reset = {
        let mut a = Asm::new(0);
        a.b(&[0xf3]) // DI
            .b(&[0x31, 0xf0, 0xdf]) // LD SP,$DFF0
            .b(&[0xed, 0x56]) // IM 1
            .b(&[0xc3, 0x00, 0x01]); // JP $0100
        a.finish()
    };
    for (at, code) in [
        reset,
        int_vector(),
        nmi_code(),
        sms_main_code(gg),
        sms_int_code(),
    ] {
        let at = usize::from(at);
        rom[at..at + code.len()].copy_from_slice(&code);
    }
    rom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sms_pieces_fit() {
        let (_, main) = sms_main_code(true);
        let (_, int) = sms_int_code();
        assert!(
            0x100 + main.len() <= usize::from(INT_HANDLER),
            "main {}",
            main.len()
        );
        assert!(usize::from(INT_HANDLER) + int.len() <= 0x2000);
        assert_eq!(sms_rom(false).len(), 0x4000);
    }

    #[test]
    fn the_pieces_do_not_overlap() {
        let (_, reset) = reset_code();
        let (_, vector) = int_vector();
        let (_, nmi) = nmi_code();
        let (_, main) = main_code();
        let (_, int) = int_code();
        assert!(reset.len() <= 0x38);
        assert!(0x38 + vector.len() <= 0x66);
        assert!(0x66 + nmi.len() <= 0x100);
        assert!(0x100 + main.len() <= usize::from(INT_HANDLER));
        assert!(usize::from(INT_HANDLER) + int.len() <= 0x2000);
    }
}
