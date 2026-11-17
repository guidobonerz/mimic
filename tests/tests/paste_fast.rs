//! Pasted text goes straight into the operating system's keyboard buffer
//! (`Machine::paste_chars`) on real machines with real ROMs (user-supplied
//! via `EMU_ROM_DIR`, else `~/.mimic/roms`): a program is pasted through
//! the session as a frontend does it, BASIC must hold it line for line,
//! and the machine's own time for it is compared with typing the keys.
//!
//!     cargo test -p mimic-tests --release --test paste_fast -- --ignored --nocapture

use mimic_core::registry::MachineConfig;
use mimic_debug::protocol::{Command, Request};
use mimic_debug::Session;

struct Setup {
    machine: &'static str,
    model: Option<&'static str>,
    options: &'static [(&'static str, &'static str)],
    /// Cycles to the BASIC prompt.
    boot: u64,
    /// Typed (with keys) once the machine is up, with cycles to wait.
    first: &'static [(&'static str, u64)],
    /// How many times faster than the keys the paste must be.
    faster: u64,
}

impl Setup {
    const fn new(machine: &'static str, model: Option<&'static str>, boot: u64) -> Self {
        Self {
            machine,
            model,
            options: &[],
            boot,
            first: &[],
            faster: 10,
        }
    }
}

/// Lines no dialect changes but for its blanks: 40 of them, some long.
fn program() -> Vec<(u64, String)> {
    (1..=40)
        .map(|i| {
            let text = match i % 4 {
                0 => format!("PRINT\"LINE {i}: THE QUICK BROWN FOX JUMPS OVER 13 LAZY DOGS\""),
                1 => format!("A={i}:B=A*2+1"),
                2 => format!("PRINT\"{i}\";A;B"),
                _ => "REM".to_string(),
            };
            (10 * i, text)
        })
        .collect()
}

fn run(s: &mut Session, cycles: u64) {
    let end = s.machine().now().0 + cycles;
    s.set_running(true);
    while s.machine().now().0 < end && s.is_running() {
        s.run_slice();
    }
    assert!(s.is_running(), "stopped");
    s.set_running(false);
    s.take_events();
}

/// The program in BASIC's memory, blanks removed.
fn listed(s: &mut Session) -> Vec<(u64, String)> {
    let v = s.handle(Request {
        id: 1,
        cmd: Command::BasicList {
            from: None,
            to: None,
        },
    });
    assert_eq!(v["ok"], true, "{v}");
    v["result"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let text = l["text"].as_str().unwrap().replace(' ', "");
            (l["line"].as_u64().unwrap(), text)
        })
        .collect()
}

/// BBC BASIC's program read from memory (no debugger for it yet): from
/// PAGE (its high byte at `$18`) lines of `$0D`, number high and low,
/// length, text; `$FF` behind the last `$0D`. PRINT is `$F1`, REM `$F4`.
fn listed_bbc(s: &mut Session) -> Vec<(u64, String)> {
    use mimic_core::{MemSpaceId, Size};
    let m = s.machine_mut();
    let cpu = m.devices()[0];
    let t = m.debug_target(cpu).unwrap();
    let peek = |a: u32| t.peek(MemSpaceId(0), a, Size::Byte).unwrap() as u8;
    let mut at = u32::from(peek(0x18)) << 8;
    let mut lines = Vec::new();
    while peek(at) == 0x0d && peek(at + 1) != 0xff && lines.len() < 1000 {
        let number = u64::from(peek(at + 1)) << 8 | u64::from(peek(at + 2));
        let len = u32::from(peek(at + 3)).max(4);
        let text: String = (at + 4..at + len)
            .map(|a| match peek(a) {
                0xf1 => "PRINT".to_string(),
                0xf4 => "REM".to_string(),
                b => char::from(b).to_string(),
            })
            .collect();
        lines.push((number, text.replace(' ', "")));
        at += len;
    }
    lines
}

/// Paste the program and return (cycles it took, cycles the keys take).
fn paste(setup: &Setup) -> (u64, u64) {
    let name = format!("{} {}", setup.machine, setup.model.unwrap_or(""));
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, setup.machine).expect("ROMs"),
        model: setup.model.map(str::to_string),
        options: setup
            .options
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..MachineConfig::default()
    };
    let m = mimic_machines::registry()
        .create(setup.machine, &config)
        .unwrap();
    let mut s = Session::new(m, setup.machine);
    run(&mut s, setup.boot);
    for (text, wait) in setup.first {
        let now = s.machine().now();
        let (events, end) = s.machine().typed_keys(text, now).unwrap();
        for e in events {
            s.input(e);
        }
        run(&mut s, end.0 - now.0 + wait);
    }

    let program = program();
    let text: String = program
        .iter()
        .map(|(n, t)| format!("{n} {t}\r\n"))
        .collect();
    let start = s.machine().now();
    let keys = s.machine().typed_keys(&text, start).unwrap().1 .0 - start.0;
    assert!(
        s.machine_mut().paste_chars("").is_some(),
        "{name}: no keyboard buffer"
    );
    s.type_text(&text).unwrap();
    s.set_running(true);
    let mut slices = 0;
    while s.typing() {
        s.run_slice();
        slices += 1;
        assert!(slices < 20_000, "{name}: the paste never ends");
    }
    s.set_running(false);
    let took = s.machine().now().0 - start.0;
    // The system still has the last characters to read.
    run(&mut s, setup.boot.min(3_000_000));

    let want: Vec<(u64, String)> = program
        .into_iter()
        .map(|(n, t)| (n, t.replace(' ', "")))
        .collect();
    let got = if setup.machine == "bbc" {
        listed_bbc(&mut s)
    } else {
        listed(&mut s)
    };
    assert_eq!(got, want, "{name}");
    println!(
        "{name}: {} characters in {took} cycles, keys {keys}: {:.0} times faster",
        text.chars().count(),
        keys as f64 / took as f64
    );
    assert!(
        took * setup.faster < keys,
        "{name}: {took} against {keys} cycles"
    );
    (took, keys)
}

#[test]
#[ignore = "needs Commodore ROMs"]
fn commodore() {
    for setup in [
        Setup::new("c64", None, 3_000_000),
        Setup::new("vic20", None, 3_000_000),
        // BASIC 1.0 and 2.0 show a character only in the picture's blank
        // (some 40 a second), whatever the buffer holds.
        Setup {
            faster: 2,
            ..Setup::new("pet", Some("2001"), 3_000_000)
        },
        Setup {
            faster: 2,
            ..Setup::new("pet", Some("3032"), 3_000_000)
        },
        Setup::new("pet", Some("4032"), 3_000_000),
        Setup::new("pet", Some("8032"), 3_000_000),
        Setup::new("plus4", None, 3_000_000),
        Setup::new("c16", None, 3_000_000),
        Setup::new("c128", None, 6_000_000),
        Setup {
            first: &[("GO64\n", 600_000), ("Y\n", 6_000_000)],
            ..Setup::new("c128", None, 6_000_000)
        },
    ] {
        paste(&setup);
    }
}

#[test]
#[ignore = "needs an MSX BIOS with BASIC"]
fn msx() {
    for (model, boot) in [("msx1", 15_000_000), ("msx2", 43_000_000)] {
        paste(&Setup {
            options: &[("disk", "off")],
            ..Setup::new("msx", Some(model), boot)
        });
    }
}

#[test]
#[ignore = "needs BBC ROMs"]
fn bbc() {
    // The Master takes longer to its prompt.
    for (model, boot) in [
        ("b", 8_000_000),
        ("bplus", 8_000_000),
        ("master", 20_000_000),
    ] {
        paste(&Setup::new("bbc", Some(model), boot));
    }
}

/// One character a frame: the OS has a single place for a key.
#[test]
#[ignore = "needs Atari ROMs"]
fn atari8() {
    for model in ["800xl", "130xe", "800"] {
        paste(&Setup {
            faster: 3,
            ..Setup::new("atari8", Some(model), 9_000_000)
        });
    }
}

/// One character a frame: the keyboard's latch holds a single key.
#[test]
#[ignore = "needs Apple II ROMs"]
fn apple2() {
    for model in ["iiplus", "iie-unenhanced", "iie"] {
        paste(&Setup {
            options: &[("slot6", "none")],
            faster: 2,
            ..Setup::new("apple2", Some(model), 3_000_000)
        });
    }
}
