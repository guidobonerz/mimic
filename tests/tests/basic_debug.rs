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
        let line = state["line"].as_u64().unwrap();
        assert!(depth == 0 || line == 100, "{name}: {state}");
        inside += depth;
    }
    assert!(
        inside > 0,
        "{name}: the frame was never seen inside a statement"
    );
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
