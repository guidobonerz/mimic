//! A tiny built-in BIOS for tests and for running without an MSX BIOS
//! (`--option roms=stub`).
//!
//! - It sets the PPI up, puts the RAM (slot 3, and its secondary slot 2
//!   in case the slot is expanded) into pages 2 and 3 and the
//!   stack below $F380, and makes the PSG's port A an input and port B an
//!   output with the trigger pins released and joystick port 1 selected.
//! - A cartridge in slot 1 or 2 whose page 1 or page 2 starts with "AB" is
//!   started at once through its INIT pointer, with pages 1 and 2 on its
//!   slot (games that call BIOS routines need a real BIOS or C-BIOS).
//! - Otherwise a demo: Graphics I with a striped screen of box patterns,
//!   one sprite the joystick in port 1 moves, a tone while trigger A is
//!   held. The interrupt (the VDP's frame interrupt, mode 1) counts fields
//!   at [`FRAMES`] and stores the PSG's port A at [`JOY`] and the keyboard's
//!   row 8 at [`KEYS`].
//!
//! Hand-assembled with the small assembler below (opcodes from the Zilog
//! Z80 CPU User Manual); the port numbers are the MSX's (see the crate
//! doc).

/// The field counter (16 bits).
pub const FRAMES: u16 = 0xe000;
/// The sprite's X and Y.
pub const SPRITE_X: u16 = 0xe002;
pub const SPRITE_Y: u16 = 0xe003;
/// The PSG's port A as last read (joystick port 1).
pub const JOY: u16 = 0xe004;
/// The keyboard's row 8 as last read (SPACE is bit 0, low when held).
pub const KEYS: u16 = 0xe005;

const VDP_DATA: u8 = 0x98;
const VDP_CTRL: u8 = 0x99;
const PSG_ADDR: u8 = 0xa0;
const PSG_WRITE: u8 = 0xa1;
const PSG_READ: u8 = 0xa2;
const PPI_A: u8 = 0xa8;
const PPI_B: u8 = 0xa9;
const PPI_C: u8 = 0xaa;
const PPI_CTRL: u8 = 0xab;

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

    /// Write `v` to PSG register `reg`.
    fn psg(&mut self, reg: u8, v: u8) -> &mut Self {
        self.out_n(PSG_ADDR, reg).out_n(PSG_WRITE, v)
    }

    /// Set the VDP's VRAM address for writing.
    fn vram_write(&mut self, addr: u16) -> &mut Self {
        self.out_n(VDP_CTRL, addr as u8)
            .out_n(VDP_CTRL, 0x40 | (addr >> 8) as u8)
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

/// $0000: the reset vector.
fn reset_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0000);
    a.b(&[0xf3]) // DI
        .b(&[0xc3, 0x00, 0x01]); // JP $0100
    a.finish()
}

/// $0100: the set-up, the cartridge check and the demo.
fn main_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0100);
    // PPI: A and C outputs, B input. Pages 3 and 2 RAM, 1 and 0 slot 0.
    a.out_n(PPI_CTRL, 0x82)
        .out_n(PPI_A, 0xf0)
        // If slot 3 is expanded its RAM is in 3-2 (else this writes RAM).
        .b(&[0x3e, 0xaa, 0x32, 0xff, 0xff]) // LD A,$AA ; LD ($FFFF),A
        .b(&[0x31, 0x80, 0xf3]) // LD SP,$F380
        .b(&[0xed, 0x56]) // IM 1
        // Port B out, port A in, the three tones on, no noise; pins high.
        .psg(7, 0xb8)
        .psg(15, 0x0f)
        .psg(8, 0)
        .psg(0, 0xfe)
        .psg(1, 0);
    // Pages 1 and 2 on slot 1 ($D4), then on slot 2 ($E8): a header at
    // $4000 or $8000 starts the cartridge.
    for (select, header, next) in [
        (0xd4, 0x4000u16, "c1"),
        (0xd4, 0x8000, "c2"),
        (0xe8, 0x4000, "c3"),
        (0xe8, 0x8000, "demo"),
    ] {
        a.out_n(PPI_A, select)
            .b(&[0x2a])
            .b(&header.to_le_bytes()) // LD HL,(header)
            .b(&[0x11, b'A', b'B']) // LD DE,"AB"
            .b(&[0xb7, 0xed, 0x52]) // OR A ; SBC HL,DE
            .jr(JR_NZ, next)
            .b(&[0x2a])
            .b(&(header + 2).to_le_bytes()) // LD HL,(header + 2)
            .b(&[0xe9]); // JP (HL)
        a.label(next);
    }
    a.out_n(PPI_A, 0xf0)
        .abs(&[0x21], "vdp_regs") // LD HL,VDP_REGS
        .b(&[0x06, 16]); // LD B,16
    a.label("regs")
        .b(&[0x7e, 0xd3, VDP_CTRL, 0x23]) // LD A,(HL) ; OUT (CTRL),A ; INC HL
        .jr(DJNZ, "regs")
        // VRAM is clear after power-on. Pattern 1: a box.
        .vram_write(0x0008)
        .abs(&[0x21], "box")
        .b(&[0x06, 8]);
    a.label("box_loop")
        .b(&[0x7e, 0xd3, VDP_DATA, 0x23])
        .jr(DJNZ, "box_loop")
        // Colour group 0: yellow on dark blue.
        .vram_write(0x2000)
        .out_n(VDP_DATA, 0xa4)
        // Sprite pattern 0: a block.
        .vram_write(0x3800)
        .b(&[0x06, 8]);
    a.label("block")
        .out_n(VDP_DATA, 0xff)
        .jr(DJNZ, "block")
        // Names: stripes of patterns 0 and 1.
        .vram_write(0x1800)
        .b(&[0x01, 0x00, 0x03]) // LD BC,$0300
        .b(&[0x16, 0x00]); // LD D,0
    a.label("names")
        .b(&[0x7a, 0xd3, VDP_DATA]) // LD A,D ; OUT (DATA),A
        .b(&[0xee, 0x01, 0x57]) // XOR 1 ; LD D,A
        .b(&[0x0b, 0x78, 0xb1]) // DEC BC ; LD A,B ; OR C
        .jr(JR_NZ, "names")
        // The sprite at (120, 96), then the end mark.
        .b(&[0x21, 0x78, 0x60, 0x22]) // LD HL,$6078 ; LD (SPRITE_X),HL
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0x21, 0x00, 0x00, 0x22]) // LD HL,0 ; LD (FRAMES),HL
        .b(&FRAMES.to_le_bytes())
        .vram_write(0x1b00)
        .out_n(VDP_DATA, 0x60)
        .out_n(VDP_DATA, 0x78)
        .out_n(VDP_DATA, 0x00)
        .out_n(VDP_DATA, 0x0f)
        .out_n(VDP_DATA, 0xd0)
        // Display and frame interrupt on.
        .out_n(VDP_CTRL, 0xe0)
        .out_n(VDP_CTRL, 0x81)
        .b(&[0xfb]); // EI
    a.label("main").b(&[0x76]).jr(JR, "main"); // HALT ; JR MAIN
    a.label("vdp_regs").b(&VDP_REGS);
    a.label("box")
        .b(&[0xff, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0xff]);
    a.finish()
}

/// $0038: the VDP's frame interrupt (mode 1).
fn int_code() -> (u16, Vec<u8>) {
    let mut a = Asm::new(0x0038);
    a.b(&[0xf5, 0xe5, 0xc5]) // PUSH AF ; PUSH HL ; PUSH BC
        .b(&[0xdb, VDP_CTRL]) // IN A,(CTRL): acknowledge
        .b(&[0x2a])
        .b(&FRAMES.to_le_bytes())
        .b(&[0x23, 0x22]) // INC HL ; LD (FRAMES),HL
        .b(&FRAMES.to_le_bytes())
        .out_n(PSG_ADDR, 14)
        .b(&[0xdb, PSG_READ, 0x4f, 0x32]) // IN A,(READ) ; LD C,A ; LD (JOY),A
        .b(&JOY.to_le_bytes())
        // Keyboard row 8; motor off, CAPS lamp off.
        .out_n(PPI_C, 0x58)
        .b(&[0xdb, PPI_B, 0x32]) // IN A,(B) ; LD (KEYS),A
        .b(&KEYS.to_le_bytes())
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
        .out_n(PSG_ADDR, 8)
        .b(&[0x3e, 0x00]) // LD A,0: channel A silent
        .b(&[0xcb, 0x61]) // BIT 4,C: trigger A
        .jr(JR_NZ, "silent")
        .b(&[0x3e, 0x0f]); // LD A,15: loud
    a.label("silent")
        .b(&[0xd3, PSG_WRITE])
        .vram_write(0x1b00)
        .b(&[0x3a])
        .b(&SPRITE_Y.to_le_bytes())
        .b(&[0xd3, VDP_DATA])
        .b(&[0x3a])
        .b(&SPRITE_X.to_le_bytes())
        .b(&[0xd3, VDP_DATA])
        .b(&[0xc1, 0xe1, 0xf1]) // POP BC ; POP HL ; POP AF
        .b(&[0xfb, 0xc9]); // EI ; RET
    a.finish()
}

/// The 32 KB image. Its byte $002B says 60 Hz, as an NTSC machine's BIOS.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0xffu8; 0x8000];
    for (at, code) in [reset_code(), int_code(), main_code()] {
        let at = usize::from(at);
        rom[at..at + code.len()].copy_from_slice(&code);
    }
    rom[0x2b] = 0x00;
    rom[0x2c] = 0x00;
    rom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pieces_do_not_overlap() {
        let (_, reset) = reset_code();
        let (_, int) = int_code();
        let (_, main) = main_code();
        assert!(reset.len() <= 0x2b);
        assert!(0x38 + int.len() <= 0x100, "{}", int.len());
        assert!(0x100 + main.len() <= 0x4000);
        let rom = rom();
        assert_eq!((rom[0x38], rom[0x100]), (0xf5, 0x3e));
    }
}
