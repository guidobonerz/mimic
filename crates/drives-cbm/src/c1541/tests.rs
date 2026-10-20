use super::*;
use emu_core::media::MediaBackend;

struct Mem(Vec<u8>);

impl MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        buf.copy_from_slice(&self.0[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Err(MediaError::WriteProtected)
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn disk() -> Media {
    Media {
        name: "test.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Mem(D64::blank(b"TEST", *b"AB").to_bytes())),
    }
}

/// A ROM (not the Commodore DOS) with `code` at $C000 as reset entry.
fn drive_with(code: &[u8]) -> CbmDrive {
    let mut rom = Box::new([0xea; ROM_SIZE]);
    rom[..code.len()].copy_from_slice(code);
    rom[0x3ffc..].copy_from_slice(&[0x00, 0xc0, 0x00, 0xc0]);
    CbmDrive::new(DriveModel::C1541, rom, 8, MemSpaceId(3))
}

fn run(d: &mut CbmDrive, bus: IecLines, n: usize) {
    for _ in 0..n {
        d.tick(bus, None);
    }
}

#[test]
fn reads_gcr_bytes_with_byte_ready_on_so() {
    // Motor on, SOE (CA2 high), then collect 16 bytes via BVC loops into $0300.
    let code = [
        0xa9, 0x6f, 0x8d, 0x02, 0x1c, // LDA #$6F; STA $1C02 (DDRB)
        0xa9, 0x04, 0x8d, 0x00, 0x1c, // LDA #$04; STA $1C00 (motor on)
        0xa9, 0x0e, 0x8d, 0x0c, 0x1c, // LDA #$0E; STA $1C0C (CA2 high: SOE)
        0xa2, 0x00, // LDX #0
        0xb8, // C011: CLV
        0x50, 0xfe, // C012: BVC *
        0xad, 0x01, 0x1c, // LDA $1C01
        0x9d, 0x00, 0x03, // STA $0300,X
        0xe8, // INX
        0xe0, 0x10, // CPX #16
        0xd0, 0xf2, // BNE $C011
        0x4c, 0x1f, 0xc0, // C01F: JMP *
    ];
    let mut d = drive_with(&code);
    d.insert(disk()).unwrap();
    run(&mut d, IecLines::default(), 20_000);
    assert!((0xc01f..=0xc021).contains(&d.pc()), "loop finished");
    let got: Vec<u8> = d.ram[0x300..0x310].to_vec();
    // Track 18 GCR stream without the SYNC bytes, from wherever the head was.
    let track = gcr::encode_track(&D64::blank(b"TEST", *b"AB"), 18);
    let stream: Vec<u8> = track
        .iter()
        .enumerate()
        .filter(|&(i, &b)| !(b == 0xff && track[(i + track.len() - 1) % track.len()] == 0xff))
        .map(|(_, &b)| b)
        .collect();
    assert!(
        stream.windows(16).any(|w| w == got.as_slice()),
        "{got:02x?} is a run of the track's data bytes"
    );
}

#[test]
fn sync_flag_and_write_protect_on_via2() {
    let code = [
        0xa9, 0x6f, 0x8d, 0x02, 0x1c, // DDRB
        0xa9, 0x04, 0x8d, 0x00, 0x1c, // motor on
        0x4c, 0x0a, 0xc0, // JMP *
    ];
    let mut d = drive_with(&code);
    d.insert(disk()).unwrap();
    let mut saw_sync = false;
    for _ in 0..20_000 {
        d.tick(IecLines::default(), None);
        let pb = d.peek(MemSpaceId(3), 0x1c00, Size::Byte).unwrap();
        assert_eq!(pb & 0x10, 0x10, "writable");
        saw_sync |= pb & 0x80 == 0;
    }
    assert!(saw_sync);
    assert!(d.motor_on());
    d.write_protect(true);
    let pb = d.peek(MemSpaceId(3), 0x1c00, Size::Byte).unwrap();
    assert_eq!(pb & 0x10, 0, "notch covered");
}

/// A disk whose backend keeps what the drive writes back.
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

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

#[test]
fn writing_a_track_reaches_the_image() {
    // Write mode, then a whole track 18 as the DOS formats it, from the GCR
    // encoder (sector 1 with a new pattern), byte by byte on byte ready.
    let mut image = D64::blank(b"TEST", *b"AB");
    image.sector_mut(18, 1).unwrap()[..4].copy_from_slice(b"NEW!");
    let track = gcr::encode_track(&image, 18);
    let backing = std::sync::Arc::new(std::sync::Mutex::new(
        D64::blank(b"TEST", *b"AB").to_bytes(),
    ));
    let mut d = drive_with(&[0x4c, 0x00, 0xc0]);
    d.insert(Media {
        name: "w.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Shared(backing.clone())),
    })
    .unwrap();
    // Motor on, head stays on track 18; DDRA out, CB2 low (write).
    d.via2.write(0x2, 0x6f);
    d.via2.write(0x0, 0x04);
    d.via2.write(0x3, 0xff);
    d.via2.write(0xc, 0xc0);
    let mut written = 0;
    let mut cycles = 0;
    while written < track.len() && cycles < 400_000 {
        d.via2.write(0x1, track[written]);
        let before = d.mech.position;
        d.tick(IecLines::default(), None);
        if d.mech.position != before {
            written += 1;
        }
        cycles += 1;
    }
    assert_eq!(written, track.len());
    // Motor off: the track is decoded back into the image and written.
    d.via2.write(0x0, 0x00);
    d.tick(IecLines::default(), None);
    let disk = D64::parse(&backing.lock().unwrap()).unwrap();
    assert_eq!(&disk.sector(18, 1).unwrap()[..4], b"NEW!");
}

#[test]
fn stepper_moves_the_head_in_half_tracks() {
    // DDRB = $6F; write phases 1, 2, 3 (step in) then 2 (step out).
    let code = [
        0xa9, 0x6f, 0x8d, 0x02, 0x1c, // DDRB
        0xa9, 0x01, 0x8d, 0x00, 0x1c, //
        0xa9, 0x02, 0x8d, 0x00, 0x1c, //
        0xa9, 0x03, 0x8d, 0x00, 0x1c, //
        0xa9, 0x02, 0x8d, 0x00, 0x1c, //
        0x4c, 0x19, 0xc0, // JMP *
    ];
    let mut d = drive_with(&code);
    assert_eq!(d.track(), 18.0);
    run(&mut d, IecLines::default(), 100);
    assert_eq!(d.track(), 19.0, "three steps in, one out");
}

#[test]
fn atn_is_acknowledged_by_hardware() {
    let mut d = drive_with(&[0x4c, 0x00, 0xc0]);
    // As the DOS sets it up: PB1 DATA out, PB3 CLK out, PB4 ATNA as outputs.
    d.via1.write(0x2, 0x1a);
    d.via1.write(0x0, 0x10); // ATNA = 1, DATA/CLK released
    assert!(d.iec_out(false).data, "ATN released, ATNA set: pulled");
    assert!(!d.iec_out(true).data, "ATN asserted, ATNA set: released");
    d.via1.write(0x0, 0x00); // ATNA = 0
    assert!(
        d.iec_out(true).data,
        "ATN asserted, not acknowledged: pulled"
    );
    assert!(!d.iec_out(false).data);
    d.via1.write(0x0, 0x0a); // DATA and CLK out
    let out = d.iec_out(false);
    assert!(out.data && out.clk);
}
#[test]
fn atn_raises_ca1_interrupt() {
    let mut d = drive_with(&[0x4c, 0x00, 0xc0]);
    d.via1.write(0xc, 0x01); // CA1 positive edge
    d.via1.write(0xe, 0x82); // enable CA1 interrupt
    run(&mut d, IecLines::default(), 5);
    assert!(!d.via1.irq());
    run(
        &mut d,
        IecLines {
            atn: true,
            ..IecLines::default()
        },
        1,
    );
    assert!(d.via1.irq());
}

#[test]
fn device_number_jumpers() {
    let d = CbmDrive::new(DriveModel::C1541, Box::new([0; ROM_SIZE]), 9, MemSpaceId(3));
    let (_, pb) = d.via1_pins(IecLines::default());
    assert_eq!((pb >> 5) & 3, 1);
}

#[test]
fn rejects_non_disks_and_bad_images() {
    let mut d = drive_with(&[0x4c, 0x00, 0xc0]);
    let tape = Media {
        name: "x".into(),
        kind: MediaKind::Tape,
        backend: Box::new(Mem(vec![])),
    };
    assert!(matches!(d.insert(tape), Err(MediaError::Unsupported(_))));
    let bad = Media {
        name: "x".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Mem(vec![0; 100])),
    };
    assert!(d.insert(bad).is_err());
    assert!(d.eject().is_none());
}

#[test]
fn savestate_round_trip() {
    let mut a = drive_with(&[0xe6, 0x10, 0x4c, 0x00, 0xc0]); // INC $10; JMP $C000
    run(&mut a, IecLines::default(), 333);
    let mut w = StateWriter::new();
    a.save(&mut w);
    let data = w.into_bytes();
    let mut b = drive_with(&[0xe6, 0x10, 0x4c, 0x00, 0xc0]);
    b.load(&mut StateReader::new(&data)).unwrap();
    run(&mut a, IecLines::default(), 100);
    run(&mut b, IecLines::default(), 100);
    assert_eq!(a.ram[..], b.ram[..]);
    assert_eq!(a.pc(), b.pc());
    assert!(b.load(&mut StateReader::new(&data[..50])).is_err());
}

/// A 1571 ROM (not the Commodore DOS) with `code` at $8000 as reset entry.
fn c1571_with(code: &[u8]) -> CbmDrive {
    let mut rom = vec![0xea; 0x8000];
    rom[..code.len()].copy_from_slice(code);
    rom[0x7ffc..].copy_from_slice(&[0x00, 0x80, 0x00, 0x80]);
    CbmDrive::new(DriveModel::C1571, rom.into(), 8, MemSpaceId(3))
}

#[test]
fn c1571_runs_at_2_mhz_when_pa5_is_set() {
    // DDRA = $66, PA5 per the low bit of $10; then count in $11.
    let code = [
        0xa9, 0x66, 0x8d, 0x03, 0x18, // LDA #$66; STA $1803
        0xa5, 0x10, 0x8d, 0x01, 0x18, // LDA $10; STA $1801
        0xe6, 0x11, // 800A: INC $11
        0x4c, 0x0a, 0x80, // JMP $800A
    ];
    let mut slow = c1571_with(&code);
    let mut fast = c1571_with(&code);
    fast.ram[0x10] = 0x20;
    run(&mut slow, IecLines::default(), 800);
    run(&mut fast, IecLines::default(), 800);
    assert!(!slow.fast_clock() && fast.fast_clock());
    let (s, f) = (slow.ram[0x11], fast.ram[0x11]);
    assert!(f > 2 * s - 4 && f < 2 * s + 4, "{s} vs {f} loops");
}

#[test]
fn c1571_track_0_sensor_and_side_select() {
    let mut d = c1571_with(&[0x4c, 0x00, 0x80]);
    d.via1.write(0x3, 0x66);
    let pa = |d: &CbmDrive| d.peek(MemSpaceId(3), 0x180f, Size::Byte).unwrap();
    assert_eq!(pa(&d) & 0x01, 0x01, "track 18");
    d.mech.half_track = 0;
    assert_eq!(pa(&d) & 0x01, 0x00, "track 1");
    // Outputs read back as driven.
    d.via1.write(0x1, 0x24);
    assert_eq!(pa(&d) & 0x66, 0x24);
    assert_eq!(d.side(), 1);
    d.via1.write(0x1, 0x00);
    assert_eq!(d.side(), 0);
}

#[test]
fn c1571_reads_side_1_of_a_d71() {
    let mut image = D64::blank_d71(b"SIDES", *b"DS");
    image.sector_mut(53, 1).unwrap()[..4].copy_from_slice(b"SIDE");
    let mut d = c1571_with(&[0x4c, 0x00, 0x80]);
    d.insert(Media {
        name: "t.d71".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Mem(image.to_bytes())),
    })
    .unwrap();
    // Head on track 18 of side 1 = image track 53.
    let side1 = &d.disk.as_ref().unwrap().tracks[track_index(1, 34)];
    assert_eq!(side1.as_ref(), Some(&gcr::encode_track(&image, 53)));
    let sectors = gcr::decode_track(side1.as_ref().unwrap());
    assert!(sectors
        .iter()
        .any(|s| s.track == 53 && s.sector == 1 && s.data.starts_with(b"SIDE")));
}

#[test]
fn c1571_fast_serial_drives_data_and_srq() {
    let mut d = c1571_with(&[0x4c, 0x00, 0x80]);
    d.via1.write(0x3, 0x66);
    // Port B as the DOS sets it up, nothing pulled (no ATN acknowledge).
    d.via1.write(0x2, 0x1a);
    // CIA serial port out, timer A 2 cycles, FSDIR clear: lines untouched.
    d.cia.write(0x4, 2);
    d.cia.write(0x5, 0);
    d.cia.write(0xe, 0x51);
    d.cia.write(0xc, 0x00);
    let mut pulled = IecLines::default();
    for _ in 0..40 {
        d.tick(IecLines::default(), None);
        let out = d.iec_out(false);
        pulled.data |= out.data;
        pulled.srq |= out.srq;
    }
    assert!(!pulled.data && !pulled.srq, "FSDIR clear");
    // FSDIR set: the byte $00 pulls DATA, the clock toggles SRQ.
    d.via1.write(0x1, 0x02);
    d.cia.write(0xc, 0x00);
    let mut srq_edges = 0;
    let mut last = false;
    for _ in 0..60 {
        d.tick(IecLines::default(), None);
        let out = d.iec_out(false);
        pulled.data |= out.data;
        srq_edges += usize::from(out.srq != last);
        last = out.srq;
    }
    assert!(pulled.data);
    assert!(srq_edges >= 16, "{srq_edges} SRQ edges");
}

#[test]
fn c1571_savestate_round_trip() {
    let code = [0xe6, 0x10, 0x4c, 0x00, 0x80]; // INC $10; JMP $8000
    let mut a = c1571_with(&code);
    a.cia.write(0x4, 0x34);
    run(&mut a, IecLines::default(), 333);
    let mut w = StateWriter::new();
    a.save(&mut w);
    let data = w.into_bytes();
    let mut b = c1571_with(&code);
    b.load(&mut StateReader::new(&data)).unwrap();
    let mut w2 = StateWriter::new();
    b.save(&mut w2);
    assert_eq!(w2.into_bytes(), data);
    // A 1541 state does not load into a 1571.
    let mut w3 = StateWriter::new();
    drive_with(&[0x4c, 0x00, 0xc0]).save(&mut w3);
    assert!(b.load(&mut StateReader::new(&w3.into_bytes())).is_err());
}

#[test]
fn d81_does_not_fit_a_1541() {
    let mut d = drive_with(&[0x4c, 0x00, 0xc0]);
    let d81 = Media {
        name: "x.d81".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Mem(D64::blank_d81(b"X", *b"XX").to_bytes())),
    };
    assert!(d.insert(d81).is_err());
}
