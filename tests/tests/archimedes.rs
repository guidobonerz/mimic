//! Archimedes with real ROMs (`EMU_ROM_DIR`, ROMs in `archimedes/`).
//!
//!     EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test archimedes -- --ignored --nocapture
//!
//! `trace_boot`: EMU_ARC_OS=3.11, EMU_FRAMES=n: where the ROM spends its
//! time, a histogram of PCs per 64 KB and the last frame's chips.

use emu_core::input::{InputEvent, InputTarget, TimedInput};
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::port::PortId;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Machine, StopReason};
use machine_archimedes::Archimedes;

fn machine(os: &str) -> Option<Archimedes> {
    let dir = std::env::var("EMU_ROM_DIR").ok()?;
    let mut config = MachineConfig {
        roms: media_backends::roms::load(std::path::Path::new(&dir), "archimedes").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("os".into(), os.into());
    Some(Archimedes::new(&config).unwrap())
}

#[test]
#[ignore = "needs EMU_ROM_DIR"]
fn trace_boot() {
    let os = std::env::var("EMU_ARC_OS").unwrap_or_else(|_| "3.11".into());
    let Some(mut m) = machine(&os) else {
        return;
    };
    let frames: u32 = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let mut dbg = DebugHooks::new();
    let watch = std::env::var("EMU_ARC_WATCH")
        .ok()
        .and_then(|s| u32::from_str_radix(&s, 16).ok());
    if let Some(stop) = std::env::var("EMU_ARC_UNTIL")
        .ok()
        .and_then(|s| u32::from_str_radix(&s, 16).ok())
    {
        let mut ring = std::collections::VecDeque::new();
        for i in 0..400_000_000u64 {
            ring.push_back((m.cpu().next_pc(), m.cpu().r, m.cpu().psr));
            if ring.len() > 24 {
                ring.pop_front();
            }
            m.step_cycle();
            if m.cpu().next_pc() == stop {
                println!("step {i} clocks {}", m.clocks());
                use emu_core::{DebugTarget, MemSpaceId, Size};
                for a in [0x10u32, 0x2010, 0x20] {
                    println!(
                        "phys {a:x}: {:08x} logical: {:08x}",
                        DebugTarget::peek(&m, MemSpaceId(1), a, Size::Long).unwrap(),
                        DebugTarget::peek(&m, MemSpaceId(0), a, Size::Long).unwrap()
                    );
                }
                for (pc, r, psr) in &ring {
                    let text = emu_core::DebugTarget::disassemble(&m, *pc).to_string();
                    println!("{pc:07x} {text:32} r0 {:08x} r1 {:08x} r2 {:08x} r3 {:08x} r13 {:08x} r14 {:08x} psr {:08x}", r[0], r[1], r[2], r[3], r[13], r[14], psr);
                }
                break;
            }
        }
    }
    if std::env::var("EMU_ARC_CONTROL").is_ok() {
        let mut last = u32::MAX;
        for i in 0..900_000u64 {
            let before = m.cpu().next_pc();
            m.step_cycle();
            let now = m.chips()[0].regs[0].value;
            if now != last {
                println!("step {i} pc {before:07x}: MEMC control {now:x}");
                last = now;
            }
        }
    }
    if let Some(pc) = watch {
        let mut last = 0;
        for i in 0..200_000_000u64 {
            m.step_cycle();
            if m.cpu().next_pc() == pc
                && (std::env::var("EMU_ARC_LOW").is_err() || m.cpu().r[3] < 0x8000)
            {
                let c = m.cpu();
                println!(
                    "step {i} at {pc:07x} clocks {}: R {:08x?} r14 {:08x}",
                    m.clocks(),
                    &c.r[..14],
                    c.r[14]
                );
                last += 1;
                if last > 12 {
                    break;
                }
            }
        }
    }
    for f in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        if f % (frames / 12).max(1) == 0 {
            let cpu = m.cpu();
            println!(
                "frame {f:5} pc {:07x} mode {} r0 {:08x} r1 {:08x} r6 {:08x} r14 {:08x}",
                cpu.next_pc(),
                cpu.mode(),
                cpu.r[0],
                cpu.r[1],
                cpu.r[6],
                cpu.r[14]
            );
        }
    }
    for c in m.chips() {
        let regs: Vec<String> = c
            .regs
            .iter()
            .map(|r| format!("{}={:x}", r.name, r.value))
            .collect();
        println!("{}: {}", c.name, regs.join(" "));
    }
    let around = std::env::var("EMU_DIS")
        .ok()
        .and_then(|s| u32::from_str_radix(&s, 16).ok());
    let dis = (0..if around.is_some() { 120 } else { 12 }).map(|i| {
        let a = around
            .unwrap_or(m.cpu().next_pc().wrapping_sub(16))
            .wrapping_add(i * 4);
        format!("{a:07x} {}", emu_core::DebugTarget::disassemble(&m, a))
    });
    for line in dis {
        println!("{line}");
    }
}

/// Key by key, as the keyboard's matrix codes.
fn type_keys(m: &mut Archimedes, keys: &[u8]) {
    let mut dbg = DebugHooks::new();
    for &k in keys {
        m.key(k, true);
        for _ in 0..4 {
            m.run_frame(&mut dbg);
        }
        m.key(k, false);
        for _ in 0..4 {
            m.run_frame(&mut dbg);
        }
    }
}

/// Phase 24's gate: RISC OS 3.11 reaches the Supervisor prompt, `*DESKTOP`
/// starts the Desktop.
#[test]
#[ignore = "needs EMU_ROM_DIR"]
fn riscos_reaches_the_desktop() {
    let os = std::env::var("EMU_ARC_OS").unwrap_or_else(|_| "3.11".into());
    let Some(mut m) = machine(&os) else {
        return;
    };
    let mut dbg = DebugHooks::new();
    for _ in 0..1500 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    // D E S K T O P Return.
    type_keys(&mut m, &[0x3e, 0x29, 0x3d, 0x43, 0x2b, 0x2f, 0x30, 0x47]);
    for _ in 0..1000 {
        m.run_frame(&mut dbg);
    }
    assert!(lit(&m) > 20_000, "the Desktop's pinboard and icon bar");
}

/// The keyboard's matrix code of a character, and whether it takes Shift.
fn matrix(c: char) -> (u8, bool) {
    const ROWS: [(&str, u8, bool); 10] = [
        ("1234567890-", 0x11, false),
        ("!", 0x11, true),
        ("$", 0x14, true),
        ("QWERTYUIOP", 0x27, false),
        ("ASDFGHJKL", 0x3c, false),
        (":", 0x45, true),
        ("ZXCVBNM", 0x4e, false),
        ("./", 0x56, false),
        (" ", 0x5f, false),
        ("\r", 0x47, false),
    ];
    let c = c.to_ascii_uppercase();
    ROWS.iter()
        .find_map(|(keys, first, shift)| keys.find(c).map(|i| (first + i as u8, *shift)))
        .unwrap_or_else(|| panic!("no key for {c:?}"))
}

/// A line of text at the prompt, Return at its end.
fn type_line(m: &mut Archimedes, text: &str) {
    const SHIFT: u8 = 0x4c;
    let mut dbg = DebugHooks::new();
    for c in text.chars().chain(['\r']) {
        let (key, shift) = matrix(c);
        if shift {
            m.key(SHIFT, true);
            m.run_frame(&mut dbg);
        }
        type_keys(m, &[key]);
        if shift {
            m.key(SHIFT, false);
            m.run_frame(&mut dbg);
        }
    }
}

struct Image(Vec<u8>);

impl MediaBackend for Image {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let at = offset as usize;
        buf.copy_from_slice(&self.0[at..at + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// Phase 24's gate: a user's ADFS disc reads through the 1772
/// (EMU_ARC_ADF=disc.adf, optional EMU_ARC_ADF1=second.adf and
/// EMU_ARC_LINE=a command before `CAT`; the picture lands in target/tmp).
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_ARC_ADF"]
fn adfs_disc_loads() {
    let (Some(mut m), Ok(path)) = (machine("3.11"), std::env::var("EMU_ARC_ADF")) else {
        return;
    };
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            name: "disc.adf".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Image(std::fs::read(path).unwrap())),
        })
        .unwrap();
    if let Ok(path) = std::env::var("EMU_ARC_ADF1") {
        m.media_slot(SlotId(1))
            .unwrap()
            .insert(Media {
                name: "disc1.adf".into(),
                kind: MediaKind::Disk,
                backend: Box::new(Image(std::fs::read(path).unwrap())),
            })
            .unwrap();
    }
    let mut dbg = DebugHooks::new();
    for _ in 0..1500 {
        m.run_frame(&mut dbg);
    }
    let before = lit(&m);
    if let Ok(line) = std::env::var("EMU_ARC_LINE") {
        type_line(&mut m, &line);
    }
    type_line(&mut m, "CAT");
    for _ in 0..1000 {
        m.run_frame(&mut dbg);
    }
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("archimedes-adfs.png");
    std::fs::write(&out, emu_preview::encode_png(&m.framebuffer())).unwrap();
    // The six names and four header lines; "Drive empty" is a tenth of it.
    let listing = lit(&m) - before;
    assert!(
        listing > 2000,
        "{listing} new pixels, see {}",
        out.display()
    );
}

/// Starts a program from a user's disc in the Desktop: EMU_ARC_ADF=disc.adf,
/// optional EMU_ARC_ADF1=second.adf, EMU_ARC_RUN=command (an application:
/// `/ADFS::0.$.!App`), EMU_FRAMES=n; 4 MB of RAM. The picture lands in
/// target/tmp.
#[test]
#[ignore = "needs EMU_ROM_DIR, EMU_ARC_ADF and EMU_ARC_RUN"]
fn adfs_program_runs() {
    let (Ok(dir), Ok(path), Ok(run)) = (
        std::env::var("EMU_ROM_DIR"),
        std::env::var("EMU_ARC_ADF"),
        std::env::var("EMU_ARC_RUN"),
    ) else {
        return;
    };
    let mut config = MachineConfig {
        roms: media_backends::roms::load(std::path::Path::new(&dir), "archimedes").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("os".into(), "3.11".into());
    config.options.insert("ram".into(), "4m".into());
    let mut m = Archimedes::new(&config).unwrap();
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            name: "disc.adf".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Image(std::fs::read(path).unwrap())),
        })
        .unwrap();
    if let Ok(path) = std::env::var("EMU_ARC_ADF1") {
        m.media_slot(SlotId(1))
            .unwrap()
            .insert(Media {
                name: "disc1.adf".into(),
                kind: MediaKind::Disk,
                backend: Box::new(Image(std::fs::read(path).unwrap())),
            })
            .unwrap();
    }
    let mut dbg = DebugHooks::new();
    for _ in 0..1500 {
        m.run_frame(&mut dbg);
    }
    type_line(&mut m, &format!("DESKTOP {run}"));
    for _ in 0..number("EMU_FRAMES", 3000u32) {
        m.run_frame(&mut dbg);
    }
    // EMU_ARC_CLICKS=n: that many Selects, EMU_ARC_MOVE counts down the
    // screen before the first and 2500 frames after each.
    let clicks: u32 = number("EMU_ARC_CLICKS", 0);
    for click in 0..clicks {
        let dy = if click == 0 {
            number("EMU_ARC_MOVE", 0)
        } else {
            0
        };
        for (dy, buttons) in [(dy, 0), (0, 1), (0, 0)] {
            m.input(TimedInput {
                at: m.now(),
                target: InputTarget::Port(PortId(0)),
                event: InputEvent::MouseRel { dx: 0, dy, buttons },
            });
            for _ in 0..10 {
                m.run_frame(&mut dbg);
            }
        }
        for _ in 0..2500 {
            m.run_frame(&mut dbg);
        }
    }
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("archimedes-run.png");
    std::fs::write(&out, emu_preview::encode_png(&m.framebuffer())).unwrap();
    println!("{} pc {:07x}", out.display(), m.cpu().next_pc());
}

/// A number from the environment.
fn number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Light pixels in the picture.
fn lit(m: &Archimedes) -> usize {
    let f = m.framebuffer();
    (0..f.height)
        .flat_map(|y| (0..f.width).map(move |x| (x, y)))
        .filter(|&(x, y)| f.rgb_at(x, y).is_some_and(|c| c.r > 0x80))
        .count()
}

/// Every OS version comes up with text on the screen; with `model` the
/// memory sizes too.
#[test]
#[ignore = "needs EMU_ROM_DIR"]
fn every_rom_and_ram_size_boots() {
    let Ok(dir) = std::env::var("EMU_ROM_DIR") else {
        return;
    };
    for (os, model) in [
        ("3.11", "a310"),
        ("3.11", "a305"),
        ("3.11", "a440"),
        ("3.10", "a3000"),
        ("3.00", "a310"),
        ("2.00", "a310"),
        ("1.20", "a310"),
    ] {
        let mut config = MachineConfig {
            model: Some(model.into()),
            roms: media_backends::roms::load(std::path::Path::new(&dir), "archimedes").unwrap(),
            ..MachineConfig::default()
        };
        config.options.insert("os".into(), os.into());
        let mut m = Archimedes::new(&config).unwrap();
        let mut dbg = DebugHooks::new();
        for _ in 0..2000 {
            m.run_frame(&mut dbg);
        }
        println!(
            "{os} {model}: {} light pixels, pc {:07x}",
            lit(&m),
            m.cpu().next_pc()
        );
        assert!(lit(&m) > 100, "{os} {model}");
    }
}
