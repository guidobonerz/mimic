//! Development aid for the VICE `drive/viavarious` tests: runs one of them
//! on the C64 with a true 1541 and prints, per sub test, the first bytes
//! the drive measured next to the reference from `viaNref.bin`.
//!
//!     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata EMU_VIA_TEST=via1 cargo test -p emu-tests --release --test via_dump -- --ignored --nocapture

use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Size, StopReason};
use emu_tests::lorenz;

#[test]
#[ignore = "development aid"]
fn dump_via_test() {
    let Ok(name) = std::env::var("EMU_VIA_TEST") else {
        return;
    };
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let data =
        std::path::PathBuf::from(std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
            .join("vice-testprogs/drive/viavarious");
    let roms = media_backends::roms::load(&rom_dir, "c64").expect("ROMs");
    let mut config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    config.options.insert("debugcart".into(), "on".into());
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let prg = std::fs::read(data.join(format!("{name}.prg"))).unwrap();
    let reference = std::fs::read(data.join(format!("{name}ref.bin"))).unwrap();
    let reference = &reference[2..];
    let tests = reference.len() / 256;
    let mut dbg = DebugHooks::new();
    for _ in 0..lorenz::BOOT_FRAMES {
        m.run_frame(&mut dbg);
    }
    lorenz::load_and_run(m.as_mut(), &prg);
    let cpu = lorenz::space(m.as_ref(), "cpu");
    dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: cpu,
        addr: 0xd7ff,
        len: 1,
    });
    let limit: u64 = std::env::var("EMU_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400_000_000);
    while m.now().0 < limit {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => {}
            StopReason::Watchpoint { .. } => break,
            other => panic!("{other:?}"),
        }
    }
    let t = m.debug_target(machine_c64::CPU).unwrap();
    let mut all_ok = true;
    for i in 0..tests {
        let ours: Vec<u8> = (0..256)
            .map(|j| {
                t.peek(cpu, 0x8000 + i as u32 * 256 + j, Size::Byte)
                    .unwrap() as u8
            })
            .collect();
        let refs = &reference[i * 256..i * 256 + 256];
        let ok = ours == refs;
        all_ok &= ok;
        let first = ours.iter().zip(refs).position(|(a, b)| a != b);
        println!(
            "test {}: {}{}",
            (b'a' + i as u8) as char,
            if ok { "ok" } else { "DIFF" },
            first.map_or(String::new(), |f| format!(" from byte {f}"))
        );
        if !ok || std::env::var_os("EMU_ALL").is_some() {
            let n: usize = std::env::var("EMU_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(40);
            let from = first.unwrap_or(0).saturating_sub(4);
            println!("  ours {:02x?}", &ours[from..(from + n).min(256)]);
            println!("  ref  {:02x?}", &refs[from..(from + n).min(256)]);
        }
    }
    println!(
        "{name}: {}",
        if all_ok {
            "all sub tests match"
        } else {
            "differences"
        }
    );
}

/// Development aid for VICE `drive/hls-protection`: runs `hlstest.prg`
/// with its G64 and prints the drive's 16 result rows (bytes between
/// syncs, the byte before and after each sync) next to the expected ones.
///
///     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test via_dump hls -- --ignored --nocapture
#[test]
#[ignore = "development aid"]
fn dump_hls_test() {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let data =
        std::path::PathBuf::from(std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
            .join("vice-testprogs/drive/hls-protection");
    let roms = media_backends::roms::load(&rom_dir, "c64").expect("ROMs");
    let mut config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    config.options.insert("debugcart".into(), "on".into());
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let slot = m
        .media_slots()
        .iter()
        .find(|s| s.kind == emu_core::media::MediaKind::Disk)
        .unwrap()
        .id;
    m.media_slot(slot)
        .unwrap()
        .insert(media_backends::image::open(&data.join("hlstest.g64")).unwrap())
        .unwrap();
    let mut prg = std::fs::read(data.join("hlstest.prg")).unwrap();
    if let Ok(t) = std::env::var("EMU_HLS_TRACK") {
        // The track byte of the M-E string ("sel_track") is the one before "e-m".
        let t: u8 = t.parse().unwrap();
        let pos = prg.windows(3).position(|w| w == b"E-M").unwrap();
        prg[pos - 3] = t;
    }
    // PETSCII "e-m", the M-E command sent backwards.
    let track = prg[prg.windows(3).position(|w| w == b"E-M").unwrap() - 3];
    let expected = std::fs::read(data.join(if track >= 18 {
        "expected_t18-35.bin"
    } else {
        "expected_t1-17.bin"
    }))
    .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..lorenz::BOOT_FRAMES {
        m.run_frame(&mut dbg);
    }
    lorenz::load_and_run(m.as_mut(), &prg);
    let cpu = lorenz::space(m.as_ref(), "cpu");
    dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: cpu,
        addr: 0xd7ff,
        len: 1,
    });
    while m.now().0 < 400_000_000 {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => {}
            StopReason::Watchpoint { .. } => break,
            other => panic!("{other:?}"),
        }
    }
    let t = m.debug_target(machine_c64::drive::DRIVE8).unwrap();
    let ours: Vec<u8> = (0..64)
        .map(|j| {
            t.peek(machine_c64::drive::DRIVE_SPACE, 0x4c0 + j, Size::Byte)
                .unwrap() as u8
        })
        .collect();
    println!("track {track}: count(lo hi) before after");
    for i in 0..16 {
        let o = &ours[i * 4..i * 4 + 4];
        let e = &expected[i * 4..i * 4 + 4];
        println!(
            "  ours {:02x?}  ref {:02x?}{}",
            o,
            e,
            if o == e { "" } else { "  <--" }
        );
    }
}

/// Development aid for the VICE `SID/env_test` programs: runs one and
/// prints its 12 × 16 matrix of ENV3 readings (left half of the screen)
/// next to the expected one (right half), differences marked.
///
///     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata EMU_ENV_TEST=env_test_adra_1 cargo test -p emu-tests --release --test via_dump env -- --ignored --nocapture
#[test]
#[ignore = "development aid"]
fn dump_env_test() {
    let Ok(name) = std::env::var("EMU_ENV_TEST") else {
        return;
    };
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let data =
        std::path::PathBuf::from(std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
            .join("vice-testprogs/SID/env_test");
    let roms = media_backends::roms::load(&rom_dir, "c64").expect("ROMs");
    let mut config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    config.options.insert("debugcart".into(), "on".into());
    config.options.insert(
        "sid".into(),
        std::env::var("EMU_SID").unwrap_or_else(|_| "8580".into()),
    );
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let prg = std::fs::read(data.join(format!("{name}.prg"))).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..lorenz::BOOT_FRAMES {
        m.run_frame(&mut dbg);
    }
    lorenz::load_and_run(m.as_mut(), &prg);
    let cpu = lorenz::space(m.as_ref(), "cpu");
    dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: cpu,
        addr: 0xd7ff,
        len: 1,
    });
    while m.now().0 < 400_000_000 {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => {}
            StopReason::Watchpoint { .. } => break,
            other => panic!("{other:?}"),
        }
    }
    let t = m.debug_target(machine_c64::CPU).unwrap();
    let peek = |a: u32| t.peek(cpu, a, Size::Byte).unwrap() as u8;
    println!(
        "{name}: exit {:02x}; rows = readings, columns = tv 15..0",
        peek(0xd7ff)
    );
    let mut bad = 0;
    for row in 0..12u32 {
        let base = 0x452 + row * 40;
        let ours: Vec<u8> = (0..16).map(|c| peek(base + c)).collect();
        let refs: Vec<u8> = (0..16).map(|c| peek(base + 20 + c)).collect();
        let marks: String = ours
            .iter()
            .zip(&refs)
            .map(|(a, b)| if a == b { "   " } else { " ^ " })
            .collect();
        println!(
            "row {row:2} ours {}",
            ours.iter().map(|v| format!("{v:02x} ")).collect::<String>()
        );
        println!(
            "       ref  {}",
            refs.iter().map(|v| format!("{v:02x} ")).collect::<String>()
        );
        if ours != refs {
            println!("            {marks}");
            bad += 1;
        }
    }
    println!("{bad} rows differ");
}

/// Development aid: run any C64 program from the VICE test data until it
/// writes the debug cartridge or EMU_FRAMES frames pass, then print the
/// CPU registers and the screen's text.
///
///     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata EMU_PRG=general/Lorenz-2.15/src/trap16.prg cargo test -p emu-tests --release --test via_dump prg -- --ignored --nocapture
#[test]
#[ignore = "development aid"]
fn dump_prg() {
    let Ok(rel) = std::env::var("EMU_PRG") else {
        return;
    };
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let data =
        std::path::PathBuf::from(std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
            .join("vice-testprogs");
    let roms = media_backends::roms::load(&rom_dir, "c64").expect("ROMs");
    let mut config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    config.options.insert("debugcart".into(), "on".into());
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let prg = std::fs::read(data.join(&rel)).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..lorenz::BOOT_FRAMES {
        m.run_frame(&mut dbg);
    }
    if let Ok(disk) = std::env::var("EMU_LOAD_DISK") {
        // Mount the disk and type LOAD"TEST",8 and RUN instead.
        let slot = m
            .media_slots()
            .into_iter()
            .find(|s| s.kind == emu_core::media::MediaKind::Disk)
            .unwrap()
            .id;
        m.media_slot(slot)
            .unwrap()
            .insert(media_backends::image::open(std::path::Path::new(&disk)).unwrap())
            .unwrap();
        for text in [
            "load\"test\",8
",
            "run
",
        ] {
            let (events, _) = machine_c64::keyboard::typed_events(text, m.now().0);
            for ev in events {
                m.input(ev);
            }
            for _ in 0..400 {
                m.run_frame(&mut dbg);
            }
        }
    } else {
        lorenz::load_and_run(m.as_mut(), &prg);
    }
    let cpu = lorenz::space(m.as_ref(), "cpu");
    dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: cpu,
        addr: 0xd7ff,
        len: 1,
    });
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut stopped = false;
    for _ in 0..frames {
        match m.run_frame(&mut dbg) {
            StopReason::FrameDone => {}
            StopReason::Watchpoint { .. } => {
                stopped = true;
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    let t = m.debug_target(machine_c64::CPU).unwrap();
    println!(
        "{rel}: {}",
        if stopped { "wrote $D7FF" } else { "timed out" }
    );
    println!("regs: {:?}", t.regs());
    for row in 0..25u32 {
        let line: String = (0..40)
            .map(|col| {
                let c = t.peek(cpu, 0x400 + row * 40 + col, Size::Byte).unwrap() as u8 & 0x7f;
                match c {
                    1..=26 => (b'A' + c - 1) as char,
                    0x20..=0x3f => c as char,
                    0 => '@',
                    _ => '.',
                }
            })
            .collect();
        if line.trim_matches(' ').is_empty() {
            continue;
        }
        println!("{row:2}: {line}");
    }
}

/// Development aid for the VICE `VIC20/via_sr` tests: runs `viasrNN.prg`
/// on the VIC-20 (`EMU_VIASR_TEST=04`, `04exp` …) and prints the 388
/// bytes the program dumped (128 reads of the register 7 cycles apart,
/// then the loop's reads) next to `dumpNN.bin`, with the differences marked.
///
///     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata EMU_VIASR_TEST=04 cargo test -p emu-tests --release --test via_dump dump_via_sr_test -- --ignored --nocapture
#[test]
#[ignore = "development aid"]
fn dump_via_sr_test() {
    let Ok(name) = std::env::var("EMU_VIASR_TEST") else {
        return;
    };
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let data =
        std::path::PathBuf::from(std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
            .join("vice-testprogs/VIC20/via_sr");
    let expanded = name.contains("exp") || name.contains("iex");
    let mode = &name[..2];
    let roms = media_backends::roms::load(&rom_dir, "vic20").expect("ROMs");
    let mut config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    config.options.insert("debugcart".into(), "on".into());
    config
        .options
        .insert("ram".into(), if expanded { "8k" } else { "none" }.into());
    let mut m = emu_machines::registry().create("vic20", &config).unwrap();
    let prg = std::fs::read(data.join(format!("viasr{name}.prg"))).unwrap();
    let ifr = name.contains("ifr") || name.contains("iex");
    let reference =
        std::fs::read(data.join(format!("dump{mode}{}.bin", if ifr { "i" } else { "" })))
            .unwrap_or_default();
    let media =
        media_backends::image::from_bytes("t.prg", prg, emu_core::media::MediaKind::Program);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == emu_core::media::MediaKind::Program)
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let cpu = lorenz::space(m.as_ref(), "cpu");
    let mut dbg = DebugHooks::new();
    dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: cpu,
        addr: 0x910f,
        len: 1,
    });
    let mut hit = false;
    for _ in 0..2000 {
        if let StopReason::Watchpoint { .. } = m.run_frame(&mut dbg) {
            hit = true;
            break;
        }
    }
    println!("exit written: {hit}");
    let screen = if expanded { 0x1000 } else { 0x1e00 };
    let t = m.debug_target(emu_core::DeviceId("cpu")).unwrap();
    let ours: Vec<u8> = (0..388)
        .map(|i| t.peek(cpu, screen + i, Size::Byte).unwrap_or(0) as u8)
        .collect();
    let mut bad = 0;
    for row in 0..388usize.div_ceil(32) {
        let range = row * 32..(row * 32 + 32).min(388);
        let line = |v: &[u8]| -> String {
            v[range.clone()]
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let r = reference.get(range.start + i);
                    let mark = if r.is_some_and(|r| r != b) { "*" } else { " " };
                    format!("{b:02x}{mark}")
                })
                .collect()
        };
        println!("{:>3} ours {}", range.start, line(&ours));
        if reference.len() >= range.end {
            println!("    ref  {}", line(&reference));
        }
        bad += range
            .clone()
            .filter(|&i| reference.get(i).is_some_and(|r| *r != ours[i]))
            .count();
    }
    println!("{bad} bytes differ");
}

/// Development aid: runs any VIC-20 program (`EMU_VIC20_PRG`, relative to
/// the VICE test data; `EMU_VIC20_RAM=8k`) until it writes the debug
/// register `$910F` and prints the text screen as hex, rows of 22.
///
///     EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata EMU_VIC20_PRG=VIC20/via_wrap/via_T2wrap2a.prg EMU_VIC20_RAM=8k cargo test -p emu-tests --release --test via_dump dump_vic20_prg -- --ignored --nocapture
#[test]
#[ignore = "development aid"]
fn dump_vic20_prg() {
    let Ok(path) = std::env::var("EMU_VIC20_PRG") else {
        return;
    };
    let ram = std::env::var("EMU_VIC20_RAM").unwrap_or_else(|_| "none".into());
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let data =
        std::path::PathBuf::from(std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
            .join("vice-testprogs");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&rom_dir, "vic20").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("debugcart".into(), "on".into());
    config.options.insert("ram".into(), ram.clone());
    let mut m = emu_machines::registry().create("vic20", &config).unwrap();
    let prg = std::fs::read(data.join(&path)).unwrap();
    let media =
        media_backends::image::from_bytes("t.prg", prg, emu_core::media::MediaKind::Program);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == emu_core::media::MediaKind::Program)
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let cpu = lorenz::space(m.as_ref(), "cpu");
    let mut dbg = DebugHooks::new();
    dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: cpu,
        addr: 0x910f,
        len: 1,
    });
    let mut code = None;
    for _ in 0..3000 {
        if let StopReason::Watchpoint { .. } = m.run_frame(&mut dbg) {
            code = m
                .chips()
                .into_iter()
                .find(|c| c.name == "debugcart")
                .and_then(|c| c.regs.iter().find(|r| r.name == "EXIT").map(|r| r.value));
            break;
        }
    }
    println!("exit code: {code:?}");
    let screen = if ram == "none" { 0x1e00 } else { 0x1000 };
    let t = m.debug_target(emu_core::DeviceId("cpu")).unwrap();
    for row in 0..23u32 {
        let line: Vec<String> = (0..22)
            .map(|c| {
                format!(
                    "{:02x}",
                    t.peek(cpu, screen + row * 22 + c, Size::Byte).unwrap_or(0)
                )
            })
            .collect();
        println!("{row:>2} {}", line.join(" "));
    }
}
