//! Records the source version for `emu_core::version`: the output of
//! `git describe --tags --always --dirty` (e.g. `v0.92alpha-3-g56a7dcb`),
//! or `MIMIC_VERSION` from the environment when set (builds from a source
//! archive without `.git`), or else the crate version from Cargo.toml.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MIMIC_VERSION");
    let version = std::env::var("MIMIC_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(git_describe)
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));
    println!("cargo:rustc-env=MIMIC_VERSION={}", version.trim());
}

/// `git describe` of the workspace, with the files that change it as
/// rerun triggers (a new commit, tag or checkout).
fn git_describe() -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let git_dir = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    let git_dir = root.join(git_dir);
    for f in ["HEAD", "packed-refs", "index"] {
        println!("cargo:rerun-if-changed={}", git_dir.join(f).display());
    }
    if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD")) {
        if let Some(r) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed={}", git_dir.join(r).display());
        }
    }
    let out = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}
