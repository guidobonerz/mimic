//! A tiny built-in lower ROM for tests without the user's ROMs: it sets
//! the CRTC to the firmware's values, mode 1 with a blue background, white
//! pen 1 and a black border, fills the screen at &C000 with pen 1 and
//! counts interrupts at &8000 (IM 1).

use crate::memory::{Roms, ROM_SIZE};

/// The program at &0100, the interrupt handler at &0038, the CRTC table at
/// &0200.
const MAIN: &[u8] = &[
    0xf3, // DI
    0x31, 0x00, 0xc0, // LD SP,&C000
    0x01, 0x00, 0xbc, // LD BC,&BC00
    0x21, 0x00, 0x02, // LD HL,&0200
    0x1e, 0x00, // LD E,0
    0xed, 0x59, // loop: OUT (C),E      select
    0x7e, // LD A,(HL)
    0x04, // INC B                       &BDxx
    0xed, 0x79, // OUT (C),A
    0x05, // DEC B
    0x23, // INC HL
    0x1c, // INC E
    0x7b, // LD A,E
    0xfe, 0x0e, // CP 14
    0x20, 0xf2, // JR NZ,loop
    0x01, 0x00, 0x7f, // LD BC,&7F00
    0x3e, 0x89, // LD A,&89 RMR: mode 1, upper ROM off
    0xed, 0x79, // OUT (C),A
    0x3e, 0x10, // border
    0xed, 0x79, //
    0x3e, 0x54, // black (20)
    0xed, 0x79, //
    0xaf, // pen 0
    0xed, 0x79, //
    0x3e, 0x44, // blue (4)
    0xed, 0x79, //
    0x3e, 0x01, // pen 1
    0xed, 0x79, //
    0x3e, 0x4b, // bright white (11)
    0xed, 0x79, //
    0x21, 0x00, 0xc0, // LD HL,&C000
    0x11, 0x01, 0xc0, // LD DE,&C001
    0x01, 0xff, 0x3f, // LD BC,&3FFF
    0x36, 0xf0, // LD (HL),&F0  (mode 1: four pixels of pen 1)
    0xed, 0xb0, // LDIR
    0xed, 0x56, // IM 1
    0xfb, // EI
    0x76, // idle: HALT
    0x18, 0xfd, // JR idle
];

/// Counts interrupts in the byte at &8000.
const HANDLER: &[u8] = &[
    0xf5, // PUSH AF
    0x3a, 0x00, 0x80, // LD A,(&8000)
    0x3c, // INC A
    0x32, 0x00, 0x80, // LD (&8000),A
    0xf1, // POP AF
    0xfb, // EI
    0xc9, // RET
];

/// The firmware's CRTC settings, R0–R13.
const CRTC: &[u8] = &[63, 40, 46, 0x8e, 38, 0, 25, 30, 0, 7, 0, 0, 0x30, 0];

pub fn roms() -> Roms {
    let mut os = Box::new([0u8; ROM_SIZE]);
    os[..3].copy_from_slice(&[0xc3, 0x00, 0x01]); // JP &0100
    os[0x38..0x38 + HANDLER.len()].copy_from_slice(HANDLER);
    os[0x100..0x100 + MAIN.len()].copy_from_slice(MAIN);
    os[0x200..0x200 + CRTC.len()].copy_from_slice(CRTC);
    Roms {
        os,
        basic: Box::new([0; ROM_SIZE]),
        dos: None,
    }
}
