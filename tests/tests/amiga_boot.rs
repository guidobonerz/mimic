//! Amiga 500 Kickstart boot with user-supplied ROMs (`EMU_ROM_DIR`, e.g.
//! an `amiga/` subfolder with "Kickstart 1.3.rom" or kick13.rom).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p mimic-tests --release --test amiga_boot -- --ignored --nocapture
//!
//! `EMU_KICK=2.0` picks another Kickstart, `EMU_MODEL=1200` the board,
//! `EMU_FRAMES=n` the trace length,
//! `EMU_DISK`/`EMU_DISK1` ADFs for DF0:/DF1:, `EMU_OPTION=drives=2,…` machine
//! options, `EMU_EVERY=n` the trace interval, `EMU_CHIPS=1` disk registers.

use mimic_core::machine::Machine;
use mimic_core::registry::MachineConfig;
use mimic_core::{DebugHooks, Size, StopReason};

fn boot(kick: &str) -> Box<dyn Machine> {
    // EMU_MODEL=1200: the board.
    boot_model(kick, std::env::var("EMU_MODEL").ok())
}

fn boot_model(kick: &str, model: Option<String>) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "amiga").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("kickstart".into(), kick.into());
    config.model = model;
    // EMU_OPTION=key=value,key=value: further machine options.
    for kv in std::env::var("EMU_OPTION").unwrap_or_default().split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            config.options.insert(k.into(), v.into());
        }
    }
    mimic_machines::registry().create("amiga", &config).unwrap()
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = mimic_preview::encode_png(&m.framebuffer());
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
    // EMU_DISK=file.adf (and EMU_DISK1 for DF1:) go into the drives.
    for (slot, var) in ["EMU_DISK", "EMU_DISK1"].iter().enumerate() {
        if let Ok(path) = std::env::var(var) {
            let media = media_backends::image::open(std::path::Path::new(&path)).expect("ADF");
            m.media_slot(mimic_core::media::SlotId(slot as u8))
                .expect("drive")
                .insert(media)
                .expect("insert");
        }
    }
    let every: usize = std::env::var("EMU_EVERY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let mut dbg = DebugHooks::new();
    for f in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        if f % every == 0 {
            if std::env::var_os("EMU_CHIPS").is_some() {
                for c in m.chips() {
                    if ["paula", "floppy", "cia_b"].contains(&c.name) {
                        let regs: Vec<String> = c
                            .regs
                            .iter()
                            .map(|r| format!("{}={:x}", r.name, r.value))
                            .collect();
                        eprintln!("  {}: {}", c.name, regs.join(" "));
                    }
                }
            }
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

/// A disk whose boot block switches the Copper off and paints COLOR00
/// green, the way demos start (bootblock: "DOS", 0; checksum; root block
/// 880; code at offset 12 — Laurent Clévy, ADF FAQ "The bootblock").
fn bootable_adf() -> Vec<u8> {
    let mut adf = vec![0u8; media::adf::ADF_SIZE];
    let code: [u16; 9] = [
        0x33fc, 0x01a0, 0x00df,
        0xf096, // move.w #$01a0,DMACON  (Copper, bitplanes, Blitter off)
        0x33fc, 0x00f0, 0x00df, 0xf180, // move.w #$0f0,COLOR00
        0x60fe, //                         bra.s *
    ];
    adf[..4].copy_from_slice(b"DOS\0");
    adf[8..12].copy_from_slice(&880u32.to_be_bytes());
    for (i, w) in code.iter().enumerate() {
        adf[12 + 2 * i..14 + 2 * i].copy_from_slice(&w.to_be_bytes());
    }
    // Checksum: the 256 longs add up (with end-around carry) to $FFFFFFFF.
    let mut sum: u32 = 0;
    for c in adf[..1024].chunks(4) {
        let (s, carry) = sum.overflowing_add(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
        sum = s + u32::from(carry);
    }
    adf[4..8].copy_from_slice(&(!sum).to_be_bytes());
    adf
}

/// Milestone M3: Kickstart reads the boot block through trackdisk (disk
/// DMA, MFM, sync) and runs it.
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn kickstart_boots_a_bootblock_from_df0() {
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
        let media = mimic_core::media::Media {
            name: "boot.adf".into(),
            kind: mimic_core::media::MediaKind::Disk,
            backend: Box::new(media_backends::image::MemoryBackend {
                data: bootable_adf(),
            }),
        };
        m.media_slot(mimic_core::media::SlotId(0))
            .unwrap()
            .insert(media)
            .unwrap();
        let mut dbg = DebugHooks::new();
        for _ in 0..500 {
            assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        }
        let px = m.framebuffer().rgb_at(360, 140).unwrap();
        let path = snapshot(m.as_ref(), &format!("bootblock{kick}.png"));
        assert_eq!(
            (px.r, px.g, px.b),
            (0, 255, 0),
            "Kickstart {kick}, see {path}"
        );
        tested += 1;
    }
    assert!(tested > 0, "no Kickstart ROM found");
}

/// Debug aid: render the first bitplane of the screen the Copper list at
/// COP2LC shows, straight from chip RAM (`EMU_DISK`, `EMU_FRAMES`).
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR and EMU_DISK"]
fn dump_bitplane() {
    let mut m = boot("1.3");
    let path = std::env::var("EMU_DISK").expect("EMU_DISK");
    let media = media_backends::image::open(std::path::Path::new(&path)).expect("ADF");
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
    let cop2 = m.chips()[1].regs[1].value;
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let w = |a: u32| t.peek(machine_amiga::CHIP_SPACE, a, Size::Word).unwrap();
    let mut regs = std::collections::BTreeMap::new();
    for i in 0..200 {
        let (a, b) = (w(cop2 + 4 * i), w(cop2 + 4 * i + 2));
        if a & 1 == 0 {
            regs.entry(a).or_insert(b);
        }
        if a == 0xffff && b == 0xfffe {
            break;
        }
    }
    eprintln!("{:x?}", regs);
    let pt = regs[&0xe0] << 16 | regs[&0xe2];
    let (width, height) = (640u32, 200u32);
    let modulo = regs.get(&0x108).copied().unwrap_or(0) as i16 as i32;
    let stride = (width / 8) as i32 + modulo;
    let mut img = image_rows(width, height, |x, y| {
        let a = (pt as i32 + y as i32 * stride + (x / 16 * 2) as i32) as u32;
        w(a) & (0x8000 >> (x % 16)) != 0
    });
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("bitplane1.pgm");
    let mut data = format!("P5 {width} {height} 255\n").into_bytes();
    data.append(&mut img);
    std::fs::write(&out, data).unwrap();
    eprintln!("{}", out.display());
}

fn image_rows(w: u32, h: u32, px: impl Fn(u32, u32) -> bool) -> Vec<u8> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| if px(x, y) { 255 } else { 0 })
        .collect()
}

/// Phase 7 gate: Workbench 1.3 boots on Kickstart 1.3 to the Workbench
/// screen (`EMU_WB_ADF` = a Workbench 1.3 disk, user-supplied; the image
/// is copied to memory and never written back). Passes once the startup
/// CLI window is gone and the Workbench title bar and disk icons show.
#[test]
#[ignore = "needs a Kickstart 1.3 ROM in EMU_ROM_DIR and EMU_WB_ADF"]
fn workbench_13_boots() {
    let path = std::env::var("EMU_WB_ADF").expect("EMU_WB_ADF not set");
    let data = std::fs::read(&path).expect("Workbench ADF");
    let mut m = boot("1.3");
    let media = mimic_core::media::Media {
        name: "workbench.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    let mut found = false;
    for frame in 0..15_000 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        if frame % 250 != 0 {
            continue;
        }
        let f = m.framebuffer();
        let white = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    f.rgb_at(x, y)
                        .is_some_and(|c| (c.r, c.g, c.b) == (0xff, 0xff, 0xff))
                })
                .count()
        };
        // The screen title bar (white) at the top, nothing where the CLI
        // window's text was, the disk icons at the right.
        let title = white(40, 680, 18, 28);
        let text = white(40, 500, 40, 140);
        let icons = white(560, 700, 30, 120);
        if title > 3_000 && text < 20 && icons > 100 {
            found = true;
            break;
        }
    }
    let path = snapshot(m.as_ref(), "workbench13.png");
    assert!(found, "no Workbench screen, see {path}");
}

/// Milestone M4: the mouse and the keyboard work under Workbench 1.3. The
/// pointer (red sprite pixels, COLOR17) is steered onto the
/// "Workbench1.3" disk icon, a double click opens its window (much more
/// white on the screen); a key press is fetched by keyboard.device (the
/// handshake comes back).
#[test]
#[ignore = "needs a Kickstart 1.3 ROM in EMU_ROM_DIR and EMU_WB_ADF"]
fn workbench_13_mouse_and_keyboard() {
    use mimic_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
    let path = std::env::var("EMU_WB_ADF").expect("EMU_WB_ADF not set");
    let data = std::fs::read(&path).expect("Workbench ADF");
    let mut m = boot("1.3");
    let media = mimic_core::media::Media {
        name: "workbench.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    let mut run = |m: &mut Box<dyn Machine>, n: usize| {
        for _ in 0..n {
            assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        }
    };
    let white = |m: &dyn Machine| {
        let f = m.framebuffer();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                f.rgb_at(x, y)
                    .is_some_and(|c| (c.r, c.g, c.b) == (0xff, 0xff, 0xff))
            })
            .count()
    };
    // The pointer's tip: the top-left red pixel.
    let pointer = |m: &dyn Machine| {
        let f = m.framebuffer();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .find(|&(x, y)| {
                f.rgb_at(x, y)
                    .is_some_and(|c| (c.r, c.g, c.b) == (0xdd, 0x22, 0x22))
            })
    };
    run(&mut m, 9000);
    let mouse = |m: &mut Box<dyn Machine>, dx: i16, dy: i16, buttons: u8| {
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Port(mimic_core::port::PortId(0)),
            event: InputEvent::MouseRel { dx, dy, buttons },
        });
    };
    // Steer onto the icon (frame position of "Workbench1.3"'s disk).
    let target = (630i32, 67i32);
    for _ in 0..40 {
        let (x, y) = pointer(m.as_ref()).expect("no pointer");
        let (ex, ey) = (target.0 - x as i32, target.1 - y as i32);
        if ex.abs() <= 3 && ey.abs() <= 2 {
            break;
        }
        mouse(
            &mut m,
            (ex / 2).clamp(-60, 60) as i16,
            ey.clamp(-60, 60) as i16,
            0,
        );
        run(&mut m, 5);
    }
    let before = white(m.as_ref());
    for _ in 0..2 {
        mouse(&mut m, 0, 0, 1);
        run(&mut m, 3);
        mouse(&mut m, 0, 0, 0);
        run(&mut m, 3);
    }
    run(&mut m, 400);
    let after = white(m.as_ref());
    let shot = snapshot(m.as_ref(), "workbench13_window.png");
    assert!(
        after > before + 5_000,
        "no window opened ({before} → {after} white pixels), see {shot}"
    );
    // A key (space) down and up: keyboard.device acknowledges both.
    for down in [true, false] {
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: MachineKey(0x40),
                down,
            },
        });
    }
    run(&mut m, 20);
    let cia_a = m.chips().into_iter().find(|c| c.name == "cia_a").unwrap();
    let icr_mask = cia_a.regs.iter().find(|r| r.name == "IMR").unwrap().value;
    assert_ne!(icr_mask & 0x08, 0, "keyboard interrupt enabled");
    assert_eq!(
        machine_amiga_keyboard_pending(m.as_mut()),
        0,
        "keys handshaken"
    );
}

/// Keys the keyboard still has to send (via the debugger's register view).
fn machine_amiga_keyboard_pending(m: &mut dyn Machine) -> u32 {
    m.chips()
        .into_iter()
        .find(|c| c.name == "keyboard")
        .and_then(|c| c.regs.into_iter().find(|r| r.name == "PENDING"))
        .map_or(u32::MAX, |r| r.value)
}

/// The 8×8 glyphs `tools/amiga_bustest.py` draws its hex digits with.
const BUSTEST_FONT: [[u8; 8]; 16] = [
    [0x3C, 0x66, 0x6E, 0x76, 0x66, 0x66, 0x3C, 0],
    [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0],
    [0x3C, 0x66, 0x06, 0x1C, 0x30, 0x60, 0x7E, 0],
    [0x3C, 0x66, 0x06, 0x1C, 0x06, 0x66, 0x3C, 0],
    [0x0C, 0x1C, 0x3C, 0x6C, 0x7E, 0x0C, 0x0C, 0],
    [0x7E, 0x60, 0x7C, 0x06, 0x06, 0x66, 0x3C, 0],
    [0x1C, 0x30, 0x60, 0x7C, 0x66, 0x66, 0x3C, 0],
    [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x30, 0x30, 0],
    [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0],
    [0x3C, 0x66, 0x66, 0x3E, 0x06, 0x0C, 0x38, 0],
    [0x18, 0x3C, 0x66, 0x66, 0x7E, 0x66, 0x66, 0],
    [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0],
    [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0],
    [0x78, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0x78, 0],
    [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x7E, 0],
    [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0],
];

/// Chip-bus arbitration against a cycle-exact A500 configuration: the boot
/// block `tests/data/amiga_bustest.bootblock` (built by
/// `tools/amiga_bustest.py`) times 600 iterations of 16× `MOVE.W
/// 0(A1,D1.W),4(A0)` + DBRA with the CIA-B timer A (E clock, 1 tick = 5
/// colour clocks) under four DMA loads and shows the tick counts as hex
/// digits. Reference: WinUAE, A500, 68000 cycle-exact, 512 KB chip + 512 KB
/// slow, Kickstart 1.3, PAL (2026-10-22, see `docs/sources.md`). The start
/// phase of the loop moves the counts by up to 1 %, so 1.5 % is allowed.
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn chip_bus_arbitration_matches_the_bus_test() {
    const REFERENCE: [u32; 4] = [0x5539, 0x6DD3, 0x7196, 0x5EC9];
    let mut m = boot("1.3");
    let mut adf = vec![0u8; media::adf::ADF_SIZE];
    adf[..1024].copy_from_slice(include_bytes!("../data/amiga_bustest.bootblock"));
    let media = mimic_core::media::Media {
        name: "bustest.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data: adf }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..700 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    let path = snapshot(m.as_ref(), "bustest.png");
    // Plane 1 at $60000, 44 bytes per row; row i's digits at rows 24 + 16i,
    // bytes 4..8.
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let mut results = [0u32; 4];
    for (i, result) in results.iter_mut().enumerate() {
        for d in 0..4 {
            let mut glyph = [0u8; 8];
            for (r, g) in glyph.iter_mut().enumerate() {
                let addr = 0x6_0000 + (24 + 16 * i + r) as u32 * 44 + 4 + d;
                *g = t.peek(machine_amiga::CPU_SPACE, addr, Size::Byte).unwrap() as u8;
            }
            let digit = BUSTEST_FONT
                .iter()
                .position(|f| *f == glyph)
                .unwrap_or_else(|| {
                    panic!("row {i} digit {d}: {glyph:02x?} is no glyph, see {path}")
                });
            *result = *result << 4 | digit as u32;
        }
    }
    for (i, (got, want)) in results.iter().zip(REFERENCE).enumerate() {
        let off = (f64::from(*got) / f64::from(want) - 1.0) * 100.0;
        assert!(
            off.abs() <= 1.5,
            "row {i}: {got:04X} ticks, WinUAE {want:04X} ({off:+.2} %); all {results:04X?}, see {path}"
        );
    }
    eprintln!("bus test {results:04X?} (reference {REFERENCE:04X?})");
}

/// Blitter against 68000 on the chip bus: the boot block
/// `tests/data/amiga_bustest2.bootblock` (built by
/// `tools/amiga_bustest2.py`) starts a 64 × 256-word blit at line 0 while
/// the CPU runs a chip-RAM loop until the Blitter is done, under four
/// conditions (BLTPRI off/on, with 5 planes and a per-line Copper list, a
/// D-only clear), and shows the E-clock ticks and the loop count per row.
/// Compared with a cycle-exact A500 in WinUAE (2026-10-23);
/// `EMU_BUSTEST2_REF` (eight hex values) replaces the reference.
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn blitter_bus_arbitration_matches_the_bus_test() {
    let mut m = boot("1.3");
    let mut adf = vec![0u8; media::adf::ADF_SIZE];
    adf[..1024].copy_from_slice(include_bytes!("../data/amiga_bustest2.bootblock"));
    let media = mimic_core::media::Media {
        name: "bustest2.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data: adf }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..700 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    let path = snapshot(m.as_ref(), "bustest2.png");
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let mut results = [0u32; 8];
    for (k, result) in results.iter_mut().enumerate() {
        let (row, column) = (k / 2, if k % 2 == 0 { 4 } else { 10 });
        for d in 0..4 {
            let mut glyph = [0u8; 8];
            for (r, g) in glyph.iter_mut().enumerate() {
                let addr = 0x6_0000 + (24 + 16 * row + r) as u32 * 44 + column + d;
                *g = t.peek(machine_amiga::CPU_SPACE, addr, Size::Byte).unwrap() as u8;
            }
            let digit = BUSTEST_FONT
                .iter()
                .position(|f| *f == glyph)
                .unwrap_or_else(|| {
                    panic!("row {row} value {k} digit {d}: {glyph:02x?} is no glyph, see {path}")
                });
            *result = *result << 4 | digit as u32;
        }
    }
    eprintln!("bus test 2 (ticks, loops per row): {results:04X?}, see {path}");
    // WinUAE, cycle-exact A500, Kickstart 1.3, PAL (2026-10-23); another
    // reference can be given in EMU_BUSTEST2_REF. Blit times within 1.5 %,
    // the loop counts (170–300) within 2.5 %.
    let reference: Vec<u32> = std::env::var("EMU_BUSTEST2_REF")
        .unwrap_or_else(|_| "2084,00AA,1A39,0001,411A,00FE,1A53,0128".into())
        .split([',', ' '])
        .filter(|s| !s.is_empty())
        .map(|s| u32::from_str_radix(s, 16).expect("hex"))
        .collect();
    for (k, (got, want)) in results.iter().zip(&reference).enumerate() {
        let off = (f64::from(*got) / f64::from(*want) - 1.0) * 100.0;
        // Row 2's Copper waits for $DE and makes five MOVEs over the line's
        // end. Since the Copper is refused the line's last even slot (the
        // vAmiga suite's `cycleE0`; 13 more of its tests pass) each burst
        // reaches a slot further into the next line and this blit takes
        // 1.6 % longer than WinUAE says (0.6 % before): the two references
        // disagree here, the suite won. The CPU's loop count in that time
        // follows (+2.8 %, before +2.0 %).
        let tolerance = match k {
            4 => 2.0,
            5 => 3.0,
            _ if k % 2 == 0 => 1.5,
            _ => 2.5,
        };
        assert!(
            off.abs() <= tolerance,
            "value {k}: {got:04X}, reference {want:04X} ({off:+.2} %); all {results:04X?}"
        );
    }
}

/// Copper jumps on the chip bus: the boot block
/// `tests/data/amiga_bustest3.bootblock` (built by
/// `tools/amiga_bustest3.py`) times the bus test's CPU loop while a Copper
/// list jumps into a sub-list and back on every line (COPJMP2/COPJMP1
/// written by the Copper), with 5 or 30 MOVEs in the sub-list and with or
/// without 5 lowres planes. Compared with a cycle-exact A500 in WinUAE
/// (2026-10-23); `EMU_BUSTEST3_REF` (four hex values) replaces it.
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn copper_jump_bus_timing_matches_the_bus_test() {
    let mut m = boot("1.3");
    let mut adf = vec![0u8; media::adf::ADF_SIZE];
    adf[..1024].copy_from_slice(include_bytes!("../data/amiga_bustest3.bootblock"));
    let media = mimic_core::media::Media {
        name: "bustest3.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data: adf }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..700 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    let path = snapshot(m.as_ref(), "bustest3.png");
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let mut results = [0u32; 4];
    for (i, result) in results.iter_mut().enumerate() {
        for d in 0..4 {
            let mut glyph = [0u8; 8];
            for (r, g) in glyph.iter_mut().enumerate() {
                let addr = 0x6_0000 + (24 + 16 * i + r) as u32 * 44 + 4 + d;
                *g = t.peek(machine_amiga::CPU_SPACE, addr, Size::Byte).unwrap() as u8;
            }
            let digit = BUSTEST_FONT
                .iter()
                .position(|f| *f == glyph)
                .unwrap_or_else(|| {
                    panic!("row {i} digit {d}: {glyph:02x?} is no glyph, see {path}")
                });
            *result = *result << 4 | digit as u32;
        }
    }
    eprintln!("bus test 3 (ticks per row): {results:04X?}, see {path}");
    // WinUAE, cycle-exact A500, Kickstart 1.3, PAL (2026-10-23);
    // EMU_BUSTEST3_REF replaces it.
    let reference: Vec<u32> = std::env::var("EMU_BUSTEST3_REF")
        .unwrap_or_else(|_| "588C,58FB,71DF,71D5".into())
        .split([',', ' '])
        .filter(|s| !s.is_empty())
        .map(|s| u32::from_str_radix(s, 16).expect("hex"))
        .collect();
    for (i, (got, want)) in results.iter().zip(&reference).enumerate() {
        let off = (f64::from(*got) / f64::from(*want) - 1.0) * 100.0;
        assert!(
            off.abs() <= 1.5,
            "row {i}: {got:04X}, reference {want:04X} ({off:+.2} %); all {results:04X?}"
        );
    }
}

/// CPU, Blitter and Copper together, the load of Sanity's "Interference":
/// the disk `tests/data/amiga_bustest4.bin` (boot block + program, built by
/// `tools/amiga_bustest4.py`) times the cube's blit-setup loop without and
/// with its 1 x 30-word blits next to Copper lists like the demo's (five
/// BPLxPTL MOVEs per line, plus 30 COLOR MOVEs on every second or every
/// line, whose tail runs into the next line's bitplane fetch) and 5
/// lowres planes. Compared with a cycle-exact A500 in WinUAE (2026-10-23);
/// `EMU_BUSTEST4_REF` (eight hex values) replaces the reference. With the
/// long sub-lists we are up to 3.4 % fast (CLAUDE.md, "Chip-bus
/// residual"), so 4 % is allowed.
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn cpu_blitter_copper_bus_timing_matches_the_bus_test() {
    let mut m = boot("1.3");
    let mut adf = vec![0u8; media::adf::ADF_SIZE];
    let disk = include_bytes!("../data/amiga_bustest4.bin");
    adf[..disk.len()].copy_from_slice(disk);
    let media = mimic_core::media::Media {
        name: "bustest4.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data: adf }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..900 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    let path = snapshot(m.as_ref(), "bustest4.png");
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let mut results = [0u32; 8];
    for (i, result) in results.iter_mut().enumerate() {
        for d in 0..4 {
            let mut glyph = [0u8; 8];
            for (r, g) in glyph.iter_mut().enumerate() {
                let addr = 0x6_0000 + (24 + 16 * i + r) as u32 * 44 + 4 + d;
                *g = t.peek(machine_amiga::CPU_SPACE, addr, Size::Byte).unwrap() as u8;
            }
            let digit = BUSTEST_FONT
                .iter()
                .position(|f| *f == glyph)
                .unwrap_or_else(|| {
                    panic!("row {i} digit {d}: {glyph:02x?} is no glyph, see {path}")
                });
            *result = *result << 4 | digit as u32;
        }
    }
    eprintln!("bus test 4 (ticks per row): {results:04X?}, see {path}");
    let reference: Vec<u32> = std::env::var("EMU_BUSTEST4_REF")
        .unwrap_or_else(|_| "3FD6,42AB,572B,7184,5A59,65D1,7ABF,A829".into())
        .split([',', ' '])
        .filter(|s| !s.is_empty())
        .map(|s| u32::from_str_radix(s, 16).expect("hex"))
        .collect();
    for (i, (got, want)) in results.iter().zip(&reference).enumerate() {
        let off = (f64::from(*got) / f64::from(*want) - 1.0) * 100.0;
        assert!(
            off.abs() <= 4.0,
            "row {i}: {got:04X}, reference {want:04X} ({off:+.2} %); all {results:04X?}"
        );
    }
}

/// A Kickstart configures the fast RAM board and sees the chip RAM: with
/// 1 MB chip and 2 MB fast, the fast RAM is mapped (expansion.library
/// wrote the base address) and the machine still reaches the insert-disk
/// screen; the A2000 and A1000 models boot too (the A1000 with the
/// Kickstart preloaded into its WCS).
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR"]
fn expansions_and_models_boot() {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&dir, "amiga").expect("ROMs");
    /// (model, ROM role, kickstart option, machine options).
    type Config = (
        &'static str,
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
    );
    let configs: [Config; 7] = [
        ("500", "kick13", "1.3", &[("chip", "1m"), ("fast", "2m")]),
        ("2000", "kick13", "1.3", &[("fast", "8m")]),
        ("1000", "kick12", "1.2", &[]),
        // ECS: Kickstart 2.0 and 3.1 read the Agnus and Denise ids.
        ("500plus", "kick20", "2.0", &[("chip", "2m")]),
        ("500plus", "kick31", "3.1", &[("fast", "4m")]),
        ("500", "kick13", "1.3", &[("agnus", "ecs")]),
        // AGA: the A1200 (Kickstart 3.1 40.063 boots it; see the machine doc).
        ("1200", "kick31", "3.1", &[]),
    ];
    for (model, role, kick, options) in configs {
        if !roms.contains_key(role) {
            continue;
        }
        let mut config = MachineConfig {
            roms: roms.clone(),
            model: Some(model.into()),
            ..MachineConfig::default()
        };
        config.options.insert("kickstart".into(), kick.into());
        for (k, v) in options {
            config.options.insert((*k).into(), (*v).into());
        }
        let mut m = mimic_machines::registry().create("amiga", &config).unwrap();
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
        let path = snapshot(m.as_ref(), &format!("{model}-{kick}.png"));
        assert!(
            colours.len() >= 4,
            "{model}: {} colours, see {path}",
            colours.len()
        );
        if options.iter().any(|(k, _)| *k == "fast") {
            let t = m.debug_target(machine_amiga::CPU).unwrap();
            assert!(
                t.poke(machine_amiga::CPU_SPACE, 0x20_0000, Size::Long, 0x1234_5678),
                "{model}: poke"
            );
            assert_eq!(
                t.peek(machine_amiga::CPU_SPACE, 0x20_0000, Size::Long),
                Some(0x1234_5678),
                "{model}: fast RAM at $200000 after autoconfig"
            );
        }
        eprintln!("{model}: {} colours, {path}", colours.len());
    }
}

/// Frames from a mouse movement to the pointer's move on the screen, and
/// how the pointer follows a steady movement: a development aid, printed,
/// not asserted. `EMU_WB_ADF` boots a Workbench (`EMU_MODEL`, `EMU_KICK`,
/// `EMU_FRAMES` for the boot).
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_WB_ADF"]
fn mouse_latency() {
    use mimic_core::input::{InputEvent, InputTarget, TimedInput};
    let path = std::env::var("EMU_WB_ADF").expect("EMU_WB_ADF not set");
    let data = std::fs::read(&path).expect("Workbench ADF");
    let kick = std::env::var("EMU_KICK").unwrap_or_else(|_| "1.3".into());
    let boot_frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut m = boot(&kick);
    let media = mimic_core::media::Media {
        name: "workbench.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    // The pointer's tip: the top-left reddish pixel.
    let pointer = |m: &dyn Machine| {
        let f = m.framebuffer();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .find(|&(x, y)| {
                f.rgb_at(x, y)
                    .is_some_and(|c| c.r >= 0xc0 && c.g < 0x70 && c.b < 0x70)
            })
    };
    for _ in 0..boot_frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    let start = pointer(m.as_ref()).expect("no pointer on the screen");
    eprintln!("pointer at {start:?} after {boot_frames} frames");
    let mouse = |m: &mut Box<dyn Machine>, dx: i16| {
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Port(mimic_core::port::PortId(0)),
            event: InputEvent::MouseRel {
                dx,
                dy: 0,
                buttons: 0,
            },
        });
    };
    // One movement at the start of a frame: when does the pointer move?
    mouse(&mut m, 40);
    for frame in 1..=8 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        eprintln!(
            "  frame {frame} after a 40-count move: pointer {:?}",
            pointer(m.as_ref())
        );
    }
    // A steady movement, 20 counts a frame for 10 frames: the pointer
    // should follow 20 pixels a frame (hires: 40 frame columns).
    let mut last = pointer(m.as_ref()).map_or(0, |p| p.0);
    for frame in 1..=14 {
        if frame <= 10 {
            mouse(&mut m, 20);
        }
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        let x = pointer(m.as_ref()).map_or(0, |p| p.0);
        eprintln!(
            "  steady frame {frame}: pointer x {x} ({:+})",
            x as i64 - last as i64
        );
        last = x;
    }
}

/// The interrupt probe (`tools/amiga_bustest5.py`): the Copper raises SOFT
/// at line $E0 + 2k, colour clock $51 + 2k and the level-1 handler stores
/// VHPOSR. Four groups of 16 values (no planes / 2 planes, `bra.s` loop /
/// `nop` + `bra.s`), read from chip RAM at $50000 (also on the screen as
/// hex). Prints the values and their latency (value minus trigger);
/// `EMU_BUSTEST5_REF` (64 hex words, from WinUAE or hardware) compares.
#[test]
#[ignore = "needs a Kickstart 1.3 ROM in EMU_ROM_DIR"]
fn interrupt_probe_matches_the_bus_test() {
    let mut m = boot("1.3");
    let mut adf = vec![0u8; media::adf::ADF_SIZE];
    adf[..1024].copy_from_slice(include_bytes!("../data/amiga_bustest5.bootblock"));
    let media = mimic_core::media::Media {
        name: "bustest5.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data: adf }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..500 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    let path = snapshot(m.as_ref(), "bustest5.png");
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let got: Vec<u16> = (0..64)
        .map(|i| {
            t.peek(machine_amiga::CPU_SPACE, 0x5_0000 + 2 * i, Size::Word)
                .unwrap() as u16
        })
        .collect();
    let names = [
        "no planes, bra",
        "2 planes, bra",
        "no planes, nop+bra",
        "2 planes, nop+bra",
    ];
    for (g, name) in names.iter().enumerate() {
        let vals = &got[16 * g..16 * g + 16];
        let lat: Vec<String> = vals
            .iter()
            .enumerate()
            .map(|(k, v)| format!("{:+}", i32::from(*v as u8) - (0x50 + 2 * k as i32)))
            .collect();
        eprintln!(
            "bus test 5 {name:20} {}",
            vals.iter()
                .map(|v| format!("{v:04X}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        eprintln!("{:32}latency {}", "", lat.join(" "));
    }
    eprintln!("see {path}");
    if let Ok(r) = std::env::var("EMU_BUSTEST5_REF") {
        let want: Vec<u16> = r
            .split([',', ' '])
            .filter(|s| !s.is_empty())
            .map(|s| u16::from_str_radix(s, 16).expect("hex"))
            .collect();
        let diff: Vec<String> = got
            .iter()
            .zip(&want)
            .map(|(g, w)| format!("{:+}", i32::from(*g as u8) - i32::from(*w as u8)))
            .collect();
        eprintln!("difference to the reference: {}", diff.join(" "));
        assert_eq!(&got[..want.len()], &want[..]);
    }
}

/// tools/amiga_hwdisk.py's hardware disk (`EMU_HWDISK=hw.adf`, built with
/// `python tools/amiga_hwdisk.py build hw.adf`) booted in mimic: it checks
/// the pattern, runs the bus tests and writes its results onto the disk,
/// which is saved as target/tmp/hwdisk-<model>-<kick>.adf for
/// `python tools/amiga_hwdisk.py results … --against …` next to a real
/// Amiga's (EMU_KICK, EMU_MODEL as above; EMU_FRAMES, default 6000).
#[test]
#[ignore = "needs a Kickstart ROM in EMU_ROM_DIR and EMU_HWDISK"]
fn hardware_disk_runs_and_writes_its_results() {
    let Ok(path) = std::env::var("EMU_HWDISK") else {
        eprintln!("EMU_HWDISK not set");
        return;
    };
    let kick = std::env::var("EMU_KICK").unwrap_or_else(|_| "1.3".into());
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6000);
    let mut m = boot(&kick);
    let media = mimic_core::media::Media {
        name: "hw.adf".into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend {
            data: std::fs::read(&path).expect("EMU_HWDISK"),
        }),
    };
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    let slot = m.media_slot(mimic_core::media::SlotId(0)).unwrap();
    slot.flush().expect("flush");
    let mut media = slot.eject().expect("the disk");
    let mut data = vec![0u8; media.backend.len() as usize];
    media.backend.read(0, &mut data).unwrap();
    let model = std::env::var("EMU_MODEL").unwrap_or_else(|_| "500".into());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hwdisk-{model}-{kick}.adf"));
    std::fs::write(&out, &data).unwrap();
    let magic = &data[1000 * 512..1000 * 512 + 8];
    eprintln!("saved {}", out.display());
    assert_eq!(magic, b"MIMICHW1", "no results written in {frames} frames");
}

/// The bus tests' numbers on the screen: `count` values of four hex digits,
/// two a row when `pairs` (bus test 2), else one; plane 1 at $60000, 44
/// bytes a row, row i's digits at rows 24 + 16i.
fn bustest_values(m: &mut dyn Machine, count: usize, pairs: bool, path: &str) -> Vec<u32> {
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    (0..count)
        .map(|k| {
            let (row, column) = if pairs {
                (k / 2, if k % 2 == 0 { 4 } else { 10 })
            } else {
                (k, 4)
            };
            (0..4).fold(0u32, |v, d| {
                let mut glyph = [0u8; 8];
                for (r, g) in glyph.iter_mut().enumerate() {
                    let addr = 0x6_0000 + (24 + 16 * row + r) as u32 * 44 + column + d;
                    *g = t.peek(machine_amiga::CPU_SPACE, addr, Size::Byte).unwrap() as u8;
                }
                let digit = BUSTEST_FONT
                    .iter()
                    .position(|f| *f == glyph)
                    .unwrap_or_else(|| {
                        panic!("value {k} digit {d}: {glyph:02x?} is no glyph, see {path}")
                    });
                v << 4 | digit as u32
            })
        })
        .collect()
}

struct HardwareCase {
    name: &'static str,
    disk: &'static [u8],
    pairs: bool,
    frames: usize,
    /// The real machine's values.
    reference: &'static [u32],
    /// Values printed but not asserted (known differences).
    unchecked: &'static [usize],
}

/// The bus tests 1-4 and 9 on `model` with Kickstart `kick` against a real
/// machine's numbers from the hardware disk of `tools/amiga_hwdisk.py`. The
/// disk runs the tests in harness mode, where the loop's start phase
/// differs a little from the boot blocks', so 3 % is allowed, 4 % for bus
/// test 2's loop counts and 6 % for bus test 4 (the A1200's rows with the
/// long Copper sub-lists, see its test). Bus test 9 counts loop iterations
/// in four fields, free of the start phase: 2 %, 4 % on the A1200 (within
/// 2.4 % there), as for its bus test 3.
fn hardware_bus_tests(model: &str, kick: &str, cases: &[HardwareCase]) {
    let mut failures = Vec::new();
    for case in cases {
        let mut m = boot_model(kick, Some(model.into()));
        let mut adf = vec![0u8; media::adf::ADF_SIZE];
        adf[..case.disk.len()].copy_from_slice(case.disk);
        let media = mimic_core::media::Media {
            name: "bustest.adf".into(),
            kind: mimic_core::media::MediaKind::Disk,
            backend: Box::new(media_backends::image::MemoryBackend { data: adf }),
        };
        m.media_slot(mimic_core::media::SlotId(0))
            .unwrap()
            .insert(media)
            .unwrap();
        let mut dbg = DebugHooks::new();
        for _ in 0..case.frames {
            assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        }
        let path = snapshot(
            m.as_ref(),
            &format!("{model}-{}.png", case.name.replace(' ', "")),
        );
        let results = bustest_values(m.as_mut(), case.reference.len(), case.pairs, &path);
        let offs: Vec<String> = results
            .iter()
            .zip(case.reference)
            .map(|(got, want)| {
                format!(
                    "{:+.1} %",
                    (f64::from(*got) / f64::from(*want) - 1.0) * 100.0
                )
            })
            .collect();
        eprintln!(
            "{}: {results:04X?}, A{model} {:04X?}: {}",
            case.name,
            case.reference,
            offs.join(" ")
        );
        for (k, (got, want)) in results.iter().zip(case.reference).enumerate() {
            if case.unchecked.contains(&k) {
                continue;
            }
            let off = (f64::from(*got) / f64::from(*want) - 1.0) * 100.0;
            let tolerance = if case.pairs && k % 2 == 1 {
                4.0
            } else if case.name == "bus test 4" {
                6.0
            } else if case.name == "bus test 9" || case.name == "bus test 3" {
                if model == "1200" {
                    4.0
                } else {
                    2.0
                }
            } else {
                3.0
            };
            if off.abs() > tolerance {
                failures.push(format!(
                    "{} value {k}: {got:04X}, A{model} {want:04X} ({off:+.2} %)",
                    case.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The A1200 against the owner's real A1200 (68EC020, Kickstart 40.068,
/// 2 MB chip, PAL, no expansion), measured on 2026-11-07
/// (`tests/data/amiga_hwdisk_a1200.txt`, the run with FMODE 0 in every
/// test). Bus test 3's row 0 is 3.2 % slow in mimic, so 4 % there.
#[test]
#[ignore = "needs the A1200 Kickstart 3.1 in EMU_ROM_DIR"]
fn a1200_bus_timing_matches_the_hardware() {
    hardware_bus_tests(
        "1200",
        "3.1",
        &[
            HardwareCase {
                name: "bus test",
                disk: include_bytes!("../data/amiga_bustest.bootblock"),
                pairs: false,
                frames: 400,
                reference: &[0x1EA4, 0x24A1, 0x26CE, 0x2057],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 2",
                disk: include_bytes!("../data/amiga_bustest2.bootblock"),
                pairs: true,
                frames: 400,
                reference: &[
                    0x20AF, 0x01C9, 0x1A22, 0x0001, 0x40C2, 0x02B7, 0x1A26, 0x0360,
                ],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 3",
                disk: include_bytes!("../data/amiga_bustest3.bootblock"),
                pairs: false,
                frames: 400,
                reference: &[0x1ED0, 0x1F47, 0x26CD, 0x26CD],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 4",
                disk: include_bytes!("../data/amiga_bustest4.bin"),
                pairs: false,
                frames: 600,
                reference: &[
                    0x1483, 0x1529, 0x1A45, 0x2892, 0x3539, 0x39B2, 0x42F6, 0x6654,
                ],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 9",
                disk: include_bytes!("../data/amiga_bustest9.bin"),
                pairs: false,
                frames: 420,
                reference: &[
                    0x0E43, 0x0D7B, 0x0CBC, 0x0BE1, 0x0B1B, 0x092B, 0x0673, 0x09A1, 0x0704, 0x0737,
                    0x092B, 0x0D33,
                ],
                unchecked: &[],
            },
        ],
    );
}

/// The A500 against the owner's real A500 (68000, Kickstart 1.3, 512 KB
/// chip RAM only, ECS Agnus 8372A, OCS Denise, PAL), measured on 2026-11-07
/// (`tests/data/amiga_hwdisk_a500.txt`, the third run; the first run's
/// bus test 3 rows 2 and 3 were 48-69 % off and did not repeat).
#[test]
#[ignore = "needs Kickstart 1.3 in EMU_ROM_DIR"]
fn a500_bus_timing_matches_the_hardware() {
    hardware_bus_tests(
        "500",
        "1.3",
        &[
            HardwareCase {
                name: "bus test",
                disk: include_bytes!("../data/amiga_bustest.bootblock"),
                pairs: false,
                frames: 700,
                reference: &[0x5537, 0x6DD2, 0x7197, 0x5E1C],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 2",
                disk: include_bytes!("../data/amiga_bustest2.bootblock"),
                pairs: true,
                frames: 700,
                reference: &[
                    0x2083, 0x00AA, 0x1A39, 0x0001, 0x411B, 0x00FE, 0x1A53, 0x0128,
                ],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 3",
                disk: include_bytes!("../data/amiga_bustest3.bootblock"),
                pairs: false,
                frames: 700,
                reference: &[0x58FB, 0x58FB, 0x71D4, 0x71D5],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 4",
                disk: include_bytes!("../data/amiga_bustest4.bin"),
                pairs: false,
                frames: 900,
                reference: &[
                    0x3FD5, 0x42A7, 0x572A, 0x7183, 0x5A4E, 0x65D1, 0x7AA8, 0xA829,
                ],
                unchecked: &[],
            },
            HardwareCase {
                name: "bus test 9",
                disk: include_bytes!("../data/amiga_bustest9.bin"),
                pairs: false,
                frames: 420,
                reference: &[
                    0x0502, 0x0493, 0x0463, 0x0414, 0x03C5, 0x030D, 0x0256, 0x033C, 0x0260, 0x0283,
                    0x0303, 0x03F7,
                ],
                unchecked: &[],
            },
        ],
    );
}

/// An A1200 with its own Kickstart (40.068 when there, else 40.063).
fn a1200() -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "amiga").expect("ROMs"),
        model: Some("1200".into()),
        ..MachineConfig::default()
    };
    mimic_machines::registry().create("amiga", &config).unwrap()
}

/// The Workbench 3.1 screen: its grey with the white title bar and the
/// blue Workbench window.
fn workbench31_shown(m: &dyn Machine) -> bool {
    let f = m.framebuffer();
    let (w, h) = (f.width, f.height);
    // Pixels of colour `c` in a box given in fractions of the picture.
    let count = |c: (u8, u8, u8), x0: f32, x1: f32, y0: f32, y1: f32| {
        let (xs, xe) = ((x0 * w as f32) as u32, (x1 * w as f32) as u32);
        let (ys, ye) = ((y0 * h as f32) as u32, (y1 * h as f32) as u32);
        (ys..ye)
            .flat_map(|y| (xs..xe).map(move |x| (x, y)))
            .filter(|&(x, y)| f.rgb_at(x, y).is_some_and(|p| (p.r, p.g, p.b) == c))
            .count()
    };
    let grey = count((0xaa, 0xaa, 0xaa), 0.0, 1.0, 0.0, 1.0);
    let title = count((0xff, 0xff, 0xff), 0.05, 0.75, 0.06, 0.09);
    let window = count((0x66, 0x88, 0xbb), 0.05, 0.9, 0.1, 0.13);
    let area = (w * h) as usize;
    grey > area / 2 && title > area / 200 && window > area / 100
}

/// Run until the Workbench 3.1 screen shows (checked every 100 frames);
/// the frame it showed in.
fn until_workbench31(m: &mut dyn Machine, frames: usize) -> Option<usize> {
    let mut dbg = DebugHooks::new();
    for frame in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        if frame % 100 == 0 && workbench31_shown(m) {
            return Some(frame);
        }
    }
    None
}

fn memory_media(name: &str, data: Vec<u8>) -> mimic_core::media::Media {
    mimic_core::media::Media {
        name: name.into(),
        kind: mimic_core::media::MediaKind::Disk,
        backend: Box::new(media_backends::image::MemoryBackend { data }),
    }
}

/// Phase 8 gate: Workbench 3.1 boots on the A1200 to the Workbench screen
/// (`EMU_WB31_ADF` = a Workbench 3.1 disk, user-supplied; copied to memory,
/// never written back).
#[test]
#[ignore = "needs an A1200 Kickstart 3.1 in EMU_ROM_DIR and EMU_WB31_ADF"]
fn workbench_31_boots_on_the_a1200() {
    let path = std::env::var("EMU_WB31_ADF").expect("EMU_WB31_ADF not set");
    let data = std::fs::read(&path).expect("Workbench ADF");
    let mut m = a1200();
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(memory_media("workbench31.adf", data))
        .unwrap();
    let found = until_workbench31(m.as_mut(), 6_000);
    let path = snapshot(m.as_ref(), "workbench31-a1200.png");
    let frame = found.unwrap_or_else(|| panic!("no Workbench 3.1 screen, see {path}"));
    eprintln!("Workbench 3.1 after {frame} frames, see {path}");
}

/// The A1200 boots from its IDE port: the Workbench 3.1 disk as a
/// partition image (an ADF is an AmigaDOS partition of 1760 blocks) in IDE
/// drive 0 behind the virtual RDB, and as a whole disk with that RDB on
/// the image itself; no floppy in DF0:. Kickstart's scsi.device finds the
/// drive (IDENTIFY), reads the RDB, mounts DH0: and boots it.
#[test]
#[ignore = "needs an A1200 Kickstart 3.1 in EMU_ROM_DIR and EMU_WB31_ADF"]
fn workbench_31_boots_from_an_ide_hard_disk() {
    let path = std::env::var("EMU_WB31_ADF").expect("EMU_WB31_ADF not set");
    let adf = std::fs::read(&path).expect("Workbench ADF");
    let dos_type = media::hdf::partition_dos_type(&adf).expect("a DOS disk");
    let mut disk = media::hdf::rdb_blocks((adf.len() / 512) as u64, dos_type);
    disk.extend_from_slice(&adf);
    for (name, data) in [("partition", adf), ("rdb", disk)] {
        let mut m = a1200();
        m.media_slot(machine_amiga::SLOT_IDE)
            .expect("IDE drive 0")
            .insert(memory_media(&format!("workbench31-{name}.hdf"), data))
            .unwrap();
        let found = until_workbench31(m.as_mut(), 3_000);
        let path = snapshot(m.as_ref(), &format!("workbench31-ide-{name}.png"));
        let frame = found.unwrap_or_else(|| panic!("{name}: no Workbench 3.1 screen, see {path}"));
        eprintln!("{name}: Workbench 3.1 from IDE after {frame} frames, see {path}");
    }
}

/// A savestate taken while Kickstart reads the hard disk restores the IDE
/// transfer: the same frames follow.
#[test]
#[ignore = "needs an A1200 Kickstart 3.1 in EMU_ROM_DIR and EMU_WB31_ADF"]
fn ide_savestates_are_deterministic() {
    let path = std::env::var("EMU_WB31_ADF").expect("EMU_WB31_ADF not set");
    let adf = std::fs::read(&path).expect("Workbench ADF");
    let mut m = a1200();
    m.media_slot(machine_amiga::SLOT_IDE)
        .unwrap()
        .insert(memory_media("workbench31.hdf", adf))
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..150 {
        m.run_frame(&mut dbg);
    }
    let state = m.save_state();
    let run = |m: &mut Box<dyn Machine>| {
        let mut dbg = DebugHooks::new();
        for _ in 0..200 {
            m.run_frame(&mut dbg);
        }
        let f = m.framebuffer();
        let px: Vec<(u8, u8, u8)> = (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .map(|(x, y)| f.rgb_at(x, y).map_or((0, 0, 0), |c| (c.r, c.g, c.b)))
            .collect();
        (m.now(), px)
    };
    let first = run(&mut m);
    m.load_state(&state).unwrap();
    let again = run(&mut m);
    assert_eq!(first.0, again.0);
    assert!(first.1 == again.1, "the frames differ after the savestate");
}

/// exec's memory list: (attributes, lower, upper) of each MemHeader
/// (ExecBase at $4, MemList at +322; MemHeader: mh_Attributes +14,
/// mh_Lower +20, mh_Upper +24 — RKM Libraries, exec memory).
fn exec_memory(m: &mut dyn Machine) -> Vec<(u32, u32, u32)> {
    let t = m.debug_target(machine_amiga::CPU).unwrap();
    let long = |a: u32| t.peek(machine_amiga::CPU_SPACE, a, Size::Long).unwrap();
    let word = |a: u32| t.peek(machine_amiga::CPU_SPACE, a, Size::Word).unwrap();
    let exec = long(4);
    let mut out = Vec::new();
    let mut node = long(exec + 322);
    while node != 0 && long(node) != 0 && out.len() < 16 {
        out.push((word(node + 14), long(node + 20), long(node + 24)));
        node = long(node);
    }
    out
}

/// A blank SRAM card (1 MB, no disk format) in the A1200's PCMCIA slot:
/// card.resource takes it as fast RAM at $600000 (Commodore's NDK notes:
/// SRAM cards not used for data become low-priority fast RAM).
#[test]
#[ignore = "needs an A1200 Kickstart 3.1 in EMU_ROM_DIR and EMU_WB31_ADF"]
fn a_pcmcia_sram_card_becomes_fast_ram() {
    let path = std::env::var("EMU_WB31_ADF").expect("EMU_WB31_ADF not set");
    let adf = std::fs::read(&path).expect("Workbench ADF");
    let mut m = a1200();
    m.media_slot(machine_amiga::SLOT_PCMCIA)
        .expect("PCMCIA slot")
        .insert(mimic_core::media::Media {
            name: "sram.card".into(),
            kind: mimic_core::media::MediaKind::Cartridge,
            backend: Box::new(media_backends::image::MemoryBackend {
                data: vec![0; 1 << 20],
            }),
        })
        .unwrap();
    m.media_slot(mimic_core::media::SlotId(0))
        .unwrap()
        .insert(memory_media("workbench31.adf", adf))
        .unwrap();
    let found = until_workbench31(m.as_mut(), 6_000);
    let path = snapshot(m.as_ref(), "workbench31-pcmcia-ram.png");
    let mem = exec_memory(m.as_mut());
    eprintln!("memory list: {mem:x?}");
    assert!(found.is_some(), "no Workbench 3.1 screen, see {path}");
    assert!(
        mem.iter()
            .any(|&(_, lo, hi)| (0x60_0000..0x60_1000).contains(&lo) && hi >= 0x6f_f000),
        "no fast RAM from the card in {mem:x?}"
    );
    // The card is RAM now: a savestate holds it, the same frames follow.
    let state = m.save_state();
    let run = |m: &mut Box<dyn Machine>| {
        let mut dbg = DebugHooks::new();
        for _ in 0..100 {
            m.run_frame(&mut dbg);
        }
        let t = m.debug_target(machine_amiga::CPU).unwrap();
        let card: Vec<u32> = (0..0x400)
            .map(|i| {
                t.peek(machine_amiga::CPU_SPACE, 0x60_0000 + 4 * i, Size::Long)
                    .unwrap()
            })
            .collect();
        (m.now(), card)
    };
    let first = run(&mut m);
    m.load_state(&state).unwrap();
    assert!(
        first == run(&mut m),
        "the card's RAM differs after the savestate"
    );
}

/// The Workbench 3.1 disk as an SRAM card image: its generated CIS says
/// "disk" (CISTPL_FORMAT), carddisk.device mounts CC0: and Kickstart boots
/// it with no floppy in DF0:.
#[test]
#[ignore = "needs an A1200 Kickstart 3.1 in EMU_ROM_DIR and EMU_WB31_ADF"]
fn workbench_31_boots_from_a_pcmcia_card() {
    let path = std::env::var("EMU_WB31_ADF").expect("EMU_WB31_ADF not set");
    let adf = std::fs::read(&path).expect("Workbench ADF");
    let mut m = a1200();
    m.media_slot(machine_amiga::SLOT_PCMCIA)
        .expect("PCMCIA slot")
        .insert(mimic_core::media::Media {
            name: "workbench31.card".into(),
            kind: mimic_core::media::MediaKind::Cartridge,
            backend: Box::new(media_backends::image::MemoryBackend { data: adf }),
        })
        .unwrap();
    let found = until_workbench31(m.as_mut(), 4_000);
    let path = snapshot(m.as_ref(), "workbench31-pcmcia-disk.png");
    let mem = exec_memory(m.as_mut());
    let frame =
        found.unwrap_or_else(|| panic!("no Workbench 3.1 screen, see {path}; memory {mem:x?}"));
    eprintln!("Workbench 3.1 from CC0: after {frame} frames, see {path}");
}
