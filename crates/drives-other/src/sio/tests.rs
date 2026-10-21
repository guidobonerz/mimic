use super::*;

const HZ: u32 = 1_773_447;

struct Mem(Vec<u8>);

impl emu_core::media::MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(self.0.get(o..o + buf.len()).ok_or(MediaError::OutOfRange)?);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        self.0
            .get_mut(o..o + data.len())
            .ok_or(MediaError::OutOfRange)?
            .copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn disk(sectors: u32) -> Media {
    let mut img = atr::blank(sectors);
    // Sector n starts with n.
    for n in 1..=sectors {
        img[atr::HEADER + (n as usize - 1) * 128] = n as u8;
    }
    Media {
        name: "test.atr".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Mem(img)),
    }
}

fn bus(model: DriveModel) -> Sio {
    let mut sio = Sio::new(HZ);
    let mut d = SioDrive::new(model);
    d.insert(disk(720)).unwrap();
    sio.drives[0] = Some(d);
    sio
}

/// Send a command frame at `now`; returns the cycle after COMMAND rises.
fn command(sio: &mut Sio, now: u64, dev: u8, cmd: u8, aux: u16) -> u64 {
    let [a1, a2] = aux.to_le_bytes();
    let mut f = vec![dev, cmd, a1, a2];
    f.push(checksum(&f));
    sio.set_command(true, now);
    for b in f {
        sio.byte_from_computer(b, now);
    }
    sio.set_command(false, now + 10);
    now + 10
}

/// Bytes the devices send from `from` until the bus is idle, with the
/// cycle each completed.
fn receive(sio: &mut Sio, from: u64, limit: u64) -> Vec<(u64, u8)> {
    let mut out = Vec::new();
    for t in from..from + limit {
        if let Some(b) = sio.tick(t).byte {
            out.push((t, b));
        }
        if sio.tx.is_idle() && !out.is_empty() {
            break;
        }
    }
    out
}

fn bytes(r: &[(u64, u8)]) -> Vec<u8> {
    r.iter().map(|&(_, b)| b).collect()
}

#[test]
fn checksum_wraps_carry() {
    assert_eq!(checksum(&[0x31, 0x53, 0x00, 0x00]), 0x84);
    assert_eq!(checksum(&[0xff, 0x02]), 0x02);
}

#[test]
fn status_command() {
    let mut sio = bus(DriveModel::A1050);
    let t = command(&mut sio, 0, 0x31, b'S', 0);
    let r = receive(&mut sio, t, 100_000);
    assert_eq!(
        bytes(&r),
        [
            b'A',
            b'C',
            0x00,
            0xff,
            0xe0,
            0x00,
            checksum(&[0, 0xff, 0xe0, 0])
        ]
    );
    // 19 200 baud: about 924 cycles per byte back to back after C.
    assert_eq!(r[3].0 - r[2].0, 10 * u64::from(HZ) / BAUD);
    sio.drives[0].as_mut().unwrap().write_protect(true);
    let t = command(&mut sio, 200_000, 0x31, b'S', 0);
    assert_eq!(bytes(&receive(&mut sio, t, 100_000))[2] & 0x08, 0x08);
}

#[test]
fn read_sector_with_seek_and_rotation() {
    let mut sio = bus(DriveModel::A810);
    let t = command(&mut sio, 0, 0x31, b'R', 1);
    let r = receive(&mut sio, t, 2_000_000);
    let b = bytes(&r);
    assert_eq!(&b[..2], [b'A', b'C']);
    assert_eq!(b.len(), 2 + 128 + 1);
    assert_eq!(b[2], 1, "sector 1's data");
    assert_eq!(b[130], checksum(&b[2..130]));
    // Sector 360 (track 19) is far away: seek 19 tracks first.
    let t0 = r.last().unwrap().0;
    let t = command(&mut sio, t0, 0x31, b'R', 360);
    let r = receive(&mut sio, t, 4_000_000);
    assert_eq!(bytes(&r)[2], 360u32 as u8);
    let delay = r[1].0 - r[0].0;
    let min = 19 * u64::from(HZ) * 10 / 1000;
    assert!(delay > min, "seek {delay} > {min}");
    let rev = u64::from(HZ) * 60 / 288;
    assert!(delay < min + 2 * rev + u64::from(HZ) / 50, "{delay}");
}

#[test]
fn bad_frames_and_sectors_are_refused() {
    let mut sio = bus(DriveModel::A1050);
    // Wrong checksum.
    sio.set_command(true, 0);
    for b in [0x31, b'S', 0, 0, 0x00] {
        sio.byte_from_computer(b, 0);
    }
    sio.set_command(false, 10);
    assert_eq!(bytes(&receive(&mut sio, 10, 100_000)), [b'N']);
    // Sector 0 and beyond the disk.
    for n in [0, 721] {
        let t = command(&mut sio, 200_000, 0x31, b'R', n);
        assert_eq!(bytes(&receive(&mut sio, t, 100_000)), [b'N'], "{n}");
    }
    // Unknown command.
    let t = command(&mut sio, 400_000, 0x31, b'Z', 0);
    assert_eq!(bytes(&receive(&mut sio, t, 100_000)), [b'N']);
    // No drive D2: silence.
    let t = command(&mut sio, 600_000, 0x32, b'S', 0);
    assert!(receive(&mut sio, t, 100_000).is_empty());
    // An empty drive is silent too (as if switched off).
    sio.drives[0].as_mut().unwrap().eject();
    let t = command(&mut sio, 800_000, 0x31, b'S', 0);
    assert!(receive(&mut sio, t, 100_000).is_empty());
}

#[test]
fn write_then_read_back() {
    let mut sio = bus(DriveModel::A1050);
    let t = command(&mut sio, 0, 0x31, b'W', 5);
    assert_eq!(bytes(&receive(&mut sio, t, 100_000)), [b'A']);
    let data: Vec<u8> = (0..128).map(|i| i as u8 ^ 0x5a).collect();
    let mut frame = data.clone();
    frame.push(checksum(&data));
    for b in frame {
        sio.byte_from_computer(b, 100_000);
    }
    assert_eq!(bytes(&receive(&mut sio, 100_000, 2_000_000)), [b'A', b'C']);
    let t = command(&mut sio, 2_000_000, 0x31, b'R', 5);
    let r = bytes(&receive(&mut sio, t, 2_000_000));
    assert_eq!(&r[2..130], &data[..]);

    // Write-protected: the drive takes the data and reports an error.
    sio.drives[0].as_mut().unwrap().write_protect(true);
    let t = command(&mut sio, 4_000_000, 0x31, b'P', 6);
    receive(&mut sio, t, 100_000);
    let mut frame = vec![0u8; 128];
    frame.push(0);
    for b in frame {
        sio.byte_from_computer(b, 4_200_000);
    }
    assert_eq!(
        bytes(&receive(&mut sio, 4_200_000, 2_000_000)),
        [b'A', b'E']
    );
}

#[test]
fn drive_models_and_densities() {
    let mut sio = Sio::new(HZ);
    let mut d = SioDrive::new(DriveModel::A810);
    d.insert(disk(1040)).unwrap();
    sio.drives[0] = Some(d);
    // An 810 cannot read an enhanced-density disk.
    let t = command(&mut sio, 0, 0x31, b'R', 1);
    assert_eq!(bytes(&receive(&mut sio, t, 2_000_000))[1], b'E');
    let mut sio = Sio::new(HZ);
    let mut d = SioDrive::new(DriveModel::A1050);
    d.insert(disk(1040)).unwrap();
    sio.drives[0] = Some(d);
    let t = command(&mut sio, 0, 0x31, b'S', 0);
    assert_eq!(
        bytes(&receive(&mut sio, t, 100_000))[2] & 0x80,
        0x80,
        "enhanced"
    );
    let t = command(&mut sio, 200_000, 0x31, b'R', 1000);
    assert_eq!(bytes(&receive(&mut sio, t, 4_000_000))[2], 1000u32 as u8);
}

#[test]
fn boot_sector_override() {
    let mut sio = Sio::new(HZ);
    sio.set_boot_sector(Some(vec![0x00, 0x01, 0x00, 0x07, 0x06, 0x07]));
    let t = command(&mut sio, 0, 0x31, b'R', 1);
    let r = bytes(&receive(&mut sio, t, 1_000_000));
    assert_eq!(&r[..4], [b'A', b'C', 0x00, 0x01]);
    assert_eq!(r.len(), 131);
    sio.set_boot_sector(None);
    let t = command(&mut sio, 2_000_000, 0x31, b'S', 0);
    assert!(receive(&mut sio, t, 100_000).is_empty(), "no drive D1");
}

#[test]
fn serial_line_shape() {
    let mut tx = SerialTx::new(HZ);
    tx.send(100, &[0x01]);
    assert!(tx.tick(99).level, "idle mark");
    let start = tx.tick(100);
    assert!(!start.level && start.busy, "start bit");
    let bit = u64::from(HZ) / BAUD;
    assert!(tx.tick(100 + bit + 2).level, "bit 0 = 1");
    assert!(!tx.tick(100 + 2 * bit + 2).level, "bit 1 = 0");
    let done = (100..100 + 11 * bit).find_map(|t| tx.tick(t).byte);
    assert_eq!(done, Some(0x01));
}

#[test]
fn savestate_round_trip() {
    let mut sio = bus(DriveModel::A1050);
    let t = command(&mut sio, 0, 0x31, b'R', 100);
    let mut w = StateWriter::new();
    sio.save(&mut w);
    let state = w.into_bytes();
    let mut other = bus(DriveModel::A1050);
    other.load(&mut StateReader::new(&state)).unwrap();
    let a = receive(&mut sio, t, 4_000_000);
    let b = receive(&mut other, t, 4_000_000);
    assert_eq!(a, b);
    let mut wrong = bus(DriveModel::A810);
    assert!(wrong.load(&mut StateReader::new(&state)).is_err());
}

#[test]
fn drive_status_follows_the_transfer() {
    let mut sio = bus(DriveModel::A1050);
    let st = |sio: &Sio| MediaSlot::status(sio.drives[0].as_ref().unwrap());
    assert_eq!(
        st(&sio),
        DriveStatus {
            active: false,
            error: false,
            track: Some(1)
        }
    );
    // Sector 360 is on track 19 (0-based): busy until the data frame ends.
    let t = command(&mut sio, 0, 0x31, b'R', 360);
    assert!(st(&sio).active);
    let r = receive(&mut sio, t, 4_000_000);
    assert_eq!(r.len(), 2 + 128 + 1);
    assert_eq!(
        st(&sio),
        DriveStatus {
            active: false,
            error: false,
            track: Some(20)
        }
    );
    // Busy survives a savestate taken mid-transfer.
    let t = command(&mut sio, 4_000_000, 0x31, b'R', 1);
    let mut w = StateWriter::new();
    sio.save(&mut w);
    let state = w.into_bytes();
    let mut other = bus(DriveModel::A1050);
    other.load(&mut StateReader::new(&state)).unwrap();
    assert!(st(&other).active);
    receive(&mut sio, t, 4_000_000);
    // A refused sector leaves an error until a command succeeds.
    let t = command(&mut sio, 8_000_000, 0x31, b'R', 0);
    receive(&mut sio, t, 100_000);
    assert!(st(&sio).error && !st(&sio).active);
    let t = command(&mut sio, 8_200_000, 0x31, b'R', 1);
    receive(&mut sio, t, 4_000_000);
    assert!(!st(&sio).error);
}
