use super::*;
use emu_core::media::{MediaBackend, MediaKind};
use media::D64;
use std::sync::{Arc, Mutex};

/// A disk whose backend keeps what the drive writes back.
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

/// The host side of the serial bus, driven step by step like the KERNAL:
/// every wait ticks the drive one microsecond.
struct Host {
    drive: VirtualDrive,
    /// What the host pulls.
    out: IecLines,
}

impl Host {
    fn new(image: Arc<Mutex<Vec<u8>>>) -> Self {
        let mut drive = VirtualDrive::new(8);
        drive
            .insert(Media {
                name: "t.d64".into(),
                kind: MediaKind::Disk,
                backend: Box::new(Shared(image)),
            })
            .unwrap();
        Self {
            drive,
            out: IecLines::default(),
        }
    }

    fn bus(&self) -> IecLines {
        IecLines::combine(&[self.out, self.drive.iec_out()])
    }

    fn tick(&mut self, n: u32) {
        for _ in 0..n {
            let b = self.bus();
            self.drive.tick(b);
        }
    }

    fn wait(&mut self, what: &str, cond: impl Fn(IecLines) -> bool) {
        for _ in 0..5000 {
            if cond(self.bus()) {
                return;
            }
            self.tick(1);
        }
        panic!("timeout waiting for {what}");
    }

    /// Send one byte as talker; `eoi` marks the last.
    fn send(&mut self, byte: u8, eoi: bool) {
        self.out.clk = false;
        self.wait("listener ready", |b| !b.data);
        if eoi {
            self.wait("EOI ack", |b| b.data);
            self.wait("EOI ack end", |b| !b.data);
        }
        self.out.clk = true;
        for bit in 0..8 {
            self.out.data = byte & (1 << bit) == 0;
            self.tick(20);
            self.out.clk = false;
            self.tick(30);
            self.out.clk = true;
            self.out.data = false;
        }
        self.wait("frame ack", |b| b.data);
    }

    /// Receive one byte as listener; returns it and whether it was the last.
    fn receive(&mut self) -> Option<(u8, bool)> {
        // Wait for the talker to be ready (CLK released).
        for _ in 0..5000 {
            if !self.bus().clk {
                break;
            }
            self.tick(1);
        }
        if self.bus().clk {
            return None;
        }
        self.out.data = false;
        let mut eoi = false;
        let mut waited = 0;
        while !self.bus().clk {
            self.tick(1);
            waited += 1;
            if waited == 250 && !eoi {
                eoi = true;
                self.out.data = true;
                self.tick(60);
                self.out.data = false;
            }
            if waited > 3000 {
                return None;
            }
        }
        let mut byte = 0u8;
        for bit in 0..8 {
            self.wait("bit valid", |b| !b.clk);
            if !self.bus().data {
                byte |= 1 << bit;
            }
            self.wait("bit end", |b| b.clk);
        }
        self.out.data = true;
        self.tick(30);
        Some((byte, eoi))
    }

    /// Bytes under ATN (the host pulls CLK and lets go of DATA, as the
    /// KERNAL does).
    fn atn(&mut self, bytes: &[u8]) {
        self.out.atn = true;
        self.out.clk = true;
        self.out.data = false;
        // The KERNAL waits a millisecond before it looks for an answer.
        self.tick(1000);
        assert!(self.bus().data, "device present");
        for &b in bytes {
            self.send(b, false);
        }
    }

    fn release_atn(&mut self) {
        self.out.atn = false;
        self.tick(20);
    }

    /// LISTEN, a secondary address, data (the last with EOI), UNLISTEN.
    fn listen_send(&mut self, secondary: u8, data: &[u8]) {
        self.atn(&[0x28, secondary]);
        self.release_atn();
        for (i, &b) in data.iter().enumerate() {
            self.send(b, i + 1 == data.len());
        }
        self.atn(&[0x3f]);
        self.release_atn();
        self.out = IecLines::default();
        self.tick(100);
    }

    /// TALK, a secondary address, turnaround, receive up to EOI, UNTALK.
    fn talk_receive(&mut self, secondary: u8) -> Vec<u8> {
        self.atn(&[0x48, secondary]);
        // Turnaround: hold DATA, release CLK, then ATN.
        self.out.data = true;
        self.out.clk = false;
        self.out.atn = false;
        let mut got = Vec::new();
        for _ in 0..500 {
            if self.bus().clk {
                break;
            }
            self.tick(1);
        }
        if self.bus().clk {
            while let Some((b, eoi)) = self.receive() {
                got.push(b);
                if eoi {
                    break;
                }
            }
        }
        self.atn(&[0x5f]);
        self.release_atn();
        self.out = IecLines::default();
        self.tick(100);
        got
    }

    fn open(&mut self, ch: u8, name: &[u8]) {
        self.listen_send(0xf0 | ch, name);
    }

    fn close(&mut self, ch: u8) {
        self.atn(&[0x28, 0xe0 | ch, 0x3f]);
        self.release_atn();
        self.out = IecLines::default();
        self.tick(100);
    }
}

fn disk_with_file() -> Arc<Mutex<Vec<u8>>> {
    let mut d = D64::blank(b"VIRTUAL", *b"VD");
    d.write_file(b"HELLO", 2, &[0x01, 0x08, 0xaa, 0xbb, 0xcc])
        .unwrap();
    Arc::new(Mutex::new(d.to_bytes()))
}

#[test]
fn load_a_file_over_the_bus() {
    let mut h = Host::new(disk_with_file());
    h.open(0, b"HEL*");
    assert_eq!(h.talk_receive(0x60), [0x01, 0x08, 0xaa, 0xbb, 0xcc]);
    h.close(0);
    assert!(h.drive.status_text().starts_with("00"));
}

#[test]
fn directory_listing() {
    let mut h = Host::new(disk_with_file());
    h.open(0, b"$");
    let listing = h.talk_receive(0x60);
    assert_eq!(&listing[..2], &[0x01, 0x04]);
    assert!(listing.windows(7).any(|w| w == b"\"HELLO\""));
    assert!(listing.windows(11).any(|w| w == b"BLOCKS FREE"));
}

#[test]
fn file_not_found_sends_nothing_and_sets_the_error() {
    let mut h = Host::new(disk_with_file());
    h.open(0, b"NOPE");
    assert!(h.talk_receive(0x60).is_empty());
    assert_eq!(h.talk_receive(0x6f), b"62,FILE NOT FOUND,00,00\r");
    // Reading the message resets it.
    assert_eq!(h.talk_receive(0x6f), b"00, OK,00,00\r");
}

#[test]
fn save_scratch_and_commands() {
    let image = disk_with_file();
    let mut h = Host::new(image.clone());
    h.open(1, b"0:NEW");
    h.listen_send(0x61, &[0x01, 0x10, 1, 2, 3]);
    h.close(1);
    let d = D64::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(d.read_file(b"NEW").unwrap().1, [0x01, 0x10, 1, 2, 3]);
    // Saving again without @ fails; with @0: it replaces the file.
    h.open(1, b"NEW");
    assert!(h.drive.status_text().starts_with("63"));
    h.open(1, b"@0:NEW");
    h.listen_send(0x61, &[0x01, 0x10, 9]);
    h.close(1);
    let d = D64::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(d.read_file(b"NEW").unwrap().1, [0x01, 0x10, 9]);
    // Command channel: scratch, then an unknown command.
    h.open(15, b"S0:HELLO");
    assert!(h.drive.status_text().starts_with("01, FILES SCRATCHED,01"));
    let d = D64::parse(&image.lock().unwrap()).unwrap();
    assert!(d.read_file(b"HELLO").is_err());
    h.listen_send(0x6f, b"X");
    assert!(h.drive.status_text().starts_with("31"));
}

#[test]
fn other_devices_are_ignored() {
    let mut h = Host::new(disk_with_file());
    h.atn(&[0x29, 0xf0]);
    h.release_atn();
    h.tick(100);
    assert_eq!(h.drive.iec_out(), IecLines::default(), "device 9 is not us");
}

#[test]
fn savestate_round_trip() {
    let mut h = Host::new(disk_with_file());
    h.open(0, b"HELLO");
    let mut w = StateWriter::new();
    h.drive.save(&mut w);
    let data = w.into_bytes();
    let mut other = VirtualDrive::new(8);
    other.load(&mut StateReader::new(&data)).unwrap();
    let mut w2 = StateWriter::new();
    other.save(&mut w2);
    assert_eq!(w2.into_bytes(), data);
}

#[test]
fn d81_images_load_and_format_as_d81() {
    let mut d = D64::blank_d81(b"BIG", *b"81");
    d.write_file(b"HELLO", 2, &[0x01, 0x08, 7]).unwrap();
    let image = Arc::new(Mutex::new(d.to_bytes()));
    let mut h = Host::new(image.clone());
    h.open(0, b"HELLO");
    assert_eq!(h.talk_receive(0x60), [0x01, 0x08, 7]);
    h.close(0);
    h.open(0, b"$");
    let listing = h.talk_receive(0x60);
    assert!(listing.windows(5).any(|w| w == b"81 3D"));
    assert!(listing.windows(16).any(|w| w == b"BLOCKS FREE.    "));
    h.open(15, b"N0:NEW,NW");
    let d = D64::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(d.format, media::Format::D81);
    assert_eq!((d.id(), d.blocks_free()), (*b"NW", 3160));
}
