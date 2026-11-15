//! KC85 with CAOS (ROMs user-supplied via `EMU_ROM_DIR` or
//! `~/.mimic/roms`, a `kc85/` subfolder with the files as KCemu names
//! them: `caos__e0.853`, `basic_c0.853`, `caos__c0.854` …).
//!
//!     cargo test -p emu-tests --release --test kc85_boot -- --ignored --nocapture

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};

fn machine(model: &str, options: &[(&str, &str)]) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let mut config = MachineConfig {
        model: Some(model.into()),
        roms: media_backends::roms::load(&dir, "kc85").expect("ROMs"),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    emu_machines::registry().create("kc85", &config).unwrap()
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
        .peek(machine_kc85::CPU_SPACE, addr, Size::Byte)
        .unwrap() as u8
}

/// CAOS' character buffer at `$B200`: 32 lines of 40 characters.
fn screen(m: &mut dyn Machine) -> Vec<String> {
    (0..32)
        .map(|row| {
            (0..40)
                .map(|col| match byte(m, 0xb200 + row * 40 + col) {
                    0 => ' ',
                    c @ 0x20..=0x7e => c as char,
                    _ => '.',
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn dump(m: &mut dyn Machine) {
    for l in screen(m).iter().filter(|l| !l.is_empty()) {
        println!("|{l}");
    }
    let regs = m.debug_target(DeviceId("cpu")).unwrap().regs();
    let line: Vec<_> = regs
        .0
        .iter()
        .map(|r| format!("{}={:X}", r.name, r.value))
        .collect();
    println!("{}", line.join(" "));
}

#[test]
#[ignore = "needs the user's KC85 ROMs"]
fn every_model_boots_to_the_caos_menu() {
    for (model, banner) in [
        ("2", "* HC900-CAOS *"),
        ("3", "* HC-CAOS 3.1 *"),
        ("4", "* KC-CAOS 4.2 *"),
        ("5", "* KC-CAOS 4.4 *"),
    ] {
        let mut m = machine(model, &[]);
        run(&mut *m, 400);
        println!("--- KC85/{model}");
        dump(&mut *m);
        let text = screen(&mut *m);
        assert!(text.iter().any(|l| l == banner), "KC85/{model}");
        assert!(text.iter().any(|l| l == "%" || l == ">"), "the prompt");
    }
}

/// Disassembly for studying the ROMs: `KC_DIS=3:e060:e0a0[:frames]`.
#[test]
#[ignore = "a tool: needs the user's KC85 ROMs"]
fn disassemble() {
    let Ok(spec) = std::env::var("KC_DIS") else {
        return;
    };
    let p: Vec<&str> = spec.split(':').collect();
    let mut m = machine(p[0], &[]);
    run(&mut *m, p.get(3).map_or(150, |f| f.parse().unwrap()));
    let mut a = u32::from_str_radix(p[1], 16).unwrap();
    let end = u32::from_str_radix(p[2], 16).unwrap();
    for chip in m.chips() {
        let regs: Vec<_> = chip
            .regs
            .iter()
            .map(|r| format!("{}={:X}", r.name, r.value))
            .collect();
        println!("{}: {}", chip.name, regs.join(" "));
    }
    let table: Vec<_> = (0x1e0..0x1f0)
        .map(|a| format!("{:02X}", byte(&mut *m, a)))
        .collect();
    println!("01E0: {}", table.join(" "));
    let ktab = u32::from(byte(&mut *m, 0x1fe)) | u32::from(byte(&mut *m, 0x1ff)) << 8;
    let keys: Vec<_> = (0..128)
        .map(|i| format!("{:02X}", byte(&mut *m, ktab + i)))
        .collect();
    println!("KTAB {ktab:04X}: {}", keys.join(" "));
    let t = m.debug_target(DeviceId("cpu")).unwrap();
    while a < end {
        let i = t.disassemble(a);
        println!("{a:04X}  {i}");
        a += i.bytes.len().max(1) as u32;
    }
}

/// Where the CPU is, field by field: `KC_WATCH=3:40`.
#[test]
#[ignore = "a tool: needs the user's KC85 ROMs"]
fn watch() {
    let Ok(spec) = std::env::var("KC_WATCH") else {
        return;
    };
    let p: Vec<&str> = spec.split(':').collect();
    let mut m = machine(p[0], &[]);
    for f in 0..p[1].parse::<usize>().unwrap() {
        run(&mut *m, 1);
        let regs = m.debug_target(DeviceId("cpu")).unwrap().regs();
        let get = |n: &str| regs.0.iter().find(|r| r.name == n).unwrap().value;
        let cursor = (byte(&mut *m, 0xb7a0), byte(&mut *m, 0xb7a1));
        println!(
            "{f:3} PC={:04X} SP={:04X} HL={:02X}{:02X} DE={:02X}{:02X} cursor={cursor:?} pioA={:02X}",
            get("PC"),
            get("SP"),
            get("H"),
            get("L"),
            get("D"),
            get("E"),
            byte(&mut *m, 0x1_0088),
        );
    }
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, _) = m.typed_keys(text, m.now()).expect("the KC85 types");
    for e in events {
        m.input(e);
    }
}

/// Typing through the keyboard's pulses: BASIC takes a program and runs
/// it (KC85/3: CAOS 3.1's interrupt routine, KC85/4: CAOS 4.2's).
#[test]
#[ignore = "needs the user's KC85 ROMs"]
fn a_typed_basic_program_runs() {
    for model in ["3", "4", "5"] {
        let mut m = machine(model, &[]);
        run(&mut *m, 400);
        type_text(&mut *m, "BASIC\n");
        run(&mut *m, 300);
        // MEMORY END ?
        type_text(&mut *m, "\n");
        run(&mut *m, 300);
        type_text(
            &mut *m,
            "10 FOR I=1 TO 3\n20 PRINT \"ok+-*/=<>?\";I*7\n30 NEXT\nRUN\n",
        );
        run(&mut *m, 1200);
        println!("--- KC85/{model}");
        dump(&mut *m);
        let text = screen(&mut *m);
        for want in ["ok+-*/=<>? 7", "ok+-*/=<>? 14", "ok+-*/=<>? 21"] {
            assert!(text.iter().any(|l| l == want), "KC85/{model}: {want}");
        }
    }
}

/// A machine code program as a KCC file: header block, then the code
/// from `$0200` with a start address.
fn kcc() -> Vec<u8> {
    // LD A,$42 / LD ($3000),A / RET, then filler into a third block.
    let mut body = vec![0x3e, 0x42, 0x32, 0x00, 0x30, 0xc9];
    body.extend((0..200).map(|i| i as u8 ^ 0x5a));
    let end = 0x0200 + body.len() as u16;
    let mut image = vec![0u8; 128];
    image[..11].copy_from_slice(b"TEST    KCC");
    image[16] = 3;
    image[17..19].copy_from_slice(&0x0200u16.to_le_bytes());
    image[19..21].copy_from_slice(&end.to_le_bytes());
    image[21..23].copy_from_slice(&0x0200u16.to_le_bytes());
    image.extend(&body);
    image.resize(128 * 3, 0);
    image
}

/// The phase's gate: LOAD reads a program from the cassette (the tape's
/// zero crossings at PIO A, timed by CAOS with CTC 2) and starts it.
#[test]
#[ignore = "needs the user's KC85 ROMs"]
fn a_program_loads_from_tape_and_starts() {
    for model in ["2", "3", "4", "5"] {
        let mut m = machine(model, &[]);
        run(&mut *m, 400);
        let image = kcc();
        let media = emu_core::media::Media {
            name: "test.kcc".into(),
            kind: emu_core::media::MediaKind::Tape,
            backend: Box::new(media_backends::image::MemoryBackend {
                data: image.clone(),
            }),
        };
        m.media_slot(machine_kc85::SLOT_TAPE)
            .unwrap()
            .insert(media)
            .unwrap();
        assert_eq!(byte(&mut *m, 0x3000), 0);
        type_text(&mut *m, "LOAD\n");
        run(&mut *m, 900);
        println!("--- KC85/{model}");
        dump(&mut *m);
        let text = screen(&mut *m);
        assert!(
            text.iter().any(|l| l.starts_with("TEST    KCC 0200 02CE")),
            "KC85/{model}: the header block"
        );
        for (i, &b) in image[128..128 + 206].iter().enumerate() {
            assert_eq!(byte(&mut *m, 0x200 + i as u32), b, "KC85/{model}: byte {i}");
        }
        assert_eq!(byte(&mut *m, 0x3000), 0x42, "KC85/{model}: started");
    }
}
