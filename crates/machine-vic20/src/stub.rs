//! Built-in KERNAL replacement for runs without Commodore ROMs
//! (`options["roms"] = "stub"`): tests and CI.
//!
//! Written for this project, not derived from the Commodore KERNAL. It
//! sets up the chips like the PAL KERNAL does (22 × 23 text screen at
//! `$1E00`, VIA2 timer 1 as the 60 Hz interrupt, RESTORE on VIA1 CA1) and
//! idles; the documented jump table entries programs call are present with
//! minimal behaviour:
//!
//! | Address | Real routine | Stub behaviour                      |
//! | ------- | ------------ | ----------------------------------- |
//! | `$FFBA`, `$FFBD` | SETLFS, SETNAM | RTS                       |
//! | `$FFD2` | BSOUT        | RTS (callers observe it with a breakpoint) |
//! | `$FFD5` | LOAD         | SEC, RTS (error)                    |
//! | `$FFE4` | GETIN        | A = 0, RTS                          |
//!
//! Interrupts dispatch through the RAM vectors `$0314` (IRQ), `$0316`
//! (BRK) and `$0318` (NMI) like the real KERNAL's.

use crate::memory::KERNAL_SIZE;

/// Address of the idle loop after reset.
pub const IDLE: u16 = 0xe00c;

const RESET: &[u8] = &[
    0x78, // E000 SEI
    0xa2, 0xff, // LDX #$FF
    0x9a, // TXS
    0xd8, // CLD
    0x20, 0x00, 0xe1, // JSR IOINIT
    0x20, 0x80, 0xe1, // JSR RESTOR
    0x58, // CLI
    0x4c, 0x0c, 0xe0, // E00C JMP $E00C
];

const IOINIT: &[u8] = &[
    0xa2, 0x0f, // E100 LDX #$0F
    0xbd, 0x40, 0xe1, // LDA VIC_REGS,X
    0x9d, 0x00, 0x90, // STA $9000,X
    0xca, // DEX
    0x10, 0xf7, // BPL E102
    0xa9, 0x80, 0x8d, 0x13, 0x91, // VIA1 DDRA: PA7 (ATN) out
    0xa9, 0x00, 0x8d, 0x1f, 0x91, // ATN released
    0xa9, 0xff, 0x8d, 0x22, 0x91, // VIA2 DDRB: keyboard columns out
    0xa9, 0x00, 0x8d, 0x23, 0x91, // VIA2 DDRA: rows in
    0xa9, 0xdc, 0x8d, 0x2c, 0x91, // VIA2 PCR: CA2, CB2 low (CLK, DATA released)
    0xa9, 0x40, 0x8d, 0x2b, 0x91, // VIA2 ACR: timer 1 free-running
    0xa9, 0x26, 0x8d, 0x24, 0x91, // timer 1 = $4826 (60 Hz at PAL clock)
    0xa9, 0x48, 0x8d, 0x25, 0x91, //
    0xa9, 0xc0, 0x8d, 0x2e, 0x91, // VIA2 IER: timer 1
    0xa9, 0x82, 0x8d, 0x1e, 0x91, // VIA1 IER: CA1 (RESTORE)
    0x60,
];

/// VIC registers `$9000–$900F`: the PAL KERNAL's screen set-up.
const VIC_REGS: [u8; 16] = [
    0x0c, 0x26, 0x96, 0x2e, 0x00, 0xf0, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1b,
];

const RESTOR: &[u8] = &[
    0xa9, 0xc0, 0x8d, 0x14, 0x03, 0xa9, 0xe1, 0x8d, 0x15, 0x03, // $0314 = $E1C0
    0xa9, 0xd0, 0x8d, 0x16, 0x03, 0xa9, 0xe1, 0x8d, 0x17, 0x03, // $0316 = $E1D0
    0xa9, 0xe8, 0x8d, 0x18, 0x03, 0xa9, 0xe1, 0x8d, 0x19, 0x03, // $0318 = $E1E8
    0x60,
];

const IRQ_DEFAULT: &[u8] = &[
    0xad, 0x24, 0x91, // E1C0 LDA $9124 (acknowledge timer 1)
    0x4c, 0xd0, 0xe1, // JMP IRQ_EXIT
];

const IRQ_EXIT: &[u8] = &[0x68, 0xa8, 0x68, 0xaa, 0x68, 0x40];

const NMI_ENTRY: &[u8] = &[0x78, 0x6c, 0x18, 0x03];

/// Acknowledge CA1 by reading port A.
const NMI_DEFAULT: &[u8] = &[0x48, 0xad, 0x11, 0x91, 0x68, 0x40];

const IRQ_ENTRY: &[u8] = &[
    0x48, 0x8a, 0x48, 0x98, 0x48, // PHA; TXA; PHA; TYA; PHA
    0xba, 0xbd, 0x04, 0x01, // TSX; LDA $0104,X (pushed P)
    0x29, 0x10, 0xf0, 0x03, // AND #$10; BEQ +3
    0x6c, 0x16, 0x03, // JMP ($0316) — BRK
    0x6c, 0x14, 0x03, // JMP ($0314) — IRQ
];

const BLOCKS: [(u16, &[u8]); 9] = [
    (0xe000, RESET),
    (0xe100, IOINIT),
    (0xe140, &VIC_REGS),
    (0xe180, RESTOR),
    (0xe1c0, IRQ_DEFAULT),
    (0xe1d0, IRQ_EXIT),
    (0xe1e0, NMI_ENTRY),
    (0xe1e8, NMI_DEFAULT),
    (0xe1f0, IRQ_ENTRY),
];

pub fn kernal() -> Box<[u8; KERNAL_SIZE]> {
    let mut k = Box::new([0xff; KERNAL_SIZE]);
    let mut put = |addr: u16, code: &[u8]| {
        let at = usize::from(addr - 0xe000);
        k[at..at + code.len()].copy_from_slice(code);
    };
    for (addr, code) in BLOCKS {
        put(addr, code);
    }
    put(0xffba, &[0x60]);
    put(0xffbd, &[0x60]);
    put(0xffd2, &[0x60]);
    put(0xffd5, &[0x38, 0x60]);
    put(0xffe4, &[0xa9, 0x00, 0x60]);
    put(0xfffa, &[0xe0, 0xe1, 0x00, 0xe0, 0xf0, 0xe1]);
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routines_do_not_overlap() {
        let mut used = [false; KERNAL_SIZE];
        for (addr, code) in BLOCKS {
            for i in 0..code.len() {
                let at = usize::from(addr - 0xe000) + i;
                assert!(!used[at], "overlap at ${:04X}", addr as usize + i);
                used[at] = true;
            }
        }
    }
}
