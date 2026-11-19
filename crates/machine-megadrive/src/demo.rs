//! The built-in test cartridge (`--option cart=demo`): this project's own
//! 68000 program, assembled here.
//!
//! It sets up the VDP (H40, plane A at $C000 with a box pattern, a sprite),
//! starts a Z80 program that keys a YM2612 note and then counts at Z80
//! $1000, and on every vertical interrupt counts [`FRAMES`], reads pad 1
//! (TH high at [`PADS`], TH low at [`PADS_TH0`]), moves the sprite with
//! the directions ([`SPRITE_X`], [`SPRITE_Y`]) and sounds a PSG tone while
//! A is held.

/// Fields counted by the vertical interrupt (word).
pub const FRAMES: u32 = 0xff_0000;
/// Pad 1 with TH high (`?1CBRLDU`) and low (`?0SA00DU`).
pub const PADS: u32 = 0xff_0002;
pub const PADS_TH0: u32 = 0xff_0003;
/// The sprite's coordinates in sprite space (words).
pub const SPRITE_X: u32 = 0xff_0004;
pub const SPRITE_Y: u32 = 0xff_0006;
/// The Z80 program's counter (Z80 address).
pub const Z80_COUNTER: u16 = 0x1000;

const VDP_DATA: u32 = 0xc0_0000;
const VDP_CTRL: u32 = 0xc0_0004;
const PSG: u32 = 0xc0_0011;
const MAIN: u32 = 0x200;
const VINT: u32 = 0x600;
const RTE_STUB: u32 = 0x7f0;
/// The data tables.
const REGS_AT: u32 = 0x800;
const TILES_AT: u32 = 0x840;

/// A minimal 68000 assembler: words, labels and branch fix-ups.
struct Asm {
    origin: u32,
    words: Vec<u16>,
    labels: Vec<(&'static str, u32)>,
    /// (word index, label, kind): 8 = short branch, 16 = displacement word.
    fixups: Vec<(usize, &'static str, u8)>,
}

impl Asm {
    fn new(origin: u32) -> Self {
        Self {
            origin,
            words: Vec::new(),
            labels: Vec::new(),
            fixups: Vec::new(),
        }
    }

    fn here(&self) -> u32 {
        self.origin + 2 * self.words.len() as u32
    }

    fn w(&mut self, words: &[u16]) -> &mut Self {
        self.words.extend_from_slice(words);
        self
    }

    fn l(&mut self, v: u32) -> &mut Self {
        self.w(&[(v >> 16) as u16, v as u16])
    }

    fn label(&mut self, name: &'static str) -> &mut Self {
        let at = self.here();
        self.labels.push((name, at));
        self
    }

    /// Bcc.S: `op` is $60 (BRA), $66 (BNE), $67 (BEQ).
    fn bcc(&mut self, op: u8, to: &'static str) -> &mut Self {
        self.fixups.push((self.words.len(), to, 8));
        self.w(&[u16::from(op) << 8])
    }

    /// DBRA Dn.
    fn dbra(&mut self, d: u16, to: &'static str) -> &mut Self {
        self.w(&[0x51c8 | d]);
        self.fixups.push((self.words.len(), to, 16));
        self.w(&[0])
    }

    /// MOVE.L #v,abs.L
    fn move_l_imm(&mut self, v: u32, dst: u32) -> &mut Self {
        self.w(&[0x23fc]).l(v).l(dst)
    }

    /// MOVE.W #v,abs.L
    fn move_w_imm(&mut self, v: u16, dst: u32) -> &mut Self {
        self.w(&[0x33fc, v]).l(dst)
    }

    /// MOVE.B #v,abs.L
    fn move_b_imm(&mut self, v: u8, dst: u32) -> &mut Self {
        self.w(&[0x13fc, u16::from(v)]).l(dst)
    }

    /// MOVE.W abs.L,abs.L
    fn move_w_abs(&mut self, src: u32, dst: u32) -> &mut Self {
        self.w(&[0x33f9]).l(src).l(dst)
    }

    /// MOVE.B abs.L,Dn
    fn move_b_to_d(&mut self, src: u32, d: u16) -> &mut Self {
        self.w(&[0x1039 | d << 9]).l(src)
    }

    /// MOVE.B Dn,abs.L
    fn move_b_from_d(&mut self, d: u16, dst: u32) -> &mut Self {
        self.w(&[0x13c0 | d]).l(dst)
    }

    /// LEA abs.L,A0
    fn lea_a0(&mut self, label_addr: u32) -> &mut Self {
        self.w(&[0x41f9]).l(label_addr)
    }

    /// MOVEQ #n,Dn
    fn moveq(&mut self, n: u8, d: u16) -> &mut Self {
        self.w(&[0x7000 | d << 9 | u16::from(n)])
    }

    /// MOVE.W #n,Dn
    fn move_w_d(&mut self, n: u16, d: u16) -> &mut Self {
        self.w(&[0x303c | d << 9, n])
    }

    /// BTST #n,Dd
    fn btst(&mut self, n: u16, d: u16) -> &mut Self {
        self.w(&[0x0800 | d, n])
    }

    /// ADDQ.W #1,abs.L / SUBQ.W #1,abs.L
    fn addq_w(&mut self, dst: u32) -> &mut Self {
        self.w(&[0x5279]).l(dst)
    }

    fn subq_w(&mut self, dst: u32) -> &mut Self {
        self.w(&[0x5379]).l(dst)
    }

    fn finish(mut self) -> (u32, Vec<u16>) {
        for (i, name, kind) in std::mem::take(&mut self.fixups) {
            let to = self
                .labels
                .iter()
                .find(|(n, _)| *n == name)
                .map(|&(_, a)| a)
                .unwrap_or_else(|| panic!("label {name}"));
            let at = self.origin + 2 * i as u32;
            if kind == 8 {
                let d = to as i32 - (at as i32 + 2);
                assert!((-128..128).contains(&d) && d != 0, "short branch to {name}");
                self.words[i] |= (d as i8 as u8) as u16;
            } else {
                let d = to as i32 - at as i32;
                self.words[i] = d as i16 as u16;
            }
        }
        (self.origin, self.words)
    }
}

/// VDP register writes: H40, mode 5, planes A $C000, window $B000, B
/// $E000, sprites $A800, hscroll $AC00, increment 2, 64 × 32 planes,
/// backdrop colour 0; display off until the end of the set-up.
const VDP_REGS: [u16; 14] = [
    0x8004, 0x8104, 0x8230, 0x832c, 0x8407, 0x8554, 0x8700, 0x8a00, 0x8b00, 0x8c81, 0x8d2b, 0x8f02,
    0x9001, 0x9100,
];

/// CRAM 0 dark blue, 1 yellow, 16 black, 17 white.
const PALETTE: [(u16, u16); 4] = [(0, 0x0600), (1, 0x00ee), (16, 0x0000), (17, 0x0eee)];

/// The YM2612 note the Z80 keys: the manual's "Grand Piano" on channel 1.
const YM_NOTE: [(u8, u8); 30] = [
    (0x30, 0x71),
    (0x34, 0x0d),
    (0x38, 0x33),
    (0x3c, 0x01),
    (0x40, 0x23),
    (0x44, 0x2d),
    (0x48, 0x26),
    (0x4c, 0x00),
    (0x50, 0x5f),
    (0x54, 0x99),
    (0x58, 0x5f),
    (0x5c, 0x94),
    (0x60, 5),
    (0x64, 5),
    (0x68, 5),
    (0x6c, 7),
    (0x70, 2),
    (0x74, 2),
    (0x78, 2),
    (0x7c, 2),
    (0x80, 0x11),
    (0x84, 0x11),
    (0x88, 0x11),
    (0x8c, 0xa6),
    (0xb0, 0x32),
    (0xb4, 0xc0),
    (0x28, 0x00),
    (0xa4, 0x22),
    (0xa0, 0x69),
    (0x28, 0xf0),
];

/// The Z80 program: key the note, then count at [`Z80_COUNTER`] forever.
pub fn z80_program() -> Vec<u8> {
    let mut z = Vec::new();
    for (r, v) in YM_NOTE {
        // LD A,r ; LD ($4000),A ; LD A,v ; LD ($4001),A
        z.extend_from_slice(&[0x3e, r, 0x32, 0x00, 0x40, 0x3e, v, 0x32, 0x01, 0x40]);
    }
    let lp = z.len() as u16;
    let [cl, ch] = Z80_COUNTER.to_le_bytes();
    // LD HL,counter ; loop: INC (HL) ; JP loop
    z.extend_from_slice(&[0x21, cl, ch, 0x34, 0xc3]);
    z.extend_from_slice(&(lp + 3).to_le_bytes());
    z
}

const Z80_CODE_AT: u32 = 0x1000;

fn main_code() -> (u32, Vec<u16>) {
    let mut a = Asm::new(MAIN);
    let z80 = z80_program();
    a.w(&[0x46fc, 0x2700]) // MOVE #$2700,SR
        // TMSS: if the version's low nibble is non-zero, write "SEGA".
        .move_b_to_d(0xa1_0001, 0)
        .w(&[0x0200, 0x000f]) // ANDI.B #$0F,D0
        .bcc(0x67, "no_tmss")
        .move_l_imm(0x5345_4741, 0xa1_4000)
        .label("no_tmss")
        // VDP registers.
        .lea_a0(REGS_AT)
        .moveq(VDP_REGS.len() as u8 - 1, 1);
    a.label("regs")
        .w(&[0x33d8])
        .l(VDP_CTRL) // MOVE.W (A0)+,ctrl
        .dbra(1, "regs");
    // CRAM.
    for (i, c) in PALETTE {
        a.move_l_imm(0xc000_0000 | u32::from(i * 2) << 16, VDP_CTRL)
            .move_w_imm(c, VDP_DATA);
    }
    // Patterns 1 (a box, colour 1) and 2 (solid colour 1), from $0020.
    a.move_l_imm(0x4020_0000, VDP_CTRL)
        .lea_a0(TILES_AT)
        .moveq(31, 1);
    a.label("tiles").w(&[0x33d8]).l(VDP_DATA).dbra(1, "tiles");
    // Plane A at $C000: 64 × 28 words alternating patterns 1 and 0.
    a.move_l_imm(0x4000_0003, VDP_CTRL)
        .move_w_d(64 * 28 / 2 - 1, 1)
        .label("names")
        .move_l_imm(0x0001_0000, VDP_DATA)
        .dbra(1, "names");
    // The sprite at (280, 224) in sprite space: palette 1, pattern 2.
    a.move_w_imm(280, SPRITE_X)
        .move_w_imm(224, SPRITE_Y)
        .move_l_imm(0x6800_0002, VDP_CTRL)
        .move_w_imm(224, VDP_DATA)
        .move_w_imm(0x0000, VDP_DATA)
        .move_w_imm(0x2002, VDP_DATA)
        .move_w_imm(280, VDP_DATA)
        // Pad 1: TH as an output.
        .move_b_imm(0x40, 0xa1_0009)
        // Z80: bus request, reset off, copy, reset, release, reset off.
        .move_w_imm(0x0100, 0xa1_1100)
        .move_w_imm(0x0100, 0xa1_1200)
        .lea_a0(Z80_CODE_AT)
        .w(&[0x43f9])
        .l(0xa0_0000) // LEA $A00000,A1
        .move_w_d(z80.len() as u16 - 1, 1);
    a.label("z80copy")
        .w(&[0x12d8]) // MOVE.B (A0)+,(A1)+
        .dbra(1, "z80copy")
        .move_w_imm(0x0000, 0xa1_1200)
        .move_w_imm(0x0000, 0xa1_1100)
        .move_w_imm(0x0100, 0xa1_1200)
        // Display, vertical interrupt and DMA on; interrupts on.
        .move_w_imm(0x8174, VDP_CTRL)
        .w(&[0x46fc, 0x2000]);
    a.label("idle").w(&[0x4e72, 0x2000]).bcc(0x60, "idle");
    a.finish()
}

/// Patterns 1 (a box) and 2 (solid), colour 1.
fn tiles() -> Vec<u16> {
    let rows = [
        0x1111_1111u32,
        0x1000_0001,
        0x1000_0001,
        0x1000_0001,
        0x1000_0001,
        0x1000_0001,
        0x1000_0001,
        0x1111_1111,
    ];
    rows.iter()
        .chain(&[0x1111_1111; 8])
        .flat_map(|&r| [(r >> 16) as u16, r as u16])
        .collect()
}

fn vint_code() -> (u32, Vec<u16>) {
    let mut a = Asm::new(VINT);
    a.w(&[0x48e7, 0xc000]) // MOVEM.L D0-D1,-(SP)
        .addq_w(FRAMES)
        // The pad: TH high, read; TH low, read.
        .move_b_imm(0x40, 0xa1_0003)
        .w(&[0x4e71, 0x4e71])
        .move_b_to_d(0xa1_0003, 0)
        .move_b_imm(0x00, 0xa1_0003)
        .w(&[0x4e71, 0x4e71])
        .move_b_to_d(0xa1_0003, 1)
        .move_b_from_d(0, PADS)
        .move_b_from_d(1, PADS_TH0)
        .btst(3, 0)
        .bcc(0x66, "no_right")
        .addq_w(SPRITE_X)
        .label("no_right")
        .btst(2, 0)
        .bcc(0x66, "no_left")
        .subq_w(SPRITE_X)
        .label("no_left")
        .btst(0, 0)
        .bcc(0x66, "no_up")
        .subq_w(SPRITE_Y)
        .label("no_up")
        .btst(1, 0)
        .bcc(0x66, "no_down")
        .addq_w(SPRITE_Y)
        .label("no_down")
        // A (TH low, bit 4): a PSG tone.
        .move_b_imm(0x9f, PSG)
        .btst(4, 1)
        .bcc(0x66, "silent")
        .move_b_imm(0x8e, PSG)
        .move_b_imm(0x0f, PSG)
        .move_b_imm(0x90, PSG)
        .label("silent")
        // The sprite.
        .move_l_imm(0x6800_0002, VDP_CTRL)
        .move_w_abs(SPRITE_Y, VDP_DATA)
        .move_w_imm(0x0000, VDP_DATA)
        .move_w_imm(0x2002, VDP_DATA)
        .move_w_abs(SPRITE_X, VDP_DATA)
        .w(&[0x4cdf, 0x0003]) // MOVEM.L (SP)+,D0-D1
        .w(&[0x4e73]);
    a.finish()
}

fn put(rom: &mut [u8], at: u32, words: &[u16]) {
    for (i, w) in words.iter().enumerate() {
        let a = at as usize + 2 * i;
        rom[a..a + 2].copy_from_slice(&w.to_be_bytes());
    }
}

/// The 8 KB test cartridge.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x2000];
    // Vectors: SSP, PC, everything else to an RTE, level 6 to VINT.
    put(&mut rom, 0, &[0x00ff, 0xfe00, 0, MAIN as u16]);
    for v in 2..64u32 {
        let target = if v == 30 { VINT } else { RTE_STUB };
        put(&mut rom, v * 4, &[(target >> 16) as u16, target as u16]);
    }
    rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
    rom[0x120..0x130].copy_from_slice(b"MIMIC TEST CART ");
    rom[0x1f0..0x1f3].copy_from_slice(b"JUE");
    for (at, words) in [main_code(), vint_code()] {
        put(&mut rom, at, &words);
    }
    put(&mut rom, RTE_STUB, &[0x4e73]);
    put(&mut rom, REGS_AT, &VDP_REGS);
    put(&mut rom, TILES_AT, &tiles());
    let z = z80_program();
    rom[Z80_CODE_AT as usize..Z80_CODE_AT as usize + z.len()].copy_from_slice(&z);
    rom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pieces_fit() {
        let (_, main) = main_code();
        let (_, vint) = vint_code();
        assert!(
            MAIN + 2 * main.len() as u32 <= VINT,
            "main: {} words",
            main.len()
        );
        assert!(VINT + 2 * vint.len() as u32 <= RTE_STUB);
        assert!(REGS_AT + 2 * VDP_REGS.len() as u32 <= TILES_AT);
        assert!(TILES_AT + 2 * tiles().len() as u32 <= Z80_CODE_AT);
        assert!(z80_program().len() < 0x400);
        assert_eq!(&rom()[0x100..0x104], b"SEGA");
    }
}
