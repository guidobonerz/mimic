//! Built-in KERNAL replacement for runs without Commodore ROMs
//! (`options["roms"] = "stub"`): tests and CI.
//!
//! Written for this project, not derived from the Commodore KERNAL. It
//! provides the documented entry points programs and test suites call, at
//! their documented addresses, with minimal behaviour:
//!
//! | Address | Real routine          | Stub behaviour                                  |
//! | ------- | --------------------- | ----------------------------------------------- |
//! | `$E000` | (reset)               | stack, CPU port, IOINIT, RESTOR, CLI, idle      |
//! | `$EA31` | default IRQ handler   | acknowledge CIA1, restore registers, RTI        |
//! | `$EA81` | IRQ exit              | PLA/TAY/PLA/TAX/PLA/RTI                         |
//! | `$FD15` | RESTOR                | vectors `$0314`/`$0316`/`$0318` to defaults     |
//! | `$FDA3` | IOINIT                | CIA/SID/port set-up, CIA1 timer A 1/60 s IRQ    |
//! | `$FE43` | NMI entry             | SEI, JMP (`$0318`)                              |
//! | `$FE47` | default NMI           | RTI                                             |
//! | `$FE66` | default BRK           | like the IRQ exit                               |
//! | `$FF48` | IRQ/BRK entry         | push registers, dispatch via `$0314`/`$0316`    |
//! | `$FF84`, `$FF8A` | IOINIT, RESTOR jump table entries               |
//! | `$FFD2` | BSOUT                 | RTS (callers observe it with a breakpoint)      |
//! | `$FFD5` | LOAD                  | SEC, RTS (error)                                |
//! | `$FFE4` | GETIN                 | A = 0, RTS                                      |
//! | `$FFBA`, `$FFBD` | SETLFS, SETNAM | RTS                                             |
//!
//! The CIA set-up values (timer `$4025` for PAL, DDRs, `$DD00 = $07`) are
//! the documented power-on configuration of the C64 (Programmer's Reference
//! Guide, memory map).

use crate::memory::KERNAL_SIZE;

/// Address of the idle loop after reset.
pub const IDLE: u16 = 0xe014;

const RESET: &[u8] = &[
    0x78, // E000 SEI
    0xa2, 0xff, // LDX #$FF
    0x9a, // TXS
    0xd8, // CLD
    // Data latch before DDR, or the ROMs vanish when the pins turn to outputs.
    0xa9, 0x37, 0x85, 0x01, // LDA #$37; STA $01
    0xa9, 0x2f, 0x85, 0x00, // LDA #$2F; STA $00
    0x20, 0xa3, 0xfd, // E00D JSR IOINIT
    0x20, 0x15, 0xfd, // E010 JSR RESTOR
    0x58, // E013 CLI
    0x4c, 0x14, 0xe0, // E014 JMP $E014
];

const IRQ_DEFAULT: &[u8] = &[
    0xad, 0x0d, 0xdc, // EA31 LDA $DC0D (acknowledge)
    0x4c, 0x81, 0xea, // JMP $EA81
];

const IRQ_EXIT: &[u8] = &[0x68, 0xa8, 0x68, 0xaa, 0x68, 0x40];

const RESTOR: &[u8] = &[
    0xa9, 0x31, 0x8d, 0x14, 0x03, // $0314 = $EA31
    0xa9, 0xea, 0x8d, 0x15, 0x03, //
    0xa9, 0x66, 0x8d, 0x16, 0x03, // $0316 = $FE66
    0xa9, 0xfe, 0x8d, 0x17, 0x03, //
    0xa9, 0x47, 0x8d, 0x18, 0x03, // $0318 = $FE47
    0xa9, 0xfe, 0x8d, 0x19, 0x03, //
    0x60,
];

const IOINIT: &[u8] = &[
    0xa9, 0x7f, 0x8d, 0x0d, 0xdc, 0x8d, 0x0d, 0xdd, 0x8d, 0x00,
    0xdc, // interrupts off, keyboard
    0xa9, 0x08, 0x8d, 0x0e, 0xdc, 0x8d, 0x0e, 0xdd, // timers stopped, one-shot
    0x8d, 0x0f, 0xdc, 0x8d, 0x0f, 0xdd, //
    0xa2, 0x00, 0x8e, 0x03, 0xdc, 0x8e, 0x03, 0xdd, 0x8e, 0x18, 0xd4, // DDRB in, SID volume 0
    0xca, 0x8e, 0x02, 0xdc, // CIA1 DDRA out
    0xa9, 0x07, 0x8d, 0x00, 0xdd, 0xa9, 0x3f, 0x8d, 0x02, 0xdd, // VIC bank 0, IEC lines
    0xa9, 0xe7, 0x85, 0x01, 0xa9, 0x2f, 0x85, 0x00, // CPU port
    0xa9, 0x25, 0x8d, 0x04, 0xdc, 0xa9, 0x40, 0x8d, 0x05, 0xdc, // timer A = $4025
    0xa9, 0x81, 0x8d, 0x0d, 0xdc, // timer A interrupt on
    0xa9, 0x11, 0x8d, 0x0e, 0xdc, // start, force load
    0x60,
];

const NMI_ENTRY: &[u8] = &[0x78, 0x6c, 0x18, 0x03];

const IRQ_ENTRY: &[u8] = &[
    0x48, 0x8a, 0x48, 0x98, 0x48, // PHA; TXA; PHA; TYA; PHA
    0xba, 0xbd, 0x04, 0x01, // TSX; LDA $0104,X (pushed P)
    0x29, 0x10, 0xf0, 0x03, // AND #$10; BEQ +3
    0x6c, 0x16, 0x03, // JMP ($0316) — BRK
    0x6c, 0x14, 0x03, // JMP ($0314) — IRQ
];

pub fn kernal() -> Box<[u8; KERNAL_SIZE]> {
    let mut k = Box::new([0xff; KERNAL_SIZE]);
    let mut put = |addr: u16, code: &[u8]| {
        let at = usize::from(addr - 0xe000);
        k[at..at + code.len()].copy_from_slice(code);
    };
    put(0xe000, RESET);
    put(0xea31, IRQ_DEFAULT);
    put(0xea81, IRQ_EXIT);
    put(0xfd15, RESTOR);
    put(0xfda3, IOINIT);
    put(0xfe43, NMI_ENTRY);
    put(0xfe47, &[0x40]);
    put(0xfe66, &[0x4c, 0x81, 0xea]);
    put(0xff48, IRQ_ENTRY);
    put(0xff84, &[0x4c, 0xa3, 0xfd]);
    put(0xff8a, &[0x4c, 0x15, 0xfd]);
    put(0xffba, &[0x60]);
    put(0xffbd, &[0x60]);
    put(0xffd2, &[0x60]);
    put(0xffd5, &[0x38, 0x60]);
    put(0xffe4, &[0xa9, 0x00, 0x60]);
    put(0xfffa, &[0x43, 0xfe, 0x00, 0xe0, 0x48, 0xff]);
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routines_do_not_overlap() {
        let mut used = [false; KERNAL_SIZE];
        let blocks: [(u16, usize); 9] = [
            (0xe000, RESET.len()),
            (0xea31, IRQ_DEFAULT.len()),
            (0xea81, IRQ_EXIT.len()),
            (0xfd15, RESTOR.len()),
            (0xfda3, IOINIT.len()),
            (0xfe43, NMI_ENTRY.len()),
            (0xff48, IRQ_ENTRY.len()),
            (0xff84, 3),
            (0xff8a, 3),
        ];
        for (addr, len) in blocks {
            for i in 0..len {
                let at = usize::from(addr - 0xe000) + i;
                assert!(!used[at], "overlap at ${:04X}", addr as usize + i);
                used[at] = true;
            }
        }
    }
}
