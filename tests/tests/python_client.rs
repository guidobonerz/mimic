//! tools/polydbg.py against the real debug server: the Python client's
//! command line end to end. Skipped (with a note) if no Python 3 is found.

use std::path::PathBuf;
use std::process::Command;

use emu_core::registry::MachineConfig;
use emu_debug::{DebugServer, ServerConfig, Session};

/// The subroutine program of the protocol test, loaded at $0800:
/// 0800 JSR $0810 / JMP $0800; 0810 LDA #1, STA $0400, JSR $0820, RTS;
/// 0820 INC $0401, RTS.
fn subroutines() -> MachineConfig {
    let mut mem = vec![0xea; 0x24];
    mem[0x00..0x06].copy_from_slice(&[0x20, 0x10, 0x08, 0x4c, 0x00, 0x08]);
    mem[0x10..0x19].copy_from_slice(&[0xa9, 0x01, 0x8d, 0x00, 0x04, 0x20, 0x20, 0x08, 0x60]);
    mem[0x20..0x24].copy_from_slice(&[0xee, 0x01, 0x04, 0x60]);
    let mut program = vec![0x00, 0x08];
    program.extend_from_slice(&mem);
    let mut config = MachineConfig::default();
    config.roms.insert("program".into(), program);
    config
}

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        Command::new(p)
            .arg("-c")
            .arg("import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)")
            .status()
            .is_ok_and(|s| s.success())
    })
}

struct Polydbg {
    python: &'static str,
    script: PathBuf,
    port: u16,
}

impl Polydbg {
    /// Run one command; (exit success, stdout, stderr).
    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let out = Command::new(self.python)
            .arg(&self.script)
            .args(["--port", &self.port.to_string()])
            .args(args)
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (ok, stdout, stderr) = self.run(args);
        assert!(ok, "polydbg {args:?} failed: {stderr}");
        stdout
    }
}

#[test]
fn command_line_client_drives_the_debugger() {
    let Some(python) = python() else {
        eprintln!("skipped: no Python 3 found");
        return;
    };
    let machine = emu_machines::registry()
        .create("sim6502", &subroutines())
        .unwrap();
    let server = DebugServer::start(
        Session::new(machine, "sim6502"),
        ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            vice: None,
            run: false,
            realtime: false,
        },
    )
    .unwrap();
    let p = Polydbg {
        python,
        script: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/polydbg.py"),
        port: server.addr().port(),
    };

    assert!(p.ok(&["info"]).contains("machine   sim6502"));
    assert_eq!(p.ok(&["break", "$0820"]).trim(), "breakpoint 0");
    let stop = p.ok(&["resume", "--wait"]);
    assert!(stop.starts_with("breakpoint 0: pc=$0820"), "{stop}");
    // Stopped at $0820 before its INC $0401: the counter is still 0.
    let dump = p.ok(&["peek", "$0400", "2"]);
    assert!(dump.starts_with("0400: 01 00 "), "{dump}");
    assert!(p.ok(&["step"]).starts_with("step_done: pc=$0823"));
    assert!(p.ok(&["finish"]).starts_with("step_done: pc=$0818"));
    assert!(p.ok(&["disasm", "$0815", "-n", "1"]).contains("JSR $0820"));

    p.ok(&["unbreak", "0"]);
    assert!(p.ok(&["breaks"]).contains("no breakpoints"));
    assert!(p
        .ok(&["freeze", "$0401", "7", "--label", "Lives"])
        .starts_with("freeze 0"));
    assert!(p.ok(&["freezes"]).contains("Lives"));
    assert!(p.ok(&["peek", "$0401", "1"]).starts_with("0401: 07*"));

    let regs: serde_json::Value = serde_json::from_str(&p.ok(&["--json", "regs"])).unwrap();
    assert_eq!(regs["PC"], 0x0818);

    let (ok, _, stderr) = p.run(&["unbreak", "99"]);
    assert!(!ok);
    assert!(stderr.contains("no breakpoint 99"), "{stderr}");
    server.shutdown();
}

/// `polydbg basic …` against the toy BASIC machine of `emu-core`.
#[test]
fn command_line_client_debugs_basic() {
    use emu_core::testing::basic::{ToyBasic, ToyStmt::*};
    let Some(python) = python() else {
        eprintln!("skipped: no Python 3 found");
        return;
    };
    let machine = ToyBasic::new(&[
        (10, &[Let('A', 5), Gosub(100), Add('A', -2)]),
        (20, &[Goto(10)]),
        (100, &[For('I', 1, 2), Add('A', 1), Next]),
        (110, &[Return]),
    ]);
    let server = DebugServer::start(
        Session::new(Box::new(machine), "toybasic"),
        ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            vice: None,
            run: false,
            realtime: false,
        },
    )
    .unwrap();
    let p = Polydbg {
        python,
        script: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/polydbg.py"),
        port: server.addr().port(),
    };

    let info = p.ok(&["basic", "info"]);
    assert!(info.starts_with("Toy BASIC (steps by statement"), "{info}");
    assert!(
        info.contains("running: line 10 statement 0, GOSUB depth 0"),
        "{info}"
    );
    let list = p.ok(&["basic"]);
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines[0], ">   10 A=5:GOSUB 100:A=A-2");
    assert_eq!(lines[2], "   100 FOR I=1 TO 2:A=A+1:NEXT");
    assert_eq!(p.ok(&["basic", "list", "20", "20"]).trim(), "20 GOTO 10");

    assert_eq!(
        p.ok(&["basic", "break", "100:1"]).trim(),
        "BASIC breakpoint 1"
    );
    let stop = p.ok(&["resume", "--wait"]);
    assert!(
        stop.starts_with("BASIC breakpoint 1 at line 100 statement 1: "),
        "{stop}"
    );
    let stack = p.ok(&["basic", "stack"]);
    let frames: Vec<&str> = stack.lines().collect();
    assert_eq!(
        frames,
        [
            "FOR I TO 2 STEP 1  (line 100 statement 0)",
            "GOSUB from line 10 statement 1"
        ]
    );
    assert!(p
        .ok(&["basic", "breaks"])
        .contains("line 100 statement 1  hits 1"));
    p.ok(&["basic", "unbreak", "1"]);
    assert!(p.ok(&["basic", "breaks"]).contains("no BASIC breakpoints"));

    p.ok(&["basic", "set", "A", "40"]);
    p.ok(&["basic", "set", "S$", "HELLO"]);
    p.ok(&["basic", "set", "@(2)", "-9"]);
    let vars = p.ok(&["basic", "vars"]);
    assert!(vars.contains("A        int    $0200  40"), "{vars}");
    assert!(vars.contains("S$       string $0260  \"HELLO\""), "{vars}");
    assert!(vars.contains("@        int    $0240  (8)"), "{vars}");
    assert_eq!(p.ok(&["basic", "array", "@", "2", "1"]).trim(), "@(2) = -9");

    let step = p.ok(&["basic", "step", "out"]);
    assert!(
        step.starts_with("BASIC step (out) at line 10 statement 2: "),
        "{step}"
    );
    assert!(p
        .ok(&["basic", "step"])
        .starts_with("BASIC step (line) at line 20 statement 0: "));
    assert!(p
        .ok(&["basic", "until", "110"])
        .starts_with("BASIC step (run_to) at line 110 statement 0: "));
    let state: serde_json::Value =
        serde_json::from_str(&p.ok(&["--json", "basic", "info"])).unwrap();
    assert_eq!(state["state"]["depth"], 1);
    // A = 40 + 2 - 2, then 5 + 2 in the second round.
    let vars: serde_json::Value =
        serde_json::from_str(&p.ok(&["--json", "basic", "vars"])).unwrap();
    assert_eq!(vars["vars"][0]["value"], 7);

    let (ok, _, stderr) = p.run(&["basic", "unbreak", "9"]);
    assert!(!ok);
    assert!(stderr.contains("no BASIC breakpoint 9"), "{stderr}");
    server.shutdown();
}
