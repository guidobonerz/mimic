//! Runs test programs of the vAmiga test suite (vAmigaTS, GPL-3.0 test
//! programs with photographs of real Amigas; not in the repository) and
//! writes a snapshot per test for comparison with the photographs:
//!
//!     EMU_ROM_DIR=~/roms EMU_VAMIGATS_DIR=/path/to/Denise/Sprites cargo test -p emu-tests --release --test vamigats -- --ignored --nocapture
//!
//! `EMU_VAMIGATS_DIR` is searched for `*.adf` files (any depth). Each ADF
//! boots on an A500 with Kickstart 1.3, 512 KB chip and 512 KB slow RAM
//! (the suite's "A500_OCS_1MB") for `EMU_FRAMES` frames (default 450 ≈
//! 9 s, the suite's wait); the last frame goes to
//! `target/tmp/vamigats/<name>.png`. `EMU_FILTER=substring` limits the run.
//! With `EMU_CLXDAT=1` the test also prints CLXDAT as read by the program
//! (sampled every frame through the chip register view), which is what
//! the collision tests turn into colour bars.

use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, StopReason};

fn adfs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            adfs(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("adf")) {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "needs EMU_ROM_DIR (Kickstart 1.3) and EMU_VAMIGATS_DIR"]
fn snapshots_of_the_test_programs() {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&rom_dir, "amiga").expect("ROMs");
    let dir = std::env::var("EMU_VAMIGATS_DIR").expect("EMU_VAMIGATS_DIR");
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(450);
    let filter = std::env::var("EMU_FILTER").unwrap_or_default();
    let clxdat = std::env::var_os("EMU_CLXDAT").is_some();
    let mut list = Vec::new();
    adfs(std::path::Path::new(&dir), &mut list);
    list.sort();
    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vamigats");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut ran = 0;
    for adf in list {
        let name = adf.file_stem().unwrap().to_string_lossy().into_owned();
        if !name.contains(&filter) {
            continue;
        }
        let mut config = MachineConfig {
            roms: roms.clone(),
            ..MachineConfig::default()
        };
        config.options.insert("kickstart".into(), "1.3".into());
        let mut m = emu_machines::registry().create("amiga", &config).unwrap();
        let media = media_backends::image::open(&adf).expect("ADF");
        m.media_slot(emu_core::media::SlotId(0))
            .unwrap()
            .insert(media)
            .expect("insert");
        let mut dbg = DebugHooks::new();
        let mut seen = std::collections::BTreeMap::new();
        for _ in 0..frames {
            assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
            if clxdat {
                let v = m
                    .chips()
                    .into_iter()
                    .find(|c| c.name == "denise")
                    .and_then(|c| c.regs.into_iter().find(|r| r.name == "CLXDAT"))
                    .map(|r| r.value);
                if let Some(v) = v {
                    *seen.entry(v).or_insert(0) += 1;
                }
            }
        }
        let png = emu_preview::encode_png(&m.framebuffer());
        let path = out_dir.join(format!("{name}.png"));
        std::fs::write(&path, png).unwrap();
        let summary: Vec<String> = seen.iter().map(|(v, n)| format!("{v:04x}x{n}")).collect();
        println!("{name}: {} {}", path.display(), summary.join(" "));
        ran += 1;
    }
    assert!(ran > 0, "no ADF under {dir}");
}

/// One test of the suite: a `.retrosh` script next to its reference.
struct Case {
    script: std::path::PathBuf,
    name: String,
    adf: std::path::PathBuf,
    raw: std::path::PathBuf,
    frames: usize,
    options: Vec<(&'static str, String)>,
}

/// The vAmiga scripts `dir` holds for `setup` ("ocs", "ecs"): commands
/// mimic has no equivalent of leave a test out, it is counted as skipped.
fn cases(
    root: &std::path::Path,
    dir: &std::path::Path,
    setup: &str,
    out: &mut Vec<Case>,
    skipped: &mut usize,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            cases(root, &p, setup, out, skipped);
            continue;
        }
        if p.extension().is_none_or(|x| x != "retrosh") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let raw = p.with_extension("raw");
        let adf = std::fs::read_dir(dir).ok().and_then(|d| {
            d.flatten()
                .map(|e| e.path())
                .find(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("adf")))
        });
        let wanted = match setup {
            "ocs" => "A500_OCS_1MB",
            "ecs" => "A500_ECS_1MB",
            other => panic!("unknown setup {other}"),
        };
        let mut matches = false;
        let mut supported = true;
        let mut frames = 450;
        let mut options = Vec::new();
        for line in text.lines().map(str::trim) {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                [] => {}
                [c, ..] if c.starts_with('#') => {}
                ["regression", "setup", s, ..] => matches = *s == wanted,
                ["regression", "run", ..] | ["screenshot", "save", ..] => {}
                ["wait", n, "seconds"] => frames = n.parse::<usize>().unwrap_or(9) * 50,
                ["mem", "set", "SLOW_RAM", n] => options.push(("slow", {
                    if *n == "0" {
                        "0".to_string()
                    } else {
                        format!("{n}k")
                    }
                })),
                // Collision enable bits for sprites and playfields, on in OCS.
                ["denise", "set", _, "true"] => {}
                _ => supported = false,
            }
        }
        if !matches {
            continue;
        }
        let (Some(adf), true, true) = (adf, supported, raw.exists()) else {
            *skipped += 1;
            continue;
        };
        let rel = p.strip_prefix(root).unwrap_or(&p);
        let name = rel.with_extension("").to_string_lossy().replace('\\', "/");
        out.push(Case {
            script: p.clone(),
            name,
            adf,
            raw,
            frames,
            options,
        });
    }
}

/// Where the suite's 716 × 285 picture lies in our frame.
const VAMIGA_SIZE: (usize, usize) = (716, 285);
/// The suite's picture starts this many columns left of our frame, which
/// in turn reaches further right: only their common part is compared.
const VAMIGA_LEFT: usize = 28;
const COMMON_WIDTH: usize = VAMIGA_SIZE.0 - VAMIGA_LEFT;

/// Differing pixels between `frame` from (`ox`, `oy`) on and `raw`. vAmiga
/// saves its pictures through a colour adjustment of its own (pure red
/// $FF0000 comes out as $EC0000), so colours are not compared by value:
/// each of our colours must stand for one reference colour throughout, and
/// no two for the same one; pixels off that best one-to-one match differ.
fn difference(
    frame: &[emu_core::frame::Rgb8],
    width: usize,
    raw: &[u8],
    ox: usize,
    oy: usize,
) -> usize {
    use std::collections::BTreeMap;
    let mut pairs: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let total = COMMON_WIDTH * VAMIGA_SIZE.1;
    for y in 0..VAMIGA_SIZE.1 {
        for x in 0..COMMON_WIDTH {
            let Some(p) = frame.get((y + oy) * width + x + ox) else {
                continue;
            };
            let r = &raw[(y * VAMIGA_SIZE.0 + x + VAMIGA_LEFT) * 3..][..3];
            let ours = u32::from(p.r) << 16 | u32::from(p.g) << 8 | u32::from(p.b);
            let theirs = u32::from(r[0]) << 16 | u32::from(r[1]) << 8 | u32::from(r[2]);
            *pairs.entry((ours, theirs)).or_default() += 1;
        }
    }
    let mut ranked: Vec<_> = pairs.into_iter().collect();
    ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let (mut ours, mut theirs) = (Vec::new(), Vec::new());
    let mut good = 0;
    for ((a, b), n) in ranked {
        if !ours.contains(&a) && !theirs.contains(&b) {
            ours.push(a);
            theirs.push(b);
            good += n;
        }
    }
    total - good
}

/// The vAmiga suite against its reference pictures, grouped by chip. They
/// come from vAmiga itself, which the suite's photographs of real Amigas
/// keep honest: a pass is agreement with vAmiga, byte for byte.
///
///     EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test vamigats vamigats_suite -- --ignored --nocapture
///
/// `EMU_VAMIGATS_ROOT` (default `testdata/vamigats`), `EMU_VAMIGATS_SETUP`
/// (ocs, ecs), `EMU_FILTER`, `EMU_VAMIGATS_DUMP=1` saves failed pictures,
/// `EMU_VAMIGATS_XY=x,y` the picture's place in the frame (default: 0,0),
/// `EMU_VAMIGATS_EXTRA=n` runs n frames more.
#[test]
#[ignore = "needs EMU_ROM_DIR (Kickstart 1.3) and the vAmigaTS data"]
fn vamigats_suite() {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&rom_dir, "amiga").expect("ROMs");
    let root = std::env::var("EMU_VAMIGATS_ROOT").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/vamigats").to_string()
    });
    let root = std::path::PathBuf::from(root);
    let setup = std::env::var("EMU_VAMIGATS_SETUP").unwrap_or_else(|_| "ocs".into());
    let filter = std::env::var("EMU_FILTER").unwrap_or_default();
    let dump = std::env::var_os("EMU_VAMIGATS_DUMP").is_some();
    let place: Option<(usize, usize)> = std::env::var("EMU_VAMIGATS_XY").ok().and_then(|v| {
        let (x, y) = v.split_once(',')?;
        Some((x.parse().ok()?, y.parse().ok()?))
    });
    let (mut list, mut skipped) = (Vec::new(), 0);
    for group in [
        "Agnus",
        "Denise",
        "Paula",
        "CIA",
        "Mainboard",
        "Memory",
        "Misc",
        "CPU",
    ] {
        cases(&root, &root.join(group), &setup, &mut list, &mut skipped);
    }
    list.retain(|c| c.name.contains(&filter));
    assert!(!list.is_empty(), "no tests under {}", root.display());
    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let pictures = out_dir.join("vamigats-fail");
    std::fs::create_dir_all(&pictures).unwrap();

    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let Some(case) = list.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    return;
                };
                let mut config = MachineConfig {
                    roms: roms.clone(),
                    ..MachineConfig::default()
                };
                config.options.insert("kickstart".into(), "1.3".into());
                config.options.insert("slow".into(), "512k".into());
                if setup == "ecs" {
                    config.options.insert("agnus".into(), "ecs".into());
                }
                for (k, v) in &case.options {
                    config.options.insert((*k).to_string(), v.clone());
                }
                let raw = std::fs::read(&case.raw).unwrap_or_default();
                let outcome = run_case(case, &config, &raw, place);
                if dump && outcome.is_some_and(|(bad, _)| bad > 0) {
                    let name = case.name.replace('/', "_");
                    write_failure(&pictures.join(&name), &config, case, &raw);
                }
                results.lock().unwrap().push((case.name.clone(), outcome));
            });
        }
    });

    let mut results = results.into_inner().unwrap();
    results.sort();
    let mut groups: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut report = String::new();
    let mut trend = Vec::new();
    for (name, outcome) in &results {
        let pass = outcome.is_some_and(|(bad, _)| bad == 0);
        let mut parts = name.split('/');
        let top = parts.next().unwrap_or("");
        let key = match (top, parts.next()) {
            ("Agnus", Some(sub)) => format!("Agnus/{sub}"),
            ("Denise", Some(sub)) => format!("Denise/{sub}"),
            _ => top.to_string(),
        };
        let g = groups.entry(key).or_default();
        g.0 += usize::from(pass);
        g.1 += 1;
        trend.push((name.clone(), pass));
        report += &match outcome {
            Some((0, _)) => format!("PASS {name}\n"),
            Some((bad, total)) => format!(
                "FAIL {name}: {bad} pixels ({:.2} %)\n",
                *bad as f64 * 100.0 / *total as f64
            ),
            None => format!("ERROR {name}\n"),
        };
    }
    std::fs::write(out_dir.join(format!("vamigats-{setup}.txt")), report).unwrap();
    let passed = trend.iter().filter(|(_, p)| *p).count();
    for (group, (p, n)) in &groups {
        println!(
            "{group:28} {p:4} / {n:4}  {:5.1} %",
            *p as f64 * 100.0 / *n as f64
        );
    }
    println!(
        "vAmigaTS {setup}: {passed}/{} passed ({:.1} %), {skipped} skipped",
        trend.len(),
        passed as f64 * 100.0 / trend.len() as f64
    );
    println!(
        "{}",
        emu_tests::trend::record(out_dir, &format!("vamigats-{setup}"), &trend)
    );
}

/// Run a test; (differing pixels, pixels of the picture) at the best place.
fn run_case(
    case: &Case,
    config: &MachineConfig,
    raw: &[u8],
    place: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    if raw.len() != VAMIGA_SIZE.0 * VAMIGA_SIZE.1 * 3 {
        return None;
    }
    let (m, _) = boot(case, config)?;
    let frame = m.framebuffer();
    let emu_core::frame::PixelData::Rgb(px) = frame.pixels else {
        return None;
    };
    let width = frame.width as usize;
    let (x, y) = place.unwrap_or((0, 0));
    let bad = difference(px, width, raw, x, y);
    Some((bad, COMMON_WIDTH * VAMIGA_SIZE.1))
}

fn boot(case: &Case, config: &MachineConfig) -> Option<(Box<dyn emu_core::Machine>, ())> {
    let mut m = emu_machines::registry().create("amiga", config).ok()?;
    let media = media_backends::image::open(&case.adf).ok()?;
    m.media_slot(emu_core::media::SlotId(0))?
        .insert(media)
        .ok()?;
    let mut dbg = DebugHooks::new();
    let extra: usize = std::env::var("EMU_VAMIGATS_EXTRA")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for _ in 0..case.frames + extra {
        if m.run_frame(&mut dbg) != StopReason::FrameDone {
            return None;
        }
    }
    Some((m, ()))
}

/// Our picture and the reference next to each other, for a look.
fn write_failure(base: &std::path::Path, config: &MachineConfig, case: &Case, raw: &[u8]) {
    let Some((m, _)) = boot(case, config) else {
        return;
    };
    std::fs::write(
        base.with_extension("png"),
        emu_preview::encode_png(&m.framebuffer()),
    )
    .ok();
    std::fs::write(base.with_extension("ref.rgb"), raw).ok();
    let _ = &case.script;
}

/// Development aid: the VPOS probe tests' measured values against their
/// expected ones, read from chip RAM instead of the picture (the test
/// program keeps both tables; `lea values,a2` and `lea expected,a5` give
/// their addresses).
///
///     EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test vamigats vpos_probe_values -- --ignored --nocapture
///
/// `EMU_FILTER` picks the tests (default: all `probe*` and `vprobe*`).
#[test]
#[ignore = "needs EMU_ROM_DIR (Kickstart 1.3) and the vAmigaTS data"]
fn vpos_probe_values() {
    use emu_core::Size;
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&rom_dir, "amiga").expect("ROMs");
    let root = std::env::var("EMU_VAMIGATS_ROOT").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/vamigats").to_string()
    });
    let dir = std::path::PathBuf::from(root).join("Agnus/Registers/VPOS");
    let filter = std::env::var("EMU_FILTER").unwrap_or_default();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| (n.starts_with("probe") || n.starts_with("vprobe")) && n.contains(&filter))
        .collect();
    names.sort();
    for name in names {
        let adf = dir.join(&name).join(format!("{name}.adf"));
        let case = Case {
            script: adf.clone(),
            name: name.clone(),
            adf,
            raw: dir.clone(),
            frames: 450,
            options: Vec::new(),
        };
        let mut config = MachineConfig {
            roms: roms.clone(),
            ..MachineConfig::default()
        };
        config.options.insert("kickstart".into(), "1.3".into());
        config.options.insert("slow".into(), "512k".into());
        let Some((mut m, _)) = boot(&case, &config) else {
            println!("{name}: did not boot");
            continue;
        };
        let t = m.debug_target(machine_amiga::CPU).unwrap();
        let word = |t: &mut dyn emu_core::DebugTarget, a: u32| {
            t.peek(machine_amiga::CPU_SPACE, a, Size::Word).unwrap_or(0) as u16
        };
        // The display routine: `lea values,a2`, a few words, then `lea
        // expected,a5` (absolute short $x8, long $x9 or PC-relative $xA;
        // the pair tells them from Kickstart's own `lea`s in RAM).
        let lea = |t: &mut dyn emu_core::DebugTarget, a: u32, reg: u16| -> Option<u32> {
            let op = word(t, a);
            if op & 0xfff8 != 0x41f8 | reg << 9 {
                return None;
            }
            let addr = match op & 7 {
                0 => word(t, a + 2) as i16 as i32 as u32 & 0xff_ffff,
                1 => u32::from(word(t, a + 2)) << 16 | u32::from(word(t, a + 4)),
                2 => (a + 2).wrapping_add(word(t, a + 2) as i16 as i32 as u32),
                _ => return None,
            };
            (addr < 0x8_0000).then_some(addr)
        };
        let (mut values, mut expected) = (None, None);
        let mut a = 0;
        while a < 0x8_0000 && values.is_none() {
            if let Some(v) = lea(t, a, 2) {
                for b in (a + 2..a + 24).step_by(2) {
                    if let Some(e) = lea(t, b, 5) {
                        values = Some(v);
                        expected = Some(e);
                        break;
                    }
                }
            }
            a += 2;
        }
        let (Some(v), Some(e)) = (values, expected) else {
            println!("{name}: tables not found");
            continue;
        };
        let got: Vec<u16> = (0..16).map(|i| word(t, v + 2 * i)).collect();
        let want: Vec<u16> = (0..16).map(|i| word(t, e + 2 * i)).collect();
        let diffs: Vec<String> = got
            .iter()
            .zip(&want)
            .map(|(g, w)| {
                let d = i32::from(*g as u8) - i32::from(*w as u8);
                // Horizontal positions wrap at $E3 (227 colour clocks).
                let d = if d > 113 {
                    d - 227
                } else if d < -113 {
                    d + 227
                } else {
                    d
                };
                format!("{d:+}")
            })
            .collect();
        let ok = got == want;
        println!(
            "{name:8} {}  got  {}",
            if ok { "PASS" } else { "FAIL" },
            got.iter()
                .map(|w| format!("{w:04X}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!(
            "{:14}want  {}",
            "",
            want.iter()
                .map(|w| format!("{w:04X}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!("{:14}diff  {}", "", diffs.join(" "));
    }
}
