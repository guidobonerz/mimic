//! A small built-in ROM for tests without the user's ROMs (`--option
//! roms=stub`): clears the screen, writes a line and echoes ENTER as a
//! block in the next row. This project's own Z80 code.

/// The line the stub writes at the top of the screen.
pub const MESSAGE: &[u8] = b"TRS-80 STUB ROM";

pub fn rom() -> Vec<u8> {
    let mut r = vec![0xffu8; 0x3000];
    #[rustfmt::skip]
    let code: &[u8] = &[
        0xf3,                   // DI
        0x31, 0x00, 0x80,       // LD SP,$8000
        0x21, 0x00, 0x3c,       // LD HL,$3C00
        0x36, 0x20,             // LD (HL),' '
        0x11, 0x01, 0x3c,       // LD DE,$3C01
        0x01, 0xff, 0x03,       // LD BC,$03FF
        0xed, 0xb0,             // LDIR
        0x21, 0x40, 0x00,       // LD HL,MESSAGE ($0040)
        0x11, 0x00, 0x3c,       // LD DE,$3C00
        0x01, MESSAGE.len() as u8, 0x00, // LD BC,len
        0xed, 0xb0,             // LDIR
        // loop: a block at $3C40 while ENTER is down.
        0x3a, 0x40, 0x38,       // LD A,($3840)
        0xe6, 0x01,             // AND 1
        0x3e, 0x20,             // LD A,' '
        0x28, 0x02,             // JR Z,+2
        0x3e, 0xbf,             // LD A,$BF (all six blocks)
        0x32, 0x40, 0x3c,       // LD ($3C40),A
        0x18, 0xf0,             // JR loop
    ];
    r[..code.len()].copy_from_slice(code);
    r[0x40..0x40 + MESSAGE.len()].copy_from_slice(MESSAGE);
    r
}

#[cfg(test)]
mod tests {
    #[test]
    fn layout() {
        let r = super::rom();
        assert_eq!(r.len(), 0x3000);
        // The loop jump lands on the LD A,($3840).
        let jr = r.iter().position(|&b| b == 0x18).unwrap();
        let target = (jr as i32 + 2 + i32::from(r[jr + 1] as i8)) as usize;
        assert_eq!(&r[target..target + 3], &[0x3a, 0x40, 0x38]);
    }
}
