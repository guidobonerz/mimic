//! The BBC Micro with its real ROMs (the user's, in `~/.mimic/roms/bbc`
//! or `EMU_ROM_DIR`).
//!
//!     cargo test -p emu-tests --release --test bbc_boot -- --ignored

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Machine, Size};

fn bbc() -> Box<dyn Machine> {
    bbc_model(None)
}

/// A model (`None`: the Model B; "bplus", "bplus128").
fn bbc_model(model: Option<&str>) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let config = MachineConfig {
        model: model.map(String::from),
        roms: media_backends::roms::load(&dir, "bbc").expect("ROMs"),
        ..MachineConfig::default()
    };
    emu_machines::registry().create("bbc", &config).unwrap()
}

fn run(m: &mut dyn Machine, frames: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
}

/// Mode 7's screen memory (`&7C00`, 25 rows of 40) as text.
fn teletext(m: &mut dyn Machine) -> String {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    let ram = emu_core::MemSpaceId(0);
    (0..1000)
        .map(|i| {
            let b = cpu.peek(ram, 0x7c00 + i, Size::Byte).unwrap() as u8 & 0x7f;
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                ' '
            }
        })
        .collect::<Vec<char>>()
        .chunks(40)
        .map(|r| r.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[ignore = "needs the BBC Micro ROMs"]
fn model_b_boots_to_basic() {
    let mut m = bbc();
    let m = m.as_mut();
    run(m, 150);
    let t = teletext(m);
    assert!(t.contains("BBC Computer 32K"), "{t}");
    assert!(t.contains("BASIC"), "{t}");
    assert!(t.lines().any(|l| l.starts_with('>')), "{t}");
}

/// The machine keys of a character (row × 16 + column; CAPS LOCK is on at
/// start, so letters need no SHIFT).
fn keys(c: char) -> Vec<u16> {
    const SHIFT: u16 = 0x00;
    let pos: &[(char, u16)] = &[
        ('Q', 0x10),
        ('3', 0x11),
        ('4', 0x12),
        ('5', 0x13),
        ('8', 0x15),
        ('-', 0x17),
        ('W', 0x21),
        ('E', 0x22),
        ('T', 0x23),
        ('7', 0x24),
        ('I', 0x25),
        ('9', 0x26),
        ('0', 0x27),
        ('1', 0x30),
        ('2', 0x31),
        ('D', 0x32),
        ('R', 0x33),
        ('6', 0x34),
        ('U', 0x35),
        ('O', 0x36),
        ('P', 0x37),
        ('A', 0x41),
        ('X', 0x42),
        ('F', 0x43),
        ('Y', 0x44),
        ('J', 0x45),
        ('K', 0x46),
        (':', 0x48),
        ('\n', 0x49),
        ('S', 0x51),
        ('C', 0x52),
        ('G', 0x53),
        ('H', 0x54),
        ('N', 0x55),
        ('L', 0x56),
        (';', 0x57),
        ('Z', 0x61),
        (' ', 0x62),
        ('V', 0x63),
        ('B', 0x64),
        ('M', 0x65),
        (',', 0x66),
        ('.', 0x67),
        ('/', 0x68),
    ];
    match c {
        '+' => vec![SHIFT, 0x57],
        '=' => vec![SHIFT, 0x17],
        '"' => vec![SHIFT, 0x31],
        '*' => vec![SHIFT, 0x48],
        '!' => vec![SHIFT, 0x30],
        _ => vec![pos.iter().find(|(ch, _)| *ch == c).expect("typable").1],
    }
}

fn type_text(m: &mut dyn Machine, text: &str) {
    for c in text.chars() {
        let ks = keys(c);
        for (down, list) in [
            (true, ks.clone()),
            (false, ks.iter().rev().copied().collect()),
        ] {
            let now = m.now().0;
            for k in list {
                m.input(TimedInput {
                    at: emu_core::Cycle(now),
                    target: InputTarget::Keyboard,
                    event: InputEvent::Key {
                        machine_key: MachineKey(k),
                        down,
                    },
                });
            }
            run(m, 3);
        }
    }
}

#[test]
#[ignore = "needs the BBC Micro ROMs"]
fn basic_computes_and_mode_2_fills_red() {
    let mut m = bbc();
    let m = m.as_mut();
    run(m, 150);
    type_text(m, "PRINT 1+2\n");
    run(m, 30);
    let t = teletext(m);
    assert!(t.lines().any(|l| l.trim() == "3"), "{t}");
    type_text(m, "MODE 2\n");
    run(m, 30);
    type_text(m, "GCOL 0,129:CLG\n");
    run(m, 60);
    let f = m.framebuffer();
    let emu_core::frame::PixelData::Indexed { pixels, .. } = f.pixels else {
        panic!("indexed")
    };
    let mid = pixels[(f.height as usize / 2) * f.width as usize + f.width as usize / 2];
    let red = pixels.iter().filter(|&&p| p == 1).count();
    assert_eq!(mid, 1, "red in the middle");
    assert!(red > 640 * 200, "{red} red pixels");
}

/// With `EMU_SNAPSHOT_DIR`: MODE 1 with some text, as a picture.
#[test]
#[ignore = "needs the BBC Micro ROMs"]
fn mode_1_text_picture() {
    let mut m = bbc();
    let m = m.as_mut();
    run(m, 150);
    type_text(
        m, "MODE 1
",
    );
    run(m, 30);
    type_text(
        m,
        "PRINT \"HELLO BBC MICRO\"
",
    );
    run(m, 60);
    if let Ok(d) = std::env::var("EMU_SNAPSHOT_DIR") {
        let profile = video_crt::Profile::preset("lite").unwrap();
        let png = emu_preview::encode_png_crt(&m.framebuffer(), &profile, 2);
        std::fs::write(std::path::Path::new(&d).join("bbc-mode1.png"), png).unwrap();
    }
}

/// The mode 7 test page: 25 rows of control codes and characters (BBC
/// codes, bit 7 set for the controls).
fn test_page() -> Vec<u8> {
    let mut rows: Vec<Vec<u8>> = Vec::new();
    let text = |s: &str| s.bytes().collect::<Vec<u8>>();
    let mut r = vec![0x8d, 0x81];
    r.extend(text("MIMIC "));
    r.push(0x83);
    r.extend(text("MODE 7 TEST PAGE"));
    rows.push(r.clone());
    rows.push(r);
    let mut colours = Vec::new();
    for (c, name) in ["RED", "GRN", "YEL", "BLU", "MAG", "CYN", "WHT"]
        .iter()
        .enumerate()
    {
        colours.push(0x81 + c as u8);
        colours.extend(text(name));
    }
    rows.push(colours);
    // Backgrounds: new background, black background.
    let mut bg = vec![0x84, 0x9d, 0x83];
    bg.extend(text(" YELLOW ON BLUE "));
    bg.push(0x9c);
    bg.extend(text(" BLACK AGAIN"));
    rows.push(bg);
    // Graphics: every mosaic of the first 32 codes, contiguous.
    let mut g1 = vec![0x92];
    g1.extend((0..32u8).map(|i| 0xa0 + i));
    rows.push(g1);
    let mut g2 = vec![0x96, 0x9a];
    g2.extend((0..32u8).map(|i| 0xe0 + i));
    rows.push(g2);
    // Hold graphics: a block held across colour changes.
    let mut hold = vec![0x93, 0xff, 0x9e, 0x91, 0x92, 0x94, 0x9f, 0x95, 0xff];
    hold.extend(text(" hold"));
    rows.push(hold);
    // Blast-through capitals in graphics mode.
    let mut blast = vec![0x97];
    blast.extend(text("BLAST THROUGH @ABCDEFGHIJ"));
    rows.push(blast);
    // Flashing text.
    let mut flash = vec![0x88];
    flash.extend(text("FLASH"));
    flash.push(0x89);
    flash.extend(text(" steady"));
    rows.push(flash);
    // The whole character set.
    for k in 0..3u8 {
        rows.push((0..32u8).map(|i| 0x20 + k * 32 + i).collect());
    }
    let mut page = Vec::with_capacity(1000);
    for i in 0..25 {
        let mut row = rows.get(i).cloned().unwrap_or_default();
        row.resize(40, b' ');
        row.truncate(40);
        page.extend(row);
    }
    page
}

/// FNV-1a over the picture's indices.
fn picture_hash(m: &dyn Machine) -> u64 {
    let f = m.framebuffer();
    let emu_core::frame::PixelData::Indexed { pixels, .. } = f.pixels else {
        panic!("indexed")
    };
    pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &p| {
        (h ^ u64::from(p)).wrapping_mul(0x100_0000_01b3)
    })
}

/// The test page in MODE 7 against its reference picture (checked by eye
/// against the SAA5050 data sheet's rules, then fixed by its hash).
#[test]
#[ignore = "needs the BBC Micro ROMs"]
fn mode_7_test_page_matches_its_reference() {
    let mut m = bbc();
    let m = m.as_mut();
    run(m, 150);
    // Cursor off (VDU 23,1,0;0;0;0;), then the page into screen memory.
    type_text(m, "VDU 23,1,0;0;0;0;\n");
    run(m, 20);
    let page = test_page();
    {
        let cpu = m.debug_target(DeviceId("cpu")).unwrap();
        for (i, &b) in page.iter().enumerate() {
            cpu.poke(
                emu_core::MemSpaceId(0),
                0x7c00 + i as u32,
                Size::Byte,
                u32::from(b),
            );
        }
    }
    // A field where flash is on.
    run(m, 2);
    let hash = picture_hash(m);
    if let Ok(d) = std::env::var("EMU_SNAPSHOT_DIR") {
        let profile = video_crt::Profile::preset("lite").unwrap();
        let png = emu_preview::encode_png_crt(&m.framebuffer(), &profile, 2);
        std::fs::write(std::path::Path::new(&d).join("bbc-mode7-test.png"), png).unwrap();
        eprintln!("picture hash {hash:016x}");
    }
    if let Ok(expected) = std::env::var("EMU_BBC_MODE7_HASH") {
        assert_eq!(format!("{hash:016x}"), expected);
    } else {
        assert_eq!(hash, MODE7_REFERENCE, "picture hash {hash:016x}");
    }
}

/// The reference picture's hash (2026-11-10; the picture is `bbc-mode7-test.png` with
/// `EMU_SNAPSHOT_DIR`).
const MODE7_REFERENCE: u64 = 0x83ee_d8d4_bee7_c524;

/// Run until `done` holds (checked every 10 frames), at most `frames`.
fn run_until(m: &mut dyn Machine, frames: u32, done: impl Fn(&str) -> bool) -> String {
    for _ in 0..frames / 10 {
        run(m, 10);
        let t = teletext(m);
        if done(&t) {
            return t;
        }
    }
    teletext(m)
}

/// `SAVE` onto a blank tape (the MOS asks for RECORD), rewind, `NEW`,
/// `LOAD` it back and `LIST`: the 6850, the serial ULA's motor relay and
/// the recorder at 1200 baud.
#[test]
#[ignore = "needs the BBC Micro ROMs"]
fn basic_saves_and_loads_on_tape() {
    use emu_core::media::{Media, MediaKind};
    let mut m = bbc();
    let m = m.as_mut();
    run(m, 150);
    let slot = machine_bbc::SLOT_CASSETTE;
    m.media_slot(slot)
        .unwrap()
        .insert(Media {
            name: "blank.uef".into(),
            kind: MediaKind::Tape,
            backend: Box::new(media_backends::image::MemoryBackend::default()),
        })
        .unwrap();
    // With DFS in the machine the disc is the filing system: back to tape.
    type_text(m, "*TAPE\n");
    type_text(m, "10 PRINT \"HI\"\n");
    type_text(m, "SAVE \"T\"\n");
    let t = run_until(m, 100, |t| t.contains("RECORD then RETURN"));
    assert!(t.contains("RECORD then RETURN"), "{t}");
    m.media_slot(slot).unwrap().control("record").unwrap();
    type_text(m, "\n");
    let t = run_until(m, 3000, |t| {
        t.lines().filter(|l| l.starts_with('>')).count() >= 5
    });
    assert!(t.contains("T          00 000D"), "the block counter: {t}");
    m.media_slot(slot).unwrap().control("rewind").unwrap();
    m.media_slot(slot).unwrap().control("play").unwrap();
    // The recording went back to the medium as a UEF: the block's sync
    // byte &2A and the file name follow the leader. Inserting it again
    // presses PLAY.
    {
        let mut md = m.media_slot(slot).unwrap().eject().unwrap();
        let mut d = vec![0; md.backend.len() as usize];
        md.backend.read(0, &mut d).unwrap();
        let tape = media::uef::Tape::parse(&d).unwrap();
        use media::uef::Event;
        let sync = tape
            .events
            .iter()
            .position(|e| *e == Event::Byte(0x2a))
            .expect("sync byte");
        assert!(
            matches!(tape.events[sync - 1], Event::Carrier(n) if n > 1000),
            "leader"
        );
        assert_eq!(tape.events[sync + 1], Event::Byte(b'T'));
        m.media_slot(slot).unwrap().insert(md).unwrap();
    }
    type_text(m, "NEW\n");
    type_text(m, "LOAD \"T\"\n");
    let t = run_until(m, 3000, |t| {
        t.lines().filter(|l| l.starts_with('>')).count() >= 6
    });
    assert_eq!(t.matches("T          00 000D").count(), 2, "{t}");
    type_text(m, "LIST\n");
    run(m, 20);
    let t = teletext(m);
    assert!(t.contains("   10 PRINT \"HI\""), "{t}");
}

/// `SOUND 1,-15,100,20`: the SN76489 behind the latch's sound strobe plays
/// a loud tone for a second, then the channel is quiet again.
#[test]
#[ignore = "needs the BBC Micro ROMs"]
fn basic_sound_plays_a_tone() {
    let mut m = bbc();
    let m = m.as_mut();
    run(m, 150);
    let mut out = Vec::new();
    m.audio(&mut out);
    let swing = |m: &mut dyn Machine, frames: u32| {
        let mut out = Vec::new();
        run(m, frames);
        m.audio(&mut out);
        let max = out.iter().copied().max().unwrap_or(0);
        let min = out.iter().copied().min().unwrap_or(0);
        i32::from(max) - i32::from(min)
    };
    let quiet = swing(m, 10);
    type_text(m, "SOUND 1,-15,100,20\n");
    let loud = swing(m, 20);
    // One channel at full volume: a quarter of the mixer's range.
    assert!(loud > 6000 && quiet < 1000, "quiet {quiet}, loud {loud}");
    // 20/20 s later the note is over (the samples of the wait dropped).
    run(m, 50);
    m.audio(&mut Vec::new());
    let after = swing(m, 10);
    assert!(after < 1000, "after {after}");
}

/// An 80-track DFS disc (SSD): the catalogue in sectors 0 and 1 (title,
/// boot option, 800 sectors), the files from sector 2 on. A file is a name
/// (directory `$`), load and execution address and its bytes.
fn dfs_disc(title: &str, boot: u8, files: &[(&str, u32, u32, &[u8])]) -> Vec<u8> {
    let mut d = vec![0u8; 80 * 10 * 256];
    let mut t = title.as_bytes().to_vec();
    t.resize(12, b' ');
    d[0..8].copy_from_slice(&t[0..8]);
    d[256..260].copy_from_slice(&t[8..12]);
    d[256 + 5] = (files.len() * 8) as u8;
    d[256 + 6] = boot << 4 | 3; // 800 sectors = &320
    d[256 + 7] = 0x20;
    let mut sector = 2u32;
    // The catalogue lists the files from the highest start sector down.
    for (i, (name, load, exec, data)) in files.iter().enumerate().rev() {
        let slot = files.len() - 1 - i;
        let mut n = name.as_bytes().to_vec();
        n.resize(7, b' ');
        n.push(b'$');
        d[8 + slot * 8..16 + slot * 8].copy_from_slice(&n);
        let len = data.len() as u32;
        let e = 256 + 8 + slot * 8;
        d[e..e + 8].copy_from_slice(&[
            *load as u8,
            (*load >> 8) as u8,
            *exec as u8,
            (*exec >> 8) as u8,
            len as u8,
            (len >> 8) as u8,
            ((*exec >> 16 & 3) << 6
                | (len >> 16 & 3) << 4
                | (*load >> 16 & 3) << 2
                | (sector >> 8 & 3)) as u8,
            sector as u8,
        ]);
        let at = sector as usize * 256;
        d[at..at + data.len()].copy_from_slice(data);
        sector += len.div_ceil(256);
    }
    d
}

fn bbc_with_disc(image: Vec<u8>) -> Box<dyn Machine> {
    bbc_model_with_disc(None, image)
}

fn bbc_model_with_disc(model: Option<&str>, image: Vec<u8>) -> Box<dyn Machine> {
    bbc_model_with_named_disc(model, "test.ssd", image)
}

fn bbc_model_with_named_disc(model: Option<&str>, name: &str, image: Vec<u8>) -> Box<dyn Machine> {
    use emu_core::media::{Media, MediaKind};
    let mut m = bbc_model(model);
    m.media_slot(machine_bbc::SLOT_DRIVE0)
        .expect("the DFS ROM (dfs-1.20.rom; B+: dfs-2.26.rom) in the ROM folder")
        .insert(Media {
            name: name.into(),
            kind: MediaKind::Disk,
            backend: Box::new(media_backends::image::MemoryBackend { data: image }),
        })
        .unwrap();
    m
}

/// SHIFT+BREAK with a disc whose boot option is 3 (`*EXEC !BOOT`): the
/// 8271 reads the catalogue and the file, BASIC runs the line it types.
#[test]
#[ignore = "needs the BBC Micro ROMs and DFS 1.20"]
fn shift_break_boots_a_dfs_disc() {
    let boot = b"PRINT \"BOOTED FROM DISC\"\r";
    let mut m = bbc_with_disc(dfs_disc("MIMIC", 3, &[("!BOOT", 0, 0, boot)]));
    let m = m.as_mut();
    run(m, 150);
    let t = teletext(m);
    assert!(t.contains("Acorn DFS"), "{t}");
    // SHIFT down, BREAK, SHIFT held while the MOS starts.
    let now = m.now().0;
    let key = |down| TimedInput {
        at: emu_core::Cycle(now),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(0x00),
            down,
        },
    };
    m.input(key(true));
    m.input(TimedInput {
        at: emu_core::Cycle(now),
        target: InputTarget::Panel,
        event: InputEvent::Control {
            id: emu_core::ControlId(machine_bbc::keyboard::BREAK),
            value: 1,
        },
    });
    run(m, 50);
    let now = m.now().0;
    m.input(TimedInput {
        at: emu_core::Cycle(now),
        ..key(false)
    });
    let t = run_until(m, 500, |t| t.contains("BOOTED FROM DISC"));
    assert!(t.contains("BOOTED FROM DISC"), "{t}");
}

/// `SAVE` onto a formatted, empty disc, `*CAT`, `NEW`, `LOAD`, `LIST`; the
/// catalogue written back to the image names the file.
#[test]
#[ignore = "needs the BBC Micro ROMs and DFS 1.20"]
fn basic_saves_and_loads_on_disc() {
    let mut m = bbc_with_disc(dfs_disc("EMPTY", 0, &[]));
    let m = m.as_mut();
    run(m, 150);
    type_text(m, "10 PRINT \"DISC\"\n");
    type_text(m, "SAVE \"PROG\"\n");
    run(m, 100);
    type_text(m, "*CAT\n");
    let t = run_until(m, 200, |t| t.contains("PROG"));
    assert!(t.contains("EMPTY") && t.contains("PROG"), "{t}");
    type_text(m, "NEW\n");
    type_text(m, "LOAD \"PROG\"\n");
    run(m, 100);
    type_text(m, "LIST\n");
    run(m, 20);
    let t = teletext(m);
    assert!(t.contains("   10 PRINT \"DISC\""), "{t}");
    let mut md = m
        .media_slot(machine_bbc::SLOT_DRIVE0)
        .unwrap()
        .eject()
        .unwrap();
    let mut d = vec![0; md.backend.len() as usize];
    md.backend.read(0, &mut d).unwrap();
    assert_eq!(&d[8..16], b"PROG   $", "catalogue entry");
}

/// A savestate taken while the 8271 reads the boot file: loading it and
/// running on gives the same picture as running on without it.
#[test]
#[ignore = "needs the BBC Micro ROMs and DFS 1.20"]
fn a_savestate_in_the_middle_of_a_disc_read_resumes_it() {
    let boot = b"PRINT \"BOOTED FROM DISC\"\r";
    let mut m = bbc_with_disc(dfs_disc("MIMIC", 3, &[("!BOOT", 0, 0, boot)]));
    let m = m.as_mut();
    run(m, 150);
    let now = m.now().0;
    let shift = |at, down| TimedInput {
        at: emu_core::Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(0x00),
            down,
        },
    };
    m.input(shift(now, true));
    m.input(TimedInput {
        at: emu_core::Cycle(now),
        target: InputTarget::Panel,
        event: InputEvent::Control {
            id: emu_core::ControlId(machine_bbc::keyboard::BREAK),
            value: 1,
        },
    });
    // The catalogue and the file are read in the first fields after BREAK.
    run(m, 3);
    let state = m.save_state();
    run(m, 200);
    let after = picture_hash(m);
    assert!(teletext(m).contains("BOOTED FROM DISC"));
    m.load_state(&state).unwrap();
    run(m, 200);
    assert_eq!(picture_hash(m), after);
}

/// SHIFT+BREAK (shift held while BREAK goes down and the MOS starts, let
/// go 50 fields later: BREAK empties the input queue, so the release is
/// queued after it).
fn shift_break(m: &mut dyn Machine) {
    let now = m.now().0;
    let shift = |at, down| TimedInput {
        at: emu_core::Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(0x00),
            down,
        },
    };
    m.input(shift(now, true));
    m.input(TimedInput {
        at: emu_core::Cycle(now),
        target: InputTarget::Panel,
        event: InputEvent::Control {
            id: emu_core::ControlId(machine_bbc::keyboard::BREAK),
            value: 1,
        },
    });
    run(m, 50);
    let now = m.now().0;
    m.input(shift(now, false));
}

/// The B+ (MOS 2.00, DFS 2.26 on the 1770) boots to BASIC.
#[test]
#[ignore = "needs the BBC B+ ROMs"]
fn bplus_boots_to_basic() {
    let mut m = bbc_model(Some("bplus"));
    let m = m.as_mut();
    run(m, 150);
    let t = teletext(m);
    assert!(t.contains("Acorn OS 64K"), "{t}");
    assert!(t.contains("Acorn 1770 DFS"), "{t}");
    assert!(t.contains("BASIC"), "{t}");
    assert!(t.lines().any(|l| l.starts_with('>')), "{t}");
}

/// MODE 135 (mode 7 with shadow RAM): the text goes to the shadow RAM,
/// main memory at &7C00 stays as it was, HIMEM is &8000 (32768).
#[test]
#[ignore = "needs the BBC B+ ROMs"]
fn bplus_shadow_mode_writes_the_shadow_ram() {
    let mut m = bbc_model(Some("bplus"));
    let m = m.as_mut();
    run(m, 150);
    type_text(m, "MODE 135\n");
    run(m, 10);
    type_text(m, "PRINT HIMEM\n");
    run(m, 20);
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    let shadow: String = (0..1000)
        .map(|i| {
            let b = cpu
                .peek(machine_bbc::SHADOW_SPACE, 0x7c00 + i, Size::Byte)
                .unwrap() as u8
                & 0x7f;
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                ' '
            }
        })
        .collect();
    assert!(shadow.contains("32768"), "{shadow}");
    let main = teletext(m);
    assert!(!main.contains("PRINT HIMEM"), "{main}");
}

/// SHIFT+BREAK on the B+: DFS 2.26 reads the single-density disc through
/// the 1770 and runs !BOOT; SAVE and LOAD work as on the Model B.
#[test]
#[ignore = "needs the BBC B+ ROMs"]
fn bplus_boots_and_saves_on_the_1770() {
    let boot = b"PRINT \"BOOTED FROM DISC\"\r";
    let mut m = bbc_model_with_disc(Some("bplus"), dfs_disc("PLUS", 3, &[("!BOOT", 0, 0, boot)]));
    let m = m.as_mut();
    run(m, 150);
    shift_break(m);
    let t = run_until(m, 500, |t| t.contains("BOOTED FROM DISC"));
    assert!(t.contains("BOOTED FROM DISC"), "{t}");
    type_text(m, "10 PRINT \"PLUS\"\n");
    type_text(m, "SAVE \"PROG\"\n");
    run(m, 100);
    type_text(m, "NEW\n");
    type_text(m, "LOAD \"PROG\"\n");
    run(m, 100);
    type_text(m, "LIST\n");
    run(m, 20);
    let t = teletext(m);
    assert!(t.contains("   10 PRINT \"PLUS\""), "{t}");
}

/// The B+ 128 (64 KB sideways RAM in slots 0, 1, 12, 13) boots like the
/// B+.
#[test]
#[ignore = "needs the BBC B+ ROMs"]
fn bplus128_boots_to_basic() {
    let mut m = bbc_model(Some("bplus128"));
    let m = m.as_mut();
    run(m, 150);
    let t = teletext(m);
    assert!(
        t.contains("Acorn OS 128K"),
        "the MOS finds the sideways RAM: {t}"
    );
    assert!(t.contains("Acorn 1770 DFS"), "{t}");
    assert!(t.lines().any(|l| l.starts_with('>')), "{t}");
}

/// The Master 128 (MOS 3.20, the CMOS RAM as configured) boots to BASIC
/// IV.
#[test]
#[ignore = "needs the BBC Master ROMs (bbc/master320)"]
fn master_boots_to_basic() {
    let mut m = bbc_model(Some("master"));
    let m = m.as_mut();
    run(m, 200);
    let t = teletext(m);
    assert!(t.contains("Acorn MOS"), "{t}");
    assert!(t.contains("Acorn 1770 DFS"), "{t}");
    assert!(t.contains("BASIC"), "{t}");
    assert!(t.lines().any(|l| l.starts_with('>')), "{t}");
}

/// The checksum of an ADFS old map sector (bytes 0–254, the carry added
/// back in, from the end down).
fn adfs_map_sum(b: &[u8]) -> u8 {
    let mut sum = 255u32;
    for i in (0..255).rev() {
        if sum > 255 {
            sum = (sum & 255) + 1;
        }
        sum += u32::from(b[i]);
    }
    sum as u8
}

/// An empty ADFS L disc (640 KB, 2560 sectors): the free space map in
/// sectors 0 and 1, the root directory "$" in sectors 2–6, the rest free.
fn adfs_l_disc() -> Vec<u8> {
    let mut d = vec![0u8; 655_360];
    let total = 2560u32;
    let (free_start, free_len) = (7u32, total - 7);
    d[0..3].copy_from_slice(&free_start.to_le_bytes()[..3]);
    d[0xfc..0xff].copy_from_slice(&total.to_le_bytes()[..3]);
    d[0xff] = adfs_map_sum(&d[0..256]);
    let s1 = 256;
    d[s1..s1 + 3].copy_from_slice(&free_len.to_le_bytes()[..3]);
    d[s1 + 0xfb] = 0x12;
    d[s1 + 0xfc] = 0x34;
    d[s1 + 0xfe] = 3;
    d[s1 + 0xff] = adfs_map_sum(&d[s1..s1 + 256]);
    let dir = 2 * 256;
    d[dir + 1..dir + 5].copy_from_slice(b"Hugo");
    let tail = dir + 0x4cb;
    d[tail] = 0;
    d[tail + 1] = b'$';
    d[tail + 2] = 0x0d;
    d[tail + 11..tail + 14].copy_from_slice(&[2, 0, 0]);
    d[tail + 14] = b'$';
    d[tail + 15] = 0x0d;
    d[dir + 0x4fb..dir + 0x4ff].copy_from_slice(b"Hugo");
    d
}

/// DFS 2.24 on the Master's 1770 (&FE24/&FE28): SHIFT+BREAK runs !BOOT,
/// SAVE and LOAD work.
#[test]
#[ignore = "needs the BBC Master ROMs (bbc/master320)"]
fn master_boots_and_saves_on_dfs() {
    let boot = b"PRINT \"BOOTED FROM DISC\"\r";
    let mut m = bbc_model_with_disc(
        Some("master"),
        dfs_disc("MASTER", 3, &[("!BOOT", 0, 0, boot)]),
    );
    let m = m.as_mut();
    run(m, 200);
    shift_break(m);
    let t = run_until(m, 500, |t| t.contains("BOOTED FROM DISC"));
    assert!(t.contains("BOOTED FROM DISC"), "{t}");
    type_text(m, "10 PRINT \"MASTER\"\n");
    type_text(m, "SAVE \"PROG\"\n");
    run(m, 100);
    type_text(m, "NEW\n");
    type_text(m, "LOAD \"PROG\"\n");
    run(m, 100);
    type_text(m, "LIST\n");
    run(m, 20);
    let t = teletext(m);
    assert!(t.contains("   10 PRINT \"MASTER\""), "{t}");
}

/// ADFS on an L disc (double density, both sides): *ADFS mounts it, SAVE,
/// *CAT, LOAD.
#[test]
#[ignore = "needs the BBC Master ROMs (bbc/master320)"]
fn master_saves_and_loads_on_adfs() {
    let mut m = bbc_model_with_named_disc(Some("master"), "test.adl", adfs_l_disc());
    let m = m.as_mut();
    run(m, 200);
    type_text(m, "*ADFS\n");
    run(m, 200);
    type_text(m, "10 PRINT \"ADFS\"\n");
    type_text(m, "SAVE \"PROG\"\n");
    run(m, 200);
    type_text(m, "*CAT\n");
    run(m, 200);
    let t = teletext(m);
    if let Ok(d) = std::env::var("EMU_SNAPSHOT_DIR") {
        let profile = video_crt::Profile::preset("lite").unwrap();
        let png = emu_preview::encode_png_crt(&m.framebuffer(), &profile, 2);
        std::fs::write(std::path::Path::new(&d).join("bbc-master-adfs.png"), png).unwrap();
    }
    assert!(t.contains("PROG"), "{t}");
    type_text(m, "NEW\n");
    type_text(m, "LOAD \"PROG\"\n");
    run(m, 200);
    type_text(m, "LIST\n");
    run(m, 20);
    let t = teletext(m);
    assert!(t.contains("   10 PRINT \"ADFS\""), "{t}");
}

/// MODE 135 on the Master: the VDU driver writes LYNNE (ACCCON E), the
/// display shows it (D), HIMEM is &8000.
#[test]
#[ignore = "needs the BBC Master ROMs (bbc/master320)"]
fn master_shadow_mode_writes_lynne() {
    let mut m = bbc_model(Some("master"));
    let m = m.as_mut();
    run(m, 200);
    type_text(m, "MODE 135\n");
    run(m, 10);
    type_text(m, "PRINT HIMEM\n");
    run(m, 20);
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    let lynne: String = (0..1000)
        .map(|i| {
            let b = cpu
                .peek(machine_bbc::SHADOW_SPACE, 0x7c00 + i, Size::Byte)
                .unwrap() as u8
                & 0x7f;
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                ' '
            }
        })
        .collect();
    assert!(lynne.contains("32768"), "{lynne}");
    assert!(!teletext(m).contains("PRINT HIMEM"));
}

/// The clock: `--option rtc=…` sets it, *TIME reads it through the slow
/// data bus (MOS 3.20 knows no century: 26 shows as 1926).
#[test]
#[ignore = "needs the BBC Master ROMs (bbc/master320)"]
fn master_time_comes_from_the_clock() {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let mut config = MachineConfig {
        model: Some("master".into()),
        roms: media_backends::roms::load(&dir, "bbc").expect("ROMs"),
        ..MachineConfig::default()
    };
    config
        .options
        .insert("rtc".into(), "1987-06-15T10:20:30".into());
    let mut m = emu_machines::registry().create("bbc", &config).unwrap();
    let m = m.as_mut();
    run(m, 200);
    type_text(m, "*TIME\n");
    run(m, 20);
    let t = teletext(m);
    assert!(t.contains("Mon,15 Jun 1987.10:20"), "{t}");
}
