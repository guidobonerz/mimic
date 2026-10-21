//! A tiny built-in "Kickstart" for tests without ROMs (`--option
//! roms=stub`): it clears the overlay, installs a level 3 (vertical blank)
//! interrupt handler that counts fields at $100, and starts a Copper list
//! that shows red above line $80 and green below.

/// Hand-assembled 68000 code at $F80008 (opcodes from the M68000
/// Programmer's Reference Manual).
const CODE: &[u16] = &[
    0x13fc, 0x0003, 0x00bf, 0xe201, // move.b #3,$bfe201      DDRA: OVL, LED out
    0x13fc, 0x0000, 0x00bf, 0xe001, // move.b #0,$bfe001      overlay off
    0x4df9, 0x00df, 0xf000, //         lea $dff000,a6
    0x3d7c, 0x7fff, 0x009a, //         move.w #$7fff,INTENA(a6)
    0x3d7c, 0x7fff, 0x009c, //         move.w #$7fff,INTREQ(a6)
    0x3d7c, 0x7fff, 0x0096, //         move.w #$7fff,DMACON(a6)
    0x21fc, 0x00f8, 0x0100, 0x006c, // move.l #vbl,$6c.w
    0x41f8, 0x1000, //                 lea $1000.w,a0
    0x20fc, 0x0180, 0x0f00, //         move.l #$01800f00,(a0)+  COLOR00 red
    0x20fc, 0x8001, 0xfffe, //         move.l #$8001fffe,(a0)+  WAIT line $80
    0x20fc, 0x0180, 0x00f0, //         move.l #$018000f0,(a0)+  COLOR00 green
    0x20fc, 0xffff, 0xfffe, //         move.l #$fffffffe,(a0)+  end
    0x2d7c, 0x0000, 0x1000, 0x0080, // move.l #$1000,COP1LC(a6)
    0x3d40, 0x0088, //                 move.w d0,COPJMP1(a6)
    0x3d7c, 0x8280, 0x0096, //         move.w #$8280,DMACON(a6)  DMAEN|COPEN
    0x3d7c, 0xc020, 0x009a, //         move.w #$c020,INTENA(a6)  INTEN|VERTB
    0x46fc, 0x2000, //                 move.w #$2000,sr
    0x60fe, //                         bra.s *
];

/// The interrupt handler at $F80100.
const VBL: &[u16] = &[
    0x52b8, 0x0100, //         addq.l #1,$100.w
    0x3d7c, 0x0020, 0x009c, // move.w #VERTB,INTREQ(a6)
    0x4e73, //                 rte
];

/// The 256 KB image.
pub fn rom() -> Vec<u8> {
    let mut words = vec![0u16; 0x2_0000];
    // Reset vectors: SSP $00080000, PC $00F80008.
    words[..4].copy_from_slice(&[0x0008, 0x0000, 0x00f8, 0x0008]);
    words[4..4 + CODE.len()].copy_from_slice(CODE);
    words[0x80..0x80 + VBL.len()].copy_from_slice(VBL);
    words.iter().flat_map(|w| w.to_be_bytes()).collect()
}
