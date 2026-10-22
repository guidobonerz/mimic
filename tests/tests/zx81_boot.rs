//! ZX81 with the Sinclair ROM (user-supplied via `EMU_ROM_DIR`, e.g. a
//! `zx81/` subfolder with `zx81.rom` or `zx81.edition3.rom`).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test zx81_boot -- --ignored --nocapture

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};
use std::sync::{Arc, Mutex};

/// System variables: D_FILE (the display file) and E_LINE.
const D_FILE: u32 = 0x400c;

fn machine() -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "zx81").expect("ROMs"),
        ..MachineConfig::default()
    };
    emu_machines::registry().create("zx81", &config).unwrap()
}

fn run(m: &mut dyn Machine, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn byte(m: &mut dyn Machine, addr: u32) -> u8 {
    m.debug_target(DeviceId("cpu"))
        .unwrap()
        .peek(machine_zx81::CPU_SPACE, addr, Size::Byte)
        .unwrap() as u8
}

/// The 24 rows of the display file as ZX81 character codes (rows end at
/// their HALT; short rows are padded with spaces).
fn screen(m: &mut dyn Machine) -> Vec<Vec<u8>> {
    let base = u32::from(byte(m, D_FILE)) | u32::from(byte(m, D_FILE + 1)) << 8;
    let mut rows = Vec::new();
    let mut a = base + 1;
    for _ in 0..24 {
        let mut row = Vec::new();
        loop {
            let c = byte(m, a);
            a += 1;
            if c == 0x76 {
                break;
            }
            row.push(c);
            if row.len() > 32 {
                break;
            }
        }
        row.resize(32, 0);
        rows.push(row);
    }
    rows
}

/// ZX81 codes of `text` (digits $1C+, letters $26+, space 0, `"` $0B).
fn codes(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            ' ' => 0,
            '"' => 0x0b,
            '0'..='9' => 0x1c + (c as u8 - b'0'),
            'A'..='Z' => 0x26 + (c as u8 - b'A'),
            _ => 0xff,
        })
        .collect()
}

fn find(screen: &[Vec<u8>], text: &str) -> Option<(usize, usize)> {
    let pat = codes(text);
    screen.iter().enumerate().find_map(|(y, row)| {
        row.windows(pat.len())
            .position(|w| w == pat)
            .map(|x| (x, y))
    })
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, _) = machine_zx81::keyboard::typed_events(text, m.now().0);
    for e in events {
        m.input(e);
    }
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

/// Where the ROM's display sits in the picture: the leftmost/topmost and
/// rightmost/bottommost black pixel.
fn ink_bounds(m: &dyn Machine) -> Option<(u32, u32, u32, u32)> {
    let f = m.framebuffer();
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..f.height {
        for x in 0..f.width {
            if f.rgb_at(x, y).is_some_and(|c| c.r < 0x80) {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

#[test]
#[ignore = "needs the ZX81 ROM in EMU_ROM_DIR"]
fn zx81_boots_and_prints() {
    let mut m = machine();
    run(m.as_mut(), 150);
    // The K cursor (inverse K, $B0) waits at the bottom left.
    let s = screen(m.as_mut());
    assert!(
        s.iter().any(|r| r[0] == 0xb0),
        "no K cursor; rows: {:?}",
        &s[..2]
    );
    // PRINT "HELLO": P is PRINT in K mode, " is SHIFT+P.
    type_text(m.as_mut(), "P\"HELLO\"\n");
    run(m.as_mut(), 200);
    let s = screen(m.as_mut());
    let path = snapshot(m.as_ref(), "zx81-print.png");
    eprintln!(
        "rows 0-1: {:?} {:?}; last: {:?}",
        &s[0][..12],
        &s[1][..12],
        &s[23][..12]
    );
    assert_eq!(
        find(&s, "HELLO"),
        Some((0, 0)),
        "HELLO at the top left, see {path}"
    );
    // The picture: the 256 × 192 display centred with equal borders.
    let (x0, y0, x1, y1) = ink_bounds(m.as_ref()).expect("ink");
    eprintln!("ink bounds x {x0}..{x1} y {y0}..{y1} ({path})");
    let f = m.framebuffer();
    let text_left = x0; // the H in HELLO is the display's first column
    assert!(
        (28..=36).contains(&text_left),
        "display column 0 at x {text_left}, window {}: adjust ula::H_OFFSET",
        f.width
    );
    // The K cursor's row is the last of the 24: its bottom edge marks the
    // display's bottom (192 lines from the top).
    let display_bottom = y1 + 1;
    let display_top = display_bottom - 192;
    assert!(
        (20..=28).contains(&display_top),
        "display row 0 at y {display_top}: adjust ula::V_OFFSET"
    );
}

/// A backend over shared bytes.
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
        d.resize(offset as usize + data.len(), 0);
        d[offset as usize..].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn tape(bytes: Arc<Mutex<Vec<u8>>>) -> Media {
    Media {
        name: "test.p".into(),
        kind: MediaKind::Tape,
        backend: Box::new(Shared(bytes)),
    }
}

/// Phase 10 gate: a program saved to a .P file loads into another machine.
#[test]
#[ignore = "needs the ZX81 ROM in EMU_ROM_DIR"]
fn a_p_file_round_trips_through_save_and_load() {
    let mut m = machine();
    run(m.as_mut(), 150);
    // 10 PRINT "HI" / SAVE "X"  (S is SAVE in K mode).
    type_text(m.as_mut(), "10P\"HI\"\n");
    run(m.as_mut(), 220);
    eprintln!("after typing: {:?}", &screen(m.as_mut())[0][..12]);
    let bytes = Arc::new(Mutex::new(vec![0u8; 1]));
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(tape(bytes.clone()))
        .unwrap();
    type_text(m.as_mut(), "S\"X\"\n");
    run(m.as_mut(), 300);
    m.media_slot(SlotId(0)).unwrap().flush().unwrap();
    let saved = bytes.lock().unwrap().clone();
    eprintln!(
        "saved {} bytes; last row: {:?}",
        saved.len(),
        &screen(m.as_mut())[23][..12]
    );
    assert!(saved.len() > 100, "saved {} bytes", saved.len());
    // A fresh machine: LOAD "" (J is LOAD), then RUN (R).
    let mut n = machine();
    run(n.as_mut(), 150);
    n.media_slot(SlotId(0))
        .unwrap()
        .insert(tape(bytes.clone()))
        .unwrap();
    type_text(n.as_mut(), "J\"\"\n");
    run(n.as_mut(), 200);
    // The ROM does not list after LOAD; run the program.
    type_text(n.as_mut(), "R\n");
    run(n.as_mut(), 200);
    let s = screen(n.as_mut());
    let path = snapshot(n.as_ref(), "zx81-loaded.png");
    assert_eq!(find(&s, "HI"), Some((0, 0)), "HI after RUN, see {path}");
}
