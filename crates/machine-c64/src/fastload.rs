//! Fast loading from the virtual drive (`options["fastload"] = "on"`):
//! KERNAL serial traps.
//!
//! The KERNAL moves every byte over the serial bus bit by bit with its own
//! timing loops (about 400 bytes a second with a 1541), and the virtual
//! drive answers at that pace. With fast loading the machine catches the
//! KERNAL's serial routines as the CPU is about to execute them and hands
//! the byte straight to the virtual drive's DOS ([`VirtualDos`]) instead:
//! LOAD, SAVE, OPEN/GET#/PRINT# and the command channel take no bus time.
//! The KERNAL's own code around them still runs (file name, messages, the
//! status byte ST at `$90`), so BASIC and programs see a normal load.
//! Fast loaders and anything that drives the bus itself are not affected;
//! they need the true drive anyway.
//!
//! Trapped entries (C64 KERNAL, jump table `$FF93–$FFB4`; Commodore 64
//! Programmer's Reference Guide, chapter 5 "KERNAL"; addresses and
//! register effects from the ROM 901227-03):
//!
//! | Routine | Entry | In | Effect |
//! | --- | --- | --- | --- |
//! | TALK | `$ED09` | A = device | `$40+n` under ATN |
//! | LISTEN | `$ED0C` | A = device | `$20+n` under ATN |
//! | SECOND | `$EDB9` | A = `$60+sa` … | sent under ATN, then ATN released |
//! | (ATN off) | `$EDBE` | | ATN released (OPEN without secondary address) |
//! | TKSA | `$EDC7` | A = `$60+sa` | sent under ATN, ATN released, turnaround |
//! | CIOUT | `$EDDD` | A = byte | byte to the listener |
//! | UNTLK | `$EDEF` | | `$5F` under ATN |
//! | UNLSN | `$EDFE` | | `$3F` under ATN |
//! | ACPTR | `$EE13` | | A = byte from the talker; ST bit 6 on the last, bit 1 if none |
//!
//! A trap only fires while the KERNAL ROM is mapped at the PC, the ROM's
//! bytes at all entries are those of the stock KERNAL (JiffyDOS and other
//! replacements run their own code), drive 8 is the virtual drive and,
//! for TALK/LISTEN, the device is 8 (other devices use the real bus).
//! After TALK/LISTEN for drive 8 the rest of that exchange is trapped
//! too, up to UNTLK/UNLSN. Each trap ends like the routine: RTS, carry
//! clear, and interrupts enabled except after CIOUT (the routines end
//! with CLI; CIOUT only buffers its byte).

use drives_cbm::dos::VirtualDos;

/// The device number the traps serve (drive 8).
pub const DEVICE: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    Talk,
    Listen,
    Second,
    AtnOff,
    Tksa,
    Ciout,
    Untlk,
    Unlsn,
    Acptr,
}

/// Entry address, trap and the stock KERNAL's first bytes there.
const ENTRIES: [(u16, Trap, [u8; 4]); 9] = [
    (0xed09, Trap::Talk, [0x09, 0x40, 0x2c, 0x09]),
    (0xed0c, Trap::Listen, [0x09, 0x20, 0x20, 0xa4]),
    (0xedb9, Trap::Second, [0x85, 0x95, 0x20, 0x36]),
    (0xedbe, Trap::AtnOff, [0xad, 0x00, 0xdd, 0x29]),
    (0xedc7, Trap::Tksa, [0x85, 0x95, 0x20, 0x36]),
    (0xeddd, Trap::Ciout, [0x24, 0x94, 0x30, 0x05]),
    (0xedef, Trap::Untlk, [0x78, 0x20, 0x8e, 0xee]),
    (0xedfe, Trap::Unlsn, [0xa9, 0x3f, 0x20, 0x11]),
    (0xee13, Trap::Acptr, [0x78, 0xa9, 0x00, 0x85]),
];

/// The trap for an opcode fetch at `pc`, if any.
#[inline]
pub fn trap_at(pc: u16) -> Option<Trap> {
    if !(0xed09..=0xee13).contains(&pc) {
        return None;
    }
    ENTRIES.iter().find(|e| e.0 == pc).map(|e| e.1)
}

/// Is `kernal` (8 KB at `$E000`) the stock KERNAL at every entry?
pub fn kernal_matches(kernal: &[u8]) -> bool {
    ENTRIES.iter().all(|(addr, _, bytes)| {
        let at = usize::from(addr - 0xe000);
        kernal.get(at..at + 4) == Some(&bytes[..])
    })
}

/// What a trap does to the CPU besides returning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// New A, if the routine returns a value.
    pub a: Option<u8>,
    /// Bits to OR into the status byte ST (`$90`).
    pub status: u8,
    /// Clear the interrupt disable flag (CLI).
    pub cli: bool,
}

/// Run `trap` against the DOS. `serving` is true while an exchange with
/// drive 8 is in progress (after a trapped TALK/LISTEN). Returns `None` if
/// the routine must run normally (another device, or no exchange with
/// drive 8).
pub fn run(trap: Trap, a: u8, serving: &mut bool, dos: &mut VirtualDos) -> Option<Outcome> {
    let done = Outcome {
        cli: true,
        ..Outcome::default()
    };
    match trap {
        Trap::Talk | Trap::Listen => {
            if a & 0x1f != DEVICE {
                return None;
            }
            *serving = true;
            dos.set_atn(true);
            dos.received(if trap == Trap::Talk { 0x40 } else { 0x20 } | DEVICE, false);
            Some(done)
        }
        _ if !*serving => None,
        Trap::Second | Trap::Tksa => {
            dos.received(a, false);
            dos.set_atn(false);
            Some(done)
        }
        Trap::AtnOff => {
            dos.set_atn(false);
            Some(done)
        }
        Trap::Ciout => {
            dos.received(a, false);
            Some(Outcome::default())
        }
        Trap::Untlk | Trap::Unlsn => {
            dos.set_atn(true);
            dos.received(if trap == Trap::Untlk { 0x5f } else { 0x3f }, false);
            dos.set_atn(false);
            *serving = false;
            Some(done)
        }
        Trap::Acptr => match dos.talking().then(|| dos.next_byte()).flatten() {
            Some((byte, last)) => {
                dos.advance();
                Some(Outcome {
                    a: Some(byte),
                    status: if last { 0x40 } else { 0 },
                    cli: true,
                })
            }
            // Nothing to send (file not found, read past the end): the
            // KERNAL's read timeout.
            None => {
                dos.stop_talking();
                Some(Outcome {
                    a: Some(0),
                    status: 0x02,
                    cli: true,
                })
            }
        },
    }
}
