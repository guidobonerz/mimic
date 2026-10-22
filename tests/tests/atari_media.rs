//! Atari 800XL cartridge and cassette with real ROMs: the OS starts an 8K
//! cartridge built here; BASIC saves a program with CSAVE onto a blank CAS
//! tape and loads it back with CLOAD.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test atari_media -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};
use machine_atari8::{CPU_SPACE, SLOT_CARTRIDGE, SLOT_TAPE};
use media_backends::image::MemoryBackend;

fn machine() -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "atari8").expect("ROMs"),
        ..MachineConfig::default()
    };
    emu_machines::registry().create("atari8", &config).unwrap()
}

fn run(m: &mut dyn Machine, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn byte(m: &mut dyn Machine, addr: u32) -> u8 {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    cpu.peek(CPU_SPACE, addr, Size::Byte).unwrap() as u8
}

fn screen(m: &mut dyn Machine) -> Vec<u8> {
    let base = u32::from(byte(m, 0x58)) | (u32::from(byte(m, 0x59)) << 8);
    (base..base + 40 * 24).map(|a| byte(m, a)).collect()
}

fn find_all(screen: &[u8], text: &str) -> Vec<usize> {
    let pat: Vec<u8> = text.bytes().map(|b| b - 0x20).collect();
    screen
        .windows(pat.len())
        .enumerate()
        .filter(|(_, w)| *w == pat)
        .map(|(i, _)| i / 40)
        .collect()
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, end) = machine_atari8::keyboard::typed_events(text, m.now().0 + 1000);
    for e in events {
        m.input(e);
    }
    while m.now().0 < end + 50_000 {
        run(m, 1);
    }
}

/// Run until a READY appears on a line below `line` (up to `max` fields).
fn wait_ready_below(m: &mut dyn Machine, line: usize, max: usize) -> bool {
    for _ in 0..max / 10 {
        run(m, 10);
        if find_all(&screen(m), "READY").iter().any(|&l| l > line) {
            return true;
        }
    }
    false
}

#[test]
#[ignore = "needs the Atari XL OS ROM in EMU_ROM_DIR"]
fn os_starts_an_8k_cartridge() {
    let mut rom = vec![0xffu8; 0x2000];
    let code = [
        0xa9, 0x42, 0x8d, 0x00, 0x06, // LDA #$42; STA $0600
        0x4c, 0x05, 0xa0, // JMP $A005
        0x60, // $A008: init RTS
    ];
    rom[..code.len()].copy_from_slice(&code);
    // $BFFA start, $BFFC present, $BFFD flags (start the cartridge),
    // $BFFE init.
    rom[0x1ffa..].copy_from_slice(&[0x00, 0xa0, 0x00, 0x04, 0x08, 0xa0]);
    let mut m = machine();
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            name: "test.rom".into(),
            kind: MediaKind::Cartridge,
            backend: Box::new(MemoryBackend { data: rom }),
        })
        .unwrap();
    // A cartridge needs no disk boot wait: the OS tries D1: only when
    // flag bit 0 asks for it.
    run(m.as_mut(), 100);
    let shot = snapshot(m.as_ref(), "atari_cart.png");
    assert_eq!(byte(m.as_mut(), 0x0600), 0x42, "cartridge ran; {shot}");
    assert_eq!(byte(m.as_mut(), 0xd013), 1, "TRIG3");
}

#[test]
#[ignore = "needs Atari XL OS and BASIC ROMs in EMU_ROM_DIR"]
fn basic_csave_and_cload_through_the_program_recorder() {
    let mut m = machine();
    m.media_slot(SLOT_TAPE)
        .unwrap()
        .insert(Media {
            name: "blank.cas".into(),
            kind: MediaKind::Tape,
            backend: Box::new(MemoryBackend {
                data: media::cas::blank(64 * 1024),
            }),
        })
        .unwrap();
    m.media_slot(SLOT_TAPE).unwrap().control("record").unwrap();
    run(m.as_mut(), 350);
    type_text(m.as_mut(), "10 PRINT \"HI\"\r");
    type_text(m.as_mut(), "CSAVE\r");
    // Two beeps, then RETURN starts the recording.
    run(m.as_mut(), 100);
    let line = find_all(&screen(m.as_mut()), "CSAVE")[0];
    type_text(m.as_mut(), "\r");
    let saved = wait_ready_below(m.as_mut(), line, 5000);
    let shot = snapshot(m.as_ref(), "atari_csave.png");
    assert!(saved, "CSAVE finished; {shot}");

    let tape = m.media_slot(SLOT_TAPE).unwrap();
    tape.control("rewind").unwrap();
    tape.control("play").unwrap();
    type_text(m.as_mut(), "NEW\r");
    type_text(m.as_mut(), "CLOAD\r");
    run(m.as_mut(), 100);
    let line = *find_all(&screen(m.as_mut()), "CLOAD").last().unwrap();
    type_text(m.as_mut(), "\r");
    let loaded = wait_ready_below(m.as_mut(), line, 5000);
    let shot = snapshot(m.as_ref(), "atari_cload.png");
    assert!(loaded, "CLOAD finished; {shot}");
    let s = screen(m.as_mut());
    assert!(find_all(&s, "ERROR").is_empty(), "no load error; {shot}");
    type_text(m.as_mut(), "LIST\r");
    let shot = snapshot(m.as_ref(), "atari_list.png");
    assert!(
        !find_all(&screen(m.as_mut()), "10 PRINT \"HI\"").is_empty(),
        "program back; {shot}"
    );
}
