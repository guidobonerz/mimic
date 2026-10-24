//! Web frontend (wasm32).
//!
//! Start profiles come as the text of a `profiles.yaml` the page loads
//! (there is no file system): [`profile_args`] turns one into the command
//! line the frontend starts its machine with, without the servers.

#![forbid(unsafe_code)]

/// The command line for profile `name` of the profile file text `yaml`;
/// the file's ROM folder stands in when the profile names none.
pub fn profile_args(yaml: &str, name: &str) -> Result<Vec<String>, String> {
    let file = emu_profile::ProfileFile::parse(yaml)?;
    let mut p = file.get(name).cloned().ok_or_else(|| {
        format!(
            "no profile '{name}' (there are: {})",
            file.names().join(", ")
        )
    })?;
    if p.roms.is_none() {
        p.roms.clone_from(&file.settings.roms);
    }
    Ok(p.to_args(emu_profile::Frontend::Web))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_becomes_a_command_line() {
        let yaml = "settings:
  roms: r
profiles:
  c:
    machine: c64
    debug: { server: true }
";
        assert_eq!(
            profile_args(yaml, "c").unwrap(),
            ["--machine", "c64", "--rom-dir", "r"]
        );
        assert!(profile_args(yaml, "x").is_err());
    }
}
