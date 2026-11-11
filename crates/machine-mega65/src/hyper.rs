//! The hypervisor services in direct start (the user's choice: no Hyppo
//! yet). A program writes A to a trap register (`$D640` + n, MEGA65 I/O
//! personality); the 45GS02 enters the hypervisor, which performs the
//! service and returns: C set on success, else C clear and an error code
//! in A; registers a service does not return are kept. Here the machine
//! runs the service after the writing instruction and the CPU waits
//! [`TRAP_CLOCKS`] master clocks.
//!
//! Services (Hyppo 1.2 as the guide describes it):
//! - `$D640`: `$00` getversion (A.X Hyppo 1.2, Y.Z HDOS 1.2), `$38`
//!   geterrorcode, `$3A` setup_transfer_area (Y ≤ `$7E`), `$70`
//!   toggle_rom_writeprotect (A bit 2: protected), `$72` toggle_force_4502
//!   (only the 45GS02 personality is there: A bit 5 set), `$74`
//!   get_mapping / `$76` set_mapping (6 bytes at `$YY00`: MAPLO and MAPHI
//!   big-endian, then the two megabytes), `$7C` serial_monitor_write (Y),
//!   `$7E` reset (warm boot); the SD card, disk image and task services
//!   answer "not implemented" (`$82`) until there is an SD card (milestone
//!   6); others "no such trap" (`$FF`).
//! - `$D641`: `$00` rom_writeprotect, `$02` rom_writeenable.
//! - `$D642`: `$06` dmagic_autoset (the F018B format for the MEGA65 ROMs);
//!   the configuration sector and freezer services "not implemented".
//! - `$D643`: serial_monitor_wait_and_write (A).
//! - `$D67F`: freeze_self: no freezer, "not implemented".
//!
//! Source: the MEGA65 user guide, `appendix-hypervisor-calls.tex` (the
//! calling convention, the services, the error codes). The implementation
//! is this project's own.

use cpu_45gs02::flag;

use crate::Mega65;

/// The master clocks a trap keeps the CPU (entering the hypervisor, the
/// service, the return): an estimate, to be measured.
pub const TRAP_CLOCKS: u32 = 256;

/// Error codes.
pub mod error {
    pub const INVALID_ADDRESS: u8 = 0x10;
    pub const ILLEGAL_VALUE: u8 = 0x11;
    pub const NOT_IMPLEMENTED: u8 = 0x82;
    pub const NO_SUCH_TRAP: u8 = 0xff;
}

/// The hypervisor's state a program can see.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hyper {
    /// The transfer area's page (setup_transfer_area).
    pub transfer: Option<u8>,
    /// The last error code.
    pub error: u8,
}

/// Characters written to the serial monitor kept at most.
const SERIAL_KEEP: usize = 4096;

impl Mega65 {
    /// Run the service of trap `trap` (0–63) called with A = `a`.
    pub(crate) fn hypervisor(&mut self, trap: u8, a: u8) {
        let result = match (trap, a) {
            (0x00, 0x00) => {
                self.cpu.a = 1;
                self.cpu.x = 2;
                self.cpu.y = 1;
                self.cpu.z = 2;
                Ok(())
            }
            (0x00, 0x38) => {
                self.cpu.a = self.hyper.error;
                Ok(())
            }
            (0x00, 0x3a) => {
                let y = self.cpu.y;
                if y > 0x7e {
                    Err(error::INVALID_ADDRESS)
                } else {
                    self.hyper.transfer = Some(y);
                    Ok(())
                }
            }
            (0x00, 0x70) => {
                let b = &mut self.board;
                b.rom_protect = !b.rom_protect;
                self.cpu.a = if b.rom_protect { 0x04 } else { 0x00 };
                Ok(())
            }
            (0x00, 0x72) => {
                // Only the 45GS02 personality exists here.
                self.cpu.a = 0x20;
                Ok(())
            }
            (0x00, 0x74) => self.get_mapping(),
            (0x00, 0x76) => self.set_mapping(),
            (0x00, 0x7c) => {
                self.serial(self.cpu.y);
                Ok(())
            }
            (0x00, 0x7e) => {
                use emu_core::Machine;
                self.reset(emu_core::ResetKind::Soft);
                return;
            }
            // SD card, disk image, task services.
            (0x00, 0x02..=0x36 | 0x3c | 0x3e | 0x40..=0x4a | 0x50..=0x6e) => {
                Err(error::NOT_IMPLEMENTED)
            }
            (0x01, 0x00) => {
                self.board.rom_protect = true;
                Ok(())
            }
            (0x01, 0x02) => {
                self.board.rom_protect = false;
                Ok(())
            }
            (0x02, 0x06) => {
                self.board.devices[0x103] |= 1;
                Ok(())
            }
            (0x02, 0x00..=0x04 | 0x10..=0x16) => Err(error::NOT_IMPLEMENTED),
            (0x03, byte) => {
                self.serial(byte);
                Ok(())
            }
            (0x3f, _) => Err(error::NOT_IMPLEMENTED),
            _ => Err(error::NO_SUCH_TRAP),
        };
        match result {
            Ok(()) => self.cpu.p |= flag::C,
            Err(e) => {
                self.cpu.p &= !flag::C;
                self.cpu.a = e;
                self.hyper.error = e;
            }
        }
    }

    fn serial(&mut self, byte: u8) {
        if self.serial_out.len() < SERIAL_KEEP {
            self.serial_out.push(byte);
        }
    }

    /// A byte at a CPU address (through the current map).
    fn cpu_byte(&mut self, a: u16) -> u8 {
        self.board.read(self.cpu.map.translate(a))
    }

    fn get_mapping(&mut self) -> Result<(), u8> {
        let y = self.cpu.y;
        if y > 0x7e {
            return Err(error::INVALID_ADDRESS);
        }
        let m = self.cpu.map;
        let word = |mask: u8, offset: u32| (u16::from(mask) << 12) | (offset >> 8) as u16;
        let lo = word(m.lo_mask, m.lo_offset);
        let hi = word(m.hi_mask, m.hi_offset);
        let bytes = [
            (lo >> 8) as u8,
            lo as u8,
            (hi >> 8) as u8,
            hi as u8,
            m.lo_mb,
            m.hi_mb,
        ];
        let base = u16::from(y) << 8;
        for (i, b) in bytes.into_iter().enumerate() {
            let at = self.cpu.map.translate(base + i as u16);
            self.board.write(at, b);
        }
        Ok(())
    }

    fn set_mapping(&mut self) -> Result<(), u8> {
        let y = self.cpu.y;
        if y > 0x7e {
            return Err(error::INVALID_ADDRESS);
        }
        let base = u16::from(y) << 8;
        let b: Vec<u8> = (0..6).map(|i| self.cpu_byte(base + i)).collect();
        let lo = u16::from_be_bytes([b[0], b[1]]);
        let hi = u16::from_be_bytes([b[2], b[3]]);
        let m = &mut self.cpu.map;
        m.lo_mask = (lo >> 12) as u8;
        m.lo_offset = u32::from(lo & 0x0fff) << 8;
        m.hi_mask = (hi >> 12) as u8;
        m.hi_offset = u32::from(hi & 0x0fff) << 8;
        m.lo_mb = b[4];
        m.hi_mb = b[5];
        Ok(())
    }
}
