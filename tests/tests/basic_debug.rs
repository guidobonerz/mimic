//! Phase 35 gate: the BASIC debugger on real machines with real ROMs
//! (user-supplied via `EMU_ROM_DIR`, else `~/.mimic/roms`). Each test types
//! a program in, then works on it with the commands of the native protocol.
//!
//!     cargo test -p emu-tests --release --test basic_debug -- --ignored

use emu_core::input::TimedInput;
use emu_core::registry::MachineConfig;
use emu_debug::protocol::{Command, Request};
use emu_debug::Session;
use serde_json::{json, Value};

/// Typed without blanks, so the listing is known to the character.
const PROGRAM: &[&str] = &[
    "10 A=5:GOSUB100:A=A-2",
    "20 FORI=1TO3:B%=B%+10:NEXT",
    "30 C$=\"AB\"+\"C\"",
    "40 GOTO10",
    "100 A=A+1:RETURN",
];

type Typist = Box<dyn Fn(&str, u64) -> (Vec<TimedInput>, u64)>;

struct Gate {
    s: Session,
    typist: Typist,
    name: String,
}

impl Gate {
    fn new(machine: &str, model: Option<&str>, typist: Typist) -> Self {
        let dir = media_backends::roms::env_dir().expect("no ROM folder");
        let config = MachineConfig {
            roms: media_backends::roms::load(&dir, machine).expect("ROMs"),
            model: model.map(str::to_string),
            ..MachineConfig::default()
        };
        let m = emu_machines::registry().create(machine, &config).unwrap();
        let mut gate = Self {
            s: Session::new(m, machine),
            typist,
            name: format!("{machine} {}", model.unwrap_or("")),
        };
        gate.run_for(3_000_000);
        gate
    }

    /// Let the machine run freely for `cycles`.
    fn run_for(&mut self, cycles: u64) {
        let end = self.s.machine().now().0 + cycles;
        self.s.set_running(true);
        while self.s.machine().now().0 < end && self.s.is_running() {
            self.s.run_slice();
        }
        assert!(self.s.is_running(), "{}: stopped while typing", self.name);
        self.s.set_running(false);
        self.s.take_events();
    }

    /// Queue `text` as key presses; returns the cycles they take.
    fn keys(&mut self, text: &str) -> u64 {
        let now = self.s.machine().now().0;
        let (events, end) = (self.typist)(text, now);
        for e in events {
            self.s.input(e);
        }
        end - now
    }

    fn type_line(&mut self, text: &str) {
        let cycles = self.keys(&format!("{text}\n"));
        self.run_for(cycles + 300_000);
    }

    fn ok(&mut self, cmd: Command) -> Value {
        let v = self.s.handle(Request { id: 1, cmd });
        assert_eq!(v["ok"], true, "{}: {v}", self.name);
        v["result"].clone()
    }

    fn err(&mut self, cmd: Command) -> String {
        let v = self.s.handle(Request { id: 1, cmd });
        assert_eq!(v["ok"], false, "{}: {v}", self.name);
        v["error"].as_str().unwrap().to_string()
    }

    /// Run an asynchronous command to its stop: (kind, line, statement).
    fn run(&mut self, cmd: Command) -> (String, u64, u64) {
        assert_eq!(self.ok(cmd)["running"], true);
        for _ in 0..2000 {
            self.s.run_slice();
            let events = self.s.take_events();
            if let Some(ev) = events.iter().find(|e| e["event"] == "stopped") {
                let r = &ev["reason"];
                return (
                    r["kind"].as_str().unwrap().to_string(),
                    r["line"].as_u64().unwrap_or(u64::MAX),
                    r["statement"].as_u64().unwrap_or(u64::MAX),
                );
            }
        }
        panic!("{}: no stop", self.name);
    }

    fn step(&mut self, kind: &str) -> (u64, u64) {
        let (k, line, statement) = self.run(Command::BasicStep {
            kind: Some(kind.into()),
        });
        assert_eq!(k, "basic_step", "{}", self.name);
        (line, statement)
    }

    fn var(&mut self, name: &str) -> Value {
        let vars = self.ok(Command::BasicVars);
        vars["vars"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .map_or(Value::Null, |v| v["value"].clone())
    }

    fn set(&mut self, name: &str, value: Value) {
        self.ok(Command::BasicSetVar {
            name: name.into(),
            index: None,
            value,
        });
    }

    fn stack(&mut self) -> Value {
        self.ok(Command::BasicStack)
    }
}

/// The gate of Phase 35 for one machine.
fn gate(mut g: Gate, dialect: &str) {
    let name = g.name.clone();
    assert_eq!(g.ok(Command::Capabilities)["basic"], true, "{name}");
    let info = g.ok(Command::BasicInfo);
    assert_eq!(info["dialect"], dialect, "{name}");
    assert_eq!(info["granularity"], "statement", "{name}");
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}");
    assert_eq!(state["line"], Value::Null, "{name}");

    for line in PROGRAM {
        g.type_line(line);
    }
    let list = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    let lines: Vec<(u64, &str)> = list["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["line"].as_u64().unwrap(), l["text"].as_str().unwrap()))
        .collect();
    assert_eq!(
        lines,
        [
            (10, "A=5:GOSUB100:A=A-2"),
            (20, "FORI=1TO3:B%=B%+10:NEXT"),
            (30, "C$=\"AB\"+\"C\""),
            (40, "GOTO10"),
            (100, "A=A+1:RETURN"),
        ],
        "{name}"
    );
    assert_eq!(list["lines"][1]["statements"], json!([0, 10, 19]), "{name}");

    // A breakpoint, then RUN.
    let bp = g.ok(Command::BasicBreakAdd {
        line: 20,
        statement: None,
        enabled: None,
        condition: None,
        ignore: None,
    })["bp"]
        .clone();
    g.keys("RUN\n");
    assert_eq!(
        g.run(Command::Resume),
        ("basic_break".into(), 20, 0),
        "{name}"
    );
    assert_eq!(g.var("A"), json!(4.0), "{name}");
    let state = g.ok(Command::BasicState);
    assert_eq!(
        (state["mode"].clone(), state["depth"].clone()),
        (json!("running"), json!(0)),
        "{name}"
    );
    g.ok(Command::BasicBreakRemove {
        bp: bp.as_u64().unwrap() as u32,
    });

    // By statement through the loop; its frame on the stack.
    assert_eq!(g.step("statement"), (20, 1), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "for", "var": "I", "limit": 3.0, "step": 1.0,
                "line": 20, "statement": 0}]),
        "{name}"
    );
    assert_eq!(g.var("I"), json!(1.0), "{name}");
    assert_eq!(g.step("statement"), (20, 2), "{name}");
    assert_eq!(g.var("B%"), json!(10), "{name}");
    assert_eq!(g.step("statement"), (20, 1), "{name}");

    // Variables written by the debugger are the ones the program uses.
    g.set("B%", json!(7));
    g.set("A", json!(40.5));
    assert_eq!(g.step("line"), (30, 0), "{name}");
    assert_eq!(g.var("B%"), json!(27), "{name}");
    assert!(g.stack()["frames"].as_array().unwrap().is_empty(), "{name}");
    assert_eq!(g.step("line"), (40, 0), "{name}");
    assert_eq!(g.var("C$"), json!("ABC"), "{name}");
    g.set("C$", json!("XYZ"));
    assert_eq!(g.var("C$"), json!("XYZ"), "{name}");
    assert_eq!(
        g.err(Command::BasicSetVar {
            name: "C$".into(),
            index: None,
            value: json!("TOO LONG"),
        }),
        "C$: not supported by this BASIC",
        "{name}"
    );

    // Into the subroutine by line, out of it, over it.
    assert_eq!(g.step("line"), (10, 0), "{name}");
    assert_eq!(g.step("statement"), (10, 1), "{name}");
    assert_eq!(
        g.err(Command::BasicStep {
            kind: Some("out".into())
        }),
        "not inside a GOSUB"
    );
    assert_eq!(g.step("line"), (100, 0), "{name}");
    let stack = g.stack();
    assert_eq!(stack["depth"], 1, "{name}");
    assert_eq!(
        stack["frames"],
        json!([{"kind": "gosub", "line": 10, "statement": 1}]),
        "{name}"
    );
    assert_eq!(g.step("out"), (10, 2), "{name}");
    assert_eq!(g.var("A"), json!(6.0), "{name}");
    assert_eq!(g.step("over"), (20, 0), "{name}");
    assert_eq!(g.var("A"), json!(4.0), "{name}");
    let to_gosub = Command::BasicRunTo {
        line: 10,
        statement: Some(1),
    };
    assert_eq!(g.run(to_gosub), ("basic_step".into(), 10, 1), "{name}");
    assert_eq!(g.step("over"), (20, 0), "{name}");
    let to_sub = Command::BasicRunTo {
        line: 100,
        statement: Some(1),
    };
    assert_eq!(g.run(to_sub), ("basic_step".into(), 100, 1), "{name}");
    assert_eq!(g.stack()["depth"], 1, "{name}");

    // Halted anywhere inside a statement the stack is still found (while
    // RETURN runs, its frame is already gone in line 100).
    let mut inside = 0;
    for _ in 0..40 {
        g.ok(Command::StepInstr { count: Some(7) });
        let state = g.ok(Command::BasicState);
        let depth = state["depth"].as_u64().unwrap();
        let line = state["line"]
            .as_u64()
            .unwrap_or_else(|| panic!("{name}: {state}"));
        assert!(depth == 0 || line == 100, "{name}: {state}");
        inside += depth;
    }
    assert!(
        inside > 0,
        "{name}: the frame was never seen inside a statement"
    );

    // A breakpoint with a condition over the program's variables: the
    // third pass of the loop, in a round after C$ was set.
    g.ok(Command::BasicBreakAdd {
        line: 20,
        statement: Some(1),
        enabled: None,
        condition: Some("i = 3 AND C$ = \"ABC\" AND NOT QQ".into()),
        ignore: None,
    });
    assert_eq!(
        g.run(Command::Resume),
        ("basic_break".into(), 20, 1),
        "{name}"
    );
    assert_eq!(g.var("I"), json!(3.0), "{name}");
    println!("{name}: {dialect} ok");
}

#[test]
#[ignore = "needs C64 ROMs"]
fn c64() {
    let typist: Typist = Box::new(machine_c64::keyboard::typed_events);
    gate(Gate::new("c64", None, typist), "Commodore BASIC V2");
}

#[test]
#[ignore = "needs VIC-20 ROMs"]
fn vic20() {
    let typist: Typist = Box::new(machine_vic20::keyboard::typed_events);
    gate(Gate::new("vic20", None, typist), "Commodore BASIC V2");
}

#[test]
#[ignore = "needs PET ROMs"]
fn pet() {
    use machine_pet::keyboard::{typed_events, Layout};
    for (model, dialect) in [
        ("2001", "Commodore BASIC 1.0"),
        ("3032", "Commodore BASIC V2"),
        ("4032", "Commodore BASIC 4.0"),
    ] {
        let typist: Typist = Box::new(|text, at| typed_events(Layout::Graphics, text, at));
        gate(Gate::new("pet", Some(model), typist), dialect);
    }
}

/// What BASIC 3.5 and 7.0 add: DO loops on the interpreter's own stack
/// beside FOR, and `extra` (a line 20 the program never reaches) for the
/// listing.
fn loops(mut g: Gate, extra: &str) {
    let name = g.name.clone();
    let line10 = "A=0:DO:A=A+1:FORI=1TO2:B=B+1:NEXT:LOOPUNTILA=2:END";
    g.type_line(&format!("10 {line10}"));
    g.type_line(&format!("20 {extra}"));
    let list = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    assert_eq!(list["lines"][0]["text"], line10, "{name}");
    assert_eq!(list["lines"][1]["text"], extra, "{name}");
    g.keys(
        "RUN
",
    );
    let inner = Command::BasicRunTo {
        line: 10,
        statement: Some(4),
    };
    assert_eq!(g.run(inner), ("basic_step".into(), 10, 4), "{name}");
    let stack = g.stack();
    assert_eq!(
        stack["frames"],
        json!([{"kind": "for", "var": "I", "limit": 2.0, "step": 1.0,
                "line": 10, "statement": 3},
               {"kind": "loop", "word": "DO", "line": 10, "statement": 1}]),
        "{name}"
    );
    assert_eq!(stack["depth"], 0, "{name}");
    // Out of the FOR loop, around the DO loop once.
    assert_eq!(g.step("statement"), (10, 5), "{name}");
    assert_eq!(g.step("statement"), (10, 4), "{name}");
    assert_eq!(g.step("statement"), (10, 5), "{name}");
    assert_eq!(g.step("statement"), (10, 6), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "loop", "word": "DO", "line": 10, "statement": 1}]),
        "{name}"
    );
    assert_eq!(g.step("statement"), (10, 2), "{name}");
    assert_eq!(g.var("A"), json!(1.0), "{name}");
    println!("{name}: loops ok");
}

#[test]
#[ignore = "needs Plus/4 ROMs"]
fn plus4() {
    for machine in ["plus4", "c16"] {
        let typist: Typist = Box::new(machine_ted::keyboard::typed_events);
        gate(Gate::new(machine, None, typist), "Commodore BASIC 3.5");
        let typist: Typist = Box::new(machine_ted::keyboard::typed_events);
        loops(Gate::new(machine, None, typist), "SOUND1,500,5:COLOR0,1");
    }
}

#[test]
#[ignore = "needs C128 ROMs"]
fn c128() {
    let c128 = || {
        let typist: Typist = Box::new(machine_c128::keyboard::typed_events);
        let mut g = Gate::new("c128", None, typist);
        g.run_for(3_000_000);
        g
    };
    gate(c128(), "Commodore BASIC 7.0");
    // Tokens of two bytes: $FE nn and $CE nn.
    loops(c128(), "BANK15:C=POT(1):SLOW");
    // C64 mode: the same machine with BASIC V2.
    let mut g = c128();
    g.type_line("GO64");
    g.type_line("Y");
    g.run_for(3_000_000);
    g.name = "c128 in C64 mode".into();
    gate(g, "Commodore BASIC V2");
}

/// Simons' BASIC and TSB on the C64: their keywords in the listing (LOOP
/// is `$64 $3A`, no colon), REPEAT and LOOP as frames, EXEC as a call.
fn extension(mut g: Gate, dialect: &str) {
    let name = g.name.clone();
    assert_eq!(g.ok(Command::BasicInfo)["dialect"], dialect, "{name}");
    for line in [
        "10 A=0:REPEAT:A=A+1:UNTIL A=3",
        "20 LOOP:D=D+1:EXIT IF D=2:END LOOP",
        "30 EXEC SUB",
        "40 B=5:HIRES 0,1:PLOT 10,10,1:LINE 0,0,100,100,1:NRM:END",
        "50 PROC SUB",
        "60 C=1:END PROC",
    ] {
        g.type_line(line);
    }
    let list = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    let lines: Vec<(u64, &str, Vec<u64>)> = list["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let statements = l["statements"].as_array().unwrap();
            (
                l["line"].as_u64().unwrap(),
                l["text"].as_str().unwrap(),
                statements.iter().map(|s| s.as_u64().unwrap()).collect(),
            )
        })
        .collect();
    assert_eq!(
        lines,
        [
            (10, "A=0:REPEAT:A=A+1:UNTIL A=3", vec![0, 4, 11, 17]),
            (20, "LOOP:D=D+1:EXIT IF D=2:END LOOP", vec![0, 5, 11, 23]),
            (30, "EXEC SUB", vec![0]),
            (
                40,
                "B=5:HIRES 0,1:PLOT 10,10,1:LINE 0,0,100,100,1:NRM:END",
                vec![0, 4, 14, 27, 46, 50],
            ),
            (50, "PROC SUB", vec![0]),
            (60, "C=1:END PROC", vec![0, 4]),
        ],
        "{name}"
    );

    g.keys("RUN\n");
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    assert_eq!(g.run(to(10, 2)), ("basic_step".into(), 10, 2), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "loop", "word": "REPEAT", "line": 10, "statement": 1}]),
        "{name}"
    );
    // Around the REPEAT loop by statement.
    assert_eq!(g.step("statement"), (10, 3), "{name}");
    assert_eq!(g.step("statement"), (10, 2), "{name}");
    assert_eq!(g.var("A"), json!(1.0), "{name}");
    assert_eq!(g.run(to(20, 2)), ("basic_step".into(), 20, 2), "{name}");
    assert_eq!(g.var("A"), json!(3.0), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "loop", "word": "LOOP", "line": 20, "statement": 0}]),
        "{name}"
    );
    // Over the procedure call, then into it and out again.
    assert_eq!(g.run(to(30, 0)), ("basic_step".into(), 30, 0), "{name}");
    assert_eq!(g.var("D"), json!(2.0), "{name}");
    assert_eq!(g.step("statement"), (60, 0), "{name}");
    let stack = g.stack();
    assert_eq!(stack["depth"], 1, "{name}");
    assert_eq!(
        stack["frames"][0],
        json!({"kind": "gosub", "line": 30, "statement": 0}),
        "{name}"
    );
    assert_eq!(g.step("out"), (40, 0), "{name}");
    assert_eq!(g.var("C"), json!(1.0), "{name}");
    assert_eq!(g.stack()["depth"], 0, "{name}");
    // The graphics commands run with the BASIC ROM switched off, in RAM
    // that also lies where the ROM has its stop point: no stop inside.
    for statement in 1..=5 {
        assert_eq!(g.step("statement"), (40, statement), "{name}");
    }
    println!("{name}: {dialect} ok");
}

/// A C64 with the user's cartridge image `file` plugged in, looked for
/// in `EMU_BASIC_EXT_DIR`, else beside the ROM folder (`~/.mimic`).
fn c64_with_cartridge(name: &str, file: &str) -> Gate {
    use emu_core::media::{Media, MediaKind};
    let dir = std::env::var_os("EMU_BASIC_EXT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let roms = media_backends::roms::env_dir()?;
            Some(roms.parent()?.to_path_buf())
        })
        .expect("no folder for the cartridge images");
    let path = dir.join(file);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let typist: Typist = Box::new(machine_c64::keyboard::typed_events);
    let mut g = Gate::new("c64", None, typist);
    g.name = name.into();
    let m = g.s.machine_mut();
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "cartridge")
        .expect("cartridge slot")
        .id;
    m.media_slot(slot)
        .unwrap()
        .insert(Media {
            name: file.into(),
            kind: MediaKind::Cartridge,
            backend: Box::new(media_backends::image::MemoryBackend { data }),
        })
        .unwrap();
    g.run_for(8_000_000);
    g
}

#[test]
#[ignore = "needs C64 ROMs and the user's TSB cartridge image"]
fn c64_tsb_cartridge() {
    let g = c64_with_cartridge("c64 with the TSB cartridge", "TSBneo_md_260228.crt");
    extension(g, "Commodore BASIC V2 + TSB");
}

#[test]
#[ignore = "needs C64 ROMs and the user's Simons' BASIC cartridge image"]
fn c64_simons_basic_cartridge() {
    let g = c64_with_cartridge("c64 with the Simons' BASIC cartridge", "Simons_Basic.crt");
    extension(g, "Commodore BASIC V2 + Simons' BASIC");
}
