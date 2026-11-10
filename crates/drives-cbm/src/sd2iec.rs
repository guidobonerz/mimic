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
//! - Host letters of either case show as plain (unshifted) PETSCII
//!   letters; a shifted letter the C64 writes becomes upper case; long
//!   names appear as an 8.3-style alias; x00 files (`.P00` …) show their
//!   internal name; `XE+` hides `.PRG/.SEQ/.USR/.REL` and takes the type
//!   from it; `XE0`–`XE4` choose how written files are stored (default 1:
//!   SEQ/USR/REL as x00, PRG as is).
//! - Commands: `S`, `R`, `C` (copy/append), `N`, `I`, `V`, `UI`, `UJ`,
//!   `U0>`+device, `X`, `XE`, `X*`, `X?`, `XI`, `XD`, `XR`, `XS`, `XW` (no
//!   EEPROM: settings last until power-off), `M-W` to the 1541's device
//!   address `$77`, `T-R`/`T-W` (no RTC: `30,SYNTAX ERROR`).
//! - Direct access in a mounted image: `#`/`##n` buffers, `U1`/`U2`,
//!   `B-R`/`B-W`, `B-P`, `B-A`/`B-F` ([`dos`]'s `direct`).
//! - REL files in the folder with `P`, `P` on plain files (`rel`).
//! - Swap lists (`XS`, `AUTOSWAP.LST`, `AUTOSWAP.GEN`) and the NEXT/PREV
//!   buttons ([`Sd2iec::button`], media slot controls `next`, `prev`,
//!   `home`) (`swap`).
//! - `M-R`: an `XR` drive ROM file, or a table with the DOS version
//!   strings for drive detection (`memory`).
//! - Not emulated: the `D` (raw card sector) commands, `M-R` of buffer
//!   memory, REL files inside images, `,M` modify mode, fast loaders.
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

pub use dos::{Sd2iecDos, SwapButton};

use crate::iec::IecLines;
use crate::virtual_drive::IecProtocol;

/// The protocol runs in microseconds.
const DEVICE_HZ: u32 = 1_000_000;

const TAG: [u8; 4] = *b"SD2I";
const VERSION: u16 = 2;

pub struct Sd2iec {
    pub dos: Sd2iecDos,
    proto: IecProtocol,
    clock: ClockDivider,
    /// NEXT and PREV held.
    buttons: [bool; 2],
}

impl Sd2iec {
    /// An SD2IEC as device `device` (4–30) next to a host clocked at
    /// `host_hz`.
    pub fn new(device: u8, host_hz: u32) -> Self {
        Self {
            dos: Sd2iecDos::new(device),
            proto: IecProtocol::default(),
            clock: ClockDivider::new(DEVICE_HZ, host_hz).expect("non-zero clock"),
            buttons: [false; 2],
        }
    }

    /// The disk change buttons: `prev` false = NEXT, true = PREV. A press
    /// steps the swap list; pressing one while the other is held is HOME.
    pub fn button(&mut self, prev: bool, pressed: bool) {
        let (me, other) = (usize::from(prev), usize::from(!prev));
        let was = self.buttons[me];
        self.buttons[me] = pressed;
        if pressed && !was {
            self.dos.swap_button(match (self.buttons[other], prev) {
                (true, _) => SwapButton::Home,
                (false, false) => SwapButton::Next,
                (false, true) => SwapButton::Prev,
            });
        }
    }

    /// Whether NEXT (`false`) or PREV (`true`) is held.
    pub fn button_held(&self, prev: bool) -> bool {
        self.buttons[usize::from(prev)]
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
            w.bool(self.buttons[0]);
            w.bool(self.buttons[1]);
            self.dos.save(w);
        });
    }

    /// Atomic: nothing changes unless the whole chunk loads.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let mut proto = IecProtocol::default();
        proto.load(&mut c)?;
        let mut clock = self.clock;
        clock.load(&mut c)?;
        // The DOS last (it changes only when it loads completely).
        let buttons = if version >= 2 {
            [c.bool()?, c.bool()?]
        } else {
            [false; 2]
        };
        self.dos.load(&mut c, version)?;
        self.proto = proto;
        self.clock = clock;
        self.buttons = buttons;
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

    /// The disk change buttons: `next`, `prev`, `home` (both).
    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        let button = match command {
            "next" => SwapButton::Next,
            "prev" => SwapButton::Prev,
            "home" => SwapButton::Home,
            _ => {
                return Err(MediaError::Control(format!(
                    "'{command}': the SD2IEC has next, prev and home"
                )))
            }
        };
        self.dos.swap_button(button);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
