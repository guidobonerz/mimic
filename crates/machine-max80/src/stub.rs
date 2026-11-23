//! A small built-in program in place of the boot ROM (`roms=stub`), this
//! project's own, so the machine runs without Lobo's ROM (tests, the
//! machine registry). It cannot boot a disk: it copies itself into RAM,
//! sets the CRTC up for 64 × 16 characters of twelve lines, loads four
//! letters into the font RAM through VLATCH's window, writes `NO ROM` to
//! the middle of the screen and halts.

/// Where the part that runs in RAM lies in the ROM and where it goes.
const BODY_AT: usize = 0x30;
const RUN_AT: u16 = 0x4000;

/// The message's place: row 7, column 29 of the first video kilobyte.
pub const MESSAGE_AT: usize = 7 * 64 + 29;
pub const MESSAGE: &[u8] = b"NO ROM";

pub fn rom() -> Vec<u8> {
    let start: &[u8] = &[
        0xf3, // DI
        0x21, 0x30, 0x00, // LD HL,$0030
        0x11, 0x00, 0x40, // LD DE,$4000
        0x01, 0xc0, 0x00, // LD BC,$00C0
        0xed, 0xb0, // LDIR
        0xc3, 0x00, 0x40, // JP $4000
    ];
    let body: &[u8] = &[
        // $4000: the CRTC's registers 0-13 from the table.
        0x21, 0x47, 0x40, // LD HL,$4047
        0x06, 0x0e, // LD B,14
        0x0e, 0x00, // LD C,0
        0x79, // LD A,C
        0x32, 0xe0, 0x07, // LD ($07E0),A
        0x7e, // LD A,(HL)
        0x32, 0xe1, 0x07, // LD ($07E1),A
        0x23, // INC HL
        0x0c, // INC C
        0x10, 0xf4, // DJNZ $4007
        // $4013: the font's first kilobyte into the window, cleared.
        0x3e, 0x04, // LD A,$04
        0x32, 0xdc, 0x07, // LD ($07DC),A
        0x21, 0x00, 0x00, // LD HL,$0000
        0x36, 0x00, // LD (HL),$00
        0x23, // INC HL
        0x7c, // LD A,H
        0xfe, 0x04, // CP $04
        0x20, 0xf8, // JR NZ,$401B
        // $4023: the shapes of M to R.
        0x21, 0x55, 0x40, // LD HL,$4055
        0x11, 0x68, 0x02, // LD DE,$0268
        0x01, 0x30, 0x00, // LD BC,$0030
        0xed, 0xb0, // LDIR
        // $402E: blanks into the first video kilobyte.
        0x21, 0x00, 0x0c, // LD HL,$0C00
        0x36, 0x20, // LD (HL),$20
        0x23, // INC HL
        0x7c, // LD A,H
        0xfe, 0x10, // CP $10
        0x20, 0xf8, // JR NZ,$4031
        // $4039: the message.
        0x21, 0x85, 0x40, // LD HL,$4085
        0x11, 0xdd, 0x0d, // LD DE,$0DDD
        0x01, 0x06, 0x00, // LD BC,6
        0xed, 0xb0, // LDIR
        0x76, // HALT
        0x18, 0xfd, // JR $4044
        // $4047: 120 characters a line, 64 shown; 22 rows of 12 lines (264
        // lines a field), 16 shown.
        0x77, 0x40, 0x5f, 0x08, 0x15, 0x00, 0x10, 0x12, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x00,
        // $4055: M N O P Q R (P and Q blank).
        0x82, 0xc6, 0xaa, 0x92, 0x82, 0x82, 0x82, 0x00, //
        0x82, 0xc2, 0xa2, 0x92, 0x8a, 0x86, 0x82, 0x00, //
        0x7c, 0x82, 0x82, 0x82, 0x82, 0x82, 0x7c, 0x00, //
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0xfc, 0x82, 0x82, 0xfc, 0x88, 0x84, 0x82, 0x00, //
    ];
    debug_assert_eq!(usize::from(RUN_AT) + body.len(), 0x4085);
    let mut rom = vec![0u8; crate::ROM_SIZE];
    rom[..start.len()].copy_from_slice(start);
    rom[BODY_AT..BODY_AT + body.len()].copy_from_slice(body);
    let msg = BODY_AT + body.len();
    rom[msg..msg + MESSAGE.len()].copy_from_slice(MESSAGE);
    rom
}
