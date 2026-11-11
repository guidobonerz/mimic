//! Media backends: image file, copy-on-write overlay, host directory, real
//! hardware.
//!
//! Real hardware sits behind cargo features (default off; golden rule 6
//! and "Drives and media" in CLAUDE.md: never needed to build):
//! - `flux`: flux decoding (PLL, Commodore GCR, Amiga and IBM MFM) and the
//!   disk images built from it — plain Rust, no device;
//! - `greaseweazle` (implies `flux`): the Greaseweazle's USB serial
//!   protocol, reading whole disks into D64/G64/D71/D81/ADF/IMG;
//! - `opencbm`: real 1541/1571/1581 drives through the OpenCBM library
//!   (ZoomFloppy/XUM1541), loaded at run time — the only module with
//!   `unsafe` code is its FFI layer.

#![cfg_attr(not(feature = "opencbm"), forbid(unsafe_code))]
#![cfg_attr(feature = "opencbm", deny(unsafe_code))]

#[cfg(feature = "flux")]
pub mod flux;
#[cfg(feature = "greaseweazle")]
pub mod greaseweazle;
pub mod host_dir;
#[cfg(not(target_arch = "wasm32"))]
pub mod host_fs;
#[cfg(not(target_arch = "wasm32"))]
pub mod host_print;
pub mod image;
#[cfg(not(target_arch = "wasm32"))]
pub mod net;
#[cfg(feature = "opencbm")]
pub mod opencbm;
pub mod overlays;
pub mod roms;

/// The host services of a native frontend, in a live journal: the real
/// network (`net` false: every network call reports "no network", the
/// clock still runs) and `folder` as the sandboxed file area, with each
/// of `mounts` (name, host folder) hung into its root as a directory
/// (`--uci-mount Usb0=E:\c64`: the Ultimate's `/Usb0`).
#[cfg(not(target_arch = "wasm32"))]
pub fn host_journal(
    net: bool,
    folder: Option<std::path::PathBuf>,
    mounts: &[(String, std::path::PathBuf)],
) -> Result<emu_core::host::Journal, String> {
    host_journal_with_printer(net, folder, mounts, None)
}

/// [`host_journal`] with a folder for the emulated printer's jobs.
#[cfg(not(target_arch = "wasm32"))]
pub fn host_journal_with_printer(
    net: bool,
    folder: Option<std::path::PathBuf>,
    mounts: &[(String, std::path::PathBuf)],
    printer: Option<host_print::PrinterOutput>,
) -> Result<emu_core::host::Journal, String> {
    use emu_core::host::{FileHost, MountFileHost};
    let root = folder.map(|f| Box::new(host_fs::FsFolder::new(f)) as Box<dyn FileHost>);
    let files = if mounts.is_empty() && printer.is_none() {
        root
    } else {
        let mut m = MountFileHost::new(root);
        for (name, dir) in mounts {
            m.mount(name, Box::new(host_fs::FsFolder::new(dir)))
                .map_err(|e| format!("mount {name}: {e}"))?;
        }
        if let Some(output) = printer {
            let folder = output.into_file_host()?;
            m.mount(host_print::MOUNT, folder)
                .map_err(|e| format!("mount {}: {e}", host_print::MOUNT))?;
        }
        Some(Box::new(m) as Box<dyn FileHost>)
    };
    Ok(emu_core::host::Journal::new(
        Some(Box::new(net::StdNet::new(net))),
        files,
    ))
}

/// Create the file area's folders now (`folder` and every mount's), not
/// only on the first write: a frontend calls this when the machine starts
/// with the Ultimate command interface (`uci=on`), so the user finds the
/// folders to put files into.
#[cfg(not(target_arch = "wasm32"))]
pub fn create_host_folders(
    folder: Option<&std::path::Path>,
    mounts: &[(String, std::path::PathBuf)],
) -> Result<(), String> {
    folder
        .into_iter()
        .chain(mounts.iter().map(|(_, d)| d.as_path()))
        .try_for_each(|d| std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display())))
}

/// The user's mimic folder for files the machines see: `.mimic` in the
/// home folder (`USERPROFILE` on Windows, else `HOME`); the working
/// directory's `.mimic` if neither is set.
#[cfg(not(target_arch = "wasm32"))]
pub fn user_files_dir() -> std::path::PathBuf {
    ["USERPROFILE", "HOME"]
        .iter()
        .find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()))
        .map_or_else(std::path::PathBuf::new, std::path::PathBuf::from)
        .join(".mimic")
}

/// The default folder of the Ultimate command interface's file area
/// (`--uci-dir`): `~/.mimic/ultimate`.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_uci_dir() -> std::path::PathBuf {
    user_files_dir().join("ultimate")
}

/// The default card folder of the C64 SD2IEC (`--sd2iec-dir`):
/// `~/.mimic/sd2iec`.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_sd2iec_dir() -> std::path::PathBuf {
    user_files_dir().join("sd2iec")
}

/// The C64 SD2IEC's card folder (`sd2iec=on` or `drive8=sd2iec`): the
/// `SD2IEC` mount of `--sd2iec-dir` if there is one, else
/// [`default_sd2iec_dir`], added to `mounts` as `SD2IEC`. Created at start
/// so the user finds it.
#[cfg(not(target_arch = "wasm32"))]
pub fn create_sd2iec_folder(
    options: &std::collections::BTreeMap<String, String>,
    mounts: &mut Vec<(String, std::path::PathBuf)>,
) -> Result<(), String> {
    let on = options.get("sd2iec").is_some_and(|v| v == "on")
        || options.get("drive8").is_some_and(|v| v == "sd2iec");
    if !on {
        return Ok(());
    }
    let dir = match mounts
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("SD2IEC"))
    {
        Some((_, d)) => d.clone(),
        None => {
            let d = default_sd2iec_dir();
            mounts.push(("SD2IEC".into(), d.clone()));
            d
        }
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// A `NAME=FOLDER` mount of `--uci-mount` (split at the first `=`).
pub fn parse_mount(s: &str) -> Result<(String, std::path::PathBuf), String> {
    match s.split_once('=') {
        Some((name, dir)) if !name.is_empty() && !dir.is_empty() => {
            Ok((name.to_string(), dir.into()))
        }
        _ => Err(format!("{s}: expected NAME=FOLDER, e.g. Usb0=E:\\c64")),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use emu_core::host::{lock, OpenMode};

    #[test]
    fn host_folders_mount_into_the_uci_root() {
        let base = std::env::temp_dir().join(format!("mimic-mount-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("usb")).unwrap();
        std::fs::write(base.join("usb").join("game.prg"), b"\x01\x08").unwrap();
        let mounts = [
            super::parse_mount(&format!("Usb0={}", base.join("usb").display())).unwrap(),
            ("SD".to_string(), base.join("sd")),
        ];
        super::create_host_folders(Some(&base.join("root")), &mounts).unwrap();
        assert!(base.join("root").is_dir() && base.join("sd").is_dir());
        let journal = super::host_journal(false, Some(base.join("root")), &mounts).unwrap();
        let files = journal.services().files.unwrap();
        let mut f = lock(&files);
        let names: Vec<_> = f
            .list_dir("/")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["SD", "Usb0"]);
        assert!(f.list_dir("/SD").unwrap().is_empty());
        assert_eq!(f.stat("/usb0/game.prg").unwrap().size, 2);
        let w = OpenMode {
            write: true,
            create: true,
            ..OpenMode::default()
        };
        let h = f.open("/SD/save.seq", w).unwrap();
        f.write(h, b"ok").unwrap();
        f.close(h);
        drop(f);
        assert_eq!(
            std::fs::read(base.join("sd").join("save.seq")).unwrap(),
            b"ok"
        );
        assert!(super::parse_mount("Usb0").is_err());
        assert!(super::parse_mount("=x").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
