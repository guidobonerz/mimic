//! Built-in test ROM for `roms=stub`: switches the ROM overlay off,
//! clears the screen, draws a line of text at the top and then inverts a
//! block on line 300 each time the mouse button is pressed. Interrupts
//! stay masked. Assembled here by a few encoders for the 68000
//! instructions it uses (M68000 Programmer's Reference Manual).

/// Text drawn on the screen (capitals and spaces).
pub const TEXT: &str = "MIMIC MACINTOSH STUB ROM";
/// Where the text starts: screen line and byte column.
pub const TEXT_LINE: u32 = 16;
pub const TEXT_BYTE: u32 = 4;
/// The line the mouse button inverts (first 32 pixels).
pub const BLOCK_LINE: u32 = 300;

#[rustfmt::skip]
const GLYPHS: [(char, [u8; 7]); 14] = [
    (' ', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
    ('A', [0x04, 0x0a, 0x11, 0x11, 0x1f, 0x11, 0x11]),
    ('B', [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e]),
    ('C', [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e]),
    ('H', [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
    ('I', [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e]),
    ('M', [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11]),
    ('O', [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
    ('R', [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11]),
    ('S', [0x0e, 0x11, 0x10, 0x0e, 0x01, 0x11, 0x0e]),
    ('T', [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
    ('X', [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11]),
];

/// The text as a 1-bit picture: `TEXT.len()` cells of 6 pixels, 8 rows,
/// each row a whole number of bytes. Returns (bytes per row, rows).
pub fn text_bitmap() -> (usize, Vec<u8>) {
    let width = TEXT.len() * 6;
    let bpr = width.div_ceil(8);
    let mut out = vec![0u8; bpr * 8];
    for (i, ch) in TEXT.chars().enumerate() {
        let glyph = GLYPHS
            .iter()
            .find(|(c, _)| *c == ch)
            .map_or([0; 7], |(_, g)| *g);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits >> (4 - col) & 1 != 0 {
                    let x = i * 6 + col;
                    out[row * bpr + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
    }
    (bpr, out)
}

/// A tiny 68000 assembler: big-endian words with branch patching.
struct Asm {
    code: Vec<u8>,
    base: u32,
}

impl Asm {
    fn w(&mut self, v: u16) {
        self.code.extend_from_slice(&v.to_be_bytes());
    }

    fn l(&mut self, v: u32) {
        self.code.extend_from_slice(&v.to_be_bytes());
    }

    fn here(&self) -> u32 {
        self.base + self.code.len() as u32
    }

    /// MOVE.B #imm,abs.L
    fn move_b_imm_abs(&mut self, imm: u8, addr: u32) {
        self.w(0x13fc);
        self.w(u16::from(imm));
        self.l(addr);
    }

    /// LEA abs.L,An
    fn lea(&mut self, addr: u32, an: u16) {
        self.w(0x41f9 | an << 9);
        self.l(addr);
    }

    /// MOVE.W #imm,Dn
    fn move_w_imm(&mut self, imm: u16, dn: u16) {
        self.w(0x303c | dn << 9);
        self.w(imm);
    }

    /// DBRA Dn,target
    fn dbra(&mut self, dn: u16, target: u32) {
        let at = self.here() + 2;
        self.w(0x51c8 | dn);
        self.w(target.wrapping_sub(at) as u16);
    }

    /// BTST #bit,abs.L
    fn btst_abs(&mut self, bit: u16, addr: u32) {
        self.w(0x0839);
        self.w(bit);
        self.l(addr);
    }

    /// Bcc.S to target (cond: 0 = BRA, 6 = BNE, 7 = BEQ).
    fn bcc(&mut self, cond: u16, target: u32) {
        let at = self.here() + 2;
        let d = target.wrapping_sub(at) as i32;
        assert!((-128..128).contains(&d) && d != 0);
        self.w(0x6000 | cond << 8 | (d as u8) as u16);
    }
}

/// VIA registers (register n at $EFE1FE + n·$200).
fn via(n: u32) -> u32 {
    0xefe1fe + n * 0x200
}

/// The stub ROM image of `size` bytes for a machine with `ram` bytes.
pub fn rom(size: usize, ram: u32) -> Vec<u8> {
    let screen = ram - 0x5900;
    let mut a = Asm {
        code: Vec::new(),
        base: 0x40_0100,
    };
    a.w(0x46fc); // MOVE.W #$2700,SR
    a.w(0x2700);
    // Port A: PA0–PA6 out; overlay off, main screen and sound buffer.
    a.move_b_imm_abs(0x7f, via(3));
    a.move_b_imm_abs(0x68, via(15));
    // Port B: PB0–PB2 (clock) and PB7 (sound) out; sound off.
    a.move_b_imm_abs(0x87, via(2));
    a.move_b_imm_abs(0x84, via(0));
    // Clear the screen.
    a.lea(screen, 0);
    a.move_w_imm((0x5580 / 4 - 1) as u16, 0);
    a.w(0x7200); // MOVEQ #0,D1
    let clear = a.here();
    a.w(0x20c1); // MOVE.L D1,(A0)+
    a.dbra(0, clear);
    // Copy the text picture.
    let (bpr, _) = text_bitmap();
    let bitmap_at = 0x40_0400;
    a.lea(bitmap_at, 1);
    a.lea(screen + TEXT_LINE * 64 + TEXT_BYTE, 0);
    a.move_w_imm(7, 0);
    let row = a.here();
    a.move_w_imm(bpr as u16 - 1, 1);
    let byte = a.here();
    a.w(0x10d9); // MOVE.B (A1)+,(A0)+
    a.dbra(1, byte);
    a.w(0xd0fc); // ADDA.W #(64-bpr),A0
    a.w(64 - bpr as u16);
    a.dbra(0, row);
    // Mouse button: invert a block on each press.
    a.lea(screen + BLOCK_LINE * 64, 2);
    let main = a.here();
    a.btst_abs(3, via(0));
    a.bcc(6, main); // up: wait
    a.w(0x4692); // NOT.L (A2)
    let wait = a.here();
    a.btst_abs(3, via(0));
    a.bcc(7, wait); // down: wait for the release
    a.bcc(0, main);

    let mut out = vec![0u8; size];
    // Reset vectors: SSP, PC.
    out[0..4].copy_from_slice(&0x0000_8000u32.to_be_bytes());
    out[4..8].copy_from_slice(&0x0040_0100u32.to_be_bytes());
    out[0x100..0x100 + a.code.len()].copy_from_slice(&a.code);
    let (_, bits) = text_bitmap();
    out[0x400..0x400 + bits.len()].copy_from_slice(&bits);
    out
}
