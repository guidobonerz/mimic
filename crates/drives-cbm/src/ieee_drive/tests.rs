use super::*;
use emu_core::media::{MediaBackend, MediaKind};
use media::D64;
use std::sync::{Arc, Mutex};

struct Shared(Arc<Mutex<Vec<u8>>>);

impl MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let d = self.0.lock().unwrap();
        buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let mut d = self.0.lock().unwrap();
        d[offset as usize..offset as usize + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// The controller (the PET) driving the handshake like its KERNAL; every
/// wait ticks the drive one microsecond.
struct Pet {
    drive: IeeeDrive,
    out: IeeeLines,
}

impl Pet {
    fn new(image: Arc<Mutex<Vec<u8>>>) -> Self {
        let mut drive = IeeeDrive::new(8);
        drive
            .insert(Media {
                name: "t.d80".into(),
                kind: MediaKind::Disk,
                backend: Box::new(Shared(image)),
            })
            .unwrap();
        Self {
            drive,
            out: IeeeLines::default(),
        }
    }

    fn bus(&self) -> IeeeLines {
        IeeeLines::combine(&[self.out, self.drive.ieee_out()])
    }

    fn tick(&mut self, n: u32) {
        for _ in 0..n {
            let b = self.bus();
            self.drive.tick(b);
        }
    }

    /// Wait for a condition; `None` after the PET's 64 ms timeout.
    fn wait(&mut self, cond: impl Fn(IeeeLines) -> bool) -> Option<()> {
        for _ in 0..65_000 {
            if cond(self.bus()) {
                return Some(());
            }
            self.tick(1);
        }
        None
    }

    /// Send one byte as talker (KERNAL `$F109`); false: device not present.
    fn send(&mut self, byte: u8, eoi: bool) -> bool {
        self.tick(5);
        let b = self.bus();
        if !b.nrfd && !b.ndac {
            return false;
        }
        self.out.data = byte;
        self.out.eoi = eoi;
        self.wait(|b| !b.nrfd).expect("listener ready");
        self.out.dav = true;
        self.wait(|b| !b.ndac).expect("byte accepted");
        self.out.dav = false;
        self.out.eoi = false;
        self.out.data = 0;
        self.tick(5);
        true
    }

    /// Receive one byte as listener (KERNAL `$F1C0`): the byte and EOI,
    /// `None` on a timeout.
    fn receive(&mut self) -> Option<(u8, bool)> {
        self.out.ndac = true;
        self.out.nrfd = false;
        self.wait(|b| b.dav)?;
        self.out.nrfd = true;
        let b = self.bus();
        self.out.ndac = false;
        self.wait(|b| !b.dav).expect("DAV released");
        self.out.ndac = true;
        Some((b.data, b.eoi))
    }

    fn atn(&mut self, bytes: &[u8]) -> bool {
        self.out.atn = true;
        self.tick(10);
        let present = bytes.iter().all(|&b| self.send(b, false));
        present
    }

    fn release_atn(&mut self) {
        self.out.atn = false;
        self.tick(10);
    }

    fn listen_send(&mut self, secondary: u8, data: &[u8]) {
        assert!(self.atn(&[0x28, secondary]), "device present");
        self.release_atn();
        for (i, &b) in data.iter().enumerate() {
            assert!(self.send(b, i + 1 == data.len()));
        }
        self.atn(&[0x3f]);
        self.release_atn();
    }

    fn talk_receive(&mut self, secondary: u8) -> Vec<u8> {
        assert!(self.atn(&[0x48, secondary]));
        // Turnaround: listen before releasing ATN.
        self.out.ndac = true;
        self.out.nrfd = true;
        self.release_atn();
        let mut got = Vec::new();
        while let Some((b, eoi)) = self.receive() {
            got.push(b);
            if eoi {
                break;
            }
        }
        self.out = IeeeLines::default();
        self.atn(&[0x5f]);
        self.release_atn();
        got
    }
}

fn disk() -> Arc<Mutex<Vec<u8>>> {
    let mut d = D64::blank_8050(b"PET", *b"80", false);
    d.write_file(b"HELLO", 2, &[0x01, 0x04, 0xaa, 0xbb])
        .unwrap();
    Arc::new(Mutex::new(d.to_bytes()))
}

#[test]
fn loads_lists_and_reports_status() {
    let mut p = Pet::new(disk());
    p.listen_send(0xf0, b"HEL*");
    assert_eq!(p.talk_receive(0x60), [0x01, 0x04, 0xaa, 0xbb]);
    p.listen_send(0xf0, b"$");
    let listing = p.talk_receive(0x60);
    assert!(
        listing.windows(8).any(|w| w == b"\"HELLO\" "),
        "{listing:02x?}"
    );
    assert!(listing.windows(5).any(|w| w == b"80 2C"));
    assert_eq!(p.talk_receive(0x6f), b"00, OK,00,00\r");
    // A missing file: nothing to send, the PET times out.
    p.listen_send(0xf0, b"NOPE");
    assert!(p.talk_receive(0x60).is_empty());
    assert!(p.drive.status_text().starts_with("62"));
}

#[test]
fn saves_into_the_image() {
    let image = disk();
    let mut p = Pet::new(image.clone());
    p.listen_send(0xf1, b"0:NEW");
    p.listen_send(0x61, &[0x01, 0x04, 1, 2, 3]);
    assert!(p.atn(&[0x28, 0xe1, 0x3f]));
    p.release_atn();
    let d = D64::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(d.read_file(b"NEW").unwrap().1, [0x01, 0x04, 1, 2, 3]);
}

#[test]
fn other_devices_are_not_addressed() {
    let mut p = Pet::new(disk());
    // Under ATN the drive listens to every command ...
    assert!(p.atn(&[0x29, 0xf0]));
    p.release_atn();
    // ... and lets go of the bus when another device was meant.
    assert_eq!(p.drive.ieee_out(), IeeeLines::default());
}

#[test]
fn savestate_round_trip() {
    let mut p = Pet::new(disk());
    p.listen_send(0xf0, b"HELLO");
    let mut w = StateWriter::new();
    p.drive.save(&mut w);
    let data = w.into_bytes();
    let mut other = IeeeDrive::new(8);
    other.load(&mut StateReader::new(&data)).unwrap();
    let mut w2 = StateWriter::new();
    other.save(&mut w2);
    assert_eq!(w2.into_bytes(), data);
    assert!(other.load(&mut StateReader::new(&data[..6])).is_err());
}

/// Entering ATN the PET frees NRFD a few microseconds before NDAC; a talker
/// that sent in that window would lose a byte (BASIC 4 DIRECTORY reads
/// with TALK/UNTALK around every byte).
#[test]
fn no_byte_is_lost_when_the_pet_takes_the_bus_back() {
    let mut p = Pet::new(disk());
    p.listen_send(0xf0, b"HELLO");
    let mut got = Vec::new();
    for _ in 0..4 {
        assert!(p.atn(&[0x48, 0x60]));
        p.out.ndac = true;
        p.out.nrfd = true;
        p.release_atn();
        let (b, _) = p.receive().expect("a byte");
        got.push(b);
        // KERNAL $F0D7: NRFD free, then NDAC free 6 µs later, then ATN.
        p.out.nrfd = false;
        p.tick(6);
        p.out.ndac = false;
        p.tick(4);
        p.atn(&[0x5f]);
        p.release_atn();
    }
    assert_eq!(got, [0x01, 0x04, 0xaa, 0xbb]);
}
