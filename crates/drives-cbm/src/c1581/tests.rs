use super::*;
use emu_core::media::MediaBackend;
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

fn disk(image: &Arc<Mutex<Vec<u8>>>) -> Media {
    Media {
        name: "t.d81".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Shared(image.clone())),
    }
}

/// A ROM (not the Commodore DOS) with `code` at $8000 as reset entry.
fn drive_with(code: &[u8]) -> Drive1581 {
    let mut rom = vec![0xea; ROM_SIZE];
    rom[..code.len()].copy_from_slice(code);
    rom[0x7ffc..].copy_from_slice(&[0x00, 0x80, 0x00, 0x80]);
    Drive1581::new(rom.into(), 8, MemSpaceId(3))
}

fn run(d: &mut Drive1581, n: u32) {
    for _ in 0..n {
        d.tick(IecLines::default(), None);
    }
}

fn pa(d: &Drive1581) -> u8 {
    d.peek(MemSpaceId(3), 0x4000, Size::Byte).unwrap() as u8
}

#[test]
fn ready_disk_change_and_jumpers() {
    let mut d = drive_with(&[0x4c, 0x00, 0x80]);
    let image = Arc::new(Mutex::new(D64::blank_d81(b"T", *b"TT").to_bytes()));
    assert_eq!(pa(&d) & 0x82, 0x02, "no disk: not ready, changed");
    assert_eq!(pa(&d) & 0x18, 0, "device 8");
    d.insert(disk(&image)).unwrap();
    // Motor on (PA2 low): ready once it is up to speed.
    d.cia.write(0x2, 0x65);
    d.cia.write(0x0, 0x00);
    run(&mut d, 1000);
    assert_eq!(pa(&d) & 0x02, 0x02, "spinning up");
    run(&mut d, SPIN_UP_US);
    assert_eq!(pa(&d) & 0x02, 0, "ready");
    // A step pulse clears the disk change latch.
    assert_eq!(pa(&d) & 0x80, 0);
    d.wd.write(0, 0x4b);
    run(&mut d, 10_000);
    assert_eq!(pa(&d) & 0x80, 0x80);
    assert_eq!(d.cylinder(), 1);
    d.eject();
    assert_eq!(pa(&d) & 0x80, 0, "changed again");
    let d9 = Drive1581::new(vec![0; ROM_SIZE].into(), 9, MemSpaceId(3));
    assert_eq!(pa(&d9) & 0x18, 0x08, "device 9");
}

#[test]
fn atn_is_acknowledged_while_atna_is_set() {
    let mut d = drive_with(&[0x4c, 0x00, 0x80]);
    d.cia.write(0x3, 0x3a);
    d.cia.write(0x1, 0x10); // ATNA
    assert!(!d.iec_out(false).data);
    assert!(d.iec_out(true).data, "hardware acknowledge");
    d.cia.write(0x1, 0x00); // the DOS serves ATN
    assert!(!d.iec_out(true).data);
    d.cia.write(0x1, 0x0a); // DATA and CLK out
    let out = d.iec_out(false);
    assert!(out.data && out.clk);
}

#[test]
fn rejects_5_25_inch_images() {
    let mut d = drive_with(&[0x4c, 0x00, 0x80]);
    let d64 = Arc::new(Mutex::new(D64::blank(b"T", *b"TT").to_bytes()));
    assert!(d.insert(disk(&d64)).is_err());
    assert!(!d.has_disk());
}

/// Drive the WD1772 directly: read a sector of the inserted D81, then
/// write one and find it in the image after the motor stops.
#[test]
fn sectors_read_and_write_through_the_wd1772() {
    let mut image = D64::blank_d81(b"WD", *b"WD");
    image.sector_mut(40, 0).unwrap()[0x80..0x84].copy_from_slice(b"ABCD");
    let backing = Arc::new(Mutex::new(image.to_bytes()));
    let mut d = drive_with(&[0x4c, 0x00, 0x80]);
    d.insert(disk(&backing)).unwrap();
    d.cia.write(0x2, 0x65);
    d.cia.write(0x0, 0x00); // side 0, motor on
                            // Seek to cylinder 39 (logical track 40).
    d.wd.write(3, 39);
    d.wd.write(0, 0x1b);
    run(&mut d, 500_000);
    assert_eq!(d.cylinder(), 39);
    // Read physical sector 1: logical sectors 0 and 1.
    d.wd.write(2, 1);
    d.wd.write(0, 0x88);
    let mut data = Vec::new();
    for _ in 0..400_000 {
        d.tick(IecLines::default(), None);
        let sense = d.sense();
        if d.wd.drq() {
            data.push(d.wd.read(3, &sense));
        }
        if !d.wd.busy() {
            break;
        }
    }
    assert_eq!(data.len(), 512);
    assert_eq!(&data[0x80..0x84], b"ABCD");
    // Write physical sector 10 of side 1: logical sectors 38 and 39.
    d.cia.write(0x0, 0x01);
    d.wd.write(2, 10);
    d.wd.write(0, 0xa8);
    let mut n = 0u32;
    for _ in 0..400_000 {
        d.tick(IecLines::default(), None);
        if d.wd.drq() {
            d.wd.write(3, (n % 251) as u8);
            n += 1;
        }
        if !d.wd.busy() {
            break;
        }
    }
    assert_eq!(n, 512);
    // Motor off writes the track back.
    d.cia.write(0x0, 0x04);
    run(&mut d, 2);
    let back = D64::parse(&backing.lock().unwrap()).unwrap();
    assert_eq!(back.sector(40, 38).unwrap()[..3], [0, 1, 2]);
    assert_eq!(back.sector(40, 39).unwrap()[0], (256 % 251) as u8);
    assert_eq!(back.sector(40, 0).unwrap()[0x80..0x84], *b"ABCD");
}

#[test]
fn savestate_round_trip() {
    let code = [0xe6, 0x10, 0x4c, 0x00, 0x80]; // INC $10; JMP $8000
    let mut a = drive_with(&code);
    a.cia.write(0x4, 0x34);
    run(&mut a, 333);
    let mut w = StateWriter::new();
    a.save(&mut w);
    let data = w.into_bytes();
    let mut b = drive_with(&code);
    b.load(&mut StateReader::new(&data)).unwrap();
    run(&mut a, 100);
    run(&mut b, 100);
    assert_eq!(a.ram[..], b.ram[..]);
    assert_eq!(a.pc(), b.pc());
    assert!(b.load(&mut StateReader::new(&data[..50])).is_err());
}

#[test]
fn status_shows_motor_led_error_and_cylinder() {
    let mut d = drive_with(&[0x4c, 0x00, 0x80]);
    // Port A outputs as the DOS sets them: motor off (PA2 high), LED off.
    d.cia.write(0x2, 0x65);
    d.cia.write(0x0, 0x04);
    let idle = DriveStatus {
        active: false,
        error: false,
        track: Some(1),
    };
    assert_eq!(d.status(), idle);
    d.cia.write(0x0, 0x40);
    assert_eq!(
        d.status(),
        DriveStatus {
            active: true,
            ..idle
        },
        "motor and LED on"
    );
    d.cia.write(0x0, 0x44);
    assert_eq!(
        d.status(),
        DriveStatus {
            active: true,
            error: true,
            ..idle
        },
        "LED lit with the motor off"
    );
    d.mech.cylinder = 39;
    assert_eq!(d.status().track, Some(40));
}
