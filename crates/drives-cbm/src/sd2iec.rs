//! SD2IEC: a serial bus mass storage device with the files of a folder on
//! its "SD card", next to (or instead of) the computer's disk drive.
//!
//! The device speaks the standard Commodore serial bus protocol at line
//! level ([`IecProtocol`], shared with the virtual drive); its storage is
//! the sandboxed folder the frontend lends the machine
//! ([`emu_core::host::HostServices::files`], recorded in the host journal,
//! so runs stay deterministic). Without a folder it answers
//! `74,DRIVE NOT READY`. It runs no fast loader (no JiffyDOS either): LOAD,
//! SAVE, OPEN/PRINT#/GET# and the command channel work at KERNAL speed.
//!
//! Behaviour after the sd2iec firmware's documentation (README of sd2iec
//! 1.0, Ingo Korb et al., <https://www.sd2iec.de>; observable behaviour
//! only, no firmware code):
//! - Subdirectories with the CMD-style syntax: `CD:name`, `CD/a/:b`,
//!   `CD//` (root), `CD:←`/`CD←` (parent, `←` = `$5F`), `MD`, `RD`; paths
//!   in file names, `LOAD"//GAMES/:ELITE",9`; partition digits are
//!   ignored (one partition), `CP`, `$=P`.
//! - Disk images (`.D64`, `.D41`, `.D71`, `.D81`) are mounted by changing
//!   into them and left with `CD:←`; inside one the files of the image are
//!   served, `N:` formats it. `N:name.D64,id` outside creates an image.
//! - Host names swap letter case (host lower case = C64 upper case); long
//!   names appear as an 8.3-style alias; x00 files (`.P00` …) show their
//!   internal name; `XE+` hides `.PRG/.SEQ/.USR/.REL` and takes the type
//!   from it; `XE0`–`XE4` choose how written files are stored (default 1:
//!   SEQ/USR/REL as x00, PRG as is).
//! - Commands: `S`, `R`, `C` (copy/append), `N`, `I`, `V`, `UI`, `UJ`,
//!   `U0>`+device, `X`, `XE`, `X*`, `X?`, `XW` (no EEPROM: settings last
//!   until power-off), `M-W` to the 1541's device address `$77`, `M-R`
//!   (answers zeros), `T-R`/`T-W` (no RTC: `30,SYNTAX ERROR`).
//! - Not emulated: direct access (`#` buffers, `U1`/`U2`, `B-P`, `D`
//!   commands), REL records and `P`, swap lists and the disk change
//!   buttons, `XR`/`XI`/`XD`, fast loaders.
//!
//! A disk image inserted into the device's media slot is mounted until it
//! is ejected (a convenience: the real device has no slot).
//!
//! [`Sd2iec::run`] advances the device by one host cycle.

pub mod dos;
pub mod names;

use emu_core::host::SharedFiles;
use emu_core::media::{DriveStatus, Media, MediaError, MediaSlot};
use emu_core::{ClockDivider, Savestate, StateError, StateReader, StateWriter};

pub use dos::Sd2iecDos;

use crate::iec::IecLines;
use crate::virtual_drive::IecProtocol;

/// The protocol runs in microseconds.
const DEVICE_HZ: u32 = 1_000_000;

const TAG: [u8; 4] = *b"SD2I";
const VERSION: u16 = 1;

pub struct Sd2iec {
    pub dos: Sd2iecDos,
    proto: IecProtocol,
    clock: ClockDivider,
}

impl Sd2iec {
    /// An SD2IEC as device `device` (4–30) next to a host clocked at
    /// `host_hz`.
    pub fn new(device: u8, host_hz: u32) -> Self {
        Self {
            dos: Sd2iecDos::new(device),
            proto: IecProtocol::default(),
            clock: ClockDivider::new(DEVICE_HZ, host_hz).expect("non-zero clock"),
        }
    }

    /// The device number (it changes with `U0>` and `M-W`).
    pub fn device(&self) -> u8 {
        self.dos.device()
    }

    /// The folder that serves as the card (`None`: no card).
    pub fn attach_files(&mut self, files: Option<SharedFiles>) {
        self.dos.attach_files(files);
    }

    /// RESET from the serial bus; `hard` = power cycle (back to the root
    /// directory, settings to their defaults).
    pub fn reset(&mut self, hard: bool) {
        self.proto = IecProtocol::default();
        if hard {
            self.dos.power_on();
        } else {
            self.dos.reset();
        }
    }

    /// The command channel's current message.
    pub fn status_text(&self) -> &str {
        self.dos.status_text()
    }

    /// Lines the device pulls on the bus.
    pub fn iec_out(&self) -> IecLines {
        self.proto.iec_out()
    }

    /// One host cycle: `others` are the lines everyone else pulls. Returns
    /// the bus levels afterwards.
    pub fn run(&mut self, others: IecLines) -> IecLines {
        for _ in 0..self.clock.tick() {
            let bus = IecLines::combine(&[others, self.proto.iec_out()]);
            self.proto.tick(bus, &mut self.dos);
        }
        IecLines::combine(&[others, self.proto.iec_out()])
    }
}

impl Savestate for Sd2iec {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            self.proto.save(w);
            self.clock.save(w);
            self.dos.save(w);
        });
    }

    /// Atomic: nothing changes unless the whole chunk loads.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut proto = IecProtocol::default();
        proto.load(&mut c)?;
        let mut clock = self.clock;
        clock.load(&mut c)?;
        // The DOS last (it changes only when it loads completely).
        self.dos.load(&mut c)?;
        self.proto = proto;
        self.clock = clock;
        Ok(())
    }
}

impl MediaSlot for Sd2iec {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        self.dos.insert(media)
    }

    fn eject(&mut self) -> Option<Media> {
        self.dos.eject()
    }

    fn write_protect(&mut self, on: bool) {
        self.dos.protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.dos.flush()
    }

    fn dirty(&self) -> bool {
        self.dos.dirty()
    }

    fn status(&self) -> DriveStatus {
        self.dos.drive_status()
    }
}

#[cfg(test)]
mod tests;
