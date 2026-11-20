//! KC85 with CAOS (ROMs user-supplied via `EMU_ROM_DIR` or
//! `~/.mimic/roms`, a `kc85/` subfolder with the files as KCemu names
//! them: `caos__e0.853`, `basic_c0.853`, `caos__c0.854` …).
//!
//!     cargo test -p mimic-tests --release --test kc85_boot -- --ignored --nocapture

use mimic_core::machine::Machine;
use mimic_core::registry::MachineConfig;
use mimic_core::{DebugHooks, DeviceId, Size, StopReason};

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
    mimic_machines::registry().create("kc85", &config).unwrap()
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
    let sp = regs
        .0
        .iter()
        .find(|r| r.name == "SP")
        .map_or(0, |r| r.value);
    let stack: Vec<_> = (0..12)
        .map(|i| {
            let a = sp + 2 * i;
            format!("{:02X}{:02X}", byte(m, a + 1), byte(m, a))
        })
        .collect();
    println!("stack: {} pioA={:02X}", stack.join(" "), byte(m, 0x1_0088));
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

/// Disassembly for studying the ROMs: `KC_DIS=3:e060:e0a0[:frames[:line]]`.
#[test]
#[ignore = "a tool: needs the user's KC85 ROMs"]
fn disassemble() {
    let Ok(spec) = std::env::var("KC_DIS") else {
        return;
    };
    let p: Vec<&str> = spec.split(':').collect();
    let mut m = machine(p[0], &[]);
    run(&mut *m, p.get(3).map_or(150, |f| f.parse().unwrap()));
    // A fifth part: a line typed first (`BASIC` switches its ROM in).
    if let Some(line) = p.get(4) {
        type_text(
            &mut *m,
            &format!(
                "{line}
"
            ),
        );
        run(&mut *m, 300);
    }
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
    // `KC_ROM=file`: that ROM file's bytes (it sits at $C000) are put
    // into RAM at $4000 and read there; relative targets are $8000 low.
    if let Ok(file) = std::env::var("KC_ROM") {
        let rom = std::fs::read(file).unwrap();
        for (i, &b) in rom.iter().enumerate().take(0x4000) {
            t.poke(
                machine_kc85::CPU_SPACE,
                0x4000 + i as u32,
                Size::Byte,
                u32::from(b),
            );
        }
        while a < end {
            let i = t.disassemble(a - 0x8000);
            println!("{a:04X}  {i}");
            a += i.bytes.len().max(1) as u32;
        }
        return;
    }
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

/// A tape of the user's in BASIC:
/// `KC_TAPE=4|file.tap|CLOAD"NAME"|1400[|RUN]` (model, image, the line
/// typed in BASIC, fields to run, a line typed after that), the screen
/// every 100 fields; a CSAVE file is compared with the memory.
/// `KC_PPM=file.ppm` writes the last picture.
#[test]
#[ignore = "a tool: needs the user's KC85 ROMs"]
fn tape_tool() {
    let Ok(spec) = std::env::var("KC_TAPE") else {
        return;
    };
    let p: Vec<&str> = spec.split('|').collect();
    let mut m = machine(p[0], &[]);
    run(&mut *m, 400);
    type_text(&mut *m, "BASIC\n");
    run(&mut *m, 300);
    type_text(&mut *m, "\n");
    run(&mut *m, 300);
    let media = mimic_core::media::Media {
        name: p[1].into(),
        kind: mimic_core::media::MediaKind::Tape,
        backend: Box::new(media_backends::image::MemoryBackend {
            data: std::fs::read(p[1]).unwrap(),
        }),
    };
    m.media_slot(machine_kc85::SLOT_TAPE)
        .unwrap()
        .insert(media)
        .unwrap();
    type_text(&mut *m, &format!("{}\n", p[2]));
    let fields: usize = p[3].parse().unwrap();
    for f in (0..fields).step_by(100) {
        run(&mut *m, 100);
        println!("--- field {}", f + 100);
        dump(&mut *m);
    }
    // A CSAVE file: D3 D3 D3, the name, the length, then the program
    // from $0401; compare it with the memory.
    let image = std::fs::read(p[1]).unwrap();
    let blocks = machine_kc85::tape::parse(&image).unwrap();
    let data: Vec<u8> = blocks.iter().flat_map(|b| b.data).collect();
    if data.starts_with(&[0xd3; 3]) {
        let len = usize::from(data[11]) | usize::from(data[12]) << 8;
        let wrong: Vec<usize> = (0..len.min(data.len() - 13))
            .filter(|&i| byte(&mut *m, 0x401 + i as u32) != data[13 + i])
            .collect();
        println!(
            "program: {len} bytes, {} differ, first {:?}",
            wrong.len(),
            wrong.first()
        );
    }
    if let Some(then) = p.get(4) {
        type_text(&mut *m, &format!("{then}\n"));
        run(&mut *m, 600);
        dump(&mut *m);
    }
    // `KC_PPM=file`: the last picture.
    if let Ok(file) = std::env::var("KC_PPM") {
        let f = m.framebuffer();
        let mut out = format!("P6 {} {} 255\n", f.width, f.height).into_bytes();
        if let mimic_core::frame::PixelData::Indexed { pixels, palette } = f.pixels {
            for &p in pixels {
                let c = palette[usize::from(p)];
                out.extend([c.r, c.g, c.b]);
            }
        }
        std::fs::write(file, out).unwrap();
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
        let media = mimic_core::media::Media {
            name: "test.kcc".into(),
            kind: mimic_core::media::MediaKind::Tape,
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

/// A boot disc for the D004 as a raw MicroDOS dump: track 0, record 1
/// starts with "SYS" and code at `$9004` (where the boot loader jumps)
/// that leaves `$42` at the D004's `$FFF8` (in the Koppel-RAM) and halts.
fn boot_disc() -> Vec<u8> {
    let mut disc = vec![0xe5u8; 819_200];
    let sector = [
        b'S', b'Y', b'S', 0x00, // the boot loader's check
        0x3e, 0x42, 0x32, 0xf8, 0xff, // LD A,$42 / LD ($FFF8),A
        0x76, // HALT
    ];
    disc[..sector.len()].copy_from_slice(&sector);
    disc
}

/// The D004 with its ROM 2.0: `JUMP FC` (ROM in slot $FC, CAOS off)
/// clears the D004, puts its boot loader into the Koppel-RAM and starts
/// it; the loader reads the first sector of the disc in drive 0 by DMA,
/// finds "SYS" and runs it.
#[test]
#[ignore = "needs the user's KC85 ROMs"]
fn jump_fc_boots_the_d004_from_disc() {
    for model in ["3", "4", "5"] {
        let mut m = machine(model, &[("d004", "on")]);
        run(&mut *m, 400);
        let media = mimic_core::media::Media {
            name: "boot.img".into(),
            kind: mimic_core::media::MediaKind::Disk,
            backend: Box::new(media_backends::image::MemoryBackend { data: boot_disc() }),
        };
        m.media_slot(machine_kc85::SLOT_DRIVE0)
            .unwrap()
            .insert(media)
            .unwrap();
        type_text(&mut *m, "JUMP FC\n");
        let mut fields = 0;
        let marker = |m: &mut dyn Machine| {
            m.debug_target(machine_kc85::d004::D004_DEVICE)
                .unwrap()
                .peek(machine_kc85::d004::D004_SPACE, 0xfff8, Size::Byte)
                .unwrap()
        };
        while marker(&mut *m) != 0x42 {
            run(&mut *m, 10);
            fields += 10;
            assert!(fields < 1000, "KC85/{model}: the boot sector did not run");
        }
        println!("--- KC85/{model}: the boot sector ran after {fields} fields");
        dump(&mut *m);
        let d = m.debug_target(machine_kc85::d004::D004_DEVICE).unwrap();
        let regs = d.regs();
        let pc = regs.0.iter().find(|r| r.name == "PC").unwrap().value;
        assert_eq!(pc, 0x900a, "KC85/{model}: halted behind the code");
    }
}

/// BASIC at its prompt (the answer to MEMORY END given).
fn basic(model: &str) -> Box<dyn Machine> {
    let mut m = machine(model, &[]);
    run(&mut *m, 400);
    type_text(&mut *m, "BASIC\n");
    run(&mut *m, 300);
    type_text(&mut *m, "\n");
    run(&mut *m, 300);
    m
}

/// A BASIC program as tools write it to a KC-TAP file: the CSAVE header
/// (three `$D3`, the name, the length), `10 PRINT 4242` and a long REM
/// line that reaches into the second block, and zeros where CSAVE itself
/// leaves the end mark `$03`.
fn csave_tap() -> Vec<u8> {
    // Per line: link (set anew after loading), number, text, 0.
    let mut program = vec![0x0b, 0x04, 10, 0, 0x9e, b' ', b'4', b'2', b'4', b'2', 0];
    program.extend([0x01, 0x05, 20, 0, 0x8e]);
    program.extend([b'x'; 150]);
    program.extend([0, 0, 0]);
    let mut data = vec![0xd3; 3];
    data.extend(b"TEST    ");
    data.extend((program.len() as u16).to_le_bytes());
    data.extend(program);
    data.resize(256, 0);
    let mut tap = b"\xc3KC-TAPE by AF. ".to_vec();
    for (number, block) in [1, 0xff].into_iter().zip(data.chunks(128)) {
        tap.push(number);
        tap.extend(block);
    }
    tap
}

/// CLOAD reads a BASIC program, and the keyboard works again afterwards:
/// BASIC goes on reading its input from the tape until the end mark
/// CSAVE writes, which the tape adds to an image without it.
#[test]
#[ignore = "needs the user's KC85 ROMs"]
fn cload_reads_a_basic_program_and_gives_the_keyboard_back() {
    for model in ["3", "4", "5"] {
        let mut m = basic(model);
        let media = mimic_core::media::Media {
            name: "test.tap".into(),
            kind: mimic_core::media::MediaKind::Tape,
            backend: Box::new(media_backends::image::MemoryBackend { data: csave_tap() }),
        };
        m.media_slot(machine_kc85::SLOT_TAPE)
            .unwrap()
            .insert(media)
            .unwrap();
        type_text(&mut *m, "CLOAD\"TEST\"\n");
        run(&mut *m, 700);
        type_text(&mut *m, "RUN\n");
        run(&mut *m, 200);
        println!("--- KC85/{model}");
        dump(&mut *m);
        let text = screen(&mut *m);
        assert!(
            text.iter().any(|l| l.ends_with("FILE FOUND")),
            "KC85/{model}"
        );
        assert!(text.iter().any(|l| l == " 4242"), "KC85/{model}: RUN");
    }
}

/// Pasted text goes into CAOS' key cell, one character a field, and
/// arrives whole however long BASIC takes for a line.
#[test]
#[ignore = "needs the user's KC85 ROMs"]
fn pasted_text_reaches_basic_faster_than_typed() {
    let mut text = String::new();
    for i in 1..=20 {
        text += &format!("{} A=A+{}:REM pasted \"text\" (line {})\n", i * 10, i, i);
    }
    text += "210 PRINT \"sum=\";A\nRUN\n";
    for model in ["2", "3", "4", "5"] {
        // The KC85/2 has BASIC in a module only: its CAOS takes the text.
        let (mut m, text, want) = if model == "2" {
            let mut m = machine(model, &[]);
            run(&mut *m, 400);
            (m, "MODIFY 3000\n".to_string(), "3000")
        } else {
            (basic(model), text.clone(), "sum= 210")
        };
        let mut feed = mimic_core::input::TextFeed::new();
        assert_eq!(feed.push(&text), text.chars().count());
        let mut dbg = DebugHooks::new();
        let mut fields = 0;
        while feed.pending() > 0 {
            let events = feed.feed(&mut *m);
            assert!(events.is_empty(), "KC85/{model}: no keys are typed");
            m.run_frame(&mut dbg);
            fields += 1;
            assert!(fields < 5000, "KC85/{model}: the text is stuck");
        }
        run(&mut *m, 100);
        println!(
            "--- KC85/{model}: {} characters in {fields} fields",
            text.len()
        );
        dump(&mut *m);
        let screen = screen(&mut *m);
        assert!(screen.iter().any(|l| l.contains(want)), "KC85/{model}");
        // Typed, a character takes 135 ms: nearly seven fields.
        assert!(fields < text.len() * 3, "KC85/{model}: {fields} fields");
    }
}

/// Key `key` pressed twice: down at field 0 and at `again`, each time for
/// `hold` fields (in tenths of a field).
fn double_tap(m: &mut dyn Machine, key: u16, hold: u64, again: u64) {
    use mimic_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
    let start = m.now().0;
    run(m, 1);
    let field = m.now().0 - start;
    let now = m.now().0;
    for (tenths, down) in [
        (0, true),
        (hold, false),
        (again, true),
        (again + hold, false),
    ] {
        m.input(TimedInput {
            at: mimic_core::Cycle(now + field * tenths / 10),
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: MachineKey(key),
                down,
            },
        });
    }
    run(m, 40);
}

/// A doubled letter typed fast on the host's keyboard counts twice: as
/// the input mapper hands it over (down a field and a half, up four),
/// and on the keyboard's own account when the key comes again while its
/// first word is still on the way (up half a field after a field down).
#[test]
#[ignore = "needs the user's KC85 ROMs"]
fn a_doubled_letter_typed_fast_counts_twice() {
    for model in ["2", "3", "4", "5"] {
        for (hold, again) in [(15, 55), (10, 15), (2, 4), (25, 30)] {
            let mut m = machine(model, &[]);
            run(&mut *m, 400);
            // Key 44: L.
            double_tap(&mut *m, 44, hold, again);
            let text = screen(&mut *m);
            let line = text.iter().rev().find(|l| !l.trim().is_empty()).cloned();
            println!("KC85/{model} {hold}/{again}: {line:?}");
            assert!(
                line.as_deref()
                    .is_some_and(|l| l.len() == 3 && l.ends_with("LL")),
                "KC85/{model}, down {hold}, again at {again}: {line:?}"
            );
        }
    }
}
