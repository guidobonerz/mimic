//! The datasette with real KERNALs (ROMs user-supplied via `EMU_ROM_DIR`):
//! LOAD from a TAP recorded by the CBM tape encoder, and SAVE onto a blank
//! tape that then loads into a fresh machine.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test tape -- --ignored

use std::sync::{Arc, Mutex};

use emu_core::input::TimedInput;
use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, MemSpaceId, Size};
use media::tap::{encode_cbm, encode_ted, Tap, TapMachine, Video};

/// A tape whose backend keeps what the datasette records.
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

/// How each machine shows text and takes typing.
struct Target {
    id: &'static str,
    model: Option<&'static str>,
    /// Screen RAM address and size (columns × rows).
    screen: u16,
    cols: usize,
    rows: usize,
    /// Start of BASIC programs.
    basic: u16,
    tap: TapMachine,
    typing: fn(&str, u64) -> (Vec<TimedInput>, u64),
}

const C64: Target = Target {
    id: "c64",
    model: None,
    screen: 0x0400,
    cols: 40,
    rows: 25,
    basic: 0x0801,
    tap: TapMachine::C64,
    typing: machine_c64::keyboard::typed_events,
};

const C128: Target = Target {
    id: "c128",
    model: None,
    screen: 0x0400,
    cols: 40,
    rows: 25,
    basic: 0x1c01,
    tap: TapMachine::C64,
    typing: machine_c128::keyboard::typed_events,
};

const VIC20: Target = Target {
    id: "vic20",
    model: None,
    screen: 0x1e00,
    cols: 22,
    rows: 23,
    basic: 0x1001,
    tap: TapMachine::Vic20,
    typing: machine_vic20::keyboard::typed_events,
};

const PET: Target = Target {
    id: "pet",
    model: Some("4032"),
    screen: 0x8000,
    cols: 40,
    rows: 25,
    basic: 0x0401,
    tap: TapMachine::Pet,
    typing: pet_typing,
};

fn pet_typing(text: &str, start: u64) -> (Vec<TimedInput>, u64) {
    machine_pet::keyboard::typed_events(machine_pet::keyboard::Layout::Graphics, text, start)
}

const PLUS4: Target = Target {
    id: "plus4",
    model: None,
    screen: 0x0c00,
    cols: 40,
    rows: 25,
    basic: 0x1001,
    tap: TapMachine::C16,
    typing: machine_ted::keyboard::typed_events,
};

const C16: Target = Target { id: "c16", ..PLUS4 };

fn machine(t: &Target) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, t.id).expect("ROMs"),
        model: t.model.map(Into::into),
        ..MachineConfig::default()
    };
    config.options.insert("drive8".into(), "none".into());
    emu_machines::registry().create(t.id, &config).unwrap()
}

fn screen(m: &mut dyn Machine, t: &Target) -> Vec<u8> {
    let space = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == "ram")
        .map_or(MemSpaceId(0), |s| s.id);
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0..(t.cols * t.rows) as u32)
        .map(|i| {
            cpu.peek(space, u32::from(t.screen) + i, Size::Byte)
                .unwrap() as u8
        })
        .collect()
}

fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

fn text(m: &mut dyn Machine, t: &Target) -> String {
    screen(m, t)
        .chunks(t.cols)
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

fn count(m: &mut dyn Machine, t: &Target, text: &str) -> usize {
    let pat = codes(text);
    let s: Vec<u8> = screen(m, t).iter().map(|c| c & 0x7f).collect();
    s.windows(pat.len()).filter(|w| *w == pat).count()
}

fn run_until(m: &mut dyn Machine, t: &Target, text: &str, n: usize, max: u32) -> bool {
    let mut dbg = DebugHooks::new();
    (0..max).any(|_| {
        m.run_frame(&mut dbg);
        count(m, t, text) >= n
    })
}

fn type_text(m: &mut dyn Machine, t: &Target, text: &str) {
    let (events, end) = (t.typing)(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end {
        m.run_frame(&mut dbg);
    }
}

/// Type a command, then wait for one more READY.
fn command(m: &mut dyn Machine, t: &Target, cmd: &str, max: u32) {
    let readies = count(m, t, "READY.");
    type_text(m, t, cmd);
    assert!(
        run_until(m, t, "READY.", readies + 1, max),
        "{cmd:?} did not finish:\n{}",
        text(m, t)
    );
}

fn tape_slot(m: &dyn Machine) -> SlotId {
    m.media_slots()
        .into_iter()
        .find(|s| s.name == "tape")
        .expect("tape slot")
        .id
}

fn insert(m: &mut dyn Machine, image: &Arc<Mutex<Vec<u8>>>) {
    let slot = tape_slot(m);
    m.media_slot(slot)
        .unwrap()
        .insert(Media {
            name: "t.tap".into(),
            kind: MediaKind::Tape,
            backend: Box::new(Shared(image.clone())),
        })
        .unwrap();
}

/// `10 PRINT "HI"` at `base`.
fn hello(base: u16) -> Vec<u8> {
    let [lo, hi] = base.to_le_bytes();
    let [nlo, nhi] = (base + 11).to_le_bytes();
    vec![
        lo, hi, nlo, nhi, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, 0x00, 0x00,
    ]
}

fn ran(m: &mut dyn Machine, t: &Target) -> bool {
    screen(m, t)
        .chunks(t.cols)
        .any(|row| row.starts_with(&codes("HI ")))
}

/// LOAD from tape, RUN: "HI".
fn load_and_run(t: &Target, image: &Arc<Mutex<Vec<u8>>>) {
    let mut m = machine(t);
    assert!(run_until(m.as_mut(), t, "READY.", 1, 1500), "boot");
    insert(m.as_mut(), image);
    command(m.as_mut(), t, "LOAD\n", 6000);
    assert_eq!(
        count(m.as_mut(), t, "FOUND HELLO"),
        1,
        "\n{}",
        text(m.as_mut(), t)
    );
    command(m.as_mut(), t, "RUN\n", 100);
    assert!(ran(m.as_mut(), t), "\n{}", text(m.as_mut(), t));
}

fn encoded(t: &Target) -> Arc<Mutex<Vec<u8>>> {
    let tap = if t.tap == TapMachine::C16 {
        encode_ted(&hello(t.basic), b"HELLO", true, Video::Pal).unwrap()
    } else {
        encode_cbm(&hello(t.basic), b"HELLO", true, t.tap, Video::Pal).unwrap()
    };
    Arc::new(Mutex::new(tap.to_bytes().unwrap()))
}

/// SAVE onto a blank tape, then LOAD it in a fresh machine.
fn save_and_reload(t: &Target) {
    let image = Arc::new(Mutex::new(drives_other::datasette::blank_tape(
        t.tap,
        Video::Pal,
        2,
    )));
    let mut m = machine(t);
    assert!(run_until(m.as_mut(), t, "READY.", 1, 1500), "boot");
    insert(m.as_mut(), &image);
    let slot = tape_slot(m.as_ref());
    m.media_slot(slot).unwrap().control("record").unwrap();
    type_text(m.as_mut(), t, "10 PRINT \"HI\"\n");
    command(m.as_mut(), t, "SAVE\"HELLO\"\n", 6000);
    m.media_slot(slot).unwrap().control("stop").unwrap();
    let recorded = Tap::parse(&image.lock().unwrap()).unwrap();
    assert!(
        recorded.pulses.len() > 20_000,
        "{} pulses",
        recorded.pulses.len()
    );
    load_and_run(t, &image);
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn c64_loads_from_tape() {
    load_and_run(&C64, &encoded(&C64));
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn c64_saves_to_tape() {
    save_and_reload(&C64);
}

#[test]
#[ignore = "needs C128 ROMs in EMU_ROM_DIR"]
fn c128_loads_from_tape() {
    load_and_run(&C128, &encoded(&C128));
}

#[test]
#[ignore = "needs C128 ROMs in EMU_ROM_DIR"]
fn c128_saves_to_tape() {
    save_and_reload(&C128);
}

#[test]
#[ignore = "needs VIC-20 ROMs in EMU_ROM_DIR"]
fn vic20_loads_from_tape() {
    load_and_run(&VIC20, &encoded(&VIC20));
}

#[test]
#[ignore = "needs VIC-20 ROMs in EMU_ROM_DIR"]
fn vic20_saves_to_tape() {
    save_and_reload(&VIC20);
}

#[test]
#[ignore = "needs PET ROMs in EMU_ROM_DIR"]
fn pet_loads_from_tape() {
    load_and_run(&PET, &encoded(&PET));
}

#[test]
#[ignore = "needs PET ROMs in EMU_ROM_DIR"]
fn pet_saves_to_tape() {
    save_and_reload(&PET);
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn plus4_loads_from_tape() {
    load_and_run(&PLUS4, &encoded(&PLUS4));
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn plus4_saves_to_tape() {
    save_and_reload(&PLUS4);
}

#[test]
#[ignore = "needs C16 ROMs in EMU_ROM_DIR"]
fn c16_loads_and_saves() {
    load_and_run(&C16, &encoded(&C16));
    save_and_reload(&C16);
}
