//! The TI-99/4A with its real ROMs (user-supplied in `~/.mimic/roms/ti99`
//! or `EMU_ROM_DIR`): the title screen, the menu, TI BASIC, a cartridge.
//!
//!     cargo test -p emu-tests --release --test ti99_boot -- --ignored

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Machine, Size};

fn ti99(options: &[(&str, &str)]) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "ti99").expect("ROMs"),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    emu_machines::registry().create("ti99", &config).unwrap()
}

fn run(m: &mut dyn Machine, frames: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
}

/// The name table (VDP register 2 × >400) as text: TI BASIC stores
/// characters with an offset of >60, the title screen without.
fn screen(m: &mut dyn Machine) -> String {
    let vram = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == "vram")
        .unwrap()
        .id;
    let base = m
        .chips()
        .into_iter()
        .find(|c| c.name == "vdp")
        .and_then(|c| c.regs.into_iter().find(|r| r.name == "R2"))
        .map_or(0, |r| (r.value & 0x0f) * 0x400);
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0..768)
        .map(|i| {
            let b = cpu.peek(vram, base + i, Size::Byte).unwrap() as u8;
            let c = if b >= 0x80 { b - 0x60 } else { b };
            if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                ' '
            }
        })
        .collect::<Vec<char>>()
        .chunks(32)
        .map(|row| row.iter().collect::<String>())
        .collect::<Vec<String>>()
        .join("\n")
}

/// The machine keys of a character (see `machine-ti99::keyboard`).
fn keys(c: char) -> Vec<u16> {
    const SHIFT: u16 = 5;
    let pos: &[(char, u16)] = &[
        ('=', 0),
        (' ', 1),
        ('\n', 2),
        ('.', 8),
        ('L', 9),
        ('O', 10),
        ('9', 11),
        ('2', 12),
        ('S', 13),
        ('W', 14),
        ('X', 15),
        (',', 16),
        ('K', 17),
        ('I', 18),
        ('8', 19),
        ('3', 20),
        ('D', 21),
        ('E', 22),
        ('C', 23),
        ('M', 24),
        ('J', 25),
        ('U', 26),
        ('7', 27),
        ('4', 28),
        ('F', 29),
        ('R', 30),
        ('V', 31),
        ('N', 32),
        ('H', 33),
        ('Y', 34),
        ('6', 35),
        ('5', 36),
        ('G', 37),
        ('T', 38),
        ('B', 39),
        ('/', 40),
        (';', 41),
        ('P', 42),
        ('0', 43),
        ('1', 44),
        ('A', 45),
        ('Q', 46),
        ('Z', 47),
    ];
    if c == '+' {
        return vec![SHIFT, 0];
    }
    if c == '*' {
        return vec![SHIFT, 19];
    }
    let k = pos.iter().find(|(ch, _)| *ch == c).expect("typable").1;
    vec![k]
}

/// Type `text`: each key held for two fields, then released for two.
fn type_text(m: &mut dyn Machine, text: &str) {
    for c in text.chars() {
        let ks = keys(c);
        let now = m.now().0;
        for &k in &ks {
            m.input(TimedInput {
                at: emu_core::Cycle(now),
                target: InputTarget::Keyboard,
                event: InputEvent::Key {
                    machine_key: MachineKey(k),
                    down: true,
                },
            });
        }
        run(m, 3);
        let now = m.now().0;
        for &k in ks.iter().rev() {
            m.input(TimedInput {
                at: emu_core::Cycle(now),
                target: InputTarget::Keyboard,
                event: InputEvent::Key {
                    machine_key: MachineKey(k),
                    down: false,
                },
            });
        }
        run(m, 3);
    }
}

#[test]
#[ignore = "needs the TI-99/4A ROMs"]
fn ti_basic_starts_and_computes() {
    let mut m = ti99(&[]);
    let m = m.as_mut();
    run(m, 120);
    let title = screen(m);
    assert!(title.contains("TEXAS INSTRUMENTS"), "{title}");
    assert!(title.contains("READY-PRESS ANY KEY TO BEGIN"), "{title}");
    type_text(m, " ");
    run(m, 60);
    let menu = screen(m);
    assert!(menu.contains("1 FOR TI BASIC"), "{menu}");
    type_text(m, "1");
    run(m, 120);
    let basic = screen(m);
    assert!(basic.contains("TI BASIC READY"), "{basic}");
    type_text(m, "PRINT 1+2\n");
    run(m, 60);
    let out = screen(m);
    assert!(
        out.lines().any(|l| l.trim() == "3"),
        "the sum on its own line:\n{out}"
    );
    // A small program: squares.
    run(m, 30);
    for line in [
        "10 FOR I=1 TO 5\n",
        "20 PRINT I,I*I\n",
        "30 NEXT I\n",
        "RUN\n",
    ] {
        type_text(m, line);
        run(m, 30);
    }
    run(m, 120);
    let out = screen(m);
    assert!(out.contains(" 5             25"), "{out}");
    snapshot(m, "ti99-basic.png");
}

/// With `EMU_SNAPSHOT_DIR` set: the picture through the "lite" monitor
/// profile (the README's screenshots).
fn snapshot(m: &dyn Machine, name: &str) {
    if let Ok(dir) = std::env::var("EMU_SNAPSHOT_DIR") {
        let profile = video_crt::Profile::preset("lite").unwrap();
        let png = emu_preview::encode_png_crt(&m.framebuffer(), &profile, 2);
        std::fs::write(std::path::Path::new(&dir).join(name), png).unwrap();
    }
}

/// A cartridge from the ROM folder's `ti99` dumps (C and G next to each
/// other, opened as one bundle), started from the menu.
#[test]
#[ignore = "needs the TI-99/4A ROMs and the Parsec cartridge dumps"]
fn a_cartridge_appears_in_the_menu_and_runs() {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let path = ["ti99/PARSECC.BIN", "PARSECC.BIN"]
        .iter()
        .map(|p| dir.join(p))
        .find(|p| p.exists())
        .expect("PARSECC.BIN");
    let media = media_backends::image::open_cart(&path).unwrap();
    assert!(media.name.ends_with(".ticart"), "bundled: {}", media.name);
    let mut m = ti99(&[]);
    let m = m.as_mut();
    let slot = m.media_slots()[0].id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    run(m, 120);
    type_text(m, " ");
    run(m, 60);
    let menu = screen(m);
    assert!(menu.contains("2 FOR PARSEC"), "{menu}");
    type_text(m, "2");
    run(m, 300);
    assert!(!screen(m).contains("FOR PARSEC"), "left the menu");
    // Into the game.
    type_text(m, " ");
    run(m, 400);
    snapshot(m, "ti99-parsec.png");
}

/// Bytes shared with the test, writable (a blank cassette).
#[derive(Clone)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl emu_core::media::MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), emu_core::media::MediaError> {
        let d = self.0.lock().unwrap();
        buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), emu_core::media::MediaError> {
        let mut d = self.0.lock().unwrap();
        let o = offset as usize;
        let n = d.len().max(o + data.len());
        d.resize(n, 0);
        d[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), emu_core::media::MediaError> {
        Ok(())
    }
}

/// Run until `text` is on the screen (at most `frames` fields).
fn wait_for(m: &mut dyn Machine, text: &str, frames: u32) -> bool {
    for _ in 0..frames {
        run(m, 1);
        if screen(m).contains(text) {
            // Let the ROM get to its keyboard scan.
            run(m, 30);
            return true;
        }
    }
    false
}

fn tape_key(m: &mut dyn Machine, key: &str) {
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "cassette")
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().control(key).unwrap();
}

/// SAVE CS1 onto a blank tape, then NEW and OLD CS1: the program is back.
#[test]
#[ignore = "needs the TI-99/4A ROMs"]
fn ti_basic_saves_to_and_loads_from_cassette() {
    let mut m = ti99(&[]);
    let m = m.as_mut();
    run(m, 120);
    type_text(m, " ");
    run(m, 60);
    type_text(m, "1");
    assert!(wait_for(m, "TI BASIC READY", 200));
    run(m, 60);
    let store = Shared(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "cassette")
        .unwrap()
        .id;
    m.media_slot(slot)
        .unwrap()
        .insert(emu_core::media::Media {
            name: "blank.wav".into(),
            kind: emu_core::media::MediaKind::Tape,
            backend: Box::new(store.clone()),
        })
        .unwrap();
    tape_key(m, "stop");
    type_text(m, "10 PRINT 42\n");
    run(m, 30);
    type_text(m, "SAVE CS1\n");
    assert!(wait_for(m, "REWIND CASSETTE", 120), "{}", screen(m));
    type_text(m, "\n");
    assert!(wait_for(m, "PRESS CASSETTE RECORD", 120), "{}", screen(m));
    tape_key(m, "record");
    type_text(m, "\n");
    assert!(
        wait_for(m, "PRESS CASSETTE STOP", 3000),
        "recording:\n{}",
        screen(m)
    );
    tape_key(m, "stop");
    type_text(m, "\n");
    assert!(wait_for(m, "CHECK TAPE", 120), "{}", screen(m));
    type_text(m, "N\n");
    run(m, 60);
    let recorded = store.0.lock().unwrap().len();
    assert!(recorded > 100_000, "a recording: {recorded} bytes");
    type_text(m, "NEW\n");
    run(m, 60);
    type_text(m, "OLD CS1\n");
    assert!(wait_for(m, "REWIND CASSETTE", 120), "{}", screen(m));
    tape_key(m, "rewind");
    type_text(m, "\n");
    assert!(wait_for(m, "PRESS CASSETTE PLAY", 120), "{}", screen(m));
    tape_key(m, "play");
    type_text(m, "\n");
    assert!(
        wait_for(m, "PRESS CASSETTE STOP", 3000),
        "reading:\n{}",
        screen(m)
    );
    let read = screen(m);
    assert!(read.contains("DATA OK"), "{read}");
    tape_key(m, "stop");
    type_text(m, "\n");
    run(m, 60);
    type_text(m, "LIST\n");
    run(m, 60);
    let list = screen(m);
    assert!(list.contains("10 PRINT 42"), "{list}");
}

/// SAVE DSK1.TEST onto a blank initialised disk, then NEW and OLD: the
/// program is back, and the disk image holds the file.
#[test]
#[ignore = "needs the TI-99/4A ROMs and the disk controller's DSR ROM"]
fn ti_basic_saves_to_and_loads_from_disk() {
    let mut m = ti99(&[("disk", "on")]);
    let m = m.as_mut();
    let store = Shared(std::sync::Arc::new(std::sync::Mutex::new(
        media::dmk::blank_ti_dsk("BLANK", 1),
    )));
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "DSK1")
        .unwrap()
        .id;
    m.media_slot(slot)
        .unwrap()
        .insert(emu_core::media::Media {
            name: "blank.dsk".into(),
            kind: emu_core::media::MediaKind::Disk,
            backend: Box::new(store.clone()),
        })
        .unwrap();
    run(m, 120);
    type_text(m, " ");
    run(m, 60);
    type_text(m, "1");
    assert!(wait_for(m, "TI BASIC READY", 200));
    run(m, 60);
    type_text(m, "10 PRINT 42\n");
    run(m, 30);
    type_text(m, "SAVE DSK1.TEST\n");
    run(m, 600);
    let after = screen(m);
    assert!(!after.contains("ERROR"), "{after}");
    m.media_slot(slot).unwrap().flush().unwrap();
    let image = store.0.lock().unwrap().clone();
    // The file directory (sector 1) points at the file's header sector.
    let fdir = u16::from_be_bytes([image[256], image[257]]) as usize;
    assert!(fdir >= 2, "a file in the directory");
    assert_eq!(&image[fdir * 256..fdir * 256 + 4], b"TEST", "its name");
    type_text(m, "NEW\n");
    run(m, 60);
    type_text(m, "OLD DSK1.TEST\n");
    run(m, 600);
    type_text(m, "LIST\n");
    run(m, 60);
    let list = screen(m);
    assert!(list.contains("10 PRINT 42"), "{list}");
}

/// The speech synthesizer's TALKING bit (the machine's "speech" chip).
fn talking(m: &dyn Machine) -> bool {
    m.chips()
        .into_iter()
        .find(|c| c.name == "speech")
        .and_then(|c| c.regs.into_iter().find(|r| r.name == "TALKING"))
        .is_some_and(|r| r.value != 0)
}

/// Parsec speaks through the Speech Synthesizer when the game starts
/// ("Welcome aboard, Captain"); with `EMU_SNAPSHOT_DIR` the sound is saved
/// as `ti99-parsec-speech.wav`.
#[test]
#[ignore = "needs the TI-99/4A ROMs, the speech ROM and the Parsec dumps"]
fn parsec_speaks() {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let path = ["ti99/PARSECC.BIN", "PARSECC.BIN"]
        .iter()
        .map(|p| dir.join(p))
        .find(|p| p.exists())
        .expect("PARSECC.BIN");
    let media = media_backends::image::open_cart(&path).unwrap();
    let mut m = ti99(&[("speech", "on")]);
    let m = m.as_mut();
    let slot = m.media_slots()[0].id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let mut audio = Vec::new();
    let mut spoke = 0;
    let mut dbg = DebugHooks::new();
    let mut step = |m: &mut dyn Machine, frames: u32, audio: &mut Vec<i16>, spoke: &mut u32| {
        for _ in 0..frames {
            m.run_frame(&mut dbg);
            m.audio(audio);
            if talking(m) {
                *spoke += 1;
            }
        }
    };
    step(m, 120, &mut audio, &mut spoke);
    type_text(m, " ");
    step(m, 60, &mut audio, &mut spoke);
    type_text(m, "2");
    step(m, 300, &mut audio, &mut spoke);
    type_text(m, " ");
    step(m, 600, &mut audio, &mut spoke);
    if let Ok(d) = std::env::var("EMU_SNAPSHOT_DIR") {
        let mut wav = Vec::new();
        let data_len = (audio.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for v in &audio {
            wav.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(std::path::Path::new(&d).join("ti99-parsec-speech.wav"), wav).unwrap();
    }
    assert!(spoke > 20, "the synthesizer talked for {spoke} fields");
}
