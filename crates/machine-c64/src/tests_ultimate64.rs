//! The machine "ultimate64": its models, the built-in command interface
//! and the turbo CPU (a test KERNAL with a small loop at $E000).

use super::*;
use crate::memory::KERNAL_SIZE;

/// `INC $0400 : JMP $E000`: 9 cycles a round, all in internal memory.
const COUNT_RAM: &[u8] = &[0xee, 0x00, 0x04, 0x4c, 0x00, 0xe0];
/// `INC $D020 : JMP $E000`: three of the 9 cycles reach the VIC-II.
const COUNT_BORDER: &[u8] = &[0xee, 0x20, 0xd0, 0x4c, 0x00, 0xe0];

fn config(code: &[u8], model: Option<&str>, options: &[(&str, &str)]) -> MachineConfig {
    let mut kernal = vec![0xea; KERNAL_SIZE];
    kernal[..code.len()].copy_from_slice(code);
    kernal[0x1ffa..].copy_from_slice(&[0x00, 0xe0, 0x00, 0xe0, 0x00, 0xe0]);
    let mut c = MachineConfig {
        model: model.map(Into::into),
        ..MachineConfig::default()
    };
    c.roms.insert("kernal".into(), kernal);
    c.roms.insert("basic".into(), vec![0xb0; BASIC_SIZE]);
    c.roms.insert("chargen".into(), vec![0xc0; CHARGEN_SIZE]);
    for (k, v) in options {
        c.options.insert((*k).into(), (*v).into());
    }
    c
}

fn u64_with(code: &[u8], options: &[(&str, &str)]) -> C64 {
    C64::new_ultimate(&config(code, None, options)).unwrap()
}

fn step(m: &mut C64, n: u32) {
    for _ in 0..n {
        assert_eq!(m.step_cycle(), StopReason::StepDone);
    }
}

fn peek(m: &C64, a: u16) -> u8 {
    m.peek(CPU_SPACE, u32::from(a), Size::Byte).unwrap() as u8
}

fn poke(m: &mut C64, a: u16, v: u8) {
    assert!(m.poke(CPU_SPACE, u32::from(a), Size::Byte, u32::from(v)));
}

/// The RAM counter after `cycles` VIC-II cycles from power-on.
fn count(options: &[(&str, &str)], cycles: u32) -> u8 {
    let mut m = u64_with(COUNT_RAM, options);
    let start = m.mem.ram[0x400];
    step(&mut m, cycles);
    m.mem.ram[0x400].wrapping_sub(start)
}

#[test]
fn models_and_the_built_in_interface() {
    for model in [None, Some("u64"), Some("c64u")] {
        let m = C64::new_ultimate(&config(COUNT_RAM, model, &[])).unwrap();
        assert!(m.uci.is_some(), "{model:?}");
        assert!(m.io.turbo.is_some());
        assert!(m.chips().iter().any(|c| c.name == "turbo"));
        assert!(m.chips().iter().any(|c| c.name == "uci"));
    }
    assert!(matches!(
        C64::new_ultimate(&config(COUNT_RAM, Some("u2"), &[])),
        Err(ConfigError::UnknownModel(_))
    ));
    // The interface can be switched off, as in the board's menu.
    assert!(u64_with(COUNT_RAM, &[("uci", "off")]).uci.is_none());
    // A C64 has neither the models nor the turbo.
    assert!(C64::new(&config(COUNT_RAM, Some("u64"), &[])).is_err());
    let c64 = C64::new(&config(COUNT_RAM, None, &[])).unwrap();
    assert!(c64.io.turbo.is_none() && c64.uci.is_none());
    assert!(!c64.chips().iter().any(|c| c.name == "turbo"));
}

#[test]
fn speed_options_follow_the_boards_table() {
    let opts = |mhz| config(COUNT_RAM, None, &[("turbo", "manual"), ("cpu_speed", mhz)]);
    assert!(C64::new_ultimate(&opts("48")).is_ok());
    // 5 MHz is in the U64's table only, 64 MHz in the Elite II's only.
    assert!(C64::new_ultimate(&opts("5")).is_ok());
    assert!(C64::new_ultimate(&opts("64")).is_err());
    let e2 = |mhz| {
        config(
            COUNT_RAM,
            Some("c64u"),
            &[("turbo", "manual"), ("cpu_speed", mhz)],
        )
    };
    assert!(C64::new_ultimate(&e2("64")).is_ok());
    assert!(C64::new_ultimate(&e2("5")).is_err());
    for bad in [("turbo", "fast"), ("cpu_speed", "x"), ("badlines", "maybe")] {
        assert!(C64::new_ultimate(&config(COUNT_RAM, None, &[bad])).is_err());
    }
}

#[test]
fn without_turbo_it_runs_as_a_c64() {
    let mut c64 = C64::new(&config(COUNT_RAM, None, &[])).unwrap();
    step(&mut c64, 900);
    let plain = c64.mem.ram[0x400];
    assert_eq!(count(&[], 900), plain);
    assert_eq!(count(&[("turbo", "manual")], 900), plain);
    assert_eq!(count(&[("turbo", "registers")], 900), plain);
    // No register answers: the VIC-II's unused registers read $FF.
    let m = u64_with(COUNT_RAM, &[("turbo", "manual"), ("cpu_speed", "4")]);
    assert_eq!((peek(&m, 0xd030), peek(&m, 0xd031)), (0xff, 0xff));
}

#[test]
fn manual_turbo_multiplies_the_cpu_cycles() {
    // 90 VIC-II cycles: 7 of reset, then 9 CPU cycles a round.
    let one = count(&[], 90);
    assert_eq!(one, 9);
    let four = count(&[("turbo", "manual"), ("cpu_speed", "4")], 90);
    assert_eq!(u32::from(four), (90 * 4 - 7) / 9);
    let max = count(&[("turbo", "manual"), ("cpu_speed", "16")], 90);
    assert_eq!(u32::from(max), (90 * 16 - 7) / 9);
}

#[test]
fn external_accesses_keep_the_bus_timing() {
    let border = |options: &[(&str, &str)]| {
        let mut m = u64_with(COUNT_BORDER, options);
        step(&mut m, 90);
        m.io.vic.peek(0x20) & 0x0f
    };
    let one = border(&[]);
    let fast = border(&[("turbo", "manual"), ("cpu_speed", "48")]);
    // Three VIC-II accesses a round take three 1 MHz cycles whatever the
    // speed: at most 30 rounds in 90 cycles, 14 in the border's 4 bits.
    assert_eq!(one, 9);
    assert!((25..=30).contains(&(u32::from(fast) + 16)), "{fast}");
}

#[test]
fn the_registers_set_the_speed() {
    let mut m = u64_with(COUNT_RAM, &[("turbo", "registers"), ("cpu_speed", "2")]);
    // $D031 starts with the menu's settings; $D030 belongs to the VIC-II.
    assert_eq!((peek(&m, 0xd030), peek(&m, 0xd031)), (0xff, 0x01));
    poke(&mut m, 0xd031, 0x8f);
    assert_eq!(peek(&m, 0xd031), 0x8f);
    let t = m.io.turbo.as_ref().unwrap();
    assert_eq!((t.factor(), t.badlines()), (48, false));
    poke(&mut m, 0xd07a, 0);
    assert_eq!(m.io.turbo.as_ref().unwrap().factor(), 1);
    poke(&mut m, 0xd07b, 0);
    assert_eq!(m.io.turbo.as_ref().unwrap().factor(), 20);
    // A reset puts the menu's settings back.
    m.reset(ResetKind::Soft);
    assert_eq!(peek(&m, 0xd031), 0x01);

    let mut m = u64_with(COUNT_RAM, &[("turbo", "enablebit"), ("cpu_speed", "8")]);
    assert_eq!(peek(&m, 0xd030), 0xfe);
    assert_eq!(m.io.turbo.as_ref().unwrap().factor(), 1);
    poke(&mut m, 0xd030, 1);
    assert_eq!(peek(&m, 0xd030), 0xff);
    assert_eq!(m.io.turbo.as_ref().unwrap().factor(), 8);
    // The Elite II board's table differs.
    let mut m =
        C64::new_ultimate(&config(COUNT_RAM, Some("c64u"), &[("turbo", "registers")])).unwrap();
    poke(&mut m, 0xd031, 0x0f);
    assert_eq!(m.io.turbo.as_ref().unwrap().factor(), 64);
}

#[test]
fn a_breakpoint_stops_between_turbo_cycles() {
    let mut m = u64_with(COUNT_RAM, &[("turbo", "manual"), ("cpu_speed", "48")]);
    let mut dbg = DebugHooks::new();
    let bp = dbg.add_breakpoint(mimic_core::BreakpointKind::Exec {
        space: CPU_SPACE,
        addr: 0xe003,
    });
    let start = m.mem.ram[0x400];
    // Every round stops once, right after its INC.
    for round in 1..=3u8 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(bp));
        assert_eq!(m.cpu.pc(), 0xe003);
        assert!(m.cpu.at_instruction_boundary());
        assert_eq!(m.mem.ram[0x400], start.wrapping_add(round));
    }
}

#[test]
fn turbo_is_deterministic_and_in_the_savestate() {
    let options = [("turbo", "registers"), ("cpu_speed", "6")];
    let mut a = u64_with(COUNT_RAM, &options);
    step(&mut a, 500);
    poke(&mut a, 0xd031, 0x85);
    let state = a.save_state();
    step(&mut a, 700);
    let after = a.save_state();

    let mut b = u64_with(COUNT_RAM, &options);
    b.load_state(&state).unwrap();
    assert_eq!(peek(&b, 0xd031), 0x85);
    step(&mut b, 700);
    assert_eq!(b.save_state(), after);
    // A C64's state has no turbo registers.
    let c64 = C64::new(&config(COUNT_RAM, None, &[])).unwrap();
    assert!(b.load_state(&c64.save_state()).is_err());
    assert_eq!(b.save_state(), after);
}

// ---- the menu and the updater -------------------------------------------

use crate::keyboard::keys;
use crate::u64menu::Flash;
use mimic_core::host::{lock, MemFileHost};
use mimic_core::input::MachineKey;

fn with_files(model: Option<&str>, files: &[(&str, Vec<u8>)]) -> C64 {
    let mut m = C64::new_ultimate(&config(COUNT_RAM, model, &[])).unwrap();
    let mut fs = MemFileHost::new();
    for (path, data) in files {
        fs.insert(path, data.clone()).unwrap();
    }
    m.attach_host(HostServices::new(None, Some(Box::new(fs))));
    m
}

fn frames(m: &mut C64, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn menu_button(m: &mut C64) {
    m.control(5, 1);
    m.control(5, 0);
}

fn press(m: &mut C64, key: MachineKey) {
    m.menu_key(key, true);
    m.menu_key(key, false);
}

fn flash(m: &C64) -> Flash {
    m.menu.as_ref().unwrap().flash().clone()
}

/// Move the browser's cursor onto `name`.
fn select(m: &mut C64, name: &str) {
    let at = m
        .menu
        .as_ref()
        .unwrap()
        .entries()
        .iter()
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} not listed"));
    for _ in 0..at {
        press(m, keys::CRSR_DOWN);
    }
}

fn flash_file(m: &C64) -> Option<Vec<u8>> {
    let files = m.host.files.clone().unwrap();
    let mut f = lock(&files);
    devices_net::uci::read_file(&mut *f, "/Flash/.firmware").ok()
}

#[test]
fn the_menu_freezes_the_c64_and_draws_over_the_picture() {
    let mut m = with_files(None, &[]);
    assert!(m.controls().iter().any(|c| c.name == "MENU"));
    let c64 = C64::new(&config(COUNT_RAM, None, &[])).unwrap();
    assert!(!c64.controls().iter().any(|c| c.name == "MENU"));

    frames(&mut m, 2);
    menu_button(&mut m);
    assert!(m.frozen);
    let before = m.mem.ram[0x400];
    frames(&mut m, 2);
    assert_eq!(m.mem.ram[0x400], before);
    let shown = match m.framebuffer().pixels {
        PixelData::Indexed { pixels, .. } => pixels.to_vec(),
        _ => panic!("indexed picture"),
    };
    assert_ne!(shown, m.io.vic.pixels());
    // Keys go to the menu, not to the C64's matrix.
    m.menu_key(keys::F3, true);
    assert_eq!(m.io.keyboard.matrix, 0);
    // RUN/STOP leaves the help, then the browser; the C64 goes on.
    press(&mut m, keys::RUN_STOP);
    press(&mut m, keys::RUN_STOP);
    assert!(!m.frozen);
    frames(&mut m, 1);
    assert_ne!(m.mem.ram[0x400], before);
    // The button leaves as well.
    menu_button(&mut m);
    menu_button(&mut m);
    assert!(!m.frozen);
}

#[test]
fn the_browser_loads_programs_and_mounts_disks() {
    let disk = media::d64::D64::blank(b"TEST", *b"01").to_bytes();
    let mut m = with_files(
        None,
        &[
            ("/games/hello.prg", vec![0x00, 0xc0, 1, 2, 3]),
            ("/disk.d64", disk),
            ("/.hidden", vec![1]),
            ("/notes.txt", vec![1]),
        ],
    );
    frames(&mut m, 1);
    menu_button(&mut m);
    let names: Vec<_> = m
        .menu
        .as_ref()
        .unwrap()
        .entries()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(names, ["games", "disk.d64", "notes.txt"]);
    // Into the folder, the program's context menu, "Load".
    press(&mut m, keys::RETURN);
    press(&mut m, keys::RETURN);
    press(&mut m, keys::CRSR_DOWN);
    press(&mut m, keys::RETURN);
    assert!(!m.frozen);
    assert_eq!(m.mem.ram[0xc000..0xc003], [1, 2, 3]);
    // Back up with DEL, then "Mount Disk".
    menu_button(&mut m);
    press(&mut m, keys::DEL);
    select(&mut m, "disk.d64");
    assert!(!m.drive.as_ref().unwrap().has_disk());
    press(&mut m, keys::RETURN);
    press(&mut m, keys::CRSR_DOWN);
    press(&mut m, keys::RETURN);
    assert!(!m.frozen);
    assert!(m.drive.as_ref().unwrap().has_disk());
    // Tools: remove the disk.
    menu_button(&mut m);
    press(&mut m, keys::F5);
    press(&mut m, keys::CRSR_DOWN);
    press(&mut m, keys::CRSR_DOWN);
    press(&mut m, keys::RETURN);
    assert!(!m.drive.as_ref().unwrap().has_disk());
    // A file with nothing to do keeps the browser (the cursor is still
    // on the disk; the text file is the next entry).
    press(&mut m, keys::CRSR_DOWN);
    press(&mut m, keys::RETURN);
    assert!(m.frozen);
}

#[test]
fn the_configuration_sets_the_turbo() {
    let mut m = with_files(None, &[]);
    frames(&mut m, 1);
    menu_button(&mut m);
    // F2 is shift + F1 on the C64's keyboard.
    m.menu_key(keys::LSHIFT, true);
    press(&mut m, keys::F1);
    m.menu_key(keys::LSHIFT, false);
    press(&mut m, keys::RETURN);
    // Turbo Control: Off -> Manual; CPU Speed: three steps up.
    press(&mut m, keys::RETURN);
    press(&mut m, keys::CRSR_DOWN);
    for _ in 0..3 {
        press(&mut m, MachineKey(40));
    }
    let t = m.io.turbo.as_ref().unwrap();
    assert_eq!((t.mode(), t.factor()), (turbo::TurboMode::Manual, 4));
    // The settings are in the savestate, the open menu too.
    let state = m.save_state();
    let mut other = with_files(None, &[]);
    other.load_state(&state).unwrap();
    assert!(other.frozen);
    assert_eq!(other.io.turbo.as_ref().unwrap().factor(), 4);
    assert_eq!(other.save_state(), state);
}

/// An update image as the 3.15a files are laid out: load address,
/// length, start address, then the image with its version text in front
/// of the title's format.
fn firmware(len: usize) -> Vec<u8> {
    let mut image: Vec<u8> = (0..len).map(|i| (i * 7 + i / 251) as u8 | 0x80).collect();
    // Across the border of two of the updater's steps (32 KB each).
    let at = 32 * 1024 - 20;
    let text = b"Unknown\x003.15a\x00\x00\x00%s (V1.%b) %s\x00";
    if len > at + text.len() {
        image[at..at + text.len()].copy_from_slice(text);
    }
    let mut file = 0x0300_0000u32.to_le_bytes().to_vec();
    file.extend_from_slice(&(len as u32).to_le_bytes());
    file.extend_from_slice(&0x0300_0234u32.to_le_bytes());
    file.extend_from_slice(&image);
    file
}

/// The Commodore 64 Ultimate's file: a header with the product's name in
/// front of the image file.
fn cfw(product: &str, len: usize) -> Vec<u8> {
    let mut file = vec![0xde, 0xad, 0x15, 0x41];
    let mut name = [0u8; 32];
    name[..product.len()].copy_from_slice(product.as_bytes());
    file.extend_from_slice(&name);
    file.extend_from_slice(&firmware(len));
    file
}

/// Open the menu, start "Run Update" on `name` and let the check run.
fn run_update(m: &mut C64, name: &str) {
    frames(m, 1);
    menu_button(m);
    select(m, name);
    press(m, keys::RETURN);
    press(m, keys::RETURN);
    frames(m, 30);
}

/// Run until the flash holds a complete update.
fn until_installed(m: &mut C64) -> bool {
    let mut seen_flashing = false;
    for _ in 0..300 {
        frames(m, 1);
        seen_flashing |= m.menu.as_ref().unwrap().flashing();
        if matches!(flash(m), Flash::Installed { .. }) {
            break;
        }
    }
    seen_flashing
}

#[test]
fn an_update_is_checked_programmed_and_verified() {
    let fw = firmware(100_000);
    let mut m = with_files(None, &[("/update.u64", fw.clone())]);
    run_update(&mut m, "update.u64");
    // "About to update. Continue?": nothing is written before the answer.
    frames(&mut m, 60);
    assert!(flash_file(&m).is_none());
    assert_eq!(flash(&m), Flash::Factory);
    press(&mut m, keys::RETURN);
    assert!(until_installed(&mut m));
    let Flash::Installed {
        name,
        version,
        size,
        crc,
    } = flash(&m)
    else {
        panic!("not installed: {:?}", flash(&m));
    };
    assert_eq!((name.as_str(), version.as_str()), ("update.u64", "3.15a"));
    assert_eq!(size as usize, fw.len());
    assert_eq!(crc, media::a2disk::crc32(&fw));
    let image = flash_file(&m).unwrap();
    assert!(image.ends_with(&fw) && image.starts_with(b"MIMICFW2"));
    // Five seconds later the updater switches the machine off (here: off
    // and on again); the flash is read again.
    assert!(m.frozen);
    press(&mut m, keys::SPACE);
    assert!(m.frozen);
    frames(&mut m, 260);
    assert!(!m.frozen);
    assert!(matches!(flash(&m), Flash::Installed { .. }));
}

#[test]
fn a_cfw_file_names_the_family_or_the_product() {
    let named = |family: &str, product: &str| {
        let mut file = cfw(family, 40_000);
        file[20..20 + product.len()].copy_from_slice(product.as_bytes());
        file
    };
    let try_file = |file: Vec<u8>| {
        let mut m = with_files(None, &[("/x.cfw", file)]);
        run_update(&mut m, "x.cfw");
        press(&mut m, keys::RETURN);
        for _ in 0..90 {
            frames(&mut m, 1);
            if matches!(flash(&m), Flash::Installed { .. }) {
                return true;
            }
        }
        assert!(flash_file(&m).is_none());
        false
    };
    // The Ultimate 64's family is U64MK1, its name "Ultimate 64".
    assert!(try_file(cfw("U64MK1", 40_000)));
    assert!(try_file(named("", "Ultimate 64")));
    assert!(try_file(named("U64MK1", "Ultimate 64")));
    assert!(try_file(named("", "")));
    assert!(!try_file(cfw("U64MK2", 40_000)));
    assert!(!try_file(named("", "Ultimate 64-II")));
    assert!(!try_file(named("U64MK1", "Ultimate II+")));
    let mut bad = cfw("U64MK1", 40_000);
    bad[0] = 0;
    assert!(!try_file(bad));
}

#[test]
fn the_answer_no_leaves_the_flash_alone() {
    let mut m = with_files(None, &[("/update.u64", firmware(50_000))]);
    run_update(&mut m, "update.u64");
    press(&mut m, keys::RUN_STOP);
    frames(&mut m, 80);
    assert!(flash_file(&m).is_none());
    assert!(m.frozen && !m.menu.as_ref().unwrap().flashing());
}

#[test]
fn wrong_and_invalid_updates_are_refused() {
    let mut bad_length = firmware(5000);
    bad_length[4] ^= 1;
    let mut bad_load = firmware(5000);
    bad_load[3] = 0x04;
    let mut m = with_files(
        None,
        &[
            ("/a.u2p", firmware(5000)),
            ("/b.cfw", cfw("U64MK2", 5000)),
            ("/c.u64", vec![]),
            ("/d.u64", vec![0x55; 5000]),
            ("/e.u64", bad_length),
            ("/f.u64", bad_load),
        ],
    );
    frames(&mut m, 1);
    menu_button(&mut m);
    for _ in 0..6 {
        // RETURN, "Run Update", the check, RETURN to a question that must
        // not have come, a key back to the browser, the next file.
        press(&mut m, keys::RETURN);
        press(&mut m, keys::RETURN);
        frames(&mut m, 10);
        press(&mut m, keys::RETURN);
        frames(&mut m, 60);
        assert!(!m.menu.as_ref().unwrap().flashing());
        assert!(flash_file(&m).is_none());
        press(&mut m, keys::SPACE);
        press(&mut m, keys::CRSR_DOWN);
    }
    assert_eq!(flash(&m), Flash::Factory);
}

#[test]
fn the_commodore_64_ultimate_takes_cfw_and_ue2_files() {
    let mut m = with_files(
        Some("c64u"),
        &[
            ("/a.u64", firmware(5000)),
            ("/b.cfw", cfw("U2P", 5000)),
            ("/c.cfw", cfw("U64MK2", 70_000)),
            ("/d.ue2", firmware(70_000)),
        ],
    );
    frames(&mut m, 1);
    menu_button(&mut m);
    // The U64's file and a file for another product: refused.
    for _ in 0..2 {
        press(&mut m, keys::RETURN);
        press(&mut m, keys::RETURN);
        frames(&mut m, 10);
        press(&mut m, keys::RETURN);
        frames(&mut m, 60);
        assert!(flash_file(&m).is_none());
        press(&mut m, keys::SPACE);
        press(&mut m, keys::CRSR_DOWN);
    }
    // Its own file: flashed, and the machine switches off and on again
    // by itself five seconds later.
    press(&mut m, keys::RETURN);
    press(&mut m, keys::RETURN);
    frames(&mut m, 10);
    press(&mut m, keys::RETURN);
    assert!(until_installed(&mut m));
    assert!(m.frozen);
    frames(&mut m, 260);
    assert!(!m.frozen);
    let Flash::Installed { name, version, .. } = flash(&m) else {
        panic!("not installed");
    };
    assert_eq!((name.as_str(), version.as_str()), ("c.cfw", "3.15a"));
    // The Elite II file as well.
    menu_button(&mut m);
    select(&mut m, "d.ue2");
    press(&mut m, keys::RETURN);
    press(&mut m, keys::RETURN);
    frames(&mut m, 10);
    press(&mut m, keys::RETURN);
    for _ in 0..300 {
        frames(&mut m, 1);
        if matches!(flash(&m), Flash::Installed { name, .. } if name == "d.ue2") {
            return;
        }
    }
    panic!("d.ue2 not installed: {:?}", flash(&m));
}

#[test]
fn a_power_cut_while_flashing_damages_the_flash() {
    let mut m = with_files(None, &[("/update.u64", firmware(600_000))]);
    run_update(&mut m, "update.u64");
    press(&mut m, keys::RETURN);
    // Past the erase, into the programming.
    for _ in 0..200 {
        frames(&mut m, 1);
        if flash_file(&m).is_some_and(|f| f.len() > 100_000) {
            break;
        }
    }
    assert!(m.menu.as_ref().unwrap().flashing());
    m.control(0, 1);
    frames(&mut m, 1);
    assert_eq!(flash(&m), Flash::Damaged);
    // The board does not start: the notice stays, whatever is pressed.
    assert!(m.frozen);
    menu_button(&mut m);
    press(&mut m, keys::RUN_STOP);
    m.control(1, 1);
    assert!(m.frozen);
    let stuck = m.mem.ram[0x400];
    frames(&mut m, 2);
    assert_eq!(m.mem.ram[0x400], stuck);
    // Without the flash file the factory firmware is back.
    let files = m.host.files.clone().unwrap();
    lock(&files).delete("/Flash/.firmware").unwrap();
    m.control(0, 1);
    frames(&mut m, 2);
    assert_eq!(flash(&m), Flash::Factory);
    assert!(!m.frozen);
}
