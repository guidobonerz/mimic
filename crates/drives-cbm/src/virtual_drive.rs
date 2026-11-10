//! Virtual drive: a serial bus device that speaks the Commodore IEC
//! protocol at line level and serves files from a D64, D71 or D81 image
//! directly (only the root directory of a D81, no partitions), with
//! no drive CPU, DOS ROM or disk mechanics. Programs that only use the
//! KERNAL's serial routines (LOAD, SAVE, OPEN/PRINT#/GET#, the command
//! channel) work and load faster; fast loaders and copy protection need the
//! true drive.
//!
//! Protocol (Commodore 64 Programmer's Reference Guide, serial bus; "Inside
//! the 1541"): the host pulls ATN and sends command bytes (LISTEN `$20+n`,
//! UNLISTEN `$3F`, TALK `$40+n`, UNTALK `$5F`, then OPEN `$F0+ch`, CLOSE
//! `$E0+ch` or DATA `$60+ch`). A byte: the talker releases CLK, the
//! listener releases DATA, the talker pulls CLK and sends eight bits, LSB
//! first, each valid while CLK is released (DATA released = 1); the
//! listener acknowledges by pulling DATA. A talker that waits more than
//! 200 µs before the first bit signals EOI (last byte); the listener
//! answers with a 60 µs DATA pulse. After TALK the host hands the bus over
//! ("turnaround"): it holds DATA, releases CLK, and the drive takes CLK.
//!
//! The DOS (channels, files, commands) is [`VirtualDos`].
//!
//! [`VirtualDrive::tick`] is one microsecond.

use emu_core::media::{DriveStatus, Media, MediaError, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};

use crate::dos::VirtualDos;
use crate::iec::IecLines;

/// Bus timing in microseconds. The KERNAL polls the port with a debounce
/// (`LDA`, `CMP` the same address); a VIC-II bad line can stop its CPU for
/// 43 cycles between the two reads, so it may take about 60 µs to see an
/// edge. Both halves of a bit (CLK pulled, then released with the bit
/// valid) therefore last 90 µs, and DATA changes only together with a CLK
/// edge.
const EOI_TIMEOUT: u32 = 200;
const EOI_ACK: u32 = 60;
const BIT_LOW: u32 = 90;
const BIT_VALID: u32 = 90;
const BYTE_GAP: u32 = 60;
/// After the turnaround the talker keeps CLK pulled this long before it
/// signals ready to send. The KERNAL's TKSA (`$EDD6`) waits for the pulled
/// CLK with the debounced poll above; with a bad line between two polls a
/// 60 µs pulse can fall between them, and the bus hangs (found with
/// `INPUT#` right after `PRINT#15`). A 1541 holds CLK while it fetches
/// the first byte.
const TALK_HOLD: u32 = 200;

/// Where the protocol stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    /// Not addressed: lines released.
    #[default]
    Idle,
    /// ATN just asserted: holding DATA until the host pulls CLK (it may
    /// still be released when ATN falls).
    AtnWaitClk,
    /// Listener: holding DATA until the talker releases CLK.
    RxWaitReady,
    /// Listener: DATA released, waiting for the talker to pull CLK.
    RxWaitClkLow,
    /// Listener: answering EOI with a DATA pulse.
    RxEoiAck,
    /// Listener: waiting for CLK to be released (bit valid).
    RxBitHigh,
    /// Listener: waiting for CLK to be pulled again after a bit.
    RxBitLow,
    /// Talker after turnaround: waiting for the host to release CLK.
    TurnWaitClk,
    /// Talker: pause, then signal ready to send.
    TxNextByte,
    /// Talker: CLK released, waiting for the listener to release DATA.
    TxWaitListener,
    /// Talker: EOI, waiting for the listener's DATA pulse to start.
    TxEoiWaitLow,
    /// Talker: EOI, waiting for the pulse to end.
    TxEoiWaitHigh,
    /// Talker: sending bits (phase in `step`).
    TxBits,
    /// Talker: waiting for the listener to acknowledge the byte.
    TxWaitAck,
    /// Talker after turnaround: holding CLK pulled ([`TALK_HOLD`]).
    TurnHold,
}

/// The DOS behind an [`IecProtocol`]: what the protocol hands over and asks
/// for, byte by byte.
pub trait IecDos {
    /// ATN asserted or released: bytes received meanwhile are commands.
    fn set_atn(&mut self, asserted: bool);
    fn listening(&self) -> bool;
    fn talking(&self) -> bool;
    /// The talker ran out of data (or found no listener).
    fn stop_talking(&mut self);
    /// The next byte of the talk channel, and whether it is the last.
    fn next_byte(&mut self) -> Option<(u8, bool)>;
    /// The byte from [`IecDos::next_byte`] was taken.
    fn advance(&mut self);
    /// A byte received as listener (a command under ATN).
    fn received(&mut self, byte: u8, eoi: bool);
}

impl IecDos for VirtualDos {
    fn set_atn(&mut self, asserted: bool) {
        VirtualDos::set_atn(self, asserted);
    }
    fn listening(&self) -> bool {
        VirtualDos::listening(self)
    }
    fn talking(&self) -> bool {
        VirtualDos::talking(self)
    }
    fn stop_talking(&mut self) {
        VirtualDos::stop_talking(self);
    }
    fn next_byte(&mut self) -> Option<(u8, bool)> {
        VirtualDos::next_byte(self)
    }
    fn advance(&mut self) {
        VirtualDos::advance(self);
    }
    fn received(&mut self, byte: u8, eoi: bool) {
        VirtualDos::received(self, byte, eoi);
    }
}

/// The serial bus protocol of a device without a CPU, at line level; the
/// bytes go to and come from an [`IecDos`].
#[derive(Clone, Debug, Default)]
pub struct IecProtocol {
    /// Lines this device pulls.
    pub(crate) out: IecLines,
    pub(crate) state: State,
    pub(crate) timer: u32,
    /// Bit number and phase while sending or receiving.
    pub(crate) bit: u8,
    pub(crate) phase: u8,
    pub(crate) byte: u8,
    pub(crate) eoi: bool,
    pub(crate) prev_atn: bool,
}

impl IecProtocol {
    /// Lines this device pulls on the bus.
    pub fn iec_out(&self) -> IecLines {
        self.out
    }

    /// One microsecond with the bus levels as they are now (`true` = low).
    pub fn tick(&mut self, bus: IecLines, dos: &mut dyn IecDos) {
        if bus.atn && !self.prev_atn {
            // ATN: every device answers by pulling DATA and listens.
            dos.set_atn(true);
            self.out = IecLines {
                atn: false,
                clk: false,
                data: true,
                srq: false,
            };
            self.enter(State::AtnWaitClk);
        } else if !bus.atn && self.prev_atn {
            self.atn_released(dos);
        }
        self.prev_atn = bus.atn;
        self.timer = self.timer.saturating_add(1);
        self.step(bus, dos);
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.timer = 0;
    }

    fn atn_released(&mut self, dos: &mut dyn IecDos) {
        dos.set_atn(false);
        if dos.talking() {
            self.enter(State::TurnWaitClk);
        } else if dos.listening() {
            // Keep DATA pulled: ready when the host is.
            self.enter(State::RxWaitReady);
        } else {
            self.out = IecLines::default();
            self.enter(State::Idle);
        }
    }

    fn step(&mut self, bus: IecLines, dos: &mut dyn IecDos) {
        match self.state {
            State::Idle => {}
            State::AtnWaitClk => {
                if bus.clk {
                    self.enter(State::RxWaitReady);
                }
            }
            State::RxWaitReady => {
                if !bus.clk {
                    self.out.data = false;
                    self.eoi = false;
                    self.enter(State::RxWaitClkLow);
                }
            }
            State::RxWaitClkLow => {
                if bus.clk {
                    self.bit = 0;
                    self.byte = 0;
                    self.enter(State::RxBitHigh);
                } else if !self.eoi && self.timer >= EOI_TIMEOUT {
                    self.eoi = true;
                    self.out.data = true;
                    self.enter(State::RxEoiAck);
                }
            }
            State::RxEoiAck => {
                if self.timer >= EOI_ACK {
                    self.out.data = false;
                    self.enter(State::RxWaitClkLow);
                }
            }
            State::RxBitHigh => {
                if !bus.clk {
                    if !bus.data {
                        self.byte |= 1 << self.bit;
                    }
                    self.bit += 1;
                    self.enter(State::RxBitLow);
                }
            }
            State::RxBitLow => {
                if bus.clk {
                    if self.bit == 8 {
                        self.out.data = true;
                        let (byte, eoi) = (self.byte, self.eoi);
                        self.enter(State::RxWaitReady);
                        dos.received(byte, eoi);
                    } else {
                        self.enter(State::RxBitHigh);
                    }
                }
            }
            State::TurnWaitClk => {
                if !bus.clk {
                    self.out.clk = true;
                    self.out.data = false;
                    self.enter(State::TurnHold);
                }
            }
            State::TurnHold => {
                if self.timer >= TALK_HOLD {
                    self.enter(State::TxNextByte);
                    self.timer = BYTE_GAP;
                }
            }
            State::TxNextByte => {
                if self.timer >= BYTE_GAP {
                    match dos.next_byte() {
                        Some((byte, last)) => {
                            self.byte = byte;
                            self.eoi = last;
                            self.out.clk = false;
                            self.enter(State::TxWaitListener);
                        }
                        // Nothing to send (file not found, end): let go of
                        // the bus; the host times out.
                        None => {
                            self.out = IecLines::default();
                            dos.stop_talking();
                            self.enter(State::Idle);
                        }
                    }
                }
            }
            State::TxWaitListener => {
                if !bus.data {
                    if self.eoi {
                        self.enter(State::TxEoiWaitLow);
                    } else {
                        self.start_bits();
                    }
                }
            }
            State::TxEoiWaitLow => {
                if bus.data {
                    self.enter(State::TxEoiWaitHigh);
                }
            }
            State::TxEoiWaitHigh => {
                if !bus.data {
                    self.start_bits();
                }
            }
            State::TxBits => self.send_bits(),
            State::TxWaitAck => {
                if bus.data {
                    dos.advance();
                    if self.eoi {
                        // Last byte sent: release CLK and wait for UNTALK.
                        self.out = IecLines::default();
                        self.enter(State::Idle);
                    } else {
                        self.enter(State::TxNextByte);
                    }
                } else if self.timer > 1000 {
                    // No listener: give up.
                    self.out = IecLines::default();
                    self.enter(State::Idle);
                }
            }
        }
    }

    /// Pull CLK and put bit 0 on DATA (released = 1).
    fn start_bits(&mut self) {
        self.out.clk = true;
        self.out.data = self.byte & 1 == 0;
        self.bit = 0;
        self.phase = 0;
        self.enter(State::TxBits);
    }

    /// Bit phases: 0 CLK pulled with the bit on DATA, then release CLK (bit
    /// valid); 1 after the valid time pull CLK and put the next bit on DATA
    /// at the same moment (after the last one, release DATA).
    fn send_bits(&mut self) {
        match self.phase {
            0 if self.timer >= BIT_LOW => {
                self.out.clk = false;
                self.phase = 1;
                self.timer = 0;
            }
            1 if self.timer >= BIT_VALID => {
                self.out.clk = true;
                self.bit += 1;
                self.phase = 0;
                self.timer = 0;
                if self.bit == 8 {
                    self.out.data = false;
                    self.enter(State::TxWaitAck);
                } else {
                    self.out.data = self.byte & (1 << self.bit) == 0;
                }
            }
            _ => {}
        }
    }

    /// Savestate of every field (inside the caller's chunk).
    pub fn save(&self, w: &mut StateWriter) {
        w.u8(state_code(self.state));
        w.u32(self.timer);
        for v in [self.bit, self.phase, self.byte] {
            w.u8(v);
        }
        for b in [self.out.clk, self.out.data, self.eoi, self.prev_atn] {
            w.bool(b);
        }
    }

    /// Counterpart of [`IecProtocol::save`].
    pub fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut p = Self {
            state: state_from(c.u8()?).ok_or(StateError::Invalid("IEC protocol state"))?,
            timer: c.u32()?,
            bit: c.u8()?.min(8),
            phase: c.u8()? % 2,
            byte: c.u8()?,
            ..Self::default()
        };
        p.out.clk = c.bool()?;
        p.out.data = c.bool()?;
        p.eoi = c.bool()?;
        p.prev_atn = c.bool()?;
        *self = p;
        Ok(())
    }
}

pub struct VirtualDrive {
    /// Channels, files and commands.
    pub dos: VirtualDos,
    proto: IecProtocol,
}

impl VirtualDrive {
    pub fn new(device: u8) -> Self {
        Self {
            dos: VirtualDos::new(device, "MIMIC VIRTUAL DRIVE"),
            proto: IecProtocol::default(),
        }
    }

    /// RESET: the bus lines and channels are released, the status shows
    /// the DOS version.
    pub fn reset(&mut self) {
        self.proto = IecProtocol::default();
        self.dos.reset();
    }

    pub fn has_disk(&self) -> bool {
        self.dos.has_disk()
    }

    /// The command channel's current message (for tests and tools).
    pub fn status_text(&self) -> &str {
        self.dos.status_text()
    }

    /// Lines this drive pulls on the bus.
    pub fn iec_out(&self) -> IecLines {
        self.proto.out
    }

    /// One microsecond with the bus levels as they are now (`true` = low).
    pub fn tick(&mut self, bus: IecLines) {
        self.proto.tick(bus, &mut self.dos);
    }
}

const TAG: [u8; 4] = *b"VDRV";
const VERSION: u16 = 1;

fn state_code(s: State) -> u8 {
    s as u8
}

fn state_from(v: u8) -> Option<State> {
    use State::*;
    [
        Idle,
        AtnWaitClk,
        RxWaitReady,
        RxWaitClkLow,
        RxEoiAck,
        RxBitHigh,
        RxBitLow,
        TurnWaitClk,
        TxNextByte,
        TxWaitListener,
        TxEoiWaitLow,
        TxEoiWaitHigh,
        TxBits,
        TxWaitAck,
        TurnHold,
    ]
    .get(usize::from(v))
    .copied()
}

impl Savestate for VirtualDrive {
    /// The image is media, not state; open channels are saved.
    fn save(&self, w: &mut StateWriter) {
        let p = &self.proto;
        w.chunk(TAG, VERSION, |w| {
            w.u8(state_code(p.state));
            w.u32(p.timer);
            let dos = &self.dos;
            for v in [p.bit, p.phase, p.byte, dos.channel] {
                w.u8(v);
            }
            for b in [
                p.out.clk,
                p.out.data,
                p.eoi,
                dos.under_atn,
                p.prev_atn,
                dos.listening,
                dos.talking,
                dos.opening,
            ] {
                w.bool(b);
            }
            w.bytes(&dos.name);
            w.bytes(&dos.command);
            w.bytes(dos.status.as_bytes());
            dos.save_channels(w);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut d = VirtualDrive::new(self.dos.device());
        let p = &mut d.proto;
        p.state = state_from(c.u8()?).ok_or(StateError::Invalid("virtual drive state"))?;
        p.timer = c.u32()?;
        p.bit = c.u8()?.min(8);
        p.phase = c.u8()? % 2;
        p.byte = c.u8()?;
        d.dos.channel = c.u8()? & 0x0f;
        p.out.clk = c.bool()?;
        p.out.data = c.bool()?;
        p.eoi = c.bool()?;
        d.dos.under_atn = c.bool()?;
        p.prev_atn = c.bool()?;
        d.dos.listening = c.bool()?;
        d.dos.talking = c.bool()?;
        d.dos.opening = c.bool()?;
        d.dos.name = c.bytes()?.to_vec();
        d.dos.command = c.bytes()?.to_vec();
        d.dos.status = String::from_utf8_lossy(c.bytes()?).into_owned();
        d.dos.load_channels(&mut c)?;
        d.dos.disk = self.dos.disk.take();
        *self = d;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

impl MediaSlot for VirtualDrive {
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

    fn dirty(&self) -> bool {
        self.dos.dirty()
    }

    /// From the [`VirtualDos`]: bus, open channels, error channel.
    fn status(&self) -> DriveStatus {
        self.dos.status()
    }
}
