//! TRS-80 Model I with the user's Level II ROM (`EMU_ROM_DIR`, a `trs80/`
//! subfolder with `M1L2_1.3.bin` or another 12 KB Level II image).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test trs80_boot -- --ignored --nocapture

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, StopReason};
use machine_trs80::Trs80;

fn machine(options: &[(&str, &str)]) -> Option<Trs80> {
    let dir = media_backends::roms::env_dir()?;
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "trs80").expect("ROMs"),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Some(Trs80::new(&config).expect("machine"))
}

fn run(m: &mut Trs80, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

/// Type `text` and run until it is typed and the ROM had time to answer.
fn type_and_run(m: &mut Trs80, text: &str) {
    m.type_text(text);
    run(m, text.chars().count() * 6 + 60);
}

/// Level II asks MEMORY SIZE?, ENTER takes all RAM, BASIC computes.
#[test]
#[ignore = "needs EMU_ROM_DIR with the TRS-80 Level II ROM"]
fn level2_boots_to_basic_and_computes() {
    let Some(mut m) = machine(&[]) else {
        return;
    };
    run(&mut m, 120);
    let s = m.screen_text();
    assert!(s.contains("MEMORY SIZE?"), "{s}");
    type_and_run(&mut m, "\n");
    let s = m.screen_text();
    assert!(s.contains("READY"), "{s}");
    type_and_run(&mut m, "PRINT 6*7\n");
    let s = m.screen_text();
    assert!(s.contains(" 42"), "{s}");
    type_and_run(&mut m, "10 FOR I=1 TO 3:PRINT I*I;:NEXT\nRUN\n");
    let s = m.screen_text();
    assert!(s.contains(" 1  4  9"), "{s}");
    println!("{s}");
}

/// The block graphics: SET draws a block the CPU reads back as $80+bits.
#[test]
#[ignore = "needs EMU_ROM_DIR with the TRS-80 Level II ROM"]
fn level2_set_draws_a_block() {
    let Some(mut m) = machine(&[]) else {
        return;
    };
    run(&mut m, 120);
    type_and_run(&mut m, "\n");
    for line in ["10 CLS:SET(0,0):SET(1,47)\n", "20 GOTO 20\n", "RUN\n"] {
        type_and_run(&mut m, line);
    }
    // SET(0,0): bit 0 of the cell at $3C00; SET(1,47): bit 5 of $3FC0.
    assert_eq!(m.peek_cpu(0x3c00) & 0x81, 0x81);
    assert_eq!(m.peek_cpu(0x3fc0) & 0xa0, 0xa0);
}

/// An in-memory tape for CSAVE/CLOAD.
struct Mem(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl emu_core::media::MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, off: u64, buf: &mut [u8]) -> Result<(), emu_core::media::MediaError> {
        let d = self.0.lock().unwrap();
        buf.copy_from_slice(&d[off as usize..off as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, off: u64, buf: &[u8]) -> Result<(), emu_core::media::MediaError> {
        let mut d = self.0.lock().unwrap();
        let end = off as usize + buf.len();
        if d.len() < end {
            d.resize(end, 0);
        }
        d[off as usize..end].copy_from_slice(buf);
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> Result<(), emu_core::media::MediaError> {
        self.0.lock().unwrap().resize(len as usize, 0);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), emu_core::media::MediaError> {
        Ok(())
    }
}

fn tape(m: &mut Trs80, data: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    m.media_slot(machine_trs80::SLOT_TAPE)
        .unwrap()
        .insert(emu_core::media::Media {
            name: "t.cas".into(),
            kind: emu_core::media::MediaKind::Tape,
            backend: Box::new(Mem(data)),
        })
        .unwrap();
}

/// CSAVE writes a CAS image of the program; after NEW, CLOAD reads it back
/// from the image and the program runs.
#[test]
#[ignore = "needs EMU_ROM_DIR with the TRS-80 Level II ROM"]
fn level2_csave_and_cload() {
    let Some(mut m) = machine(&[]) else {
        return;
    };
    run(&mut m, 120);
    type_and_run(&mut m, "\n");
    type_and_run(&mut m, "10 PRINT \"HELLO\"*2\n");
    type_and_run(&mut m, "10 PRINT 123*2\n");
    let data = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    tape(&mut m, data.clone());
    m.type_text("CSAVE\"A\"\n");
    run(&mut m, 2000);
    m.media_slot(machine_trs80::SLOT_TAPE)
        .unwrap()
        .flush()
        .unwrap();
    let cas = data.lock().unwrap().clone();
    let a5 = cas.iter().position(|&b| b != 0).expect("data");
    assert_eq!(cas[a5], 0xa5, "sync byte after the leader");
    assert_eq!(
        &cas[a5 + 1..a5 + 5],
        &[0xd3, 0xd3, 0xd3, b'A'],
        "BASIC header and name"
    );
    // Rewind (insert again), NEW, CLOAD, RUN.
    tape(&mut m, data.clone());
    type_and_run(
        &mut m, "NEW
",
    );
    m.type_text(
        "CLOAD
",
    );
    run(&mut m, 2000);
    type_and_run(
        &mut m, "RUN
",
    );
    let s = m.screen_text();
    assert!(s.contains(" 246"), "{s}");
}

/// Boot sector of the test disk (loaded at $4200 by Level II): writes a
/// line to the screen, then writes itself to sector 1 of track 0 through
/// the FD1771 (polling DRQ and BUSY as the DOSes do) and leaves the final
/// status at $3C40.
fn boot_sector() -> Vec<u8> {
    #[rustfmt::skip]
    let code: &[u8] = &[
        0x21, 0x80, 0x42,       // LD HL,$4280 (message)
        0x11, 0x00, 0x3c,       // LD DE,$3C00
        0x01, 0x0c, 0x00,       // LD BC,12
        0xed, 0xb0,             // LDIR
        0x3e, 0x01,             // LD A,1
        0x32, 0xe1, 0x37,       // LD ($37E1),A   drive 0
        0x3e, 0x01,             // LD A,1
        0x32, 0xee, 0x37,       // LD ($37EE),A   sector 1
        0x21, 0x00, 0x42,       // LD HL,$4200    source: this sector
        0x11, 0xef, 0x37,       // LD DE,$37EF    data register
        0x3e, 0xa8,             // LD A,$A8       WRITE SECTOR ($FB mark)
        0x32, 0xec, 0x37,       // LD ($37EC),A
        // loop ($4220):
        0x3a, 0xec, 0x37,       // LD A,($37EC)
        0xcb, 0x4f,             // BIT 1,A        DRQ?
        0x28, 0x06,             // JR Z,+6
        0x7e,                   // LD A,(HL)
        0x12,                   // LD (DE),A
        0x23,                   // INC HL
        0xc3, 0x20, 0x42,       // JP loop
        0xcb, 0x47,             // BIT 0,A        BUSY?
        0x20, 0xef,             // JR NZ,loop
        0x32, 0x40, 0x3c,       // LD ($3C40),A   final status
        0x18, 0xfe,             // JR $
    ];
    let mut s = vec![0u8; 256];
    s[..code.len()].copy_from_slice(code);
    s[0x80..0x8c].copy_from_slice(b"DISK BOOT OK");
    s
}

/// A 35-track JV1 image whose track 0 sector 0 is the boot sector.
fn boot_disk() -> Vec<u8> {
    let mut d = media::dmk::blank_jv1(35);
    d[..256].copy_from_slice(&boot_sector());
    d
}

fn disk(m: &mut Trs80, drive: u8, data: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
    m.media_slot(emu_core::media::SlotId(
        machine_trs80::SLOT_DRIVE0.0 + drive,
    ))
    .unwrap()
    .insert(emu_core::media::Media {
        name: "d.dsk".into(),
        kind: emu_core::media::MediaKind::Disk,
        backend: Box::new(Mem(data)),
    })
    .unwrap();
}

/// Phase 10's gate for the TRS-80: with the Expansion Interface Level II
/// boots a disk (track 0, sector 0 read through the FD1771 into $4200),
/// and the boot code writes a sector that lands in the JV1 image.
#[test]
#[ignore = "needs EMU_ROM_DIR with the TRS-80 Level II ROM"]
fn level2_boots_a_disk_through_the_expansion_interface() {
    let Some(mut m) = machine(&[("ei", "on")]) else {
        return;
    };
    let data = std::sync::Arc::new(std::sync::Mutex::new(boot_disk()));
    disk(&mut m, 0, data.clone());
    run(&mut m, 240);
    let s = m.screen_text();
    assert!(s.starts_with("DISK BOOT OK"), "{s}");
    assert_eq!(
        m.peek_cpu(0x3c40) & 0x1d,
        0,
        "status after the write: {:02x}",
        m.peek_cpu(0x3c40)
    );
    m.media_slot(machine_trs80::SLOT_DRIVE0)
        .unwrap()
        .flush()
        .unwrap();
    let image = data.lock().unwrap().clone();
    assert_eq!(
        &image[256..512],
        &boot_sector()[..],
        "sector 1 holds the written bytes"
    );
}

/// With the Expansion Interface and no disk, BREAK held at power-on skips
/// the disk boot.
#[test]
#[ignore = "needs EMU_ROM_DIR with the TRS-80 Level II ROM"]
fn level2_with_expansion_interface_and_break_starts_basic() {
    let Some(mut m) = machine(&[("ei", "on"), ("ram", "48k")]) else {
        return;
    };
    m.input(emu_core::input::TimedInput {
        at: emu_core::Cycle(0),
        target: emu_core::input::InputTarget::Keyboard,
        event: emu_core::input::InputEvent::Key {
            machine_key: machine_trs80::keyboard::keys::BREAK,
            down: true,
        },
    });
    run(&mut m, 30);
    m.input(emu_core::input::TimedInput {
        at: m.now(),
        target: emu_core::input::InputTarget::Keyboard,
        event: emu_core::input::InputEvent::Key {
            machine_key: machine_trs80::keyboard::keys::BREAK,
            down: false,
        },
    });
    run(&mut m, 120);
    let s = m.screen_text();
    assert!(s.contains("MEMORY SIZE?"), "{s}");
    type_and_run(&mut m, "\n");
    // The ROM tests 48 KB of RAM before READY.
    run(&mut m, 240);
    type_and_run(&mut m, "PRINT MEM\n");
    let s = m.screen_text();
    // 48 KB less BASIC's own: about 48 500 bytes free.
    assert!(s.contains(" 48"), "{s}");
}

/// Boot sector of the Model III test disk (loaded at $4300 by the ROM):
/// a line on the screen, then itself to sector 2 of track 0 through the
/// FD1793 in double density (OTIR with WAIT, as the Model III DOSes write),
/// the final status at $3C40.
fn m3_boot_sector() -> Vec<u8> {
    #[rustfmt::skip]
    let code: &[u8] = &[
        0xf3,                   // DI
        0x21, 0x80, 0x43,       // LD HL,$4380 (message)
        0x11, 0x00, 0x3c,       // LD DE,$3C00
        0x01, 0x0c, 0x00,       // LD BC,12
        0xed, 0xb0,             // LDIR
        0x3e, 0x81,             // LD A,$81       drive 0, double density
        0xd3, 0xf4,             // OUT ($F4),A
        0x3e, 0x02,             // LD A,2
        0xd3, 0xf2,             // OUT ($F2),A    sector 2
        0x3e, 0xa0,             // LD A,$A0       WRITE SECTOR
        0xd3, 0xf0,             // OUT ($F0),A
        0x3e, 0xc1,             // LD A,$C1       ... with WAIT
        0xd3, 0xf4,             // OUT ($F4),A
        0x21, 0x00, 0x43,       // LD HL,$4300    source: this sector
        0x01, 0xf3, 0x00,       // LD BC,$00F3    256 bytes to port $F3
        0xed, 0xb3,             // OTIR
        // wait for the end:
        0xdb, 0xf0,             // IN A,($F0)
        0xcb, 0x47,             // BIT 0,A        BUSY?
        0x20, 0xfa,             // JR NZ,-6
        0x32, 0x40, 0x3c,       // LD ($3C40),A   final status
        0x18, 0xfe,             // JR $
    ];
    let mut s = vec![0u8; 256];
    s[..code.len()].copy_from_slice(code);
    s[0x80..0x8c].copy_from_slice(b"DISK BOOT OK");
    s
}

/// A one-track, single-sided DMK image in double density: 18 sectors of
/// 256 bytes numbered 1-18, sector 1 the boot sector.
fn m3_boot_disk() -> Vec<u8> {
    use media::dmk::{build_track, SectorSpec};
    let boot = m3_boot_sector();
    let specs: Vec<SectorSpec> = (1..=18u8)
        .map(|s| SectorSpec {
            track: 0,
            side: 0,
            sector: s,
            size: 256,
            dam: 0xfb,
            data: if s == 1 {
                boot.clone()
            } else {
                vec![0xe5; 256]
            },
            mfm: true,
        })
        .collect();
    let track = build_track(&specs, true);
    let mut bytes = vec![0u8; 16];
    bytes[1] = 1;
    let len = (track.data.len() + 128) as u16;
    bytes[2..4].copy_from_slice(&len.to_le_bytes());
    bytes[4] = 0x10; // single-sided
    let mut table = [0u8; 128];
    for (i, p) in track.idam.iter().enumerate() {
        let v = (p & 0x3fff) + 128;
        table[i * 2..i * 2 + 2].copy_from_slice(&(v | 0x8000).to_le_bytes());
    }
    bytes.extend_from_slice(&table);
    bytes.extend_from_slice(&track.data);
    bytes
}

fn press(m: &mut Trs80, key: emu_core::input::MachineKey, down: bool) {
    m.input(emu_core::input::TimedInput {
        at: m.now(),
        target: emu_core::input::InputTarget::Keyboard,
        event: emu_core::input::InputEvent::Key {
            machine_key: key,
            down,
        },
    });
}

/// Model III: BREAK at power-on skips the disk boot; the ROM asks Cass?
/// and Memory Size?, then BASIC computes.
#[test]
#[ignore = "needs EMU_ROM_DIR with the TRS-80 Model III ROM"]
fn model3_basic_computes() {
    let Some(mut m) = model3() else {
        return;
    };
    press(&mut m, machine_trs80::keyboard::keys::BREAK, true);
    run(&mut m, 3);
    press(&mut m, machine_trs80::keyboard::keys::BREAK, false);
    run(&mut m, 60);
    let s = m.screen_text();
    assert!(s.contains("Cass?"), "{s}");
    type_and_run(&mut m, "\n");
    let s = m.screen_text();
    assert!(s.contains("Memory Size?"), "{s}");
    type_and_run(&mut m, "\n");
    run(&mut m, 240);
    type_and_run(&mut m, "PRINT 6*7\n");
    let s = m.screen_text();
    assert!(s.contains(" 42"), "{s}");
    println!("{s}");
}

/// Model III: the ROM boots a double-density disk (track 0 sector 1 read
/// with INI under WAIT, ended by the controller's NMI) and the boot code
/// writes sector 2 through the FD1793.
#[test]
#[ignore = "needs EMU_ROM_DIR with the TRS-80 Model III ROM"]
fn model3_boots_a_double_density_disk() {
    let Some(mut m) = model3() else {
        return;
    };
    let data = std::sync::Arc::new(std::sync::Mutex::new(m3_boot_disk()));
    disk(&mut m, 0, data.clone());
    run(&mut m, 300);
    let s = m.screen_text();
    assert!(s.starts_with("DISK BOOT OK"), "{s}");
    assert_eq!(
        m.peek_cpu(0x3c40) & 0x1d,
        0,
        "status after the write: {:02x}",
        m.peek_cpu(0x3c40)
    );
    m.media_slot(machine_trs80::SLOT_DRIVE0)
        .unwrap()
        .flush()
        .unwrap();
    let image = media::dmk::Image::parse(&data.lock().unwrap()).unwrap();
    let sectors = media::dmk::decode_sectors(&image.tracks[0]);
    let two = sectors.iter().find(|s| s.sector == 2).unwrap();
    assert!(two.crc_ok);
    assert_eq!(
        two.data,
        m3_boot_sector(),
        "sector 2 holds the written bytes"
    );
}

fn model3() -> Option<Trs80> {
    let dir = media_backends::roms::env_dir()?;
    let config = MachineConfig {
        model: Some("3".into()),
        roms: media_backends::roms::load(&dir, "trs80").expect("ROMs"),
        ..MachineConfig::default()
    };
    Some(Trs80::new(&config).expect("Model III"))
}
