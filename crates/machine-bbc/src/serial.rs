//! The serial side of the Model B: the 6850 ACIA (`&FE08`/`&FE09`), the
//! serial ULA (`&FE10`, write only) and the cassette recorder behind it.
//!
//! **ACIA.** Control (write `&FE08`): bits 0–1 the clock divide (`11`
//! master reset), 2–4 the word format, 5–6 the transmitter control (`01`
//! transmit interrupt on), 7 the receive interrupt. Status (read
//! `&FE08`): bit 0 receive register full, 1 transmit register empty, 2
//! data carrier detect, 3 clear to send (low), 4 framing, 5 overrun, 6
//! parity, 7 interrupt. Data: read the received byte (`&FE09`), write the
//! byte to send. IRQ: receive interrupts on and a byte, an overrun or a
//! carrier change waiting; or transmit interrupts on and the transmit
//! register empty.
//!
//! **Serial ULA.** Bit 7 the cassette motor relay, 6 RS423 (1) or the
//! cassette (0), 3–5 the receive and 0–2 the transmit rate. On the
//! cassette the ULA turns tones into bits and back; here the recorder
//! hands the ACIA whole bytes at the tape's baud rate. The ULA's run-in
//! detector drives the ACIA's DCD: high after [`RUN_IN_BITS`] of unbroken
//! carrier, low again with the first start bit or silence (the length is
//! this project's choice; the hardware guide names the detector, not its
//! time). While DCD is high the 6850 holds its receiver in reset, so the
//! leader's latched DCD interrupt tells the MOS a block comes, and the
//! bytes arrive with DCD low.
//!
//! **Recorder** (media slot "cassette": UEF or WAV, `media::uef`): keys
//! "play", "record", "stop", "rewind" ([`MediaSlot::control`]); inserting
//! a tape presses PLAY. The tape moves while a key is down and the motor
//! runs. Recording replaces the tape from where it started with the
//! ACIA's bytes and the carrier between them, written back as an
//! uncompressed UEF on STOP, flush or eject; an empty file is a blank
//! tape.
//!
//! Sources: Motorola, *MC6850 Asynchronous Communications Interface
//! Adapter* data sheet (registers, status bits, interrupts); Acorn, *BBC
//! Microcomputer Advanced User Guide* (1983), the serial ULA and the
//! cassette interface. The implementation is this project's own.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::uef::{Event, Tape};

use crate::CPU_HZ;

/// Bit times of unbroken carrier before the run-in detector raises DCD.
pub const RUN_IN_BITS: u32 = 24;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Acia {
    pub control: u8,
    rdr: u8,
    tdr: u8,
    rdrf: bool,
    tdre: bool,
    ovrn: bool,
    /// The DCD input (a tone on the tape) and its latched status bit.
    dcd_in: bool,
    dcd: bool,
    /// A byte waits to be sent.
    tx_pending: bool,
    /// The status was read (the next data read clears a carrier change).
    status_read: bool,
}

impl Acia {
    pub fn new() -> Self {
        Self {
            tdre: true,
            ..Self::default()
        }
    }

    pub fn irq(&self) -> bool {
        let rx = self.control & 0x80 != 0 && (self.rdrf || self.ovrn || self.dcd);
        let tx = self.control & 0x60 == 0x20 && self.tdre;
        rx || tx
    }

    pub fn status(&self) -> u8 {
        u8::from(self.rdrf)
            | u8::from(self.tdre) << 1
            | u8::from(self.dcd || self.dcd_in) << 2
            | u8::from(self.ovrn) << 5
            | u8::from(self.irq()) << 7
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        if addr & 1 == 0 {
            self.status_read = true;
            self.status()
        } else {
            let v = self.rdr;
            self.rdrf = false;
            self.ovrn = false;
            if self.status_read {
                self.dcd = false;
            }
            self.status_read = false;
            v
        }
    }

    pub fn peek(&self, addr: u16) -> u8 {
        if addr & 1 == 0 {
            self.status()
        } else {
            self.rdr
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        if addr & 1 == 0 {
            if v & 3 == 3 {
                // Master reset.
                let dcd_in = self.dcd_in;
                *self = Self::new();
                self.dcd_in = dcd_in;
            }
            self.control = v;
        } else {
            self.tdr = v;
            self.tdre = false;
            self.tx_pending = true;
        }
    }

    /// A byte from the tape.
    pub fn receive(&mut self, v: u8) {
        if self.dcd_in {
            // DCD high: the receiver is held in reset.
            return;
        }
        if self.rdrf {
            self.ovrn = true;
        }
        self.rdr = v;
        self.rdrf = true;
    }

    /// The DCD input: a change latches the status bit.
    pub fn set_dcd(&mut self, tone: bool) {
        if tone != self.dcd_in {
            self.dcd = true;
        }
        self.dcd_in = tone;
    }

    /// The byte to send, if one waits; the register is free again.
    fn take_tx(&mut self) -> Option<u8> {
        std::mem::take(&mut self.tx_pending).then_some(self.tdr)
    }

    fn tx_done(&mut self) {
        self.tdre = true;
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.control);
        w.u8(self.rdr);
        w.u8(self.tdr);
        for b in [
            self.rdrf,
            self.tdre,
            self.ovrn,
            self.dcd_in,
            self.dcd,
            self.tx_pending,
            self.status_read,
        ] {
            w.bool(b);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut n = Self::new();
        n.control = r.u8()?;
        n.rdr = r.u8()?;
        n.tdr = r.u8()?;
        n.rdrf = r.bool()?;
        n.tdre = r.bool()?;
        n.ovrn = r.bool()?;
        n.dcd_in = r.bool()?;
        n.dcd = r.bool()?;
        n.tx_pending = r.bool()?;
        n.status_read = r.bool()?;
        *self = n;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Keys {
    #[default]
    Stop,
    Play,
    Record,
}

pub struct Cassette {
    tape: Option<(Media, Tape)>,
    keys: Keys,
    /// The event under the head and the bit times into it.
    event: usize,
    bit: u32,
    /// 2 MHz cycles into the current bit time (in baud units: a bit time
    /// is `CPU_HZ` of them).
    phase: u32,
    /// Recording: the events since `rec_from`, the byte being sent and its
    /// bits left.
    rec_from: usize,
    recorded: Vec<Event>,
    sending: Option<(u8, u32)>,
    /// Bit times of unbroken carrier under the head.
    run_in: u32,
    dirty: bool,
    protected: bool,
    moving: bool,
}

impl Default for Cassette {
    fn default() -> Self {
        Self::new()
    }
}

impl Cassette {
    pub fn new() -> Self {
        Self {
            tape: None,
            keys: Keys::Stop,
            event: 0,
            bit: 0,
            phase: 0,
            rec_from: 0,
            recorded: Vec::new(),
            sending: None,
            run_in: 0,
            dirty: false,
            protected: false,
            moving: false,
        }
    }

    pub fn has_tape(&self) -> bool {
        self.tape.is_some()
    }

    fn baud(&self) -> u32 {
        self.tape.as_ref().map_or(1200, |(_, t)| t.baud)
    }

    /// One 2 MHz cycle with the motor relay `motor`.
    pub fn tick(&mut self, motor: bool, acia: &mut Acia) {
        self.moving = motor && self.keys != Keys::Stop && self.tape.is_some();
        if !self.moving {
            if self.keys != Keys::Record {
                acia.set_dcd(false);
            }
            return;
        }
        self.phase += self.baud();
        if self.phase < CPU_HZ {
            return;
        }
        self.phase -= CPU_HZ;
        match self.keys {
            Keys::Play => self.play_bit(acia),
            Keys::Record => self.record_bit(acia),
            Keys::Stop => {}
        }
    }

    fn play_bit(&mut self, acia: &mut Acia) {
        let Some((_, tape)) = &self.tape else {
            return;
        };
        let Some(&e) = tape.events.get(self.event) else {
            acia.set_dcd(false);
            return;
        };
        self.run_in = match e {
            Event::Carrier(_) => self.run_in.saturating_add(1),
            _ => 0,
        };
        acia.set_dcd(self.run_in >= RUN_IN_BITS);
        self.bit += 1;
        if self.bit >= Tape::bits(&e) {
            self.bit = 0;
            self.event += 1;
            if let Event::Byte(b) = e {
                acia.receive(b);
            }
        }
    }

    fn record_bit(&mut self, acia: &mut Acia) {
        if self.sending.is_none() {
            if let Some(b) = acia.take_tx() {
                self.sending = Some((b, 10));
            }
        }
        match &mut self.sending {
            Some((b, left)) => {
                *left -= 1;
                if *left == 0 {
                    self.recorded.push(Event::Byte(*b));
                    self.sending = None;
                    acia.tx_done();
                }
            }
            None => match self.recorded.last_mut() {
                Some(Event::Carrier(n)) => *n += 1,
                _ => self.recorded.push(Event::Carrier(1)),
            },
        }
    }

    /// Recording ends: the recorded events replace the tape from where it
    /// started.
    fn finish_recording(&mut self) {
        if self.keys != Keys::Record {
            return;
        }
        self.keys = Keys::Stop;
        let rec = std::mem::take(&mut self.recorded);
        let from = self.rec_from;
        if let Some((_, tape)) = &mut self.tape {
            if !self.protected {
                tape.events.truncate(from.min(tape.events.len()));
                tape.events.extend(rec);
                self.event = tape.events.len();
                self.bit = 0;
                self.dirty = true;
            }
        }
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        self.finish_recording();
        if let Some((media, tape)) = &mut self.tape {
            if std::mem::take(&mut self.dirty) {
                let bytes = tape.to_uef();
                if media.backend.len() != bytes.len() as u64 {
                    media.backend.set_len(bytes.len() as u64)?;
                }
                media.backend.write(0, &bytes)?;
            }
        }
        Ok(())
    }

    /// Position and keys (the tape itself is media).
    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.keys as u8);
        w.u32(self.event as u32);
        w.u32(self.bit);
        w.u32(self.phase);
        w.u32(self.rec_from as u32);
        w.u32(self.recorded.len() as u32);
        for e in &self.recorded {
            match *e {
                Event::Carrier(n) => {
                    w.u8(0);
                    w.u32(n);
                }
                Event::Byte(b) => {
                    w.u8(1);
                    w.u32(u32::from(b));
                }
                Event::Gap(n) => {
                    w.u8(2);
                    w.u32(n);
                }
            }
        }
        w.bool(self.sending.is_some());
        let (b, l) = self.sending.unwrap_or((0, 0));
        w.u8(b);
        w.u32(l);
        w.u32(self.run_in);
    }

    /// Take the position and keys of `other` (loaded from a savestate);
    /// the tape stays.
    pub fn adopt(&mut self, other: Cassette) {
        (self.keys, self.event, self.bit, self.phase) =
            (other.keys, other.event, other.bit, other.phase);
        (self.rec_from, self.recorded, self.sending) =
            (other.rec_from, other.recorded, other.sending);
        self.run_in = other.run_in;
        if self.tape.is_none() {
            self.keys = Keys::Stop;
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let keys = match r.u8()? {
            0 => Keys::Stop,
            1 => Keys::Play,
            2 => Keys::Record,
            _ => return Err(StateError::Invalid("tape keys")),
        };
        let event = r.u32()? as usize;
        let bit = r.u32()?;
        let phase = r.u32()? % CPU_HZ;
        let rec_from = r.u32()? as usize;
        let n = r.u32()? as usize;
        if n > 10_000_000 {
            return Err(StateError::Invalid("tape recording"));
        }
        let mut recorded = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            let kind = r.u8()?;
            let v = r.u32()?;
            recorded.push(match kind {
                0 => Event::Carrier(v),
                1 => Event::Byte(v as u8),
                2 => Event::Gap(v),
                _ => return Err(StateError::Invalid("tape event")),
            });
        }
        let has = r.bool()?;
        let b = r.u8()?;
        let l = r.u32()?.min(10);
        self.run_in = r.u32()?;
        (self.keys, self.event, self.bit, self.phase) = (keys, event, bit, phase);
        (self.rec_from, self.recorded) = (rec_from, recorded);
        self.sending = has.then_some((b, l));
        Ok(())
    }
}

impl MediaSlot for Cassette {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut data = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut data)?;
        let tape = if data.is_empty() {
            Tape {
                baud: 1200,
                events: Vec::new(),
            }
        } else {
            Tape::parse(&data)?
        };
        let _ = self.eject();
        self.tape = Some((media, tape));
        (self.event, self.bit, self.phase) = (0, 0, 0);
        self.keys = Keys::Play;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = self.write_back();
        self.keys = Keys::Stop;
        self.tape.take().map(|(m, _)| m)
    }

    fn write_protect(&mut self, on: bool) {
        self.protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        let recording = self.keys == Keys::Record;
        self.write_back()?;
        if recording {
            self.keys = Keys::Record;
            self.rec_from = self.event;
        }
        match &mut self.tape {
            Some((m, _)) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.dirty || self.keys == Keys::Record
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        if self.tape.is_none() {
            return Err(MediaError::NoMedia);
        }
        match command {
            "play" => {
                self.finish_recording();
                self.keys = Keys::Play;
            }
            "record" => {
                if self.protected {
                    return Err(MediaError::WriteProtected);
                }
                self.finish_recording();
                self.keys = Keys::Record;
                self.rec_from = self.event;
                self.recorded.clear();
                self.sending = None;
            }
            "stop" => {
                self.write_back()?;
                self.keys = Keys::Stop;
            }
            "rewind" => {
                self.write_back()?;
                self.keys = Keys::Stop;
                (self.event, self.bit, self.phase) = (0, 0, 0);
            }
            other => {
                return Err(MediaError::Control(format!(
                    "'{other}' (play, record, stop or rewind)"
                )))
            }
        }
        Ok(())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.moving,
            error: false,
            track: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_acia_takes_bytes_and_interrupts() {
        let mut a = Acia::new();
        a.write(0, 0x03);
        a.write(0, 0x95); // receive interrupt, /64
        assert_eq!(a.status() & 0x02, 0x02, "transmit register empty");
        a.receive(0x41);
        assert!(a.irq());
        assert_eq!(a.read(0) & 0x81, 0x81);
        assert_eq!(a.read(1), 0x41);
        assert!(!a.irq());
        a.receive(1);
        a.receive(2);
        assert_eq!(a.read(0) & 0x20, 0x20, "overrun");
    }

    #[test]
    fn playing_hands_bytes_and_carrier_to_the_acia() {
        let mut c = Cassette::new();
        c.tape = Some((
            Media {
                name: "t.uef".into(),
                kind: MediaKind::Tape,
                backend: Box::new(media_blank()),
            },
            Tape {
                baud: 1200,
                events: vec![Event::Carrier(30), Event::Byte(0x2a)],
            },
        ));
        c.keys = Keys::Play;
        let mut a = Acia::new();
        // The run-in raises DCD (latched), the start bit drops it again.
        for _ in 0..(28 * CPU_HZ / 1200) {
            c.tick(true, &mut a);
        }
        assert!(a.dcd_in, "run-in detected");
        for _ in 0..(14 * CPU_HZ / 1200) {
            c.tick(true, &mut a);
        }
        assert_eq!(a.read(0) & 0x05, 0x05, "DCD latched, byte received");
        assert_eq!(a.read(1), 0x2a);
        assert_eq!(a.status() & 0x04, 0, "DCD follows the pin after the reads");
    }

    struct Blank;
    impl emu_core::media::MediaBackend for Blank {
        fn len(&self) -> u64 {
            0
        }
        fn read(&mut self, _o: u64, _b: &mut [u8]) -> Result<(), MediaError> {
            Ok(())
        }
        fn write(&mut self, _o: u64, _d: &[u8]) -> Result<(), MediaError> {
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }
    fn media_blank() -> Blank {
        Blank
    }

    #[test]
    fn recording_collects_bytes_and_carrier() {
        let mut c = Cassette::new();
        c.tape = Some((
            Media {
                name: "t.uef".into(),
                kind: MediaKind::Tape,
                backend: Box::new(media_blank()),
            },
            Tape {
                baud: 1200,
                events: Vec::new(),
            },
        ));
        c.control("record").unwrap();
        let mut a = Acia::new();
        let bit = CPU_HZ / 1200;
        for _ in 0..3 * bit {
            c.tick(true, &mut a);
        }
        a.write(1, 0x55);
        for _ in 0..12 * bit {
            c.tick(true, &mut a);
        }
        c.finish_recording();
        let events = &c.tape.as_ref().unwrap().1.events;
        assert!(matches!(events[0], Event::Carrier(n) if n >= 2));
        assert!(events.contains(&Event::Byte(0x55)));
    }
}
