use super::*;

/// A drive with a 40-track single-sided disc: 9 sectors &C1–&C9 of 512
/// bytes, each filled with its record number.
#[derive(Clone)]
struct TestDrive {
    cyl: u8,
    disc: bool,
    protected: bool,
    data: Vec<Vec<Vec<u8>>>,
    ids: Vec<Vec<[u8; 4]>>,
}

impl TestDrive {
    fn new() -> Self {
        let order = [0xc1u8, 0xc6, 0xc2, 0xc7, 0xc3, 0xc8, 0xc4, 0xc9, 0xc5];
        Self {
            cyl: 5,
            disc: true,
            protected: false,
            data: (0..40)
                .map(|_| order.iter().map(|&r| vec![r; 512]).collect())
                .collect(),
            ids: (0..40u8)
                .map(|t| order.iter().map(|&r| [t, 0, r, 2]).collect())
                .collect(),
        }
    }
}

impl FloppyDrive for TestDrive {
    fn ready(&self) -> bool {
        self.disc
    }
    fn write_protected(&self) -> bool {
        self.protected
    }
    fn two_sided(&self) -> bool {
        false
    }
    fn cylinder(&self) -> u8 {
        self.cyl
    }
    fn step(&mut self, outward: bool) {
        if outward {
            self.cyl = self.cyl.saturating_sub(1);
        } else {
            self.cyl = (self.cyl + 1).min(41);
        }
    }
    fn sectors(&self, side: u8) -> Vec<SectorInfo> {
        if side != 0 || usize::from(self.cyl) >= self.ids.len() {
            return Vec::new();
        }
        self.ids[usize::from(self.cyl)]
            .iter()
            .map(|&id| SectorInfo { id, st1: 0, st2: 0 })
            .collect()
    }
    fn read(&self, _side: u8, index: usize) -> Vec<u8> {
        self.data[usize::from(self.cyl)][index].clone()
    }
    fn write(&mut self, _side: u8, index: usize, data: &[u8]) -> bool {
        self.data[usize::from(self.cyl)][index] = data.to_vec();
        true
    }
    fn format(&mut self, _side: u8, n: u8, _gap3: u8, filler: u8, ids: &[[u8; 4]]) -> bool {
        let c = usize::from(self.cyl);
        self.ids[c] = ids.to_vec();
        self.data[c] = ids.iter().map(|_| vec![filler; 128 << n]).collect();
        true
    }
}

struct Rig {
    fdc: Fdc,
    drives: [TestDrive; 2],
}

impl Rig {
    fn new() -> Self {
        let mut r = Self {
            fdc: Fdc::new(),
            drives: [TestDrive::new(), TestDrive::new()],
        };
        r.fdc.motor = true;
        r
    }

    fn tick(&mut self, us: u32) {
        for _ in 0..us {
            self.fdc.tick(&mut self.drives);
        }
    }

    fn command(&mut self, bytes: &[u8]) {
        for &b in bytes {
            assert_eq!(self.fdc.status() & (RQM | DIO), RQM, "ready for a command");
            self.fdc.write_data(b, &mut self.drives);
        }
    }

    /// Wait for the result phase; collect what the execution phase sends.
    fn run_read(&mut self) -> (Vec<u8>, Vec<u8>) {
        let mut data = Vec::new();
        for _ in 0..1_000_000 {
            let s = self.fdc.status();
            if s & (RQM | DIO | EXM) == RQM | DIO | EXM {
                data.push(self.fdc.read_data());
            } else if s & (RQM | DIO | EXM) == RQM | DIO {
                break;
            }
            self.tick(1);
        }
        let mut result = Vec::new();
        while self.fdc.status() & DIO != 0 {
            result.push(self.fdc.read_data());
        }
        (data, result)
    }

    fn sense_interrupt(&mut self) -> Vec<u8> {
        self.command(&[0x08]);
        let mut r = Vec::new();
        while self.fdc.status() & DIO != 0 {
            r.push(self.fdc.read_data());
        }
        r
    }
}

#[test]
fn recalibrate_and_seek() {
    let mut r = Rig::new();
    r.command(&[0x03, 0xa1, 0x03]); // step 12 ms at 4 MHz
    r.command(&[0x07, 0x00]);
    assert_eq!(r.fdc.status() & 1, 1, "drive A seeking");
    assert_eq!(r.sense_interrupt(), vec![0x80], "nothing yet");
    r.tick(6 * 12_000 + 10);
    assert_eq!(r.drives[0].cyl, 0);
    assert_eq!(r.sense_interrupt(), vec![0x20, 0]);
    r.command(&[0x0f, 0x00, 7]);
    r.tick(8 * 12_000);
    assert_eq!(r.sense_interrupt(), vec![0x20, 7]);
    assert_eq!(r.drives[0].cyl, 7);
}

#[test]
fn read_data_one_sector() {
    let mut r = Rig::new();
    r.drives[0].cyl = 3;
    // C=3 H=0 R=&C2 N=2 EOT=&C2.
    r.command(&[0x46, 0x00, 3, 0, 0xc2, 2, 0xc2, 0x2a, 0xff]);
    let (data, result) = r.run_read();
    assert_eq!(data, vec![0xc2; 512]);
    // End of cylinder (no TC): C+1, R=1.
    assert_eq!(result, vec![0x40, 0x80, 0x00, 4, 0, 1, 2]);
}

#[test]
fn read_data_several_sectors_takes_rotation_time() {
    let mut r = Rig::new();
    r.drives[0].cyl = 0;
    r.command(&[0x46, 0x00, 0, 0, 0xc1, 2, 0xc3, 0x2a, 0xff]);
    let start = r.fdc.rotation;
    let (data, result) = r.run_read();
    assert_eq!(data.len(), 3 * 512);
    assert_eq!(&data[512..516], &[0xc2; 4]);
    assert_eq!(result[1], EN);
    // C1, C2, C3 lie at positions 0, 2 and 4 of 9: under a turn.
    let elapsed = (r.fdc.rotation + REV_US - start) % REV_US;
    assert!(elapsed > 4 * REV_US / 9, "{elapsed}");
}

#[test]
fn missing_sector_and_not_ready() {
    let mut r = Rig::new();
    r.command(&[0x46, 0x00, 5, 0, 0x41, 2, 0x41, 0x2a, 0xff]);
    let (data, result) = r.run_read();
    assert!(data.is_empty());
    assert_eq!(&result[..3], &[0x40, ND, 0]);
    r.drives[1].disc = false;
    r.command(&[0x46, 0x01, 5, 0, 0xc1, 2, 0xc1, 0x2a, 0xff]);
    let (_, result) = r.run_read();
    assert_eq!(result[0], 0x40 | NR | 1);
}

#[test]
fn overrun_when_the_cpu_is_late() {
    let mut r = Rig::new();
    r.command(&[0x46, 0x00, 5, 0, 0xc1, 2, 0xc1, 0x2a, 0xff]);
    // Never read the data.
    for _ in 0..(2 * REV_US) {
        r.tick(1);
        if r.fdc.status() & EXM == 0 {
            break;
        }
    }
    let mut result = Vec::new();
    while r.fdc.status() & DIO != 0 {
        result.push(r.fdc.read_data());
    }
    assert_eq!(result[1] & OR, OR);
}

#[test]
fn write_then_read_back() {
    let mut r = Rig::new();
    r.command(&[0x45, 0x00, 5, 0, 0xc4, 2, 0xc4, 0x2a, 0xff]);
    let mut n = 0u32;
    for _ in 0..1_000_000 {
        let s = r.fdc.status();
        if s & (RQM | DIO | EXM) == RQM | EXM {
            r.fdc.write_data(n as u8, &mut r.drives);
            n += 1;
        } else if s & EXM == 0 {
            break;
        }
        r.tick(1);
    }
    while r.fdc.status() & DIO != 0 {
        r.fdc.read_data();
    }
    assert_eq!(n, 512);
    r.command(&[0x46, 0x00, 5, 0, 0xc4, 2, 0xc4, 0x2a, 0xff]);
    let (data, _) = r.run_read();
    assert_eq!(data[..4], [0, 1, 2, 3]);
    r.drives[0].protected = true;
    r.command(&[0x45, 0x00, 5, 0, 0xc4, 2, 0xc4, 0x2a, 0xff]);
    let (_, result) = r.run_read();
    assert_eq!(&result[..2], &[0x40, NW]);
}

#[test]
fn format_and_read_id() {
    let mut r = Rig::new();
    // N=2, 4 sectors, GPL &52, filler &AA.
    r.command(&[0x4d, 0x00, 2, 4, 0x52, 0xaa]);
    let mut k = 0;
    for _ in 0..1_000_000 {
        let s = r.fdc.status();
        if s & (RQM | DIO | EXM) == RQM | EXM {
            let id = [5, 0, 0x41 + k / 4, 2];
            r.fdc.write_data(id[usize::from(k % 4)], &mut r.drives);
            k += 1;
        } else if s & EXM == 0 {
            break;
        }
        r.tick(1);
    }
    while r.fdc.status() & DIO != 0 {
        r.fdc.read_data();
    }
    assert_eq!(r.drives[0].ids[5].len(), 4);
    r.command(&[0x4a, 0x00]);
    let (_, result) = r.run_read();
    assert_eq!(result[0] & 0xc0, 0);
    assert!((0x41..=0x44).contains(&result[5]), "{result:?}");
}

#[test]
fn sense_drive_status_and_invalid() {
    let mut r = Rig::new();
    r.drives[0].cyl = 0;
    r.drives[0].protected = true;
    r.command(&[0x04, 0x00]);
    assert_eq!(r.fdc.read_data(), 0x70);
    r.command(&[0x10]);
    assert_eq!(r.fdc.read_data(), 0x80, "VERSION is invalid on the 765A");
}

#[test]
fn savestate_round_trip_mid_transfer() {
    let mut r = Rig::new();
    r.command(&[0x46, 0x00, 5, 0, 0xc1, 2, 0xc1, 0x2a, 0xff]);
    r.tick(5000);
    let mut w = StateWriter::new();
    r.fdc.save(&mut w);
    let bytes = w.into_bytes();
    let mut f = Fdc::new();
    f.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(f, r.fdc);
}
