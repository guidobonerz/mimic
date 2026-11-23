//! The machine "ultimate64": the Elite II board around the C64, with
//! small RISC-V programs in place of the firmware (the firmware itself
//! is the user's; the test with it is `#[ignore]`).

use super::*;
use crate::memory::KERNAL_SIZE;
use crate::u64board::Halt;
use mimic_core::host::MemFileHost;

fn config(options: &[(&str, &str)]) -> MachineConfig {
    let mut kernal = vec![0xea; KERNAL_SIZE];
    // RESET, IRQ and NMI at $E000: NOPs, then they run on into the next.
    kernal[0x1ffa..].copy_from_slice(&[0x00, 0xe0, 0x00, 0xe0, 0x00, 0xe0]);
    let mut c = MachineConfig::default();
    c.roms.insert("kernal".into(), kernal);
    c.roms.insert("basic".into(), vec![0xb0; BASIC_SIZE]);
    c.roms.insert("chargen".into(), vec![0xc0; CHARGEN_SIZE]);
    for (k, v) in options {
        c.options.insert((*k).into(), (*v).into());
    }
    c
}

/// An image record: the program at `$30000`, started there.
fn record(code: &[u32]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(0x3_0000u32.to_le_bytes());
    v.extend((code.len() as u32 * 4).to_le_bytes());
    v.extend(0x3_0000u32.to_le_bytes());
    for w in code {
        v.extend(w.to_le_bytes());
    }
    v
}

/// Writes 1 to the C64's `$0400` by DMA, prints "A", loops.
const PROGRAM: [u32; 7] = [
    0x1005_0537, // lui a0,0x10050     the DMA window
    0x0010_0593, // li a1,1
    0x40b5_0023, // sb a1,0x400(a0)
    0x1000_0537, // lui a0,0x10000     the ITU
    0x0410_0593, // li a1,'A'
    0x00b5_0823, // sb a1,0x10(a0)     UART data
    0x0000_006f, // j .
];

fn frames(m: &mut C64, n: u32) {
    let mut dbg = DebugHooks::default();
    for _ in 0..n {
        m.run_frame(&mut dbg);
    }
}

fn with_files(options: &[(&str, &str)], files: &[(&str, Vec<u8>)]) -> C64 {
    let mut m = C64::new_ultimate(&config(options)).unwrap();
    let mut fs = MemFileHost::new();
    for (path, data) in files {
        fs.insert(path, data.clone()).unwrap();
    }
    m.attach_host(HostServices::new(None, Some(Box::new(fs))));
    m
}

fn board(m: &C64) -> &u64board::Elite2 {
    &m.board.as_ref().unwrap().board
}

#[test]
fn models_and_options() {
    let mut c = config(&[]);
    assert!(C64::new_ultimate(&c).is_ok());
    c.model = Some("c64u".into());
    let m = C64::new_ultimate(&c).unwrap();
    assert_eq!(board(&m).kind(), u64board::Kind::C64Ultimate);
    c.model = Some("u64".into());
    assert!(C64::new_ultimate(&c).is_err());
    // The command interface and the REU are the board's here.
    assert!(C64::new_ultimate(&config(&[("uci", "on")])).is_err());
    assert!(C64::new_ultimate(&config(&[("reu", "512")])).is_err());
    assert!(C64::new_ultimate(&config(&[("c64roms", "none")])).is_err());
    // A plain C64 has no board.
    assert!(C64::new(&config(&[])).unwrap().board.is_none());
}

#[test]
fn without_firmware_the_c64_runs_and_says_so() {
    let mut m = with_files(&[], &[]);
    assert_eq!(board(&m).halt(), Halt::NoFirmware);
    assert!(!m.board.as_ref().unwrap().holds());
    frames(&mut m, BOOT_FRAMES as u32 + 2);
    // "ULTIMATE" in screen codes on row 8.
    assert_eq!(
        m.mem.ram[0x0400 + 8 * 40..][..8],
        [21, 12, 20, 9, 13, 1, 20, 5]
    );
}

#[test]
fn the_application_starts_from_the_flash_file() {
    // The Elite II's application lies at $3C0000 of the flash.
    let mut flash = vec![0xffu8; 0x3c_0000];
    flash.extend(record(&PROGRAM));
    let mut m = with_files(&[], &[("/Board/elite2-flash.bin", flash.clone())]);
    assert_eq!(board(&m).halt(), Halt::Running);
    frames(&mut m, 1);
    assert_eq!(m.mem.ram[0x400], 1);
    let b = &mut m.board.as_mut().unwrap().board;
    assert_eq!(b.take_console(), b"A");
    assert_eq!(b.cpu.pc(), 0x3_0018);
    // The Commodore 64 Ultimate's lies at $220000: nothing there.
    let mut c = config(&[]);
    c.model = Some("c64u".into());
    let mut m = C64::new_ultimate(&c).unwrap();
    let mut fs = MemFileHost::new();
    fs.insert("/Board/c64u-flash.bin", flash).unwrap();
    m.attach_host(HostServices::new(None, Some(Box::new(fs))));
    assert_eq!(board(&m).halt(), Halt::NoFirmware);
}

#[test]
fn an_update_file_starts_once() {
    let mut m = with_files(
        &[("update", "test.ue2")],
        &[("/test.ue2", record(&PROGRAM))],
    );
    assert_eq!(board(&m).halt(), Halt::Running);
    frames(&mut m, 1);
    assert_eq!(m.mem.ram[0x400], 1);
    // Power off and on: the flash is empty, the updater is not run again.
    m.reset(ResetKind::Hard);
    assert_eq!(board(&m).halt(), Halt::NoFirmware);
}

#[test]
fn menu_button_and_keys_reach_the_board() {
    let mut m = with_files(&[], &[]);
    m.control(5, 1);
    assert!(board(&m).itu.menu_button());
    assert!(m.controls().iter().any(|c| c.name == "MENU"));
    m.control(5, 0);
    assert!(!board(&m).itu.menu_button());
    // RETURN: column 0, row 1 of the matrix, for the C64 and the board.
    let now = m.now();
    m.input(TimedInput {
        at: now,
        target: InputTarget::Keyboard,
        event: mimic_core::input::InputEvent::Key {
            machine_key: mimic_core::input::MachineKey(1),
            down: true,
        },
    });
    frames(&mut m, 1);
    assert!(board(&m).u64io.keys().iter().any(|&k| k != 0));
}

#[test]
fn savestate_round_trip() {
    let mut flash = vec![0xffu8; 0x3c_0000];
    flash.extend(record(&PROGRAM));
    let files = [("/Board/elite2-flash.bin", flash)];
    let mut m = with_files(&[], &files);
    frames(&mut m, 2);
    let state = m.save_state();
    frames(&mut m, 2);
    let after = m.save_state();
    let mut n = with_files(&[], &[]);
    n.load_state(&state).unwrap();
    assert_eq!(board(&n).halt(), Halt::Running);
    frames(&mut n, 2);
    let got = n.save_state();
    if let Some(at) = after.iter().zip(&got).position(|(a, b)| a != b) {
        let from = at.saturating_sub(64);
        panic!(
            "states differ at {at} of {}/{}: {:02x?} / {:02x?}",
            after.len(),
            got.len(),
            &after[from..at + 8],
            &got[from..at + 8]
        );
    }
    assert_eq!(after.len(), got.len());
    // A plain C64's state has no board.
    let plain = C64::new(&config(&[])).unwrap().save_state();
    assert!(n.load_state(&plain).is_err());
    // Cut states never panic.
    for cut in [0, 10, state.len() / 2, state.len() - 1] {
        assert!(n.load_state(&state[..cut]).is_err());
    }
}

#[test]
fn no_sound_while_the_board_holds_the_c64() {
    let mut m = with_files(&[], &[]);
    // A voice at full volume: triangle, gate on, sustain at the top.
    for (reg, v) in [(0x18, 0x0f), (0x01, 0x20), (0x06, 0xf0), (0x04, 0x11)] {
        m.io.sid.write(reg, v);
    }
    frames(&mut m, 3);
    let mut out = Vec::new();
    m.audio(&mut out);
    assert!(out.iter().any(|&s| s != 0));
    // The menu's stop: silence, though the SID runs on.
    m.board.as_mut().unwrap().lines.stop = true;
    frames(&mut m, 2);
    out.clear();
    m.audio(&mut out);
    frames(&mut m, 2);
    out.clear();
    m.audio(&mut out);
    assert!(out.iter().all(|&s| s == 0), "{:?}", &out[..8]);
    // Released: the sound is back.
    m.board.as_mut().unwrap().lines.stop = false;
    frames(&mut m, 3);
    out.clear();
    m.audio(&mut out);
    assert!(out.iter().any(|&s| s != 0));
}

/// A medium in memory.
struct MemDisk(Vec<u8>, bool);

impl mimic_core::media::MediaBackend for MemDisk {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), mimic_core::media::MediaError> {
        let at = offset as usize;
        buf.copy_from_slice(&self.0[at..at + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), mimic_core::media::MediaError> {
        let at = offset as usize;
        self.0[at..at + data.len()].copy_from_slice(data);
        self.1 = true;
        Ok(())
    }
    fn commit(&mut self) -> Result<(), mimic_core::media::MediaError> {
        self.1 = false;
        Ok(())
    }
    fn dirty(&self) -> bool {
        self.1
    }
}

#[test]
fn sd_and_usb_slots_take_block_media() {
    use chips_ultimate::Storage;
    use mimic_core::media::{Media, MediaKind};
    let mut m = with_files(&[], &[]);
    let names: Vec<_> = m.media_slots().iter().map(|s| s.name).collect();
    for n in ["sd card", "usb 1", "usb 2", "usb 3"] {
        assert!(names.contains(&n), "{names:?}");
    }
    let disk = |len: usize, kind| Media {
        name: "stick.img".into(),
        kind,
        backend: Box::new(MemDisk(vec![0; len], false)),
    };
    let usb2 = SlotId(SLOT_BOARD_SD.0 + 2);
    // Not a block medium: too small, or no disk.
    assert!(m
        .media_slot(usb2)
        .unwrap()
        .insert(disk(1000, MediaKind::Disk))
        .is_err());
    assert!(m
        .media_slot(usb2)
        .unwrap()
        .insert(disk(1 << 20, MediaKind::Tape))
        .is_err());
    m.media_slot(usb2)
        .unwrap()
        .insert(disk(1 << 20, MediaKind::Disk))
        .unwrap();
    let st = &mut m.board.as_mut().unwrap().storage;
    assert_eq!(st.sectors(chips_ultimate::unit::USB0 + 1), Some(2048));
    assert_eq!(st.sectors(chips_ultimate::unit::SD), None);
    let g = st.generation(chips_ultimate::unit::USB0 + 1);
    assert!(st.write(chips_ultimate::unit::USB0 + 1, 5, &[7; 512]));
    let mut buf = [0u8; 512];
    assert!(st.read(chips_ultimate::unit::USB0 + 1, 5, &mut buf));
    assert_eq!(buf[0], 7);
    assert!(!st.read(chips_ultimate::unit::USB0 + 1, 2048, &mut buf));
    // Written and quiet: flushed by the machine itself.
    assert!(m.media_slot(usb2).unwrap().dirty());
    frames(&mut m, u64board::storage::QUIET_FRAMES + 1);
    assert!(!m.media_slot(usb2).unwrap().dirty());
    // Write protect, eject.
    m.media_slot(usb2).unwrap().write_protect(true);
    let st = &mut m.board.as_mut().unwrap().storage;
    assert!(!st.write(chips_ultimate::unit::USB0 + 1, 5, &[7; 512]));
    assert!(m.media_slot(usb2).unwrap().eject().is_some());
    let st = &m.board.as_ref().unwrap().storage;
    assert_eq!(st.sectors(chips_ultimate::unit::USB0 + 1), None);
    assert_ne!(st.generation(chips_ultimate::unit::USB0 + 1), g);
}

/// The user's update file (`MIMIC_U64_UPDATE`): its updater flashes the
/// board, the machine switches off, and the firmware then starts from
/// the flash and shows its menu.
#[test]
#[ignore]
fn the_real_update_and_firmware() {
    let Ok(path) = std::env::var("MIMIC_U64_UPDATE") else {
        eprintln!("MIMIC_U64_UPDATE not set");
        return;
    };
    let file = std::fs::read(path).unwrap();
    let mut m = with_files(&[("update", "update.ue2")], &[("/update.ue2", file)]);
    let key = |m: &mut C64, k: mimic_core::input::MachineKey| {
        for down in [true, false] {
            let now = m.now();
            m.input(TimedInput {
                at: now,
                target: InputTarget::Keyboard,
                event: mimic_core::input::InputEvent::Key {
                    machine_key: k,
                    down,
                },
            });
            frames(m, 10);
        }
    };
    let text = |m: &C64| -> String {
        m.mem.ram[0x400..0x7e8]
            .iter()
            .map(|&c| match c & 0x7f {
                // The firmware's own character set is laid out as ASCII.
                c @ 0x20..=0x7e => c as char,
                _ => ' ',
            })
            .collect()
    };
    let y = mimic_core::input::MachineKey(25);
    // "About to update. Continue?"
    frames(&mut m, 400);
    assert!(text(&m).contains("Continue?"), "{}", text(&m));
    key(&mut m, y);
    // "Reset Configuration?"
    for _ in 0..200 {
        frames(&mut m, 50);
        if text(&m).contains("Reset Configuration?") {
            break;
        }
    }
    assert!(text(&m).contains("Reset Configuration?"), "{}", text(&m));
    key(&mut m, y);
    for _ in 0..100 {
        frames(&mut m, 50);
        if board(&m).halt() == Halt::PowerOff {
            break;
        }
    }
    assert_eq!(board(&m).halt(), Halt::PowerOff, "{}", text(&m));
    // Power on: the firmware from the flash; its menu on the button.
    m.reset(ResetKind::Hard);
    assert_eq!(board(&m).halt(), Halt::Running);
    frames(&mut m, 600);
    m.control(5, 1);
    frames(&mut m, 10);
    m.control(5, 0);
    frames(&mut m, 150);
    assert!(text(&m).contains("Ultimate 64"), "{}", text(&m));
    let console = m.board.as_mut().unwrap().board.take_console();
    println!("{}", String::from_utf8_lossy(&console).replace('\r', ""));
}

/// A drive ROM (not the Commodore DOS) with `code` at `$C000` as reset
/// entry, as the firmware leaves it in drive A's area of board memory:
/// twice, at `+$8000` and `+$C000`.
fn put_drive_rom(m: &mut C64, code: &[u8]) {
    let mut rom = vec![0xea; 0x4000];
    rom[..code.len()].copy_from_slice(code);
    rom[0x3ffc..].copy_from_slice(&[0x00, 0xc0, 0x00, 0xc0]);
    let ram = m.board.as_mut().unwrap().board.ram_mut();
    let area = 0x00ee_0000;
    ram[area + 0x8000..area + 0xc000].copy_from_slice(&rom);
    ram[area + 0xc000..area + 0x1_0000].copy_from_slice(&rom);
}

/// Track 18 of a blank disk at `$100000` of board memory, the track
/// table pointing there; the other tracks a short empty one.
fn put_track_18(m: &mut C64) -> Vec<u8> {
    let track = media::gcr::encode_track(&media::D64::blank(b"TEST", *b"AB"), 18);
    let b = &mut m.board.as_mut().unwrap().board;
    b.ram_mut()[0x10_0000..0x10_0000 + track.len()].copy_from_slice(&track);
    b.ram_mut()[0x20_0000..0x20_0101].fill(0);
    let d = &mut b.c64.drive_a;
    for half in 0..84u32 {
        let (addr, len) = if half == 34 {
            (0x10_0000u32, track.len() as u32)
        } else {
            (0x20_0000, 0x101)
        };
        let words = [addr, (5_000_000 / len) << 16 | (len - 1)];
        for (w, v) in words.iter().enumerate() {
            for (i, byte) in v.to_le_bytes().into_iter().enumerate() {
                d.write(0x1000 + (half * 2 + w as u32) * 4 + i as u32, byte);
            }
        }
    }
    track
}

/// What the firmware does to start drive A with a writable disk in it.
fn start_drive_a(m: &mut C64) {
    let d = &mut m.board.as_mut().unwrap().board.c64.drive_a;
    d.write(3, 1); // SENSOR light
    d.write(4, 1); // INSERTED
    d.write(2, 0); // device 8
    d.write(0, 1); // POWER
    d.write(1, 1); // RESET held
    d.write(1, 6); // let go, follow the C64's reset, stop when frozen
}

#[test]
fn drive_a_is_drive_8_and_reads_its_disk_from_board_memory() {
    let mut m = C64::new_ultimate(&config(&[])).unwrap();
    // No media slot for drive 8: the firmware mounts its disks.
    assert!(!m.media_slots().iter().any(|s| s.id == SLOT_DRIVE8));
    assert!(m.media_slot(SLOT_DRIVE8).is_none());
    assert!(m.devices().contains(&DRIVE8));
    // Drive 8 option: the board's.
    assert!(C64::new_ultimate(&config(&[("drive8", "1541")])).is_err());
    // Motor on, byte ready on SO, 16 GCR bytes into $0300.
    put_drive_rom(
        &mut m,
        &[
            0xa9, 0x6f, 0x8d, 0x02, 0x1c, 0xa9, 0x04, 0x8d, 0x00, 0x1c, 0xa9, 0x0e, 0x8d, 0x0c,
            0x1c, 0xa2, 0x00, 0xb8, 0x50, 0xfe, 0xad, 0x01, 0x1c, 0x9d, 0x00, 0x03, 0xe8, 0xe0,
            0x10, 0xd0, 0xf2, 0x4c, 0x1f, 0xc0,
        ],
    );
    let track = put_track_18(&mut m);
    // Off: the drive stands.
    frames(&mut m, 2);
    let pc = m.drive.as_ref().unwrap().cbm().unwrap().pc();
    frames(&mut m, 1);
    assert_eq!(m.drive.as_ref().unwrap().cbm().unwrap().pc(), pc);
    start_drive_a(&mut m);
    frames(&mut m, 3);
    let c = m.drive.as_ref().unwrap().cbm().unwrap();
    assert!((0xc01f..=0xc021).contains(&c.pc()), "ran the board's ROM");
    let got = c.ram()[0x301..0x310].to_vec();
    let framed = |shift: u32| -> Vec<u8> {
        (0..track.len())
            .map(|i| {
                let word = u16::from(track[i]) << 8 | u16::from(track[(i + 1) % track.len()]);
                (word >> (8 - shift)) as u8
            })
            .collect()
    };
    assert!((0..8).any(|s| framed(s).windows(15).any(|w| w == got)));
    let b = &m.board.as_ref().unwrap().board;
    // The drive's RAM in its area, as the firmware reads `$78` there.
    assert_eq!(&b.ram()[0xee_0301..0xee_0310], &got[..]);
    // The head as the firmware sees it: track 18, motor on, not writing.
    assert_eq!(b.c64.peek_with(0x02_0008, None), 34);
    assert_eq!(b.c64.peek_with(0x02_0009, None), 1);
    assert_eq!(b.c64.peek_with(0x02_0800, None), 0);
    // The jumpers.
    m.board.as_mut().unwrap().board.c64.drive_a.write(2, 1);
    frames(&mut m, 1);
    assert_eq!(m.drive.as_ref().unwrap().cbm().unwrap().device(), 9);
}

#[test]
fn drive_a_writes_into_board_memory_and_sets_dirty() {
    let mut m = C64::new_ultimate(&config(&[])).unwrap();
    // Motor on, port A out with $A5, CB2 low: write mode.
    put_drive_rom(
        &mut m,
        &[
            0xa9, 0x6f, 0x8d, 0x02, 0x1c, 0xa9, 0x04, 0x8d, 0x00, 0x1c, 0xa9, 0xff, 0x8d, 0x03,
            0x1c, 0xa9, 0xa5, 0x8d, 0x01, 0x1c, 0xa9, 0xc0, 0x8d, 0x0c, 0x1c, 0x4c, 0x19, 0xc0,
        ],
    );
    let track = put_track_18(&mut m);
    start_drive_a(&mut m);
    frames(&mut m, 3);
    let b = &m.board.as_ref().unwrap().board;
    let now = &b.ram()[0x10_0000..0x10_0000 + track.len()];
    let a5 = now.iter().filter(|&&v| v == 0xa5).count();
    assert!(a5 > 1000, "{a5} bytes written");
    // DIRTY for track index 17 and "any"; STATUS motor, writing, busy.
    assert_eq!(b.c64.peek_with(0x02_0800 + 17, None), 0x81);
    assert_eq!(b.c64.peek_with(0x02_0800 + 16, None), 0x80);
    assert_eq!(b.c64.peek_with(0x02_0009, None), 7);
    // The firmware's write-back clears them.
    let d = &mut m.board.as_mut().unwrap().board.c64.drive_a;
    d.write(0x800, 0x80);
    d.write(0x800 + 17, 0);
    // A dark sensor: no more bytes reach the disk, the head still writes.
    d.write(3, 0);
    m.board.as_mut().unwrap().board.ram_mut()[0x10_0000..0x10_0000 + track.len()].fill(0x55);
    frames(&mut m, 2);
    let b = &m.board.as_ref().unwrap().board;
    assert!(b.ram()[0x10_0000..0x10_0000 + track.len()]
        .iter()
        .all(|&v| v == 0x55));
    assert_eq!(b.c64.peek_with(0x02_0800 + 17, None), 0x81);
    // Without a disk inserted the head marks nothing.
    let d = &mut m.board.as_mut().unwrap().board.c64.drive_a;
    d.write(0x800, 0x80);
    d.write(0x800 + 17, 0);
    d.write(4, 0);
    frames(&mut m, 1);
    assert_eq!(
        m.board
            .as_ref()
            .unwrap()
            .board
            .c64
            .peek_with(0x02_0811, None),
        0
    );
}

#[test]
fn board_drives_savestate_round_trip() {
    let code = [0xe6, 0x10, 0x4c, 0x00, 0xc0]; // INC $10; JMP $C000
    let make = |model: Option<&str>| {
        let mut c = config(&[]);
        c.model = model.map(Into::into);
        let mut m = C64::new_ultimate(&c).unwrap();
        put_drive_rom(&mut m, &code);
        put_track_18(&mut m);
        m
    };
    let mut a = make(Some("c64u"));
    // The Commodore 64 Ultimate has drive B as a second debug target.
    assert!(a.devices().contains(&drive::DRIVE9));
    start_drive_a(&mut a);
    frames(&mut a, 2);
    let s = a.save_state();
    let mut b = make(Some("c64u"));
    // Another ROM in the new machine's memory: the state's comes back.
    put_drive_rom(&mut b, &[0x4c, 0x00, 0xc0]);
    b.load_state(&s).unwrap();
    frames(&mut a, 2);
    frames(&mut b, 2);
    let ram = |m: &C64| m.drive.as_ref().unwrap().cbm().unwrap().ram().to_vec();
    assert_eq!(ram(&a), ram(&b));
    assert!(ram(&a)[0x10] != 0);
    // An Elite II state does not fit the other board.
    let elite = make(None);
    assert!(b.load_state(&elite.save_state()).is_err());
}

/// The board's boot cartridge as the firmware starts it (`start_cartridge`):
/// an 8 KB cartridge from board memory, started by the C64's reset, that
/// switches itself off with `$40` to `$DFFF`.
mod board_cartridge {
    use super::*;
    use chips_ultimate::c64port::carts::ROM_BASE;

    /// Cold start at $8009: marks $0400, copies its $8004 to $0402, jumps
    /// to the RAM part.
    const ROM: [u8; 24] = [
        0x09, 0x80, 0x09, 0x80, 0xc3, 0xc2, 0xcd, 0x38, 0x30, // header
        0xa9, 0x42, // LDA #$42
        0x8d, 0x00, 0x04, // STA $0400
        0xad, 0x04, 0x80, // LDA $8004
        0x8d, 0x02, 0x04, // STA $0402
        0x4c, 0x40, 0x03, // JMP $0340
        0x00,
    ];

    /// At $0340: the cartridge off, then $8004 (now RAM) to $0401.
    const RAM: [u8; 14] = [
        0xa9, 0x40, // LDA #$40
        0x8d, 0xff, 0xdf, // STA $DFFF
        0xad, 0x04, 0x80, // LDA $8004
        0x8d, 0x01, 0x04, // STA $0401
        0x4c, 0x4b, 0x03, // JMP *
    ];

    /// A KERNAL whose reset starts the cartridge as the real one does.
    fn machine() -> C64 {
        let mut c = config(&[]);
        c.roms.get_mut("kernal").unwrap()[..3].copy_from_slice(&[0x6c, 0x00, 0x80]);
        let mut m = C64::new_ultimate(&c).unwrap();
        m.attach_host(HostServices::new(None, Some(Box::new(MemFileHost::new()))));
        m
    }

    /// Everything up to the release of the reset.
    fn start(m: &mut C64) {
        frames(m, 2);
        let b = &mut m.board.as_mut().unwrap().board;
        b.ram_mut()[ROM_BASE as usize..][..ROM.len()].copy_from_slice(&ROM);
        m.mem.ram[0x340..][..RAM.len()].copy_from_slice(&RAM);
        m.mem.ram[0x8004] = 0x77;
        // Stop at once, ULTIMAX for the VIC-II's registers and back: the
        // map follows each write at once.
        m.board_io(0x04_0002, Some(2));
        m.board_io(0x04_0001, Some(1));
        m.board_io(0x04_0000, Some(0x02));
        assert!(m.mem.ultimax());
        m.board_io(0x04_0000, Some(0x00));
        assert!(!m.mem.ultimax());
        // Reset, the type while it is held.
        m.board_io(0x04_0000, Some(0x04));
        m.board_io(0x04_0001, Some(0));
        m.board_io(0x04_0005, Some(0x41));
        assert!(!m.mem.exrom && m.mem.game, "8 KB: /EXROM low");
        assert_eq!(m.board_io(0x04_0006, None), 1);
        // The board's DMA sees the cartridge where the PLA maps it.
        assert_eq!(m.board_io(0x05_8004, None), 0xc3);
        // The debugger too.
        let dbg = m.debug_target(mimic_core::DeviceId("cpu")).unwrap();
        assert_eq!(dbg.peek(CPU_SPACE, 0x8004, Size::Byte), Some(0xc3));
    }

    fn release_and_check(m: &mut C64) {
        m.board_io(0x04_0000, Some(0x08));
        frames(m, 2);
        assert_eq!(m.mem.ram[0x400], 0x42, "the cartridge started");
        assert_eq!(m.mem.ram[0x402], 0xc3, "its ROM at $8004");
        assert_eq!(m.mem.ram[0x401], 0x77, "RAM at $8004 once it is off");
        assert_eq!(m.board_io(0x04_0006, None), 0, "C64_CARTRIDGE_ACTIVE");
        assert!(m.mem.exrom && m.mem.game);
        assert_eq!(m.board_io(0x05_8004, None), 0x77);
    }

    #[test]
    fn boot_cartridge_starts_and_switches_off() {
        let mut m = machine();
        start(&mut m);
        release_and_check(&mut m);
        // restoreCart: type none, the force strobe twice; a C64 reset
        // then starts without a cartridge.
        m.board_io(0x04_0005, Some(0x00));
        m.board_io(0x04_0006, Some(2));
        m.board_io(0x04_0006, Some(2));
        m.reset(ResetKind::Soft);
        assert!(!board(&m).c64.carts.active());
    }

    #[test]
    fn the_cartridge_is_in_the_savestate() {
        let mut m = machine();
        start(&mut m);
        let state = m.save_state();
        let mut n = machine();
        n.load_state(&state).unwrap();
        assert!(board(&n).c64.carts.active());
        assert!(!n.mem.exrom);
        assert_eq!(n.board_io(0x05_8004, None), 0xc3);
        release_and_check(&mut n);
    }

    #[test]
    fn no_cartridge_slot_of_its_own() {
        let mut m = machine();
        assert!(m.media_slots().iter().all(|s| s.id != SLOT_CARTRIDGE));
        assert!(m.media_slot(SLOT_CARTRIDGE).is_none());
    }
}

/// The installed firmware (`MIMIC_U64_FLASH`: the flash file, its `.reg`
/// beside it): menu on the button, the console printed.
#[test]
#[ignore]
fn installed_firmware_menu() {
    let Ok(path) = std::env::var("MIMIC_U64_FLASH") else {
        eprintln!("MIMIC_U64_FLASH not set");
        return;
    };
    let flash = std::fs::read(&path).unwrap();
    let reg = std::fs::read(path.replace(".bin", ".reg")).unwrap();
    let mut m = with_files(
        &[],
        &[
            ("/Board/elite2-flash.bin", flash),
            ("/Board/elite2-flash.reg", reg),
        ],
    );
    let print = |m: &mut C64| {
        let console = m.board.as_mut().unwrap().board.take_console();
        print!("{}", String::from_utf8_lossy(&console).replace('\r', ""));
    };
    frames(&mut m, 800);
    print(&mut m);
    m.control(5, 1);
    frames(&mut m, 10);
    m.control(5, 0);
    frames(&mut m, 200);
    print(&mut m);
    let text: String = m.mem.ram[0x400..0x7e8]
        .iter()
        .map(|&c| match c & 0x7f {
            c @ 0x20..=0x7e => c as char,
            _ => ' ',
        })
        .collect();
    for row in text.as_bytes().chunks(40) {
        println!("|{}|", String::from_utf8_lossy(row));
    }
}
