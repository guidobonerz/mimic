//! A tiny built-in BIOS for tests and for running without the Coleco
//! BIOS (`--option roms=stub`).
//!
//! - A cartridge whose first two bytes are $AA $55 or $55 $AA starts at
//!   once through its start pointer at $800A (games that call BIOS
//!   routines need the real BIOS).
//! - Otherwise a demo: Graphics I with a striped screen of box patterns,
//!   one sprite the joystick of controller 1 moves, a tone while its left
//!   fire button is held. The NMI (the VDP's frame interrupt) counts
//!   fields at [`FRAMES`]; the maskable interrupt (a spinner step) counts
//!   at [`SPINS`] and reads both controllers.
//!
//! Hand-assembled with the small assembler below (opcodes from the Zilog
//! Z80 CPU User Manual); the port numbers are the ColecoVision's (see the
//! crate doc).

/// The field counter (16 bits).
pub const FRAMES: u16 = 0x7000;
/// The sprite's X and Y.
pub const SPRITE_X: u16 = 0x7002;
pub const SPRITE_Y: u16 = 0x7003;
/// Spinner interrupts counted.
pub const SPINS: u16 = 0x7004;

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

    fn out(&mut self, port: u8) -> &mut Self {
        self.b(&[0xd3, port])
    }

    /// LD A,n ; OUT (port),A
    fn out_n(&mut self, port: u8, n: u8) -> &mut Self {
        self.b(&[0x3e, n, 0xd3, port])
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
const JR_Z: u8 = 0x28;
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

/// $0000: the reset vector.
fn reset_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0000);
    a.b(&[0xf3]) // DI
        .b(&[0x31, 0x00, 0x74]) // LD SP,$7400
        .b(&[0xed, 0x56]) // IM 1
        .b(&[0xc3, 0x00, 0x01]); // JP $0100
    a.finish()
}

/// $0100: the cartridge check and the demo.
fn main_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0100);
    a.b(&[0x2a, 0x00, 0x80]) // LD HL,($8000)
        .b(&[0x7d, 0xfe, 0xaa]) // LD A,L ; CP $AA
        .jr(JR_NZ, "not_aa55")
        .b(&[0x7c, 0xfe, 0x55]) // LD A,H ; CP $55
        .jr(JR_Z, "cart");
    a.label("not_aa55")
        .b(&[0x7d, 0xfe, 0x55])
        .jr(JR_NZ, "demo")
        .b(&[0x7c, 0xfe, 0xaa])
        .jr(JR_NZ, "demo");
    a.label("cart")
        .b(&[0x2a, 0x0a, 0x80]) // LD HL,($800A)
        .b(&[0xe9]); // JP (HL)
    a.label("demo")
        // Sound off: all four attenuations to 15; tone 0 period $0FE.
        .out_n(0xff, 0x9f)
        .out_n(0xff, 0xbf)
        .out_n(0xff, 0xdf)
        .out_n(0xff, 0xff)
        .out_n(0xff, 0x8e)
        .out_n(0xff, 0x0f)
        .abs(&[0x21], "vdp_regs") // LD HL,VDP_REGS
        .b(&[0x06, 16]); // LD B,16
    a.label("regs")
        .b(&[0x7e, 0xd3, 0xbf, 0x23]) // LD A,(HL) ; OUT ($BF),A ; INC HL
        .jr(DJNZ, "regs")
        // VRAM is clear after power-on. Pattern 1: a box.
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
        // The sprite at (120, 96), then the end mark.
        .b(&[0x21, 0x78, 0x60, 0x22]) // LD HL,$6078 ; LD (SPRITE_X),HL
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0x21, 0x00, 0x00, 0x22]) // LD HL,0 ; LD (FRAMES),HL
        .b(&FRAMES.to_le_bytes())
        .b(&[0xaf, 0x32]) // XOR A ; LD (SPINS),A
        .b(&SPINS.to_le_bytes())
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

/// $0038: a spinner step (maskable interrupt, mode 1).
fn int_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0038);
    a.b(&[0xf5]) // PUSH AF
        .b(&[0xdb, 0xfc, 0xdb, 0xff]) // IN A,($FC) ; IN A,($FF)
        .b(&[0x3a])
        .b(&SPINS.to_le_bytes())
        .b(&[0x3c, 0x32]) // INC A ; LD (SPINS),A
        .b(&SPINS.to_le_bytes())
        .b(&[0xf1, 0xfb, 0xed, 0x4d]); // POP AF ; EI ; RETI
    a.finish()
}

/// $0066: the VDP's frame interrupt.
fn nmi_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0066);
    a.b(&[0xf5, 0xe5, 0xc5]) // PUSH AF ; PUSH HL ; PUSH BC
        .b(&[0xdb, 0xbf]) // IN A,($BF): acknowledge
        .b(&[0x2a])
        .b(&FRAMES.to_le_bytes())
        .b(&[0x23, 0x22]) // INC HL ; LD (FRAMES),HL
        .b(&FRAMES.to_le_bytes())
        .b(&[0xd3, 0xc0]) // OUT ($C0),A: joystick segment
        .b(&[0xdb, 0xfc, 0x4f]) // IN A,($FC) ; LD C,A
        .b(&[0x21])
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0xcb, 0x59]) // BIT 3,C: left
        .jr(JR_NZ, "no_left")
        .b(&[0x35]); // DEC (HL)
    a.label("no_left")
        .b(&[0xcb, 0x49]) // BIT 1,C: right
        .jr(JR_NZ, "no_right")
        .b(&[0x34]); // INC (HL)
    a.label("no_right")
        .b(&[0x23]) // INC HL: Y
        .b(&[0xcb, 0x41]) // BIT 0,C: up
        .jr(JR_NZ, "no_up")
        .b(&[0x35]);
    a.label("no_up")
        .b(&[0xcb, 0x51]) // BIT 2,C: down
        .jr(JR_NZ, "no_down")
        .b(&[0x34]);
    a.label("no_down")
        .b(&[0x3e, 0x9f]) // LD A,$9F: tone 0 off
        .b(&[0xcb, 0x71]) // BIT 6,C: left fire
        .jr(JR_NZ, "silent")
        .b(&[0x3e, 0x90]); // LD A,$90: loud
    a.label("silent")
        .b(&[0xd3, 0xff])
        .vram_write(0x1b00)
        .b(&[0x3a])
        .b(&SPRITE_Y.to_le_bytes())
        .out(0xbe)
        .b(&[0x3a])
        .b(&SPRITE_X.to_le_bytes())
        .out(0xbe)
        .b(&[0xc1, 0xe1, 0xf1]) // POP BC ; POP HL ; POP AF
        .b(&[0xed, 0x45]); // RETN
    a.finish()
}

/// The 8 KB image.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0xffu8; 0x2000];
    for (at, code) in [reset_code(), int_code(), nmi_code(), main_code()] {
        let at = usize::from(at);
        rom[at..at + code.len()].copy_from_slice(&code);
    }
    rom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pieces_do_not_overlap() {
        let (_, reset) = reset_code();
        let (_, int) = int_code();
        let (_, nmi) = nmi_code();
        let (_, main) = main_code();
        assert!(reset.len() <= 0x38);
        assert!(0x38 + int.len() <= 0x66);
        assert!(0x66 + nmi.len() <= 0x100);
        assert!(0x100 + main.len() <= 0x2000);
        let rom = rom();
        assert_eq!((rom[0x38], rom[0x66], rom[0x100]), (0xf5, 0xf5, 0x2a));
    }
}
