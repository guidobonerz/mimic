//! PET with the virtual IEEE-488 drive and real ROMs (user-supplied via
//! `EMU_ROM_DIR`): BASIC 4 disk commands on a D80, LOAD/SAVE on BASIC 2,
//! and DEVICE NOT PRESENT without a drive.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test pet_drive -- --ignored

use std::sync::{Arc, Mutex};

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, MemSpaceId, Size};
use machine_pet::keyboard::Layout;
use media::D64;

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

fn pet(model: &str, drive: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "pet").expect("ROMs"),
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("drive8".into(), drive.into());
    emu_machines::registry().create("pet", &config).unwrap()
}

fn screen(m: &mut dyn Machine) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0x8000..0x8000 + 1000)
        .map(|a| cpu.peek(MemSpaceId(0), a, Size::Byte).unwrap() as u8)
        .collect()
}

fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

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
    let pat = codes(text);
    let s: Vec<u8> = screen(m).iter().map(|c| c & 0x7f).collect();
    s.windows(pat.len()).filter(|w| *w == pat).count()
}

fn run_until(m: &mut dyn Machine, text: &str, n: usize, max: u32) -> bool {
    let mut dbg = DebugHooks::new();
    (0..max).any(|_| {
        m.run_frame(&mut dbg);
        count(m, text) >= n
    })
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, end) = machine_pet::keyboard::typed_events(Layout::Graphics, text, m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end {
        m.run_frame(&mut dbg);
    }
}

/// Type a command and wait until the last line on the screen is READY.
/// again (the screen may have scrolled).
fn command(m: &mut dyn Machine, cmd: &str) {
    type_text(m, cmd);
    let mut dbg = DebugHooks::new();
    let done = (0..1500).any(|_| {
        m.run_frame(&mut dbg);
        let t = text(m);
        t.lines().rev().find(|l| !l.trim().is_empty()) == Some("READY.")
    });
    assert!(done, "{cmd:?} did not finish:\n{}", text(m));
}

fn drive8(m: &dyn Machine) -> SlotId {
    m.media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .expect("drive 8")
        .id
}

fn insert(m: &mut dyn Machine, image: &Arc<Mutex<Vec<u8>>>) {
    let slot = drive8(m);
    m.media_slot(slot)
        .unwrap()
        .insert(Media {
            name: "t.d80".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Shared(image.clone())),
        })
        .unwrap();
}

/// `10 PRINT "HI"` at `$0401`.
const HELLO: [u8; 15] = [
    0x01, 0x04, 0x0c, 0x04, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, 0x00, 0x00,
];

fn ran(m: &mut dyn Machine) -> bool {
    screen(m)
        .chunks(40)
        .any(|row| row.starts_with(&codes("HI ")))
}

#[test]
#[ignore = "needs PET ROMs in EMU_ROM_DIR"]
fn basic4_disk_commands_on_a_d80() {
    let mut d = D64::blank_8050(b"PET DISK", *b"80", false);
    d.write_file(b"HELLO", 2, &HELLO).unwrap();
    let image = Arc::new(Mutex::new(d.to_bytes()));
    let mut m = pet("4032", "virtual");
    assert!(run_until(m.as_mut(), "READY.", 1, 300), "boot");
    command(m.as_mut(), "PRINT DS$\n");
    assert_eq!(
        count(m.as_mut(), "73,MIMIC IEEE DRIVE"),
        1,
        "\n{}",
        text(m.as_mut())
    );
    insert(m.as_mut(), &image);
    command(m.as_mut(), "DIRECTORY\n");
    let shown = text(m.as_mut());
    assert_eq!(
        count(m.as_mut(), "\"PET DISK        \" 80 2C"),
        1,
        "\n{shown}"
    );
    assert_eq!(count(m.as_mut(), "\"HELLO\""), 1, "\n{shown}");
    assert_eq!(count(m.as_mut(), "2051 BLOCKS FREE"), 1, "\n{shown}");
    command(m.as_mut(), "DLOAD\"HELLO\"\n");
    command(m.as_mut(), "RUN\n");
    assert!(ran(m.as_mut()), "\n{}", text(m.as_mut()));
    // DSAVE writes into the image; DS$ reports OK.
    type_text(m.as_mut(), "20 PRINT \"SAVED\"\n");
    command(m.as_mut(), "DSAVE\"NEW\"\n");
    command(m.as_mut(), "PRINT DS$\n");
    assert!(
        count(m.as_mut(), "00, OK,00,00") >= 1,
        "\n{}",
        text(m.as_mut())
    );
    let back = D64::parse(&image.lock().unwrap()).unwrap();
    let (_, prg) = back.load(b"NEW").unwrap();
    assert!(prg.data.windows(5).any(|w| w == b"SAVED"));
}

#[test]
#[ignore = "needs PET ROMs in EMU_ROM_DIR"]
fn basic2_loads_and_saves_on_a_d64() {
    let mut d = D64::blank(b"OLD PET", *b"20");
    d.write_file(b"HELLO", 2, &HELLO).unwrap();
    let image = Arc::new(Mutex::new(d.to_bytes()));
    let mut m = pet("3032", "virtual");
    assert!(run_until(m.as_mut(), "READY.", 1, 300), "boot");
    insert(m.as_mut(), &image);
    command(m.as_mut(), "LOAD\"HELLO\",8\n");
    command(m.as_mut(), "RUN\n");
    assert!(ran(m.as_mut()), "\n{}", text(m.as_mut()));
    command(m.as_mut(), "SAVE\"COPY\",8\n");
    let back = D64::parse(&image.lock().unwrap()).unwrap();
    assert_eq!(back.read_file(b"COPY").unwrap().1, HELLO);
}

#[test]
#[ignore = "needs PET ROMs in EMU_ROM_DIR"]
fn without_a_drive_the_device_is_not_present() {
    let mut m = pet("4032", "none");
    assert!(run_until(m.as_mut(), "READY.", 1, 300), "boot");
    assert!(m.media_slots().iter().all(|s| s.name != "drive 8"));
    command(m.as_mut(), "LOAD\"X\",8\n");
    assert_eq!(
        count(m.as_mut(), "DEVICE NOT PRESENT"),
        1,
        "\n{}",
        text(m.as_mut())
    );
}
