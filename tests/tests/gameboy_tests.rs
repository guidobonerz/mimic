//! Game Boy test ROMs (Phase 40): Shay Green's ("blargg") test programs,
//! Matt Currie's dmg-acid2 (MIT) and Joonas Javanainen's mooneye test
//! suite (MIT), fetched into `$EMU_TESTDATA_DIR/gb` by
//! `tools/fetch-testdata.sh`, never committed.
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p mimic-tests --release --test gameboy_tests -- --ignored --nocapture
//!
//! blargg's programs print their verdict through the serial port (the
//! older ones) and leave it in the cartridge's RAM (the newer ones: the
//! signature `DE B0 61` at `$A001`, the status at `$A000`, `$80` while
//! running, the text from `$A004`). dmg-acid2 draws a face that is right
//! only if the picture unit is; its picture is compared with the
//! reference picture of its repository, shade for shade. A mooneye test
//! ends in `LD B,B` and a loop with Fibonacci numbers in the registers
//! when it passed; all of `acceptance` and `emulator-only` that hold for
//! the model are run (the name's ending tells: `-dmgABC`, `-mgb`, `-GS`
//! …), for the DMG and the Pocket.

use std::path::PathBuf;

use machine_gameboy::{peek_cpu, GameBoy, SLOT_CARTRIDGE};
use mimic_core::frame::PixelData;
use mimic_core::media::{Media, MediaKind};
use mimic_core::registry::MachineConfig;
use mimic_core::{DebugHooks, Machine, StopReason};

fn testdata() -> PathBuf {
    PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR")).join("gb")
}

fn machine(file: &str) -> GameBoy {
    let rom = std::fs::read(testdata().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    let mut m = GameBoy::new(&MachineConfig::default()).unwrap();
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            name: file.into(),
            kind: MediaKind::Cartridge,
            backend: Box::new(media_backends::image::MemoryBackend { data: rom }),
        })
        .unwrap();
    m
}

/// Run a blargg test to its verdict: (passed, its text).
fn blargg(file: &str, frames: u32) -> (bool, String) {
    let mut m = machine(file);
    let mut dbg = DebugHooks::new();
    let mut serial = Vec::new();
    for _ in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone, "{file}");
        serial.extend(m.take_serial());
        let text = String::from_utf8_lossy(&serial).to_string();
        if text.contains("Passed") || text.contains("Failed") {
            return (text.contains("Passed"), text);
        }
        let signature = [0xa001, 0xa002, 0xa003].map(|a| peek_cpu(&m, a));
        let status = peek_cpu(&m, 0xa000);
        if signature == [0xde, 0xb0, 0x61] && status != 0x80 {
            let text: String = (0xa004..0xa200)
                .map(|a| peek_cpu(&m, a))
                .take_while(|b| *b != 0)
                .map(char::from)
                .collect();
            return (status == 0, text);
        }
    }
    // Some print on the screen only: the background map holds the
    // characters' codes.
    let screen: String = (0x9800..0x9c00)
        .map(|a| peek_cpu(&m, a))
        .map(|b| {
            if (0x20..0x7f).contains(&b) {
                char::from(b)
            } else {
                ' '
            }
        })
        .collect();
    if screen.contains("Passed") || screen.contains("Failed") {
        return (screen.contains("Passed"), screen);
    }
    (
        false,
        format!("no verdict; serial: {}", String::from_utf8_lossy(&serial)),
    )
}

/// Every listed test must pass; the table is printed first.
fn suite(tests: &[(&str, u32)]) {
    let mut failed = Vec::new();
    for (file, frames) in tests {
        let (passed, text) = blargg(file, *frames);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        println!("{} {file}: {text}", if passed { "ok  " } else { "FAIL" });
        if !passed {
            failed.push(*file);
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn blargg_cpu_instrs() {
    let names = [
        "01-special",
        "02-interrupts",
        "03-op sp,hl",
        "04-op r,imm",
        "05-op rp",
        "06-ld r,r",
        "07-jr,jp,call,ret,rst",
        "08-misc instrs",
        "09-op r,r",
        "10-bit ops",
        "11-op a,(hl)",
    ];
    let files: Vec<String> = names
        .iter()
        .map(|n| format!("blargg/cpu_instrs/individual/{n}.gb"))
        .collect();
    let tests: Vec<(&str, u32)> = files.iter().map(|f| (f.as_str(), 3000)).collect();
    suite(&tests);
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn blargg_timing() {
    suite(&[
        ("blargg/instr_timing/instr_timing.gb", 600),
        ("blargg/mem_timing/individual/01-read_timing.gb", 600),
        ("blargg/mem_timing/individual/02-write_timing.gb", 600),
        ("blargg/mem_timing/individual/03-modify_timing.gb", 600),
        ("blargg/mem_timing-2/rom_singles/01-read_timing.gb", 600),
        ("blargg/mem_timing-2/rom_singles/02-write_timing.gb", 600),
        ("blargg/mem_timing-2/rom_singles/03-modify_timing.gb", 600),
        ("blargg/halt_bug.gb", 600),
    ]);
}

/// The sound unit. Three of the twelve are known to fail: they test what
/// the DMG's wave RAM shows and takes while its channel plays (only in
/// the two clocks the channel reads it) and what a trigger while playing
/// does to it, which the sound unit here does not model.
#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn blargg_dmg_sound() {
    let pass = [
        "01-registers",
        "02-len ctr",
        "03-trigger",
        "04-sweep",
        "05-sweep details",
        "06-overflow on trigger",
        "07-len sweep period sync",
        "08-len ctr during power",
        "11-regs after power",
    ];
    let known = [
        "09-wave read while on",
        "10-wave trigger while on",
        "12-wave write while on",
    ];
    let file = |n: &str| format!("blargg/dmg_sound/rom_singles/{n}.gb");
    let files: Vec<String> = pass.iter().map(|n| file(n)).collect();
    let tests: Vec<(&str, u32)> = files.iter().map(|f| (f.as_str(), 1200)).collect();
    suite(&tests);
    for n in known {
        let (passed, _) = blargg(&file(n), 1200);
        println!(
            "{} {n} (known to fail)",
            if passed { "ok  " } else { "FAIL" }
        );
    }
}

/// The object table's corruption bug is not modelled: of blargg's
/// `oam_bug` the tests pass that need the display's timing only, the
/// others are known to fail and only reported.
#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn blargg_oam_bug() {
    let pass = ["1-lcd_sync", "3-non_causes", "6-timing_no_bug"];
    let known = [
        "2-causes",
        "4-scanline_timing",
        "5-timing_bug",
        "7-timing_effect",
        "8-instr_effect",
    ];
    let file = |n: &str| format!("blargg/oam_bug/rom_singles/{n}.gb");
    let files: Vec<String> = pass.iter().map(|n| file(n)).collect();
    let tests: Vec<(&str, u32)> = files.iter().map(|f| (f.as_str(), 1200)).collect();
    suite(&tests);
    for n in known {
        let (passed, _) = blargg(&file(n), 1200);
        println!(
            "{} {n} (known to fail)",
            if passed { "ok  " } else { "FAIL" }
        );
    }
}

/// The reference picture's four greys as shades 0 (white) to 3.
fn reference() -> Vec<u8> {
    let file = std::fs::File::open(testdata().join("dmg-acid2-reference.png")).unwrap();
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (160, 144));
    let step = info.color_type.samples();
    buf[..info.buffer_size()]
        .chunks(step)
        .map(|p| 3 - p[0] / 0x55)
        .collect()
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn dmg_acid2_matches_its_reference() {
    let mut m = machine("dmg-acid2.gb");
    let mut dbg = DebugHooks::new();
    for _ in 0..60 {
        m.run_frame(&mut dbg);
    }
    let frame = m.framebuffer();
    let PixelData::Indexed { pixels, .. } = frame.pixels else {
        panic!("indexed");
    };
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("dmg-acid2.png");
    std::fs::write(&out, mimic_preview::encode_png(&m.framebuffer())).unwrap();
    let want = reference();
    let wrong = pixels.iter().zip(&want).filter(|(a, b)| a != b).count();
    let first = pixels.iter().zip(&want).position(|(a, b)| a != b);
    assert_eq!(
        wrong,
        0,
        "{wrong} pixels differ, the first at {:?}; picture: {}",
        first.map(|i| (i % 160, i / 160)),
        out.display()
    );
}

/// Does a mooneye test apply to the model? Its name ends in the models
/// it holds on: none (all), `dmgABC`, `mgb`, `dmg0`, `sgb`, `cgb` … or
/// the letters G (DMG and Pocket), S (Super Game Boy), C (Color), A
/// (Advance).
fn mooneye_applies(name: &str, model: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    let Some((_, models)) = file.rsplit_once('-') else {
        return true;
    };
    if models.chars().all(|c| "GSCA".contains(c)) {
        return models.contains('G');
    }
    match model {
        "pocket" => models.contains("mgb"),
        _ => models.contains("dmgABC"),
    }
}

/// Run a mooneye test: at its end it executes `LD B,B` and stays in a
/// `JR` to itself, with 3, 5, 8, 13, 21, 34 in B, C, D, E, H, L when it
/// passed and `$42` in all six when it failed.
fn mooneye(path: &std::path::Path, model: &str) -> Option<bool> {
    let rom = std::fs::read(path).unwrap();
    let config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    let mut m = GameBoy::new(&config).unwrap();
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            name: path.display().to_string(),
            kind: MediaKind::Cartridge,
            backend: Box::new(media_backends::image::MemoryBackend { data: rom }),
        })
        .unwrap();
    let mut dbg = DebugHooks::new();
    let cpu = m.devices()[0];
    for _ in 0..7200 {
        m.run_frame(&mut dbg);
        let regs = m.debug_target(cpu).unwrap().regs();
        let pc = regs.get("PC").unwrap() as u16;
        let code = [pc.wrapping_sub(1), pc, pc.wrapping_add(1)].map(|a| peek_cpu(&m, a));
        if code == [0x40, 0x18, 0xfe] {
            let got = ["B", "C", "D", "E", "H", "L"].map(|r| regs.get(r).unwrap());
            return Some(got == [3, 5, 8, 13, 21, 34]);
        }
    }
    None
}

/// The mooneye tests that are known to fail, by model; everything else
/// in `acceptance` and `emulator-only` must pass.
const MOONEYE_KNOWN: &[(&str, &str)] = &[];

fn mooneye_suite(model: &str) {
    let root = testdata().join("mooneye");
    let mut files = Vec::new();
    let mut dirs = vec![root.join("acceptance"), root.join("emulator-only")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "gb") {
                files.push(path);
            }
        }
    }
    files.sort();
    let (mut passed, mut total) = (0, 0);
    let mut wrong = Vec::new();
    for path in files {
        let name = path
            .strip_prefix(&root)
            .unwrap()
            .with_extension("")
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if !mooneye_applies(&name, model) {
            continue;
        }
        total += 1;
        let result = mooneye(&path, model);
        let known = MOONEYE_KNOWN.contains(&(model, name.as_str()));
        let word = match result {
            Some(true) => "ok  ",
            Some(false) => "FAIL",
            None => "TIME",
        };
        println!("{word} {name}{}", if known { " (known)" } else { "" });
        if result == Some(true) {
            passed += 1;
        }
        if (result == Some(true)) == known {
            wrong.push(name);
        }
    }
    println!("mooneye {model}: {passed} of {total}");
    assert!(wrong.is_empty(), "differs from the known list: {wrong:?}");
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn mooneye_dmg() {
    mooneye_suite("dmg");
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn mooneye_pocket() {
    mooneye_suite("pocket");
}
