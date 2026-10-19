//! Runs the emu-headless binary end to end.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emu-headless"))
}

fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn snapshots_are_deterministic() {
    let dir = out_dir("snapshots");
    let mut pngs = Vec::new();
    for i in 0..2 {
        let png = dir.join(format!("run{i}.png"));
        let out = bin()
            .args(["--frames", "3", "--snapshot"])
            .arg(&png)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("3 frame(s)"), "{stdout}");
        pngs.push(std::fs::read(&png).unwrap());
    }
    assert_eq!(&pngs[0][1..4], b"PNG");
    assert_eq!(pngs[0], pngs[1]);
}

#[test]
fn savestate_continues_identically() {
    let dir = out_dir("states");
    let state = dir.join("after2.state");
    let direct = dir.join("direct.png");
    let resumed = dir.join("resumed.png");

    let run = |args: &[&std::ffi::OsStr]| {
        let out = bin().args(args).output().unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    run(&[
        "--frames".as_ref(),
        "4".as_ref(),
        "--snapshot".as_ref(),
        direct.as_os_str(),
    ]);
    run(&[
        "--frames".as_ref(),
        "2".as_ref(),
        "--save-state".as_ref(),
        state.as_os_str(),
    ]);
    run(&[
        "--load-state".as_ref(),
        state.as_os_str(),
        "--frames".as_ref(),
        "2".as_ref(),
        "--snapshot".as_ref(),
        resumed.as_os_str(),
    ]);
    assert_eq!(
        std::fs::read(direct).unwrap(),
        std::fs::read(resumed).unwrap()
    );
}

#[test]
fn cpu_jam_exits_with_code_2() {
    let dir = out_dir("jam");
    let prg = dir.join("jam.prg");
    std::fs::write(&prg, [0x00, 0x08, 0x02]).unwrap();
    let out = bin().arg("--prg").arg(&prg).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("CpuJam"));
}

#[test]
fn unknown_machine_fails() {
    let out = bin().args(["--machine", "zx81"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown machine"));
}

#[test]
fn lists_machines() {
    let out = bin().arg("--list").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("sim6502"));
}
