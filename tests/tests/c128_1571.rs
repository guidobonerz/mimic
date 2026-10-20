//! C128 with a true-emulated 1571 (ROMs user-supplied via `EMU_ROM_DIR`):
//! the drive identifies itself, reads both sides of a D71, loads with fast
//! serial transfers and formats double-sided disks.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test c128_1571 -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size};
use media::D64;
use media_backends::image::MemoryBackend;

fn boot(drive: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "c128").expect("ROMs"),
        ..MachineConfig::default()
    };
    let role = format!("dos{drive}");
    assert!(config.roms.contains_key(&role), "{role} ROM missing");
    config.options.insert("drive8".into(), drive.into());
    emu_machines::registry().create("c128", &config).unwrap()
}

/// The 40-column screen (screen codes).
fn screen(m: &mut dyn Machine) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0x0400..0x07e8)
        .map(|a| cpu.peek(machine_c128::RAM_SPACE, a, Size::Byte).unwrap() as u8)
        .collect()
}

fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

/// The screen as text, for failure messages.
fn text(m: &mut dyn Machine) -> String {
    screen(m)
        .chunks(40)
        .map(|row| {
            row.iter()
                .map(|&c| match c & 0x7f {
                    c @ 1..=26 => (c + 0x40) as char,
                    c @ 0x20..=0x3f => c as char,
                    _ => ' ',
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn count(m: &mut dyn Machine, text: &str) -> usize {
    // Reverse video (bit 7) counts as the same character.
    let pat = codes(text);
    let s: Vec<u8> = screen(m).iter().map(|c| c & 0x7f).collect();
    s.windows(pat.len()).filter(|w| *w == pat).count()
}

/// Run frames until `text` shows up `n` times; returns the frames it took.
fn run_until(m: &mut dyn Machine, text: &str, n: usize, max: u32) -> Option<u32> {
    let mut dbg = DebugHooks::new();
    (1..=max).find(|_| {
        m.run_frame(&mut dbg);
        count(m, text) >= n
    })
}

/// Type `text`, then wait for one more READY. than before.
fn command(m: &mut dyn Machine, text: &str, max: u32) -> Option<u32> {
    let readies = count(m, "READY.");
    let (events, end) = machine_c128::keyboard::typed_events(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    let mut frames = 0;
    while m.now().0 < end {
        m.run_frame(&mut dbg);
        frames += 1;
    }
    run_until(m, "READY.", readies + 1, max).map(|f| f + frames)
}

fn drive8(m: &dyn Machine) -> SlotId {
    m.media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .expect("drive 8 present")
        .id
}

fn insert(m: &mut dyn Machine, image: Vec<u8>) {
    let slot = drive8(m);
    let disk = Media {
        name: "test.d71".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend { data: image }),
    };
    m.media_slot(slot).unwrap().insert(disk).unwrap();
}

/// A D71 with a 100-block file on side 0 and `10 PRINT "HI"` on side 1.
fn two_sided_disk() -> Vec<u8> {
    let mut d = D64::blank_d71(b"TWO SIDES", *b"71");
    let mut big = vec![0x01, 0x1c];
    big.extend((0..100 * 254 - 2).map(|i| i as u8));
    d.write_file(b"BIG", 2, &big).unwrap();
    // Side 0 full: the next file goes to side 1.
    let filler = vec![0xea; (664 - 100) * 254];
    d.write_file(b"FILLER", 2, &filler).unwrap();
    let program = [
        0x01, 0x1c, 0x0c, 0x1c, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, 0x00, 0x00,
    ];
    d.write_file(b"HELLO", 2, &program).unwrap();
    assert!(d.read_file(b"HELLO").unwrap().0.track > 35, "on side 1");
    d.to_bytes()
}

#[test]
#[ignore = "needs C128 ROMs and the 1571 DOS ROM in EMU_ROM_DIR"]
fn power_on_status_names_the_1571() {
    let mut m = boot("1571");
    run_until(m.as_mut(), "READY.", 1, 1500).expect("READY.");
    command(m.as_mut(), "PRINT DS$\n", 300).expect("PRINT DS$");
    assert_eq!(count(m.as_mut(), "73,CBM DOS V3.0 1571,00,00"), 1);
}

#[test]
#[ignore = "needs C128 ROMs and the 1571 DOS ROM in EMU_ROM_DIR"]
fn reads_both_sides_of_a_d71() {
    let mut m = boot("1571");
    run_until(m.as_mut(), "READY.", 1, 1500).expect("READY.");
    insert(m.as_mut(), two_sided_disk());
    command(m.as_mut(), "DIRECTORY\n", 1500).expect("DIRECTORY");
    let shown = text(m.as_mut());
    assert_eq!(
        count(m.as_mut(), "\"TWO SIDES       \" 71 2A"),
        1,
        "header:\n{shown}"
    );
    assert_eq!(
        count(m.as_mut(), "663 BLOCKS FREE"),
        1,
        "both sides counted"
    );
    command(m.as_mut(), "RUN\"HELLO\"\n", 1500).expect("RUN");
    let s = screen(m.as_mut());
    assert!(
        s.chunks(40).any(|row| row.starts_with(&codes("HI "))),
        "program from side 1 did not run"
    );
}

/// The C128 KERNAL and the 1571 switch to fast serial by themselves; the
/// same file from a 1541 (slow serial only) takes many times longer.
#[test]
#[ignore = "needs C128 ROMs and the 1541 and 1571 DOS ROMs in EMU_ROM_DIR"]
fn fast_serial_loads_much_faster() {
    let mut frames = Vec::new();
    for drive in ["1571", "1541"] {
        let mut m = boot(drive);
        run_until(m.as_mut(), "READY.", 1, 1500).expect("READY.");
        insert(m.as_mut(), two_sided_disk());
        let f = command(m.as_mut(), "BLOAD\"BIG\",B0,P8192\n", 6000)
            .unwrap_or_else(|| panic!("{drive}: BLOAD did not finish:\n{}", text(m.as_mut())));
        let cpu = m.debug_target(DeviceId("cpu")).unwrap();
        let loaded: Vec<u8> = (0x2000..0x2010)
            .map(|a| cpu.peek(machine_c128::RAM_SPACE, a, Size::Byte).unwrap() as u8)
            .collect();
        assert_eq!(loaded, (0..16).collect::<Vec<u8>>(), "{drive}: data");
        eprintln!("{drive}: 100 blocks in {f} frames");
        frames.push(f);
    }
    assert!(
        frames[0] * 4 < frames[1],
        "1571 {} vs 1541 {} frames",
        frames[0],
        frames[1]
    );
}

#[test]
#[ignore = "needs C128 ROMs and the 1571 DOS ROM in EMU_ROM_DIR"]
fn formats_a_double_sided_disk() {
    let mut m = boot("1571");
    run_until(m.as_mut(), "READY.", 1, 1500).expect("READY.");
    // An unformatted double-sided disk.
    insert(m.as_mut(), vec![0; 349_696]);
    command(
        m.as_mut(),
        "OPEN 15,8,15,\"N0:FRESH,F7\":CLOSE 15\n",
        20_000,
    )
    .expect("format finished");
    command(m.as_mut(), "PRINT DS$\n", 300).expect("PRINT DS$");
    assert_eq!(count(m.as_mut(), "00, OK,00,00"), 1, "drive status");
    // Let the motor stop, then look at the image.
    let mut dbg = DebugHooks::new();
    for _ in 0..300 {
        m.run_frame(&mut dbg);
    }
    let slot = drive8(m.as_ref());
    let mut media = m.media_slot(slot).unwrap().eject().expect("disk");
    let mut bytes = vec![0; media.backend.len() as usize];
    media.backend.read(0, &mut bytes).unwrap();
    let d = D64::parse(&bytes).unwrap();
    assert!(d.double_sided);
    assert_eq!(d.sector(18, 0).unwrap()[3], 0x80, "double-sided BAM flag");
    assert_eq!(&d.sector(18, 0).unwrap()[0x90..0x95], b"FRESH");
    assert_eq!(d.blocks_free(), 1328);
}
