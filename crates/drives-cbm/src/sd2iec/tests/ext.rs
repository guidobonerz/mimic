//! Direct access, REL files, swap lists, `XI`/`XD`/`XR` and `M-R`.

use super::*;

/// A folder with `/disk.d64` (file INSIDE) and the given extra setup.
fn with_image(setup: impl FnOnce(&mut MemFileHost)) -> Host {
    let mut d = D64::blank(b"IMAGE", *b"IM");
    d.write_file(b"INSIDE", 2, &[0x01, 0x08, 0x42]).unwrap();
    let bytes = d.to_bytes();
    Host::new(move |m| {
        m.insert("/disk.d64", bytes).unwrap();
        setup(m);
    })
}

#[test]
fn direct_access_reads_and_writes_blocks() {
    let mut h = with_image(|_| {});
    h.open(2, b"#");
    // Blocks need an image.
    assert!(h.command(b"U1:2,0,18,0").starts_with("74"));
    assert!(h.command(b"CD:DISK.D64").starts_with("00"));
    assert!(h.command(b"U1:2,0,18,0").starts_with("00"));
    let bam = h.talk_receive(0x62);
    assert_eq!(bam.len(), 256);
    assert_eq!(&bam[..3], &[18, 1, 0x41]);
    // Three bytes at 1, then the block onto 1/0.
    assert!(h.command(b"B-P:2,1").starts_with("00"));
    h.listen_send(0x62, b"XYZ");
    assert!(h.command(b"U2 2 0 1 0").starts_with("00"));
    let image = D64::parse(&h.file("/disk.d64").unwrap()).unwrap();
    assert_eq!(&image.sector(1, 0).unwrap()[1..4], b"XYZ");
    // B-W stores the last byte's pointer in byte 0, B-R stops there.
    assert!(h.command(b"B-P 2 1").starts_with("00"));
    h.listen_send(0x62, b"AB");
    assert!(h.command(b"B-W:2,0,1,1").starts_with("00"));
    assert!(h.command(b"B-R:2,0,1,1").starts_with("00"));
    assert_eq!(h.talk_receive(0x62), b"AB");
    assert!(h
        .command(b"U1:2,0,40,0")
        .starts_with("66,ILLEGAL TRACK OR SECTOR,40,00"));
    assert!(h.command(b"U1:5,0,18,0").starts_with("70"));
    // The BAM.
    assert!(h.command(b"B-A:0,1,0").starts_with("00"));
    assert!(h.command(b"B-A:0,1,0").starts_with("65,NO BLOCK,01,01"));
    assert!(h.command(b"B-F:0,1,0").starts_with("00"));
    assert!(h.command(b"B-A:0,1,0").starts_with("00"));
    h.close(2);
    // A large buffer starts at byte 0.
    h.open(3, b"##2");
    assert!(h.command(b"U1:3,0,18,0").starts_with("00"));
    assert_eq!(h.talk_receive(0x63).len(), 512);
    h.close(3);
}

#[test]
fn rel_files_hold_records() {
    let mut h = standard();
    h.open(2, b"DATA,L,\x0a");
    assert!(h.status().starts_with("00"));
    let r00 = h.file("/data.r00").unwrap();
    assert_eq!((r00.len(), r00[25]), (26, 10));
    // Reading a record that is not there: CR and 50.
    assert!(h.command(b"P\x62\x01\x00\x01").starts_with("50"));
    assert_eq!(h.talk_receive(0x62), b"\r");
    assert!(h.status().starts_with("50,RECORD NOT PRESENT"));
    // Record 3 does not exist yet; writing it adds 1 and 2 empty.
    assert!(h
        .command(b"P\x62\x03\x00\x01")
        .starts_with("50,RECORD NOT PRESENT"));
    h.listen_send(0x62, b"HELLO\r");
    assert!(h.command(b"P\x62\x01\x00\x01").starts_with("00"));
    assert_eq!(h.talk_receive(0x62), [0xff]);
    assert!(h.command(b"P\x62\x03\x00\x01").starts_with("00"));
    assert_eq!(h.talk_receive(0x62), b"HELLO\r");
    // From byte 3 of the record.
    assert!(h.command(b"P\x62\x03\x00\x03").starts_with("00"));
    assert_eq!(h.talk_receive(0x62), b"LLO\r");
    // Too much for a record.
    assert!(h.command(b"P\x62\x02\x00\x01").starts_with("00"));
    h.listen_send(0x62, b"0123456789AB");
    assert!(h.status().starts_with("51,OVERFLOW IN RECORD"));
    h.close(2);
    let r00 = h.file("/data.r00").unwrap();
    assert_eq!(r00.len(), 26 + 30);
    assert_eq!(&r00[36..46], b"0123456789");
    // Opened again by name it is a REL file; the listing says so.
    h.open(3, b"DATA");
    assert!(h.command(b"P\x63\x03\x00\x01").starts_with("00"));
    assert_eq!(h.talk_receive(0x63), b"HELLO\r");
    h.close(3);
    let text = String::from_utf8_lossy(&h.load(b"$")).into_owned();
    assert!(text.contains("\"DATA\"             REL"), "{text}");
    // A wrong record length.
    h.open(4, b"DATA,L,\x05");
    assert!(h.status().starts_with("50"));
    // P on a plain file seeks to a byte.
    h.open(5, b"HELLO,P,R");
    assert!(h.command(b"P\x65\x02").starts_with("00"));
    assert_eq!(h.talk_receive(0x65), [0xaa, 0xbb]);
    h.close(5);
}

fn swap_folder() -> Host {
    let disk = |name: &[u8]| {
        let mut d = D64::blank(name, *b"SW");
        d.write_file(name, 2, &[0x01, 0x08, name[0]]).unwrap();
        d.to_bytes()
    };
    let (a, b) = (disk(b"ONE"), disk(b"TWO"));
    Host::new(move |m| {
        m.insert("/game/a.d64", a).unwrap();
        m.insert("/game/b.d64", b).unwrap();
        m.insert(
            "/game/disks.lst",
            b"; the game\r\na.d64\r\n\r\nb.d64\r\n".to_vec(),
        )
        .unwrap();
    })
}

fn press(h: &mut Host, prev: bool) {
    h.sd.button(prev, true);
    h.sd.button(prev, false);
}

#[test]
fn swap_lists_change_disks() {
    let mut h = swap_folder();
    assert!(h.command(b"CD:GAME").starts_with("00"));
    assert!(h.command(b"XS:DISKS.LST").starts_with("00"));
    assert_eq!(h.load(b"*"), [0x01, 0x08, b'O']);
    press(&mut h, false);
    assert_eq!(h.load(b"*"), [0x01, 0x08, b'T']);
    press(&mut h, false);
    assert_eq!(h.load(b"*"), [0x01, 0x08, b'O'], "NEXT wraps");
    press(&mut h, true);
    assert_eq!(h.load(b"*"), [0x01, 0x08, b'T'], "PREV wraps");
    // Both at once: HOME.
    h.sd.button(false, true);
    h.sd.button(true, true);
    h.sd.button(true, false);
    h.sd.button(false, false);
    assert_eq!(h.load(b"*"), [0x01, 0x08, b'O']);
    // A CD keeps an XS list.
    assert!(h.command(b"CD:\x5f").starts_with("00"));
    assert!(h.sd.dos.swap.is_some());
    assert!(h.command(b"XS").starts_with("00"));
    assert!(h.sd.dos.swap.is_none());
    assert!(h.command(b"XS:NOLIST").starts_with("62"));
}

#[test]
fn home_without_a_list_generates_one() {
    let mut h = swap_folder();
    assert!(h.command(b"CD:GAME").starts_with("00"));
    // NEXT without AUTOSWAP.LST does nothing.
    MediaSlot::control(&mut h.sd, "next").unwrap();
    assert!(!h.sd.dos.mounted());
    MediaSlot::control(&mut h.sd, "home").unwrap();
    assert!(h.sd.dos.mounted());
    let generated = h.file("/game/AUTOSWAP.GEN").unwrap();
    assert_eq!(generated, b"#PETSCII\r\nA.D64\r\nB.D64\r\n");
    MediaSlot::control(&mut h.sd, "next").unwrap();
    assert_eq!(h.load(b"*"), [0x01, 0x08, b'T']);
    // A found list ends with the next CD.
    assert!(h.command(b"CD:\x5f").starts_with("00"));
    assert!(h.sd.dos.swap.is_none());
    assert!(MediaSlot::control(&mut h.sd, "eject").is_err());
}

#[test]
fn image_display_and_drive_configuration() {
    let mut h = with_image(|_| {});
    assert!(h.command(b"XI1").starts_with("00"));
    let text = String::from_utf8_lossy(&h.load(b"$")).into_owned();
    assert!(text.contains("\"DISK.D64\"         DIR"), "{text}");
    assert_eq!(listing_names(&h.load(b"$=D")), ["DISK.D64"]);
    assert!(h.command(b"XI2").starts_with("00"));
    assert_eq!(listing_names(&h.load(b"$")), ["DISK.D64", "DISK.D64"]);
    assert!(h.command(b"XI0").starts_with("00"));
    assert!(h.command(b"XD?").starts_with("03,D:00=04,09,00"));
    assert!(h.command(b"XD1=4").starts_with("31"), "one slot per device");
    assert!(h.command(b"XD1=0").starts_with("31"), "no ATA here");
    assert!(h.command(b"XD0=15").starts_with("00"));
    assert!(h.command(b"XD1=4").starts_with("00"));
    assert!(h.command(b"XD?").starts_with("03,D:01=04,09,00"));
    assert!(!h.load(b"$").is_empty());
    // All slots none is the default again.
    assert!(h.command(b"XD1=15").starts_with("00"));
    assert!(!h.load(b"$").is_empty());
}

fn memory_read(h: &mut Host, addr: u16, n: u8) -> Vec<u8> {
    let [lo, hi] = addr.to_le_bytes();
    h.listen_send(0x6f, &[b'M', b'-', b'R', lo, hi, n]);
    h.talk_receive(0x6f)
}

#[test]
fn memory_read_answers_drive_detection() {
    let d71 = D64::blank_d71(b"D", *b"71").to_bytes();
    let d81 = D64::blank_d81(b"D", *b"81").to_bytes();
    let mut rom = vec![0u8; 16384 + 2];
    rom[2] = 0xab;
    *rom.last_mut().unwrap() = 0xcd;
    let mut h = with_image(move |m| {
        m.insert("/two.d71", d71).unwrap();
        m.insert("/four.d81", d81).unwrap();
        m.insert("/dos1541.bin", rom).unwrap();
    });
    assert_eq!(memory_read(&mut h, 0xe5c6, 1), b"4");
    assert_eq!(memory_read(&mut h, 0xe5b8, 3), b"BM ");
    assert_eq!(memory_read(&mut h, 0x77, 2), [0x29, 0x49]);
    assert!(h.command(b"CD:TWO.D71").starts_with("00"));
    assert_eq!(memory_read(&mut h, 0xe5c6, 1), b"7");
    assert!(h.command(b"CD//:FOUR.D81").starts_with("00"));
    assert_eq!(memory_read(&mut h, 0xa6e9, 1), b"8");
    assert_eq!(memory_read(&mut h, 0xe5c6, 1), [0]);
    assert!(h.command(b"CD:\x5f").starts_with("00"));
    // XR: the ROM file (its 2-byte header skipped) ends at $FFFF.
    assert!(h.command(b"XR:DOS1541.BIN").starts_with("00"));
    assert_eq!(memory_read(&mut h, 0xc000, 1), [0xab]);
    assert_eq!(memory_read(&mut h, 0xffff, 1), [0xcd]);
    assert_eq!(memory_read(&mut h, 0x77, 1), [0x29], "RAM from the table");
    assert!(h.command(b"XR").starts_with("00"));
    assert_eq!(memory_read(&mut h, 0xc000, 1), [0]);
}

#[test]
fn savestate_keeps_swap_list_and_buffers() {
    let mut h = swap_folder();
    assert!(h.command(b"CD:GAME").starts_with("00"));
    assert!(h.command(b"XS:DISKS.LST").starts_with("00"));
    h.open(2, b"#");
    assert!(h.command(b"U1:2,0,18,0").starts_with("00"));
    assert!(h.command(b"XI1").starts_with("00"));
    let mut w = StateWriter::new();
    h.sd.save(&mut w);
    let state = w.into_bytes();
    h.sd.reset(true);
    assert!(h.sd.dos.swap.is_none());
    h.sd.load(&mut StateReader::new(&state)).unwrap();
    assert_eq!(h.talk_receive(0x62).len(), 256);
    assert_eq!(h.sd.dos.xi_mode, 1);
    press(&mut h, false);
    assert_eq!(h.load(b"*"), [0x01, 0x08, b'T']);
}

#[test]
fn version_1_states_still_load() {
    let mut h = swap_folder();
    assert!(h.command(b"CD:GAME").starts_with("00"));
    let mut w = StateWriter::new();
    w.chunk(*b"SD2I", 1, |w| {
        IecProtocol::default().save(w);
        ClockDivider::new(1_000_000, 1_000_000).unwrap().save(w);
        h.sd.dos.save_v1(w);
    });
    let state = w.into_bytes();
    h.sd.reset(true);
    h.sd.load(&mut StateReader::new(&state)).unwrap();
    assert_eq!(h.sd.dos.cwd(), "/game");
    assert!(h.sd.dos.swap.is_none());
}

#[test]
fn rel_like_the_kernal_sends_it() {
    let mut h = standard();
    h.open(15, b"");
    h.open(2, b"DATA,L,\x14");
    h.listen_send(0x6f, b"P\x62\x03\x00\x01\r");
    h.listen_send(0x62, b"REC THREE\r");
    h.listen_send(0x6f, b"P\x62\x03\x00\x01\r");
    assert_eq!(h.talk_receive(0x62), b"REC THREE\r");
    h.close(2);
    h.close(15);
    assert_eq!(h.file("/data.r00").unwrap().len(), 26 + 60);
}
