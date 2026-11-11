//! Where screen overlays come from: the user's pictures in
//! `~/.mimic/overlays/<machine>/` (`EMU_OVERLAY_DIR` instead of
//! `~/.mimic/overlays`), one PNG per game, named like the game's
//! cartridge file (`Scramble.vec` → `scramble.png`, case does not
//! matter). The game built into the machine has a fixed name (the
//! Vectrex's Mine Storm: `minestorm.png`). Overlays are artwork of the
//! games' makers: they are never in the repository; [`video_crt::overlay`]
//! lays them over the picture (`video-crt` is a frontend stage: machines
//! never see them, golden rules 9 and 10).
//!
//! [`video_crt::overlay`]: ../../video_crt/overlay/index.html

use std::path::{Path, PathBuf};

/// The overlay folder: `EMU_OVERLAY_DIR`, else `~/.mimic/overlays`.
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("EMU_OVERLAY_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    ["USERPROFILE", "HOME"]
        .iter()
        .find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()))
        .map_or_else(PathBuf::new, PathBuf::from)
        .join(".mimic")
        .join("overlays")
}

/// The name of the game built into a machine (shown without a
/// cartridge), if it has one.
pub fn builtin(machine: &str) -> Option<&'static str> {
    match machine {
        "vectrex" => Some("minestorm"),
        _ => None,
    }
}

/// The overlay picture for `machine` with the cartridge `cart` (its file
/// path or name; `None`: the built-in game) in `root`, if there is one.
pub fn find_in(root: &Path, machine: &str, cart: Option<&str>) -> Option<PathBuf> {
    let want = match cart {
        Some(c) => Path::new(c).file_stem()?.to_string_lossy().to_lowercase(),
        None => builtin(machine)?.to_string(),
    };
    let mut hits: Vec<PathBuf> = std::fs::read_dir(root.join(machine))
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
                && p.file_stem()
                    .is_some_and(|s| s.to_string_lossy().to_lowercase() == want)
        })
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// [`find_in`] the overlay folder ([`dir`]).
pub fn find(machine: &str, cart: Option<&str>) -> Option<PathBuf> {
    find_in(&dir(), machine, cart)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_cartridge_name_and_the_built_in_game() {
        let root = std::env::temp_dir().join(format!("mimic-overlays-{}", std::process::id()));
        let d = root.join("vectrex");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Scramble.PNG"), b"x").unwrap();
        std::fs::write(d.join("minestorm.png"), b"x").unwrap();
        std::fs::write(d.join("berzerk.txt"), b"x").unwrap();
        let f = find_in(&root, "vectrex", Some("C:/games/scramble.vec")).unwrap();
        assert_eq!(f.file_name().unwrap(), "Scramble.PNG");
        assert!(find_in(&root, "vectrex", None).is_some());
        assert!(find_in(&root, "vectrex", Some("berzerk.bin")).is_none());
        assert!(find_in(&root, "c64", None).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
