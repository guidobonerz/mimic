//! The build's source version, shown by the frontends (desktop footer,
//! web page footer, `--help`/`--version` of the command-line tools) so a
//! user can say which build a picture or a report comes from.
//!
//! Set by `build.rs`: `git describe --tags --always --dirty`, so a tagged
//! commit gives `v0.92alpha`, later commits `v0.92alpha-3-g56a7dcb` and
//! uncommitted changes add `-dirty`. `MIMIC_VERSION` in the environment
//! overrides it (source archives without `.git`); without either it is
//! the crate version from Cargo.toml.

/// The version string, e.g. `v0.92alpha-3-g56a7dcb`.
pub const VERSION: &str = env!("MIMIC_VERSION");

/// `mimic <VERSION>` for a usage line or a footer.
pub fn banner() -> String {
    format!("mimic {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_set() {
        assert!(!VERSION.is_empty());
        assert!(!VERSION.contains(char::is_whitespace), "{VERSION:?}");
        assert_eq!(banner(), format!("mimic {VERSION}"));
    }
}
