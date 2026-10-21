//! Virtual IEEE-488 drive: a PET disk drive at the bus handshake level,
//! serving files from a D64, D71, D81, D80 or D82 image through
//! [`VirtualDos`], with no drive CPU or DOS ROM. Programs that use the
//! KERNAL's IEEE routines (LOAD, SAVE, OPEN/PRINT#/GET#, DS$) work.
//!
//! Handshake (PET KERNAL `$F109` send, `$F1C0` receive; IEEE-488): the
//! talker waits until no listener holds NRFD, puts the byte on DIO1–8
//! (negative logic) with EOI on the last one and asserts DAV; each
//! listener pulls NRFD, takes the byte and releases NDAC; when NDAC is free
//! the talker releases DAV and the listeners pull NDAC again and release
//! NRFD. While ATN is asserted every device listens (commands LISTEN
//! `$20+n`, UNLISTEN `$3F`, TALK `$40+n`, UNTALK `$5F`, secondary `$60`,
//! CLOSE `$E0`, OPEN `$F0`); a device that holds neither NRFD nor NDAC is
//! not present. After TALK the controller releases ATN and listens.
//!
//! The talker sends only after the listeners have shown ready (NDAC held,
//! NRFD free) for [`READY_US`]: entering ATN the PET frees NRFD a few
//! microseconds before NDAC (KERNAL `$F0D8`), and a byte put on the bus in
//! that window would look accepted without being read.
//!
//! [`IeeeDrive::tick`] is one microsecond.

use emu_core::media::{DriveStatus, Media, MediaError, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};

use crate::dos::VirtualDos;
use crate::ieee::IeeeLines;

/// How long the listeners must stay ready before the talker sends.
pub const READY_US: u8 = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Not addressed: lines released.
    #[default]
    Idle,
    /// Listener ready: NDAC pulled, NRFD released, waiting for DAV.
    Ready,
    /// Listener took a byte: NRFD pulled, NDAC released, waiting for the
    /// talker to release DAV.
    Accepted,
    /// Talker: waiting until the listeners are ready.
    TalkWaitReady,
    /// Talker: DAV asserted, waiting until the byte is accepted.
    TalkWaitAccept,
}

pub struct IeeeDrive {
    /// Channels, files and commands.
    pub dos: VirtualDos,
    /// Lines this drive pulls.
    out: IeeeLines,
    state: State,
    prev_atn: bool,
    /// The byte on the bus is the last of the channel.
    last: bool,
    /// Microseconds the listeners have been ready.
    ready: u8,
}

impl IeeeDrive {
    pub fn new(device: u8) -> Self {
        Self {
            dos: VirtualDos::new(device, "MIMIC IEEE DRIVE"),
            out: IeeeLines::default(),
            state: State::Idle,
            prev_atn: false,
            last: false,
            ready: 0,
        }
    }

    /// RESET (the PET's reset line reaches the drive): bus released,
    /// channels closed, power-on status.
    pub fn reset(&mut self) {
        self.out = IeeeLines::default();
        self.state = State::Idle;
        self.prev_atn = false;
        self.last = false;
        self.dos.reset();
    }

    pub fn has_disk(&self) -> bool {
        self.dos.has_disk()
    }

    pub fn status_text(&self) -> &str {
        self.dos.status_text()
    }

    /// Lines this drive pulls.
    pub fn ieee_out(&self) -> IeeeLines {
        self.out
    }

    fn listen(&mut self) {
        self.out = IeeeLines {
            ndac: true,
            ..IeeeLines::default()
        };
        self.state = State::Ready;
    }

    fn release(&mut self) {
        self.out = IeeeLines::default();
        self.state = State::Idle;
    }

    /// One microsecond with the bus levels as they are now (`true` = low).
    pub fn tick(&mut self, bus: IeeeLines) {
        if bus.atn != self.prev_atn {
            self.prev_atn = bus.atn;
            self.dos.set_atn(bus.atn);
            if bus.atn {
                // Every device listens to the commands.
                self.listen();
            } else if self.dos.talking() {
                self.out = IeeeLines::default();
                self.state = State::TalkWaitReady;
            } else if self.dos.listening() {
                self.listen();
            } else {
                self.release();
            }
            return;
        }
        match self.state {
            State::Idle => {}
            State::Ready => {
                if bus.dav {
                    self.out.nrfd = true;
                    self.out.ndac = false;
                    self.state = State::Accepted;
                    self.dos.received(bus.data, bus.eoi);
                }
            }
            State::Accepted => {
                if !bus.dav {
                    self.out.ndac = true;
                    self.out.nrfd = false;
                    self.state = State::Ready;
                }
            }
            State::TalkWaitReady => {
                // Listeners present (NDAC held) and all ready (NRFD free),
                // for a while.
                self.ready = if bus.ndac && !bus.nrfd {
                    self.ready.saturating_add(1)
                } else {
                    0
                };
                if self.ready >= READY_US {
                    self.ready = 0;
                    match self.dos.next_byte() {
                        Some((byte, last)) => {
                            self.out.data = byte;
                            self.out.eoi = last;
                            self.out.dav = true;
                            self.last = last;
                            self.state = State::TalkWaitAccept;
                        }
                        // Nothing to send (file not found, end): the PET
                        // times out.
                        None => {
                            self.dos.stop_talking();
                            self.release();
                        }
                    }
                }
            }
            State::TalkWaitAccept => {
                if !bus.ndac {
                    self.out = IeeeLines::default();
                    self.dos.advance();
                    self.state = if self.last {
                        State::Idle
                    } else {
                        State::TalkWaitReady
                    };
                }
            }
        }
    }
}

impl MediaSlot for IeeeDrive {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        self.dos.insert(media)
    }

    fn eject(&mut self) -> Option<Media> {
        self.dos.eject()
    }

    fn write_protect(&mut self, on: bool) {
        self.dos.write_protect(on);
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.dos.flush()
    }

    /// From the [`VirtualDos`]: bus, open channels, error channel.
    fn status(&self) -> DriveStatus {
        self.dos.status()
    }
}

const TAG: [u8; 4] = *b"IEEE";
const VERSION: u16 = 1;

impl Savestate for IeeeDrive {
    /// The image is media, not state; open channels are saved.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.state as u8);
            let o = self.out;
            for b in [
                o.atn,
                o.eoi,
                o.dav,
                o.nrfd,
                o.ndac,
                self.prev_atn,
                self.last,
            ] {
                w.bool(b);
            }
            w.u8(o.data);
            w.u8(self.ready);
            self.dos.save_fields(w);
        });
    }

    /// Atomic: nothing changes unless the whole chunk is valid.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut d = IeeeDrive::new(self.dos.device());
        d.state = match c.u8()? {
            0 => State::Idle,
            1 => State::Ready,
            2 => State::Accepted,
            3 => State::TalkWaitReady,
            4 => State::TalkWaitAccept,
            _ => return Err(StateError::Invalid("IEEE drive state")),
        };
        d.out = IeeeLines {
            atn: c.bool()?,
            eoi: c.bool()?,
            dav: c.bool()?,
            nrfd: c.bool()?,
            ndac: c.bool()?,
            data: 0,
        };
        d.prev_atn = c.bool()?;
        d.last = c.bool()?;
        d.out.data = c.u8()?;
        d.ready = c.u8()?;
        d.dos.load_fields(&mut c)?;
        d.dos.disk = self.dos.disk.take();
        *self = d;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
