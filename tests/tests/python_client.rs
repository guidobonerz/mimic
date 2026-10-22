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
            gdb: None,
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
