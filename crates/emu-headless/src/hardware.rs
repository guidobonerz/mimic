//! Real hardware from the command line (cargo features `greaseweazle` and
//! `opencbm`): import a disk with a Greaseweazle, copy a disk from or to a
//! real Commodore drive through OpenCBM (ZoomFloppy), or put a real drive
//! into the emulated machine's first disk drive. The flags are always
//! parsed; without the feature they explain how to build with it.

use emu_core::media::Media;

pub const USAGE: &str = "\
real hardware (build with --features greaseweazle and/or opencbm):
  --import-flux FILE    read a disk with a Greaseweazle into FILE and exit:
                        .d64/.g64 (1541), .d71 (1571), .d81 (1581), .adf
                        (Amiga), .img/.st (IBM MFM: PC, Atari ST)
  --gw-port PORT        the Greaseweazle's serial port (COM5, /dev/ttyACM0;
                        default auto: the only one found)
  --gw-drive D          a or b (IBM PC cable, default a), 0-2 (Shugart)
  --gw-format F         1541, 1571, 1581, amiga, amiga-hd, ibm, ibm-dd,
                        ibm-hd, ibm360 (default: from the file name)
  --gw-tracks N         tracks (1541: 35, 40, 42) or cylinders (IBM)
  --gw-step S           auto, 1 or 2 (2: a 40-track disk in an 80-track
                        drive; auto finds out for 5.25\" formats)
  --gw-revs N           revolutions per read (default 2)
  --gw-retries N        extra reads of a track with bad sectors (default 3)
  --import-drive FILE   copy the disk in a real drive (OpenCBM) into FILE
                        (.d64, .d71, .d81) and exit
  --export-drive FILE   write FILE (.d64, .d71, .d81) to the disk in a real
                        drive and exit (the disk must be formatted)
  --real-drive N        use the real drive N (OpenCBM) as the disk in the
                        machine's first drive; writes stay in memory unless
                        --real-drive-write
  --real-drive-write    write changed sectors to the real disk
  --cbm-device N        drive number for --import-drive/--export-drive
                        (default 8)
  --cbm-adapter NAME    OpenCBM adapter, e.g. xum1541 (default: OpenCBM's)
  --cbm-type T          1541, 1570, 1571, 1581 instead of asking the drive
  --cbm-tracks N        35 or 40 (1541 images)";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HardwareArgs {
    pub import_flux: Option<String>,
    pub gw_port: Option<String>,
    pub gw_drive: Option<String>,
    pub gw_format: Option<String>,
    pub gw_tracks: Option<u8>,
    pub gw_step: Option<String>,
    pub gw_revs: Option<u8>,
    pub gw_retries: Option<u8>,
    pub import_drive: Option<String>,
    pub export_drive: Option<String>,
    pub real_drive: Option<u8>,
    pub real_drive_write: bool,
    pub cbm_device: Option<u8>,
    pub cbm_adapter: Option<String>,
    pub cbm_type: Option<String>,
    pub cbm_tracks: Option<u8>,
}

const VALUE_FLAGS: &[&str] = &[
    "--import-flux",
    "--gw-port",
    "--gw-drive",
    "--gw-format",
    "--gw-tracks",
    "--gw-step",
    "--gw-revs",
    "--gw-retries",
    "--import-drive",
    "--export-drive",
    "--real-drive",
    "--cbm-device",
    "--cbm-adapter",
    "--cbm-type",
    "--cbm-tracks",
];

pub fn takes_value(flag: &str) -> bool {
    VALUE_FLAGS.contains(&flag)
}

pub fn is_switch(flag: &str) -> bool {
    flag == "--real-drive-write"
}

fn number(flag: &str, v: &str) -> Result<u8, String> {
    v.parse()
        .map_err(|_| format!("{flag} expects a number, got '{v}'"))
}

impl HardwareArgs {
    pub fn set(&mut self, flag: &str, v: String) -> Result<(), String> {
        match flag {
            "--import-flux" => self.import_flux = Some(v),
            "--gw-port" => self.gw_port = Some(v),
            "--gw-drive" => self.gw_drive = Some(v),
            "--gw-format" => self.gw_format = Some(v),
            "--gw-tracks" => self.gw_tracks = Some(number(flag, &v)?),
            "--gw-step" => match v.as_str() {
                "auto" | "1" | "2" => self.gw_step = Some(v),
                _ => return Err(format!("--gw-step: auto, 1 or 2, got '{v}'")),
            },
            "--gw-revs" => self.gw_revs = Some(number(flag, &v)?.clamp(1, 20)),
            "--gw-retries" => self.gw_retries = Some(number(flag, &v)?),
            "--import-drive" => self.import_drive = Some(v),
            "--export-drive" => self.export_drive = Some(v),
            "--real-drive" => self.real_drive = Some(number(flag, &v)?),
            "--cbm-device" => self.cbm_device = Some(number(flag, &v)?),
            "--cbm-adapter" => self.cbm_adapter = Some(v),
            "--cbm-type" => match v.as_str() {
                "1541" | "1570" | "1571" | "1581" => self.cbm_type = Some(v),
                _ => return Err(format!("--cbm-type: 1541, 1570, 1571 or 1581, got '{v}'")),
            },
            "--cbm-tracks" => self.cbm_tracks = Some(number(flag, &v)?),
            _ => return Err(format!("unknown option '{flag}'")),
        }
        Ok(())
    }

    pub fn switch(&mut self, flag: &str) {
        if flag == "--real-drive-write" {
            self.real_drive_write = true;
        }
    }

    /// A hardware tool runs instead of a machine.
    pub fn tool_requested(&self) -> bool {
        self.import_flux.is_some() || self.import_drive.is_some() || self.export_drive.is_some()
    }

    /// Run the requested tool.
    pub fn run_tool(&self) -> Result<(), String> {
        if let Some(path) = &self.import_flux {
            return self.import_flux(path);
        }
        if let Some(path) = &self.import_drive {
            return self.import_drive(path);
        }
        if let Some(path) = &self.export_drive {
            return self.export_drive(path);
        }
        Ok(())
    }

    #[cfg(not(feature = "greaseweazle"))]
    fn import_flux(&self, _path: &str) -> Result<(), String> {
        Err(without("greaseweazle"))
    }

    #[cfg(feature = "greaseweazle")]
    fn import_flux(&self, path: &str) -> Result<(), String> {
        use media_backends::flux::disk::{import, DiskFormat, ImportOptions, Step};
        use media_backends::greaseweazle::{self, DriveSelect};

        let (mut format, output) =
            DiskFormat::for_name(path, self.gw_format.as_deref()).map_err(|e| e.to_string())?;
        if let Some(n) = self.gw_tracks {
            format = format.with_tracks(n);
        }
        let options = ImportOptions {
            step: match self.gw_step.as_deref() {
                Some("1") => Step::Single,
                Some("2") => Step::Double,
                _ => Step::Auto,
            },
            revs: self.gw_revs.unwrap_or(2),
            retries: self.gw_retries.unwrap_or(3),
        };
        let drive = DriveSelect::parse(self.gw_drive.as_deref().unwrap_or("a"))
            .map_err(|e| e.to_string())?;
        let mut gw = greaseweazle::open(self.gw_port.as_deref().unwrap_or("auto"))
            .map_err(|e| e.to_string())?;
        eprintln!(
            "Greaseweazle: firmware {}.{}, model {}.{}, {} MHz sampling",
            gw.info.fw_major,
            gw.info.fw_minor,
            gw.info.hw_model,
            gw.info.hw_submodel,
            gw.info.sample_freq / 1_000_000
        );
        gw.select(drive).map_err(|e| e.to_string())?;
        let result = import(&mut gw, format, output, options, &mut |r| {
            let expected = if r.expected == 0 {
                "?".to_string()
            } else {
                r.expected.to_string()
            };
            eprintln!(
                "cyl {:2} head {}: {:2}/{} sectors",
                r.cylinder, r.head, r.good, expected
            );
        });
        let release = gw.release();
        let out = result.map_err(|e| e.to_string())?;
        release.map_err(|e| e.to_string())?;
        std::fs::write(path, &out.image).map_err(|e| format!("cannot write {path}: {e}"))?;
        eprintln!(
            "{path}: {} bytes ({format:?}, head step {:?})",
            out.image.len(),
            out.step
        );
        if out.missing.is_empty() {
            eprintln!("all sectors read");
        } else {
            for (c, h, n) in &out.missing {
                eprintln!("cylinder {c} head/side {h}: {n} sector(s) missing");
            }
            if path.to_lowercase().ends_with(".d64") {
                eprintln!("the D64 carries error bytes for them; a .g64 keeps the raw tracks");
            }
        }
        Ok(())
    }

    #[cfg(feature = "opencbm")]
    fn drive_options(&self, device: u8, layout_from: Option<&str>) -> Result<RealOptions, String> {
        use media_backends::opencbm::{DriveKind, Layout};
        let mut layout = layout_from.and_then(Layout::for_name);
        if let (Some(n), Some(Layout::D64 { .. })) = (self.cbm_tracks, layout) {
            layout = Some(Layout::D64 { tracks: n });
        }
        if let (Some(n), None) = (self.cbm_tracks, layout) {
            layout = Some(Layout::D64 { tracks: n });
        }
        Ok(media_backends::opencbm::RealDriveOptions {
            adapter: self.cbm_adapter.clone(),
            device,
            kind: self.cbm_type.as_deref().and_then(DriveKind::parse),
            layout,
        })
    }

    #[cfg(not(feature = "opencbm"))]
    fn import_drive(&self, _path: &str) -> Result<(), String> {
        Err(without("opencbm"))
    }

    #[cfg(feature = "opencbm")]
    fn import_drive(&self, path: &str) -> Result<(), String> {
        use media_backends::opencbm::{open_drive, read_disk, Layout};
        let opts = self.drive_options(self.cbm_device.unwrap_or(8), Some(path))?;
        if opts.layout.is_none() {
            return Err(format!("{path}: write a .d64, .d71 or .d81"));
        }
        let mut drive = open_drive(&opts).map_err(|e| e.to_string())?;
        let layout = opts.layout.unwrap_or(Layout::of(drive.kind));
        eprintln!(
            "drive {}: {:?}, reading {layout:?}",
            opts.device, drive.kind
        );
        let read = read_disk(&mut drive, layout, &mut |t, n| eprint!("\rtrack {t}/{n}"))
            .map_err(|e| e.to_string())?;
        eprintln!();
        std::fs::write(path, &read.image).map_err(|e| format!("cannot write {path}: {e}"))?;
        for (t, s, code) in &read.errors {
            eprintln!("track {t} sector {s}: DOS error {code}");
        }
        eprintln!("{path}: {} bytes", read.image.len());
        Ok(())
    }

    #[cfg(not(feature = "opencbm"))]
    fn export_drive(&self, _path: &str) -> Result<(), String> {
        Err(without("opencbm"))
    }

    #[cfg(feature = "opencbm")]
    fn export_drive(&self, path: &str) -> Result<(), String> {
        use media_backends::opencbm::{open_drive, write_disk};
        let image = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let opts = self.drive_options(self.cbm_device.unwrap_or(8), None)?;
        let mut drive = open_drive(&opts).map_err(|e| e.to_string())?;
        eprintln!("drive {}: {:?}, writing {path}", opts.device, drive.kind);
        write_disk(&mut drive, &image, &mut |t, n| eprint!("\rtrack {t}/{n}"))
            .map_err(|e| e.to_string())?;
        eprintln!("\ndone");
        Ok(())
    }

    /// The real drive as media, for the first disk drive.
    #[cfg(not(feature = "opencbm"))]
    pub fn real_drive_media(&self) -> Result<Option<Media>, String> {
        match self.real_drive {
            Some(_) => Err(without("opencbm")),
            None => Ok(None),
        }
    }

    /// The real drive as media, for the first disk drive.
    #[cfg(feature = "opencbm")]
    pub fn real_drive_media(&self) -> Result<Option<Media>, String> {
        let Some(device) = self.real_drive else {
            return Ok(None);
        };
        let opts = self.drive_options(device, None)?;
        let media = media_backends::opencbm::open_media(&opts, self.real_drive_write)
            .map_err(|e| e.to_string())?;
        eprintln!(
            "{} attached{}",
            media.name,
            if self.real_drive_write {
                " (writes go to the disk)"
            } else {
                " (writes stay in memory)"
            }
        );
        Ok(Some(media))
    }
}

#[cfg(feature = "opencbm")]
type RealOptions = media_backends::opencbm::RealDriveOptions;

#[cfg_attr(all(feature = "greaseweazle", feature = "opencbm"), allow(dead_code))]
fn without(feature: &str) -> String {
    format!(
        "this mimic-headless was built without real-hardware support; build it with \
         `cargo build --release -p emu-headless --features {feature}`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        let mut h = HardwareArgs::default();
        assert!(takes_value("--gw-port") && !takes_value("--disk"));
        h.set("--import-flux", "a.adf".into()).unwrap();
        h.set("--gw-tracks", "40".into()).unwrap();
        assert!(h.set("--gw-step", "3".into()).is_err());
        assert!(h.set("--cbm-type", "1551".into()).is_err());
        assert!(h.set("--real-drive", "x".into()).is_err());
        h.switch("--real-drive-write");
        assert!(h.real_drive_write && h.tool_requested());
        assert_eq!(h.gw_tracks, Some(40));
    }
}
