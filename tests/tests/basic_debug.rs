//! Phase 35 gate: the BASIC debugger on real machines with real ROMs
//! (user-supplied via `EMU_ROM_DIR`, else `~/.mimic/roms`). Each test types
//! a program in, then works on it with the commands of the native protocol.
//!
//!     cargo test -p mimic-tests --release --test basic_debug -- --ignored

use mimic_core::input::TimedInput;
use mimic_core::registry::MachineConfig;
use mimic_debug::protocol::{Command, Request};
use mimic_debug::Session;
use serde_json::{json, Value};

/// Typed without blanks, so the listing is known to the character.
const PROGRAM: &[&str] = &[
    "10 A=5:GOSUB100:A=A-2",
    "20 FORI=1TO3:B%=B%+10:NEXT",
    "30 C$=\"AB\"+\"C\"",
    "40 GOTO10",
    "100 A=A+1:RETURN",
];

type Typing<'a> = dyn Fn(&str, u64) -> (Vec<TimedInput>, u64) + 'a;
type Typist = Box<Typing<'static>>;

struct Gate {
    s: Session,
    typist: Typist,
    name: String,
}

impl Gate {
    fn new(machine: &str, model: Option<&str>, typist: Typist) -> Self {
        Self::with_options(machine, model, &[], typist)
    }

    /// A machine that needs `key` held down while it starts.
    fn with_key_held(
        machine: &str,
        model: Option<&str>,
        key: mimic_core::input::MachineKey,
        typist: Typist,
    ) -> Self {
        let mut gate = Self::build(machine, model, &[], typist);
        for (down, cycles) in [(true, 200_000), (false, 3_000_000)] {
            let at = gate.s.machine().now();
            gate.s.input(TimedInput {
                at,
                target: mimic_core::input::InputTarget::Keyboard,
                event: mimic_core::input::InputEvent::Key {
                    machine_key: key,
                    down,
                },
            });
            gate.run_for(cycles);
        }
        gate
    }

    fn with_options(
        machine: &str,
        model: Option<&str>,
        options: &[(&str, &str)],
        typist: Typist,
    ) -> Self {
        let mut gate = Self::build(machine, model, options, typist);
        gate.run_for(3_000_000);
        gate
    }

    fn build(machine: &str, model: Option<&str>, options: &[(&str, &str)], typist: Typist) -> Self {
        let dir = media_backends::roms::env_dir().expect("no ROM folder");
        let config = MachineConfig {
            roms: media_backends::roms::load(&dir, machine).expect("ROMs"),
            model: model.map(str::to_string),
            options: options
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..MachineConfig::default()
        };
        let m = mimic_machines::registry().create(machine, &config).unwrap();
        Self {
            s: Session::new(m, machine),
            typist,
            name: format!("{machine} {}", model.unwrap_or("")),
        }
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
fn gate(g: Gate, dialect: &str) {
    gate_with(g, dialect, PROGRAM, &LISTING, [0, 10, 19]);
}

/// The program as the Commodore machines list it.
const LISTING: [&str; 5] = [
    "A=5:GOSUB100:A=A-2",
    "FORI=1TO3:B%=B%+10:NEXT",
    "C$=\"AB\"+\"C\"",
    "GOTO10",
    "A=A+1:RETURN",
];

/// `program`: the five lines as typed; `listing`: as the dialect lists
/// them; `statements`: where line 20's statements begin in its text.
fn gate_with(
    mut g: Gate,
    dialect: &str,
    program: &[&str],
    listing: &[&str; 5],
    statements: [u32; 3],
) {
    let name = g.name.clone();
    assert_eq!(g.ok(Command::Capabilities)["basic"], true, "{name}");
    let info = g.ok(Command::BasicInfo);
    assert_eq!(info["dialect"], dialect, "{name}");
    assert_eq!(info["granularity"], "statement", "{name}");
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}");
    assert_eq!(state["line"], Value::Null, "{name}");

    for line in program {
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
    let numbers = [10, 20, 30, 40, 100];
    let want: Vec<(u64, &str)> = numbers.into_iter().zip(listing.iter().copied()).collect();
    assert_eq!(lines, want, "{name}");
    assert_eq!(list["lines"][1]["statements"], json!(statements), "{name}");

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

/// Applesoft on the II+ and both IIe: the listing is LIST's, with a
/// blank before and behind every keyword.
#[test]
#[ignore = "needs Apple II ROMs"]
fn apple2() {
    for model in ["iiplus", "iie-unenhanced", "iie"] {
        let typist: Typist = Box::new(machine_apple2::typing::typed_events);
        // Without the Disk II card: the machine starts into BASIC.
        gate_with(
            Gate::with_options("apple2", Some(model), &[("slot6", "none")], typist),
            "Applesoft BASIC",
            PROGRAM,
            &[
                "A = 5: GOSUB 100:A = A - 2",
                " FOR I = 1 TO 3:B% = B% + 10: NEXT ",
                "C$ = \"AB\" + \"C\"",
                " GOTO 10",
                "A = A + 1: RETURN ",
            ],
            [0, 16, 29],
        );
    }
}

/// What Microsoft's Z80 BASICs add: integer loops, double precision,
/// arrays, and the colons LIST hides before ELSE and the tick.
/// `double`: how the dialect names the double precision variable `D#`.
fn z80_extras(mut g: Gate, double: &str) {
    let name = g.name.clone();
    g.type_line("NEW");
    for line in [
        "10 DIMA%(2,1):A%(1,1)=7:D#=1.5:FORI%=1TO5STEP2:NEXT'NOTE:X",
        "20 IFD#>1THENX=1ELSEX=2",
        "30 QZ=9:END",
    ] {
        g.type_line(line);
    }
    let list = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    let l = &list["lines"];
    assert_eq!(
        l[0]["text"], "DIMA%(2,1):A%(1,1)=7:D#=1.5:FORI%=1TO5STEP2:NEXT'NOTE:X",
        "{name}"
    );
    // The tick is a statement of its own, the colon behind it is not.
    assert_eq!(l[0]["statements"], json!([0, 11, 21, 28, 44, 48]), "{name}");
    assert_eq!(l[1]["text"], "IFD#>1THENX=1ELSEX=2", "{name}");
    assert_eq!(l[1]["statements"], json!([0, 13]), "{name}");
    g.keys(
        "RUN
",
    );
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    assert_eq!(g.run(to(10, 4)), ("basic_step".into(), 10, 4), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "for", "var": "I%", "limit": 5, "step": 2,
                "line": 10, "statement": 3}]),
        "{name}"
    );
    assert_eq!(g.var("I%"), json!(1), "{name}");
    assert_eq!(g.var(double), json!(1.5), "{name}");
    let vars = g.ok(Command::BasicVars);
    let array = vars["vars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "A%")
        .unwrap_or_else(|| panic!("{name}: {vars}"))
        .clone();
    assert_eq!(array["dims"], json!([3, 2]), "{name}");
    let values = g.ok(Command::BasicArray {
        name: "A%".into(),
        first: Some(3),
        count: Some(3),
    });
    assert_eq!(values["values"], json!([0, 7, 0]), "{name}");
    // Three passes, then the tick, then line 20 with the value written.
    assert_eq!(g.step("statement"), (10, 4), "{name}");
    assert_eq!(g.step("statement"), (10, 4), "{name}");
    assert_eq!(g.step("statement"), (10, 5), "{name}");
    g.set(double, json!(-2.25));
    assert_eq!(g.var(double), json!(-2.25), "{name}");
    assert_eq!(g.step("statement"), (20, 0), "{name}");
    assert_eq!(g.step("statement"), (30, 0), "{name}");
    assert_eq!(g.var("X"), json!(2.0), "{name}");
    // A name of two characters.
    assert_eq!(g.step("statement"), (30, 1), "{name}");
    assert_eq!(g.var("QZ"), json!(9.0), "{name}");
    println!("{name}: extras ok");
}

/// What only MSX BASIC has: constants packed by the tokeniser (the
/// listing is compared with what LIST writes on the screen), decimal
/// floats, and a single precision loop's frame.
fn msx_extras(mut g: Gate) {
    let name = g.name.clone();
    g.type_line("NEW");
    let lines = [
        "10 FORI!=1TO2.5STEP.5:NEXT:AB=&H1F+&O17+12345+.001+1.5#+100000!",
        "20 AB=AB+1E10+123456789+.25+-3.5E-7:GOTO40",
        "30 DATA1:2,\"X\":REMA:B",
        "40 IFAB>0THEN50ELSE60'TICK",
        "50 END",
    ];
    for line in lines {
        g.type_line(line);
    }
    g.type_line("LIST");
    g.run_for(4_000_000);
    // Screen 0: the name table is at the start of the video RAM.
    let cpu = g.s.machine().devices()[0];
    let target = g.s.machine_mut().debug_target(cpu).unwrap();
    let screen: String = (0..960)
        .map(|a| {
            let b = target.peek(machine_msx::VRAM_SPACE, a, mimic_core::Size::Byte);
            char::from(b.unwrap_or(0) as u8)
        })
        .filter(|c| *c != ' ')
        .collect();
    let list = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    for (i, typed) in lines.iter().enumerate() {
        let l = &list["lines"][i];
        let listed = format!("{}{}", l["line"], l["text"].as_str().unwrap());
        assert!(
            screen.contains(&listed.replace(' ', "")),
            "{name}: {listed} is not what LIST shows: {screen}"
        );
        println!("{name}: {typed} -> {listed}");
    }
    // A colon ends DATA too; the one behind REM does not.
    assert_eq!(list["lines"][2]["statements"], json!([0, 6, 12]), "{name}");
    assert_eq!(list["lines"][3]["statements"], json!([0, 12, 18]), "{name}");
    g.keys(
        "RUN
",
    );
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    assert_eq!(g.run(to(10, 1)), ("basic_step".into(), 10, 1), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "for", "var": "I!", "limit": 2.5, "step": 0.5,
                "line": 10, "statement": 0}]),
        "{name}"
    );
    assert_eq!(g.run(to(50, 0)), ("basic_step".into(), 50, 0), "{name}");
    assert_eq!(g.var("I!"), json!(3.0), "{name}");
    assert_eq!(g.var("AB"), json!(10_123_569_181.751), "{name}");
    g.set("I!", json!(-0.0125));
    assert_eq!(g.var("I!"), json!(-0.0125), "{name}");
    // The line number behind GOTO is a pointer now; the listing is the same.
    let again = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    assert_eq!(
        again["lines"][1]["text"], list["lines"][1]["text"],
        "{name}"
    );
    println!("{name}: MSX extras ok");
}

/// Level I BASIC on the TRS-80: a Tiny BASIC without tokens, one FOR loop
/// in memory, GOSUB frames in a chain.
#[test]
#[ignore = "needs the TRS-80 Level I ROM"]
fn trs80_level1() {
    let typist: Typist =
        Box::new(|text, at| machine_trs80::keyboard::typed_events(text, at, 90_000, 90_000));
    let mut g = Gate::with_options("trs80", Some("1"), &[("level", "1")], typist);
    let name = g.name.clone();
    assert_eq!(g.ok(Command::Capabilities)["basic"], true);
    assert_eq!(g.ok(Command::BasicInfo)["dialect"], "TRS-80 Level I BASIC");
    assert_eq!(g.ok(Command::BasicState)["mode"], "direct");
    let program = [
        "10 A=5:GOSUB100:A=A-2",
        "20 FORI=1TO3:B=B+10:NEXTI",
        "30 A$=\"AB:C\":REM X:Y",
        "40 GOTO10",
        "100 A=A+1:RETURN",
    ];
    for line in program {
        g.type_line(line);
    }
    let list = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    let lines: Vec<String> = list["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| format!("{}{}", l["line"], l["text"].as_str().unwrap()))
        .collect();
    // The text is stored as typed, the blank behind the number included.
    assert_eq!(lines, program, "{name}");
    assert_eq!(list["lines"][1]["statements"], json!([0, 11, 18]), "{name}");
    // No statement behind REM, none in the string.
    assert_eq!(list["lines"][2]["statements"], json!([0, 11]), "{name}");
    // Numbers as the interpreter stores them.
    g.type_line("A=1:B=-2.5:C=100:D=.5:E=0:F=-1E-5:A$=\"HELLO\"");
    for (var, value) in [
        ("A", 1.0),
        ("B", -2.5),
        ("C", 100.0),
        ("D", 0.5),
        ("E", 0.0),
    ] {
        assert_eq!(g.var(var), json!(value), "{name}: {var}");
    }
    let f = g.var("F").as_f64().unwrap();
    assert!((f + 1e-5).abs() < 1e-11, "{name}: {f}");
    assert_eq!(g.var("A$"), json!("HELLO"), "{name}");

    let bp = g.ok(Command::BasicBreakAdd {
        line: 20,
        statement: None,
        enabled: None,
        condition: None,
        ignore: None,
    })["bp"]
        .clone();
    g.keys(
        "RUN
",
    );
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
    // By statement through the loop; it is the one loop in memory.
    assert_eq!(g.step("statement"), (20, 1), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "for", "var": "I", "limit": 3, "step": 1,
                "line": 20, "statement": 0}]),
        "{name}"
    );
    assert_eq!(g.step("statement"), (20, 2), "{name}");
    assert_eq!(g.var("B"), json!(7.5), "{name}");
    assert_eq!(g.step("statement"), (20, 1), "{name}");
    g.set("B", json!(-100));
    g.set("A", json!(40.5));
    assert_eq!(g.step("line"), (30, 0), "{name}");
    assert_eq!(g.var("B"), json!(-80.0), "{name}");
    assert!(g.stack()["frames"].as_array().unwrap().is_empty(), "{name}");
    assert_eq!(g.step("line"), (40, 0), "{name}");
    assert_eq!(g.var("A$"), json!("AB:C"), "{name}");
    g.set("A$", json!("WXYZ"));
    assert_eq!(g.var("A$"), json!("WXYZ"), "{name}");
    // Into the subroutine, out of it, over it.
    assert_eq!(g.step("line"), (10, 0), "{name}");
    assert_eq!(g.step("statement"), (10, 1), "{name}");
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
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    assert_eq!(g.run(to(100, 1)), ("basic_step".into(), 100, 1), "{name}");
    assert_eq!(g.stack()["depth"], 1, "{name}");
    // Halted anywhere inside a statement the line and the frame hold.
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

    // Loops inside loops and a subroutine: the loops that are not running
    // are saved on the stack, by FOR and by GOSUB.
    let mut g = {
        let typist: Typist =
            Box::new(|text, at| machine_trs80::keyboard::typed_events(text, at, 90_000, 90_000));
        Gate::with_options("trs80", Some("1"), &[("level", "1")], typist)
    };
    for line in [
        "10 FORI=1TO2:FORJ=3TO9STEP2:GOSUB100:NEXTJ:NEXTI",
        "20 END",
        "100 FORK=5TO6:NEXTK:RETURN",
    ] {
        g.type_line(line);
    }
    g.keys(
        "RUN
",
    );
    assert_eq!(g.run(to(100, 1)), ("basic_step".into(), 100, 1), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([
            {"kind": "for", "var": "K", "limit": 6, "step": 1, "line": 100, "statement": 0},
            {"kind": "gosub", "line": 10, "statement": 2},
            {"kind": "for", "var": "J", "limit": 9, "step": 2, "line": 10, "statement": 1},
            {"kind": "for", "var": "I", "limit": 2, "step": 1, "line": 10, "statement": 0},
        ]),
        "{name}"
    );
    assert_eq!(g.step("out"), (10, 3), "{name}");
    assert_eq!(g.stack()["frames"].as_array().unwrap().len(), 2, "{name}");
    println!("{name}: TRS-80 Level I BASIC ok");
}

/// MSX BASIC 1.0 and 2.0 with a real BIOS (C-BIOS has no BASIC).
#[test]
#[ignore = "needs an MSX BIOS with BASIC"]
fn msx() {
    for (model, dialect) in [("msx1", "MSX BASIC 1.0"), ("msx2", "MSX BASIC 2.0")] {
        let ready = || {
            let typist: Typist = Box::new(machine_msx::typing::typed_events);
            let mut g = Gate::with_options("msx", Some(model), &[("disk", "off")], typist);
            g.run_for(if model == "msx2" {
                40_000_000
            } else {
                12_000_000
            });
            g
        };
        gate(ready(), dialect);
        z80_extras(ready(), "D");
        msx_extras(ready());
    }
}

/// Level II BASIC on the TRS-80 Model I and the Model III: the text
/// pointer lives in HL, place and stack come from SAVTXT and SAVSTK.
#[test]
#[ignore = "needs TRS-80 ROMs"]
fn trs80() {
    let ready = |model: &str| {
        let typist: Typist =
            Box::new(|text, at| machine_trs80::keyboard::typed_events(text, at, 90_000, 90_000));
        // The Model III skips its disk boot with BREAK held, then asks
        // "Cass?"; both ask "Memory Size?".
        let mut g = if model == "3" {
            let key = machine_trs80::keyboard::keys::BREAK;
            let mut g = Gate::with_key_held("trs80", Some(model), key, typist);
            g.type_line("");
            g
        } else {
            Gate::new("trs80", Some(model), typist)
        };
        g.type_line("");
        g.run_for(9_000_000);
        g
    };
    for (model, dialect) in [
        ("1", "TRS-80 Level II BASIC"),
        ("3", "TRS-80 Model III BASIC"),
    ] {
        gate(ready(model), dialect);
        z80_extras(ready(model), "D#");
    }
}

/// Sinclair keyboards enter a keyword with one key (`l` is LET where a
/// statement begins), so the programs are written as keys; `{F}` is the
/// key F with `shift` held (SYMBOL SHIFT on the Spectrum: `{F}` TO, `{G}`
/// THEN, `{D}` STEP, `{A}` STOP; SHIFT on the ZX81: `{4}` TO).
fn keyword_keys(
    text: &str,
    at: u64,
    plain: &Typing<'_>,
    key: &dyn Fn(char) -> mimic_core::input::MachineKey,
    shift: mimic_core::input::MachineKey,
    (hold, gap): (u64, u64),
) -> (Vec<TimedInput>, u64) {
    use mimic_core::input::{InputEvent, InputTarget};
    let mut events = Vec::new();
    let mut t = at;
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let (typed, end) = plain(&rest[..open], t);
        events.extend(typed);
        t = end;
        let k = key(rest[open + 1..].chars().next().unwrap());
        for (machine_key, down, at) in [
            (shift, true, t),
            (k, true, t + 1),
            (k, false, t + hold),
            (shift, false, t + hold + 1),
        ] {
            events.push(TimedInput {
                at: mimic_core::Cycle(at),
                target: InputTarget::Keyboard,
                event: InputEvent::Key { machine_key, down },
            });
        }
        t += hold + gap;
        rest = &rest[open + 3..];
    }
    let (typed, end) = plain(rest, t);
    events.extend(typed);
    (events, end)
}

/// A Spectrum at its BASIC prompt: the 48K takes keys, the 128K and the
/// +2 text in 128 BASIC, which is `downs` steps down their menu (1), and
/// keys in 48 BASIC (3).
fn spectrum_ready(model: &str, downs: usize) -> Gate {
    use machine_spectrum::keyboard::{key_for_char, keys, typed_events};
    let hz: u64 = if model == "48k" { 3_500_000 } else { 3_546_900 };
    let typist: Typist = Box::new(move |text, at| {
        keyword_keys(
            text,
            at,
            &|text, at| typed_events(text, at, hz),
            &|c| key_for_char(c).unwrap().0,
            keys::SYMBOL_SHIFT,
            (hz / 50 * 4, hz / 50 * 4),
        )
    });
    let mut g = Gate::with_options("spectrum", Some(model), &[], typist);
    g.run_for(8_000_000);
    if model != "48k" {
        g.type_line(&"\u{2193}".repeat(downs));
        g.run_for(8_000_000);
    }
    g
}

/// Type program lines into a Sinclair machine, which lists the program
/// anew after each.
fn sinclair_lines(g: &mut Gate, lines: &[&str], pause: u64) {
    for line in lines {
        g.type_line(line);
        g.run_for(pause);
    }
}

fn listing(g: &mut Gate) -> Vec<(u64, String, Value)> {
    let list = g.ok(Command::BasicList {
        from: None,
        to: None,
    });
    list["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["line"].as_u64().unwrap(),
                l["text"].as_str().unwrap().to_string(),
                l["statements"].clone(),
            )
        })
        .collect()
}

/// The gate on a Spectrum. Its BASIC wants LET and a variable to exist
/// before it is read (line 5), has no integer variables, and keeps a
/// loop in its variable instead of on a stack.
fn spectrum_gate(mut g: Gate, dialect: &str, program: &[&str], run: &str) {
    let name = g.name.clone();
    assert_eq!(g.ok(Command::Capabilities)["basic"], true, "{name}");
    let info = g.ok(Command::BasicInfo);
    assert_eq!(info["dialect"], dialect, "{name}");
    assert_eq!(info["granularity"], "statement", "{name}");
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}");
    assert_eq!(state["line"], Value::Null, "{name}");

    sinclair_lines(&mut g, program, 6_000_000);
    let lines = listing(&mut g);
    let want = [
        (5, "LET b=0", json!([0])),
        (10, "LET a=5: GO SUB 100: LET a=a-2", json!([0, 8, 20])),
        (20, "FOR i=1 TO 3: LET b=b+10: NEXT i", json!([0, 13, 25])),
        (30, "LET c$=\"AB\"+\"C\"", json!([0])),
        (40, "GO TO 10", json!([0])),
        (100, "LET a=a+1: RETURN ", json!([0, 10])),
    ]
    .map(|(n, text, statements)| (n, text.to_string(), statements));
    assert_eq!(lines, want, "{name}");

    let bp = g.ok(Command::BasicBreakAdd {
        line: 20,
        statement: None,
        enabled: None,
        condition: None,
        ignore: None,
    })["bp"]
        .clone();
    g.keys(run);
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

    // By statement through the loop, which is open while it runs.
    assert_eq!(g.step("statement"), (20, 1), "{name}");
    let frame = json!([{"kind": "for", "var": "I", "limit": 3.0, "step": 1.0,
                        "line": 20, "statement": 0}]);
    assert_eq!(g.stack()["frames"], frame, "{name}");
    assert_eq!(g.var("I"), json!(1.0), "{name}");
    assert_eq!(g.step("statement"), (20, 2), "{name}");
    assert_eq!(g.var("B"), json!(10.0), "{name}");
    assert_eq!(g.step("statement"), (20, 1), "{name}");

    g.set("B", json!(7));
    g.set("a", json!(40.5));
    assert_eq!(g.step("line"), (30, 0), "{name}");
    assert_eq!(g.var("B"), json!(27.0), "{name}");
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
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    assert_eq!(g.run(to(10, 1)), ("basic_step".into(), 10, 1), "{name}");
    assert_eq!(g.step("over"), (20, 0), "{name}");
    assert_eq!(g.run(to(100, 1)), ("basic_step".into(), 100, 1), "{name}");
    assert_eq!(g.stack()["depth"], 1, "{name}");

    // Halted anywhere inside a statement the place and the frame hold.
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

/// What only Sinclair BASIC has: a statement number for what follows
/// THEN, arrays of numbers and of characters, long names, and after the
/// program has stopped the editor is not a running program.
fn spectrum_extras(mut g: Gate, program: &[&str], run: &str) {
    let name = g.name.clone();
    sinclair_lines(&mut g, program, 6_000_000);
    let lines = listing(&mut g);
    assert_eq!(
        lines[1],
        (
            20,
            "LET count=1.5: IF count>1 THEN LET x=1: LET x=x+0.25".to_string(),
            json!([0, 14, 31, 39])
        ),
        "{name}"
    );
    g.keys(run);
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    // The statement behind THEN is the line's third.
    assert_eq!(g.run(to(20, 2)), ("basic_step".into(), 20, 2), "{name}");
    assert_eq!(g.var("COUNT"), json!(1.5), "{name}");
    assert_eq!(g.step("statement"), (20, 3), "{name}");
    assert_eq!(g.step("statement"), (30, 0), "{name}");
    assert_eq!(g.var("X"), json!(1.25), "{name}");
    let vars = g.ok(Command::BasicVars);
    let dims = |n: &str| {
        let all = vars["vars"].as_array().unwrap();
        let v = all.iter().find(|v| v["name"] == n && v["value"].is_null());
        v.unwrap_or_else(|| panic!("{name}: {vars}"))["dims"].clone()
    };
    assert_eq!(dims("A"), json!([2, 3]), "{name}");
    assert_eq!(dims("B$"), json!([2, 4]), "{name}");
    let array = |g: &mut Gate, n: &str, first, count| {
        g.ok(Command::BasicArray {
            name: n.into(),
            first: Some(first),
            count: Some(count),
        })["values"]
            .clone()
    };
    assert_eq!(array(&mut g, "A", 2, 3), json!([0.0, 7.0, 0.0]), "{name}");
    assert_eq!(array(&mut g, "b$", 0, 5), json!(["H", "I", " ", " ", " "]));
    for (n, index, value) in [("A", 0, json!(-2.5)), ("B$", 7, json!("!"))] {
        g.ok(Command::BasicSetVar {
            name: n.into(),
            index: Some(index),
            value,
        });
    }
    assert_eq!(array(&mut g, "A", 0, 1), json!([-2.5]), "{name}");
    assert_eq!(array(&mut g, "B$", 7, 9), json!(["!"]), "{name}");
    for (v, value) in [("X", 1e10), ("X", -0.001), ("X", 70000.0), ("X", -3.0)] {
        g.set(v, json!(value));
        let read = g.var(v).as_f64().unwrap();
        assert!((read - value).abs() <= value.abs() * 1e-9, "{name}: {read}");
    }
    // A loop that counts down around a subroutine.
    assert_eq!(g.run(to(100, 0)), ("basic_step".into(), 100, 0), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([
            {"kind": "gosub", "line": 30, "statement": 1},
            {"kind": "for", "var": "N", "limit": 1.0, "step": -1.0, "line": 30, "statement": 0},
        ]),
        "{name}"
    );
    assert_eq!(g.step("out"), (30, 2), "{name}");
    assert_eq!(g.run(to(40, 0)), ("basic_step".into(), 40, 0), "{name}");
    assert!(g.stack()["frames"].as_array().unwrap().is_empty(), "{name}");
    // STOP: back in the editor.
    g.ok(Command::Resume);
    g.run_for(6_000_000);
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}: {state}");
    assert_eq!(state["line"], Value::Null, "{name}");
    println!("{name}: extras ok");
}

/// Sinclair BASIC on the Spectrum 48K (keywords by key) and, in 128
/// BASIC, on the 128K and the +2, whose editor ROM has a statement loop
/// of its own.
#[test]
#[ignore = "needs Spectrum ROMs"]
fn spectrum() {
    let keys = [
        "5lb=0",
        "10la=5:h100:la=a-2",
        "20fi=1{F}3:lb=b+10:ni",
        "30lc$=\"AB\"+\"C\"",
        "40g10",
        "100la=a+1:y",
    ];
    let extra_keys = [
        "10da(2,3):db$(2,4):la(2,1)=7:lb$(1)=\"HI\"",
        "20lcount=1.5:ucount>1{G}lx=1:lx=x+0.25",
        "30fn=3{F}1{D}-1:h100:nn",
        "40{A}",
        "100y",
    ];
    spectrum_gate(spectrum_ready("48k", 0), "ZX Spectrum BASIC", &keys, "r\n");
    spectrum_extras(spectrum_ready("48k", 0), &extra_keys, "r\n");
    let text = [
        "5 LET b=0",
        "10 LET a=5: GO SUB 100: LET a=a-2",
        "20 FOR i=1 TO 3: LET b=b+10: NEXT i",
        "30 LET c$=\"AB\"+\"C\"",
        "40 GO TO 10",
        "100 LET a=a+1: RETURN",
    ];
    let extra_text = [
        "10 DIM a(2,3): DIM b$(2,4): LET a(2,1)=7: LET b$(1)=\"HI\"",
        "20 LET count=1.5: IF count>1 THEN LET x=1: LET x=x+0.25",
        "30 FOR n=3 TO 1 STEP -1: GO SUB 100: NEXT n",
        "40 STOP",
        "100 RETURN",
    ];
    for model in ["128k", "plus2"] {
        let dialect = "ZX Spectrum 128 BASIC";
        spectrum_gate(spectrum_ready(model, 1), dialect, &text, "RUN\n");
        spectrum_extras(spectrum_ready(model, 1), &extra_text, "RUN\n");
        // 48 BASIC on the same machine: the other ROM's loop.
        spectrum_gate(spectrum_ready(model, 3), dialect, &keys, "r\n");
    }
}

/// The ZX81: one statement to a line, the listing compared with what
/// the ROM lists on the screen.
#[test]
#[ignore = "needs the ZX81 ROM"]
fn zx81() {
    use machine_zx81::keyboard::{key_for_char, keys, typed_events};
    let typist: Typist = Box::new(|text, at| {
        keyword_keys(
            text,
            at,
            &typed_events,
            &|c| key_for_char(c).unwrap().0,
            keys::SHIFT,
            (3_250_000 / 50 * 5, 3_250_000 / 50 * 12),
        )
    });
    let mut g = Gate::with_options("zx81", None, &[], typist);
    g.run_for(10_000_000);
    let name = g.name.clone();
    assert_eq!(g.ok(Command::Capabilities)["basic"], true, "{name}");
    let info = g.ok(Command::BasicInfo);
    assert_eq!(info["dialect"], "Sinclair ZX81 BASIC", "{name}");
    assert_eq!(g.ok(Command::BasicState)["mode"], "direct", "{name}");
    let program = [
        "10 la=5",
        "20 h100",
        "30 lb=0",
        "40 fi=1{4}3",
        "50 lb=b+10",
        "60 ni",
        "70 lc$=\"AB\"+\"C\"",
        "80 g20",
        "90 {A}",
        "100 la=a+1",
        "110 y",
    ];
    sinclair_lines(&mut g, &program, 8_000_000);
    let lines = listing(&mut g);
    let want = [
        (10, "LET A=5"),
        (20, "GOSUB 100"),
        (30, "LET B=0"),
        (40, "FOR I=1 TO 3"),
        (50, "LET B=B+10"),
        (60, "NEXT I"),
        (70, "LET C$=\"AB\"+\"C\""),
        (80, "GOTO 20"),
        (90, "STOP "),
        (100, "LET A=A+1"),
        (110, "RETURN "),
    ]
    .map(|(n, text)| (n, text.to_string(), json!([0])));
    assert_eq!(lines, want, "{name}");
    // The display file: the program as the ROM lists it.
    let cpu = g.s.machine().devices()[0];
    let target = g.s.machine_mut().debug_target(cpu).unwrap();
    let peek = |a: u32| {
        let b = target.peek(machine_zx81::CPU_SPACE, a, mimic_core::Size::Byte);
        b.unwrap_or(0) as u8
    };
    let d_file = u32::from(peek(0x400c)) | u32::from(peek(0x400d)) << 8;
    const CHARS: &[u8; 64] = b" ..........\"#$:?()><=+-*/;,.0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let screen: String = (d_file..d_file + 33 * 24)
        .map(|a| match peek(a) {
            0x76 => '\n',
            b => char::from(CHARS[usize::from(b & 0x3f)]),
        })
        .collect();
    for (n, text, _) in &lines {
        let listed = format!("{n:>4} {text}");
        let listed = listed.trim_end().replace("110 ", "110>");
        assert!(
            screen.lines().any(|l| l.trim_end() == listed),
            "{name}: {listed:?} is not what the ROM lists:\n{screen}"
        );
    }

    let bp = g.ok(Command::BasicBreakAdd {
        line: 40,
        statement: None,
        enabled: None,
        condition: None,
        ignore: None,
    })["bp"]
        .clone();
    g.keys("r\n");
    assert_eq!(
        g.run(Command::Resume),
        ("basic_break".into(), 40, 0),
        "{name}"
    );
    assert_eq!(g.var("A"), json!(6.0), "{name}");
    let state = g.ok(Command::BasicState);
    assert_eq!(
        (state["mode"].clone(), state["depth"].clone()),
        (json!("running"), json!(0)),
        "{name}"
    );
    g.ok(Command::BasicBreakRemove {
        bp: bp.as_u64().unwrap() as u32,
    });
    assert_eq!(g.step("statement"), (50, 0), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([{"kind": "for", "var": "I", "limit": 3.0, "step": 1.0,
                "line": 40, "statement": 0}]),
        "{name}"
    );
    assert_eq!(g.var("I"), json!(1.0), "{name}");
    assert_eq!(g.step("statement"), (60, 0), "{name}");
    assert_eq!(g.var("B"), json!(10.0), "{name}");
    assert_eq!(g.step("line"), (50, 0), "{name}");
    g.set("B", json!(7));
    g.set("A", json!(40.5));
    let to = |line| Command::BasicRunTo {
        line,
        statement: None,
    };
    assert_eq!(g.run(to(70)), ("basic_step".into(), 70, 0), "{name}");
    assert_eq!(g.var("B"), json!(27.0), "{name}");
    assert!(g.stack()["frames"].as_array().unwrap().is_empty(), "{name}");
    assert_eq!(g.step("line"), (80, 0), "{name}");
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
    // Into the subroutine, out of it, over it.
    assert_eq!(g.step("statement"), (20, 0), "{name}");
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
        json!([{"kind": "gosub", "line": 20, "statement": 0}]),
        "{name}"
    );
    assert_eq!(g.step("out"), (30, 0), "{name}");
    assert_eq!(g.var("A"), json!(41.5), "{name}");
    assert_eq!(g.run(to(20)), ("basic_step".into(), 20, 0), "{name}");
    assert_eq!(g.step("over"), (30, 0), "{name}");
    assert_eq!(g.run(to(110)), ("basic_step".into(), 110, 0), "{name}");
    assert_eq!(g.stack()["depth"], 1, "{name}");
    // Halted anywhere inside a line the place and the frame hold.
    let mut inside = 0;
    for _ in 0..40 {
        g.ok(Command::StepInstr { count: Some(7) });
        let state = g.ok(Command::BasicState);
        let depth = state["depth"].as_u64().unwrap();
        let line = state["line"]
            .as_u64()
            .unwrap_or_else(|| panic!("{name}: {state}"));
        assert!(depth == 0 || line == 110, "{name}: {state}");
        inside += depth;
    }
    assert!(
        inside > 0,
        "{name}: the frame was never seen inside a statement"
    );
    // A condition, then the program's end: back in the editor.
    g.ok(Command::BasicBreakAdd {
        line: 50,
        statement: None,
        enabled: None,
        condition: Some("I = 3 AND C$ = \"ABC\"".into()),
        ignore: None,
    });
    assert_eq!(
        g.run(Command::Resume),
        ("basic_break".into(), 50, 0),
        "{name}"
    );
    assert_eq!(g.var("I"), json!(3.0), "{name}");
    // BREAK: back in the editor.
    g.keys(" ");
    g.ok(Command::Resume);
    g.run_for(12_000_000);
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}: {state}");
    println!("{name}: Sinclair ZX81 BASIC ok");
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
    use mimic_core::media::{Media, MediaKind};
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

/// The text screen of an Atari in mode 0, as one string without blanks.
fn atari_screen(g: &mut Gate) -> String {
    let cpu = g.s.machine().devices()[0];
    let target = g.s.machine_mut().debug_target(cpu).unwrap();
    let peek = |a: u32| {
        let b = target.peek(machine_atari8::CPU_SPACE, a, mimic_core::Size::Byte);
        b.unwrap_or(0) as u8
    };
    // SAVMSC; screen codes are ATASCII with the first three quarters turned.
    let start = u32::from(peek(0x58)) | u32::from(peek(0x59)) << 8;
    (start..start + 960)
        .map(|a| match peek(a) & 0x7f {
            c @ 0x00..=0x3f => char::from(c + 0x20),
            c @ 0x40..=0x5f => char::from(c - 0x40),
            c => char::from(c),
        })
        .filter(|c| *c != ' ')
        .collect()
}

/// Every listed line is on the screen as the ROM's LIST wrote it.
fn atari_listed(g: &mut Gate) {
    let name = g.name.clone();
    g.type_line("GRAPHICS 0:LIST");
    g.run_for(4_000_000);
    let screen = atari_screen(g);
    for (n, text, _) in listing(g) {
        let listed = format!("{n}{text}").replace(' ', "");
        assert!(
            screen.contains(&listed),
            "{name}: {listed} is not what LIST shows: {screen}"
        );
    }
}

/// Atari BASIC: every line tokenised when entered, a run time stack of
/// its own, strings with a dimensioned length, no integer variables and
/// no string expressions.
fn atari_gate(mut g: Gate) {
    let name = g.name.clone();
    assert_eq!(g.ok(Command::Capabilities)["basic"], true, "{name}");
    let info = g.ok(Command::BasicInfo);
    assert_eq!(info["dialect"], "Atari BASIC", "{name}");
    assert_eq!(info["granularity"], "statement", "{name}");
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}");
    assert_eq!(state["line"], Value::Null, "{name}");

    for line in [
        "5 DIM C$(5)",
        "10 A=5:GOSUB100:A=A-2",
        "20 FORI=1TO3:B=B+10:NEXTI",
        "30 C$=\"ABC\"",
        "40 GOTO10",
        "100 A=A+1:RETURN",
    ] {
        g.type_line(line);
    }
    // A stored line is not a running program.
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}: {state}");
    let lines = listing(&mut g);
    let want = [
        (5, "DIM C$(5)", json!([0])),
        (10, "A=5:GOSUB 100:A=A-2", json!([0, 4, 14])),
        (20, "FOR I=1 TO 3:B=B+10:NEXT I", json!([0, 13, 20])),
        (30, "C$=\"ABC\"", json!([0])),
        (40, "GOTO 10", json!([0])),
        (100, "A=A+1:RETURN ", json!([0, 6])),
    ]
    .map(|(n, text, statements)| (n, text.to_string(), statements));
    assert_eq!(lines, want, "{name}");
    atari_listed(&mut g);

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
    assert_eq!(g.var("B"), json!(10.0), "{name}");
    assert_eq!(g.step("statement"), (20, 1), "{name}");

    g.set("B", json!(7));
    g.set("a", json!(40.5));
    assert_eq!(g.step("line"), (30, 0), "{name}");
    assert_eq!(g.var("B"), json!(27.0), "{name}");
    assert!(g.stack()["frames"].as_array().unwrap().is_empty(), "{name}");
    assert_eq!(g.step("line"), (40, 0), "{name}");
    assert_eq!(g.var("C$"), json!("ABC"), "{name}");
    // A string takes any length up to its dimension.
    for text in ["XYZ", "", "HELLO"] {
        g.set("C$", json!(text));
        assert_eq!(g.var("C$"), json!(text), "{name}");
    }
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
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    assert_eq!(g.run(to(10, 1)), ("basic_step".into(), 10, 1), "{name}");
    assert_eq!(g.step("over"), (20, 0), "{name}");
    assert_eq!(g.run(to(100, 1)), ("basic_step".into(), 100, 1), "{name}");
    assert_eq!(g.stack()["depth"], 1, "{name}");

    // Halted anywhere inside a statement the place and the frame hold.
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

    g.set("C$", json!("ABC"));
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
    println!("{name}: Atari BASIC ok");
}

/// What only Atari BASIC has: a statement of its own behind THEN, arrays
/// of one and two dimensions, names of any length, numbers as LIST writes
/// them, DATA and REM that keep their text.
fn atari_extras(mut g: Gate) {
    let name = g.name.clone();
    for line in [
        "10 DIM A(2,1),E(4):A(1,1)=7:E(3)=2.5",
        "20 COUNT=1.5:IF COUNT>1 THEN X=1:X=X+0.25",
        "30 FOR N=3 TO 1 STEP -1:GOSUB 100:NEXT N",
        "40 DATA 1,2:REM A:B",
        "50 Z=1E10+0.001+123456789012+.05+100:REM X:Y",
        "60 IF NOT (X>1 AND Z<0 OR X=3) THEN STOP",
        "100 RETURN",
    ] {
        g.type_line(line);
    }
    let lines = listing(&mut g);
    assert_eq!(
        lines[1],
        (
            20,
            "COUNT=1.5:IF COUNT>1 THEN X=1:X=X+0.25".to_string(),
            json!([0, 10, 26, 30])
        ),
        "{name}"
    );
    // DATA takes the line's rest; a colon in a REM is text.
    assert_eq!(lines[3].1, "DATA 1,2:REM A:B", "{name}");
    assert_eq!(lines[3].2, json!([0]), "{name}");
    assert_eq!(lines[4].2, json!([0, 40]), "{name}");
    atari_listed(&mut g);

    g.keys("RUN\n");
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    // The statement behind THEN is the line's third.
    assert_eq!(g.run(to(20, 2)), ("basic_step".into(), 20, 2), "{name}");
    assert_eq!(g.var("COUNT"), json!(1.5), "{name}");
    assert_eq!(g.step("statement"), (20, 3), "{name}");
    assert_eq!(g.step("statement"), (30, 0), "{name}");
    assert_eq!(g.var("X"), json!(1.25), "{name}");
    let vars = g.ok(Command::BasicVars);
    let dims = |n: &str| {
        let all = vars["vars"].as_array().unwrap();
        let v = all.iter().find(|v| v["name"] == n && v["value"].is_null());
        v.unwrap_or_else(|| panic!("{name}: {vars}"))["dims"].clone()
    };
    assert_eq!(dims("A"), json!([3, 2]), "{name}");
    assert_eq!(dims("E"), json!([5]), "{name}");
    let array = |g: &mut Gate, n: &str, first, count| {
        g.ok(Command::BasicArray {
            name: n.into(),
            first: Some(first),
            count: Some(count),
        })["values"]
            .clone()
    };
    assert_eq!(array(&mut g, "A", 2, 3), json!([0.0, 7.0, 0.0]), "{name}");
    assert_eq!(array(&mut g, "e", 3, 9), json!([2.5, 0.0]), "{name}");
    g.ok(Command::BasicSetVar {
        name: "A".into(),
        index: Some(0),
        value: json!(-2.5),
    });
    assert_eq!(array(&mut g, "A", 0, 1), json!([-2.5]), "{name}");
    for value in [1e10, -0.001, 70000.0, -3.0, 0.5, 1e-20, 9.99999999e97, 12.0] {
        g.set("X", json!(value));
        let read = g.var("X").as_f64().unwrap();
        assert!((read - value).abs() <= value.abs() * 1e-9, "{name}: {read}");
    }
    // A loop that counts down around a subroutine.
    assert_eq!(g.run(to(100, 0)), ("basic_step".into(), 100, 0), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([
            {"kind": "gosub", "line": 30, "statement": 1},
            {"kind": "for", "var": "N", "limit": 1.0, "step": -1.0, "line": 30, "statement": 0},
        ]),
        "{name}"
    );
    assert_eq!(g.step("out"), (30, 2), "{name}");
    assert_eq!(g.run(to(50, 0)), ("basic_step".into(), 50, 0), "{name}");
    assert!(g.stack()["frames"].as_array().unwrap().is_empty(), "{name}");
    assert_eq!(g.step("line"), (60, 0), "{name}");
    let z = g.var("Z").as_f64().unwrap();
    assert!((z - 133_456_789_112.051).abs() < 100.0, "{name}: {z}");
    // STOP: back in the editor.
    g.ok(Command::Resume);
    g.run_for(3_000_000);
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}: {state}");
    assert_eq!(state["line"], Value::Null, "{name}");
    println!("{name}: extras ok");
}

/// Atari BASIC built into the 800XL and the 130XE and as a cartridge in
/// the 800.
#[test]
#[ignore = "needs Atari ROMs"]
fn atari8() {
    for model in ["800xl", "130xe", "800"] {
        let ready = || {
            let typist: Typist = Box::new(machine_atari8::keyboard::typed_events);
            let mut g = Gate::with_options("atari8", Some(model), &[], typist);
            g.run_for(6_000_000);
            g
        };
        atari_gate(ready());
        atari_extras(ready());
    }
}

/// Locomotive BASIC 1.0 on the 464 and 1.1 on the 664 and the 6128: the
/// gate's program with the blanks this BASIC wants around its words.
#[test]
#[ignore = "needs CPC ROMs"]
fn cpc() {
    for (model, dialect) in [
        ("464", "Locomotive BASIC 1.0"),
        ("664", "Locomotive BASIC 1.1"),
        ("6128", "Locomotive BASIC 1.1"),
    ] {
        let ready = || {
            let typist: Typist = Box::new(machine_cpc::keyboard::typed_events);
            let mut g = Gate::with_options("cpc", Some(model), &[], typist);
            g.run_for(12_000_000);
            g
        };
        let listing = [
            "a=5:GOSUB 100:a=a-2",
            "FOR i=1 TO 3:b%=b%+10:NEXT",
            "c$=\"AB\"+\"C\"",
            "GOTO 10",
            "a=a+1:RETURN",
        ];
        let numbers = [10, 20, 30, 40, 100];
        let program: Vec<String> = numbers
            .iter()
            .zip(listing)
            .map(|(n, text)| format!("{n} {text}"))
            .collect();
        let program: Vec<&str> = program.iter().map(String::as_str).collect();
        gate_with(ready(), dialect, &program, &listing, [0, 13, 22]);
        cpc_extras(ready());
    }
}

/// The CPC's screen in mode 1 as text without blanks: every cell of the
/// bitmap compared with the characters in the OS ROM (pen 1 on paper 0).
fn cpc_screen(g: &mut Gate) -> String {
    let cpu = g.s.machine().devices()[0];
    let target = g.s.machine_mut().debug_target(cpu).unwrap();
    let peek = |space, a: u32| target.peek(space, a, mimic_core::Size::Byte).unwrap_or(0) as u8;
    let font: Vec<(char, [u8; 8])> = (33..127u8)
        .map(|c| {
            let at = 0x3800 + 8 * u32::from(c);
            let rows = std::array::from_fn(|y| peek(machine_cpc::ROM_SPACE, at + y as u32));
            (char::from(c), rows)
        })
        .collect();
    let mut out = String::new();
    for cell in 0..25 * 40u32 {
        let at = 0xc000 + cell / 40 * 80 + cell % 40 * 2;
        let rows: [u8; 8] = std::array::from_fn(|y| {
            let a = at + 0x800 * y as u32;
            let (left, right) = (
                peek(machine_cpc::RAM_SPACE, a),
                peek(machine_cpc::RAM_SPACE, a + 1),
            );
            left & 0xf0 | right >> 4
        });
        if rows != [0; 8] {
            let found = font.iter().find(|(_, f)| *f == rows);
            out.push(found.map_or('?', |(c, _)| *c));
        }
    }
    out
}

/// What Locomotive BASIC adds: WHILE loops and integer loops on its
/// stack, arrays whose first index runs fastest, the colons LIST hides
/// before ELSE and the tick, numbers in five forms, functions behind a
/// second token, an RSX.
fn cpc_extras(mut g: Gate) {
    let name = g.name.clone();
    let lines = [
        "10 DEF FNq(x)=x*2:DIM d(2,3),e%(4),s$(2):d(1,1)=7:e%(3)=-2:s$(1)=\"hey\":count!=1.5",
        "20 FOR j%=5 TO 1 STEP -2:WHILE a<3:a=a+1:GOSUB 100:WEND:NEXT j%",
        "30 IF a>1 THEN x=1 ELSE x=2 ' note: x",
        "40 z=&1F+&X101+1.5+12345+100000+0.001+1E+20+1.5E-10+0.5:REM a:b",
        "50 z=123456789+1234567890+0.000012345+-7+255+256:STOP",
        "60 s$(2)=s$(1)+\"!\":|TAPE:PRINT USING \"##\";FNq(2);ABS(-1)",
        "100 RETURN",
    ];
    for line in lines {
        g.type_line(line);
    }
    g.type_line("MODE 1:LIST");
    g.run_for(8_000_000);
    let screen = cpc_screen(&mut g);
    let listed = listing(&mut g);
    for ((n, text, _), typed) in listed.iter().zip(lines) {
        let line = format!("{n}{text}").replace(' ', "");
        assert!(
            screen.contains(&line),
            "{name}: {line} is not what LIST shows: {screen}"
        );
        println!("{name}: {typed} -> {n} {text}");
    }
    assert_eq!(listed[1].2, json!([0, 22, 32, 38, 48, 53]), "{name}");
    // The colons before ELSE and the tick are not listed.
    assert_eq!(listed[2].1, "IF a>1 THEN x=1 ELSE x=2 ' note: x", "{name}");
    assert_eq!(listed[2].2, json!([0, 16, 25]), "{name}");

    g.keys(
        "RUN
",
    );
    let to = |line, statement| Command::BasicRunTo {
        line,
        statement: Some(statement),
    };
    assert_eq!(g.run(to(100, 0)), ("basic_step".into(), 100, 0), "{name}");
    assert_eq!(
        g.stack()["frames"],
        json!([
            {"kind": "gosub", "line": 20, "statement": 3},
            {"kind": "loop", "word": "WHILE", "line": 20, "statement": 1},
            {"kind": "for", "var": "J%", "limit": 1, "step": -2, "line": 20, "statement": 0},
        ]),
        "{name}"
    );
    assert_eq!(g.var("J%"), json!(5), "{name}");
    assert_eq!(g.var("COUNT"), json!(1.5), "{name}");
    let vars = g.ok(Command::BasicVars);
    let dims = |n: &str| {
        let all = vars["vars"].as_array().unwrap();
        let v = all.iter().find(|v| v["name"] == n && v["value"].is_null());
        v.unwrap_or_else(|| panic!("{name}: {vars}"))["dims"].clone()
    };
    assert_eq!(dims("D"), json!([3, 4]), "{name}");
    assert_eq!(dims("E%"), json!([5]), "{name}");
    assert_eq!(dims("S$"), json!([3]), "{name}");
    let array = |g: &mut Gate, n: &str, first, count| {
        g.ok(Command::BasicArray {
            name: n.into(),
            first: Some(first),
            count: Some(count),
        })["values"]
            .clone()
    };
    // d(1,1): the first index runs fastest.
    assert_eq!(array(&mut g, "d", 3, 3), json!([0.0, 7.0, 0.0]), "{name}");
    assert_eq!(array(&mut g, "E%", 3, 9), json!([-2, 0]), "{name}");
    assert_eq!(array(&mut g, "S$", 0, 9), json!(["", "hey", ""]), "{name}");
    for (n, index, value) in [("D", 0, json!(-2.5)), ("E%", 4, json!(-300))] {
        g.ok(Command::BasicSetVar {
            name: n.into(),
            index: Some(index),
            value,
        });
    }
    assert_eq!(array(&mut g, "D", 0, 1), json!([-2.5]), "{name}");
    assert_eq!(array(&mut g, "E%", 4, 1), json!([-300]), "{name}");
    // A string assigned from a constant is the program's own text.
    let constant = g.err(Command::BasicSetVar {
        name: "S$".into(),
        index: Some(1),
        value: json!("HEY"),
    });
    assert!(
        constant.ends_with("not supported by this BASIC"),
        "{name}: {constant}"
    );
    assert!(listing(&mut g)[0].1.contains("\"hey\""), "{name}");
    for value in [1e10, -0.001, 70000.0, -3.0, 0.5, 1e-20, 12.0] {
        g.set("count!", json!(value));
        let read = g.var("COUNT").as_f64().unwrap();
        assert!((read - value).abs() <= value.abs() * 1e-9, "{name}: {read}");
    }
    assert_eq!(
        g.err(Command::BasicSetVar {
            name: "J%".into(),
            index: None,
            value: json!(40000),
        }),
        "J%: out of range",
        "{name}"
    );
    // Out of the subroutine, around the WHILE loop, out of it.
    assert_eq!(g.step("out"), (20, 4), "{name}");
    assert_eq!(g.step("statement"), (20, 2), "{name}");
    assert_eq!(g.run(to(20, 5)), ("basic_step".into(), 20, 5), "{name}");
    assert_eq!(g.var("A"), json!(3.0), "{name}");
    assert_eq!(g.stack()["frames"].as_array().unwrap().len(), 1, "{name}");
    // IF runs what follows THEN itself; ELSE is the next statement and
    // skips the line's rest.
    assert_eq!(g.run(to(30, 0)), ("basic_step".into(), 30, 0), "{name}");
    assert!(g.stack()["frames"].as_array().unwrap().is_empty(), "{name}");
    assert_eq!(g.step("statement"), (30, 1), "{name}");
    assert_eq!(g.var("X"), json!(1.0), "{name}");
    assert_eq!(g.step("statement"), (40, 0), "{name}");
    assert_eq!(g.step("statement"), (40, 1), "{name}");
    let z = g.var("Z").as_f64().unwrap();
    assert!((z - (1e20 + 112_383.001)).abs() < 1e12, "{name}: {z}");
    assert_eq!(g.step("line"), (50, 0), "{name}");
    assert_eq!(g.step("statement"), (50, 1), "{name}");
    assert_eq!(g.var("Z"), json!(1_358_025_183.0), "{name}");
    // STOP: back at the prompt.
    g.ok(Command::Resume);
    g.run_for(6_000_000);
    let state = g.ok(Command::BasicState);
    assert_eq!(state["mode"], "direct", "{name}: {state}");
    assert_eq!(state["line"], Value::Null, "{name}");
    println!("{name}: extras ok");
}

/// Disassembly or a dump for reading an interpreter in the user's ROMs:
/// `BASIC_DIS=apple2:iiplus:d7d2:d830`, `BASIC_DIS=apple2:ii:xe000:e100`
/// (an `x` before the first address: bytes instead of instructions), with
/// machine options as a fifth part: `BASIC_DIS=trs80:1:0380:03c5:level=1`.
#[test]
#[ignore = "a tool: needs the user's ROMs"]
fn disassemble() {
    let Ok(spec) = std::env::var("BASIC_DIS") else {
        return;
    };
    let p: Vec<&str> = spec.split(':').collect();
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, p[0]).expect("ROMs"),
        model: Some(p[1].to_string()).filter(|m| !m.is_empty()),
        options: p
            .get(4)
            .into_iter()
            .flat_map(|o| o.split(','))
            .filter_map(|o| o.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..MachineConfig::default()
    };
    let mut m = mimic_machines::registry().create(p[0], &config).unwrap();
    // `BASIC_DIS_RUN=100`: frames to run first, for ROMs the machine's
    // start-up code switches in.
    let frames = std::env::var("BASIC_DIS_RUN").map_or(0, |n| n.parse().unwrap());
    let mut hooks = mimic_core::DebugHooks::default();
    for _ in 0..frames {
        m.run_frame(&mut hooks);
    }
    // `BASIC_DIS_LOAD=file@c000`: a file's bytes poked in first, for a
    // ROM the CPU does not see at the moment (the CPC's upper ROM).
    if let Ok(load) = std::env::var("BASIC_DIS_LOAD") {
        let (file, at) = load.rsplit_once('@').unwrap();
        let at = u32::from_str_radix(at, 16).unwrap();
        let cpu = m.devices()[0];
        let space = m.memory_spaces()[0].id;
        let t = m.debug_target(cpu).unwrap();
        for (i, b) in std::fs::read(file).unwrap().into_iter().enumerate() {
            t.poke(space, at + i as u32, mimic_core::Size::Byte, u32::from(b));
        }
    }
    let dump = p[2].starts_with('x');
    let mut a = u32::from_str_radix(p[2].trim_start_matches('x'), 16).unwrap();
    let end = u32::from_str_radix(p[3], 16).unwrap();
    let cpu = m.devices()[0];
    let space = m.memory_spaces()[0].id;
    let t = m.debug_target(cpu).unwrap();
    while a < end {
        if dump {
            let bytes: Vec<u8> = (0..16)
                .map(|i| t.peek(space, a + i, mimic_core::Size::Byte).unwrap_or(0) as u8)
                .collect();
            let hex: Vec<_> = bytes.iter().map(|b| format!("{b:02X}")).collect();
            let text: String = bytes
                .iter()
                .map(|b| match b & 0x7f {
                    c @ 0x20..=0x7e => char::from(c),
                    _ => '.',
                })
                .collect();
            println!("{a:04X}  {}  {text}", hex.join(" "));
            a += 16;
        } else {
            let i = t.disassemble(a);
            let hex: Vec<_> = i.bytes.iter().map(|b| format!("{b:02X}")).collect();
            println!("{a:04X}  {:<9} {i}", hex.join(" "));
            a += i.bytes.len().max(1) as u32;
        }
    }
}
