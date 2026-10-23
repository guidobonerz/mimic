//! ColecoVision with the Coleco BIOS (user-supplied via `EMU_ROM_DIR`, e.g.
//! a `coleco/` subfolder with `coleco.rom`), and optionally cartridges.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test coleco_boot -- --ignored --nocapture
//!
//! `EMU_COLECO_CARTS=/path` runs every `.col`/`.rom`/`.bin` there for a few
//! seconds and saves a picture of each in the test's temporary folder
//! (Phase 11 gate: "user-supplied cartridges run"); `EMU_FRAMES=n` the
//! length (default 900 fields).

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, StopReason};

fn machine(model: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "coleco").expect("ROMs"),
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    emu_machines::registry().create("coleco", &config).unwrap()
}

fn run(m: &mut dyn Machine, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

/// Distinct colours in the picture.
fn colours(m: &dyn Machine) -> usize {
    let f = m.framebuffer();
    let mut set = std::collections::BTreeSet::new();
    for y in 0..f.height {
        for x in 0..f.width {
            let c = f.rgb_at(x, y).unwrap();
            set.insert((c.r, c.g, c.b));
        }
    }
    set.len()
}

#[test]
#[ignore = "needs the ColecoVision BIOS in EMU_ROM_DIR"]
fn bios_shows_its_screen_without_a_cartridge() {
    for model in ["ntsc", "pal"] {
        let mut m = machine(model);
        run(m.as_mut(), 300);
        let path = snapshot(m.as_ref(), &format!("coleco-{model}.png"));
        let n = colours(m.as_ref());
        eprintln!("{model}: {n} colours, {path}");
        // The BIOS draws its title and the "turn game off" text: more than
        // the backdrop.
        assert!(n >= 2, "{model}: only the backdrop, see {path}");
    }
}

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

#[test]
#[ignore = "needs the ColecoVision BIOS in EMU_ROM_DIR and EMU_COLECO_CARTS"]
fn user_cartridges_run() {
    let dir = std::env::var("EMU_COLECO_CARTS").expect("EMU_COLECO_CARTS not set");
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "col" | "rom" | "bin"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no cartridges in {dir}");
    let mut failed = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut m = machine("ntsc");
        let bytes = std::fs::read(&path).unwrap();
        let slot = m.media_slot(SlotId(0)).unwrap();
        if let Err(e) = slot.insert(Media {
            kind: MediaKind::Cartridge,
            name: name.clone(),
            backend: Box::new(Bytes(bytes)),
        }) {
            eprintln!("{name}: not loaded: {e}");
            failed.push(name);
            continue;
        }
        run(m.as_mut(), frames);
        let n = colours(m.as_ref());
        let shot = snapshot(m.as_ref(), &format!("coleco-{name}.png"));
        eprintln!("{name}: {n} colours, {shot}");
        if n < 2 {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "no picture: {failed:?}");
}
