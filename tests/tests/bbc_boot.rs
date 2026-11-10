//! The BBC Micro with its real ROMs (the user's, in `~/.mimic/roms/bbc`
//! or `EMU_ROM_DIR`).
//!
//!     cargo test -p emu-tests --release --test bbc_boot -- --ignored

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Machine, Size};

fn bbc() -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let config = MachineConfig {
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
    type_text(m, "10 PRINT \"HI\"\n");
    type_text(m, "SAVE \"T\"\n");
    let t = run_until(m, 100, |t| t.contains("RECORD then RETURN"));
    assert!(t.contains("RECORD then RETURN"), "{t}");
    m.media_slot(slot).unwrap().control("record").unwrap();
    type_text(m, "\n");
    let t = run_until(m, 3000, |t| {
        t.lines().filter(|l| l.starts_with('>')).count() >= 4
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
        t.lines().filter(|l| l.starts_with('>')).count() >= 5
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
