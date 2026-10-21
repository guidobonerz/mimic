//! Amiga 500 Kickstart boot with user-supplied ROMs (`EMU_ROM_DIR`, e.g.
//! an `amiga/` subfolder with "Kickstart 1.3.rom" or kick13.rom).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test amiga_boot -- --ignored --nocapture
//!
//! `EMU_KICK=2.0` picks another Kickstart, `EMU_FRAMES=n` the trace length.

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Size, StopReason};

fn boot(kick: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "amiga").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("kickstart".into(), kick.into());
    emu_machines::registry().create("amiga", &config).unwrap()
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn trace_kickstart() {
    let kick = std::env::var("EMU_KICK").unwrap_or_else(|_| "1.3".into());
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let mut m = boot(&kick);
    let mut dbg = DebugHooks::new();
    for f in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        if f % 20 == 0 {
            let peek = |m: &mut Box<dyn Machine>, a: u32| {
                m.debug_target(machine_amiga::CPU)
                    .unwrap()
                    .peek(machine_amiga::CPU_SPACE, a, Size::Word)
                    .unwrap()
            };
            let (intena, intreq, dmacon) = (
                peek(&mut m, 0xdf_f01c),
                peek(&mut m, 0xdf_f01e),
                peek(&mut m, 0xdf_f002),
            );
            let t = m.debug_target(machine_amiga::CPU).unwrap();
            let r = t.regs();
            let pc = r.get("PC").unwrap();
            eprintln!(
                "frame {f}: pc {pc:06x} sr {:04x} a7 {:08x} {:<28} intena {intena:04x} intreq {intreq:04x} dmacon {dmacon:04x}",
                r.get("SR").unwrap(),
                r.get("A7").unwrap(),
                t.disassemble(pc).to_string(),
            );
        }
    }
    eprintln!("{}", snapshot(m.as_ref(), &format!("kick{kick}.png")));
}

/// Milestone M1 gate: every Kickstart found reaches its "insert disk"
/// screen (1.x: the hand with the disk, white, black, blue and grey; 2.0+:
/// the drive animation on many colours).
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn kickstarts_show_the_insert_disk_screen() {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&dir, "amiga").expect("ROMs");
    let mut tested = 0;
    for (kick, role) in [
        ("1.2", "kick12"),
        ("1.3", "kick13"),
        ("2.0", "kick20"),
        ("3.1", "kick31"),
    ] {
        if !roms.contains_key(role) {
            continue;
        }
        let mut m = boot(kick);
        let mut dbg = DebugHooks::new();
        for _ in 0..400 {
            assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        }
        let f = m.framebuffer();
        let mut colours = std::collections::BTreeSet::new();
        for y in 0..f.height {
            for x in 0..f.width {
                let c = f.rgb_at(x, y).unwrap();
                colours.insert((c.r, c.g, c.b));
            }
        }
        let path = snapshot(m.as_ref(), &format!("kick{kick}.png"));
        assert!(
            colours.len() >= 4,
            "Kickstart {kick}: {} colours, see {path}",
            colours.len()
        );
        tested += 1;
    }
    assert!(tested > 0, "no Kickstart ROM found");
}
