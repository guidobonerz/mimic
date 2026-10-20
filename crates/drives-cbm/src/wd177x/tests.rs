use super::*;
use media::mfm::{self, decode_track, format_track};

/// A drive under the controller: tracks per cylinder (one side), the head
/// position and the rotation, one microsecond per tick.
struct Drive {
    tracks: Vec<Option<Vec<u16>>>,
    cylinder: u8,
    us: u32,
    protected: bool,
    steps: u32,
}

impl Drive {
    fn new() -> Self {
        let tracks = (0..80u8)
            .map(|c| {
                let sectors: Vec<[u8; 512]> = (0..10u8).map(|r| [c ^ r; 512]).collect();
                Some(format_track(c, 0, &sectors))
            })
            .collect();
        Self {
            tracks,
            cylinder: 0,
            us: 0,
            protected: false,
            steps: 0,
        }
    }

    fn pos(&self) -> usize {
        (self.us / BYTE_US) as usize % TRACK_BYTES
    }

    fn pulse(&self) -> Pulse {
        let byte = self.us.is_multiple_of(BYTE_US);
        Pulse {
            byte,
            index: byte && self.pos() == 0,
            index_active: self.pos() < 125,
        }
    }
}

impl FloppyIo for Drive {
    fn read(&self) -> Option<u16> {
        self.tracks
            .get(usize::from(self.cylinder))?
            .as_ref()
            .map(|t| t[self.pos()])
    }
    fn write(&mut self, byte: u16) {
        let pos = self.pos();
        if let Some(t) = self.tracks.get_mut(usize::from(self.cylinder)) {
            t.get_or_insert_with(|| vec![0x4e; TRACK_BYTES])[pos] = byte;
        }
    }
    fn step(&mut self, outward: bool) {
        self.steps += 1;
        if outward {
            self.cylinder = self.cylinder.saturating_sub(1);
        } else if self.cylinder < 82 {
            self.cylinder += 1;
        }
    }
    fn track0(&self) -> bool {
        self.cylinder == 0
    }
    fn protected(&self) -> bool {
        self.protected
    }
}

fn tick(wd: &mut Wd177x, d: &mut Drive) {
    let p = d.pulse();
    wd.tick(d, p);
    d.us += 1;
}

/// Run until the command is done (or `max` µs); returns the µs taken.
fn run(wd: &mut Wd177x, d: &mut Drive, max: u32) -> u32 {
    for t in 0..max {
        if !wd.busy() {
            return t;
        }
        tick(wd, d);
    }
    panic!("command still busy after {max} µs");
}

/// Read a sector; returns the bytes and the final status.
fn read_sector(wd: &mut Wd177x, d: &mut Drive, cmd: u8) -> (Vec<u8>, u8) {
    wd.write(0, cmd);
    let mut out = Vec::new();
    for _ in 0..2_000_000 {
        if !wd.busy() {
            break;
        }
        tick(wd, d);
        if wd.drq() {
            out.push(wd.read(3, d));
        }
    }
    (out, wd.read(0, d))
}

#[test]
fn restore_seek_and_step() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    d.cylinder = 5;
    // Restore with h (no spin-up), 3 ms steps.
    wd.write(0, 0x0b);
    let t = run(&mut wd, &mut d, 1_000_000);
    assert_eq!((d.cylinder, wd.track), (0, 0));
    assert!((15_000..16_000).contains(&t), "5 steps of 3 ms: {t} µs");
    let st = wd.read(0, &d);
    assert_eq!(st & 0x05, 0x04, "not busy, track 0");
    assert_ne!(st & 0x80, 0, "motor on");
    // Seek to 40 with verify.
    wd.write(3, 40);
    wd.write(0, 0x1f);
    run(&mut wd, &mut d, 2_000_000);
    assert_eq!((d.cylinder, wd.track), (40, 40));
    assert_eq!(wd.read(0, &d) & 0x18, 0, "verified");
    // Step in without update, then step out with update.
    wd.write(0, 0x4b);
    run(&mut wd, &mut d, 100_000);
    assert_eq!((d.cylinder, wd.track), (41, 40));
    wd.write(0, 0x7b);
    run(&mut wd, &mut d, 100_000);
    assert_eq!((d.cylinder, wd.track), (40, 39));
    // Verify against a wrong track register: seek error.
    wd.write(0, 0x5f);
    run(&mut wd, &mut d, 2_000_000);
    assert_ne!(wd.read(0, &d) & 0x10, 0, "seek error");
}

#[test]
fn spin_up_waits_six_index_pulses() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    wd.write(0, 0x03);
    let t = run(&mut wd, &mut d, 2_000_000);
    assert!((1_000_000..1_250_000).contains(&t), "{t} µs");
    assert_ne!(wd.read(0, &d) & 0x20, 0, "spin-up done");
    // Motor goes off 9 revolutions later.
    for _ in 0..1_900_000 {
        tick(&mut wd, &mut d);
    }
    assert!(!wd.motor());
}

#[test]
fn read_sector_and_multiple() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    d.cylinder = 7;
    wd.track = 7;
    wd.sector = 3;
    let (data, st) = read_sector(&mut wd, &mut d, 0x88);
    assert_eq!(data, vec![7 ^ 2; 512]);
    assert_eq!(st & 0x1f, 0, "{st:02x}");
    // Multiple: sectors 9 and 10, then record not found ends it.
    wd.sector = 9;
    let (data, st) = read_sector(&mut wd, &mut d, 0x98);
    assert_eq!(data.len(), 1024);
    assert_eq!(&data[512..], &[7 ^ 9; 512]);
    assert_ne!(st & 0x10, 0);
    // A missing sector: record not found after 5 revolutions.
    wd.sector = 11;
    let (data, st) = read_sector(&mut wd, &mut d, 0x88);
    assert!(data.is_empty());
    assert_ne!(st & 0x10, 0);
}

#[test]
fn slow_reader_loses_data() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    wd.write(0, 0x88);
    run(&mut wd, &mut d, 2_000_000);
    assert_ne!(wd.read(0, &d) & 0x04, 0, "lost data");
}

#[test]
fn write_sector_reaches_the_track() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    d.cylinder = 3;
    wd.track = 3;
    wd.sector = 5;
    wd.write(0, 0xaa);
    let mut n = 0u32;
    for _ in 0..2_000_000 {
        if !wd.busy() {
            break;
        }
        tick(&mut wd, &mut d);
        if wd.drq() {
            wd.write(3, n as u8);
            n += 1;
        }
    }
    assert_eq!(n, 512);
    assert_eq!(wd.read(0, &d) & 0x5c, 0);
    let sectors = decode_track(d.tracks[3].as_ref().unwrap());
    assert_eq!(sectors.len(), 10);
    let s5 = sectors.iter().find(|s| s.sector == 5).unwrap();
    assert_eq!(s5.data, (0..512).map(|i| i as u8).collect::<Vec<_>>());
    // Write protect stops it before anything is written.
    d.protected = true;
    wd.write(0, 0xaa);
    run(&mut wd, &mut d, 100);
    assert_ne!(wd.read(0, &d) & 0x40, 0);
}

#[test]
fn write_track_formats_like_the_1581() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    d.tracks[12] = None;
    d.cylinder = 12;
    wd.track = 12;
    // The byte sequence the 1581 DOS sends (format_track's layout, with
    // $F5 for the marks and $F7 for each CRC).
    let mut feed: Vec<u8> = vec![0x4e; 32];
    for r in 1..=10u8 {
        feed.extend([0u8; 12]);
        feed.extend([0xf5; 3]);
        feed.extend([0xfe, 12, 1, r, 2, 0xf7]);
        feed.extend([0x4e; 22]);
        feed.extend([0u8; 12]);
        feed.extend([0xf5; 3]);
        feed.push(0xfb);
        feed.extend([r; 512]);
        feed.push(0xf7);
        feed.extend([0x4e; 35]);
    }
    wd.write(0, 0xf8);
    let mut i = 0;
    for _ in 0..1_000_000 {
        if !wd.busy() {
            break;
        }
        if wd.drq() {
            wd.write(3, feed.get(i).copied().unwrap_or(0x4e));
            i += 1;
        }
        tick(&mut wd, &mut d);
    }
    assert!(!wd.busy());
    assert_eq!(wd.read(0, &d) & 0x04, 0, "no lost data");
    let sectors = decode_track(d.tracks[12].as_ref().unwrap());
    assert_eq!(sectors.len(), 10);
    assert!(sectors
        .iter()
        .all(|s| s.track == 12 && s.side == 1 && s.data == vec![s.sector; 512]));
    // Read address returns an ID of the new format.
    wd.write(0, 0xc8);
    let mut id = Vec::new();
    for _ in 0..1_000_000 {
        if !wd.busy() {
            break;
        }
        tick(&mut wd, &mut d);
        if wd.drq() {
            id.push(wd.read(3, &d));
        }
    }
    assert_eq!(&id[..4], &[12, 1, id[2], 2]);
    assert_eq!(wd.sector, 12, "track number into the sector register");
    assert_eq!(wd.read(0, &d) & 0x08, 0, "CRC right");
    let _ = mfm::SECTORS;
}

#[test]
fn force_interrupt_stops_a_search() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    wd.sector = 99;
    wd.write(0, 0x88);
    for _ in 0..1000 {
        tick(&mut wd, &mut d);
    }
    assert!(wd.busy());
    wd.write(0, 0xd0);
    assert!(!wd.busy());
}

#[test]
fn savestate_round_trip() {
    let mut wd = Wd177x::new();
    let mut d = Drive::new();
    wd.write(0, 0x88);
    for _ in 0..50_000 {
        tick(&mut wd, &mut d);
    }
    let mut w = StateWriter::new();
    wd.save(&mut w);
    let data = w.into_bytes();
    let mut back = Wd177x::new();
    back.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(back, wd);
    assert!(back.load(&mut StateReader::new(&data[..10])).is_err());
}
