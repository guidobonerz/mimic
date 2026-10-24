//! Start profiles: named machine setups in one YAML file, shared by the
//! desktop, headless and web frontends.
//!
//! **File** (`profiles.yaml`): general `settings` and the `profiles`, a
//! mapping from profile name to profile. The file is looked for at the
//! path given with `--profiles`, else in the working folder, else in the
//! user's configuration folder (`%APPDATA%\mimic` on Windows,
//! `$XDG_CONFIG_HOME/mimic` or `~/.config/mimic` elsewhere). Relative paths
//! inside the file count from its folder.
//!
//! ```yaml
//! settings:
//!   roms: C:/retro/roms           # ROM folder when a profile names none
//!   snapshots: C:/retro/shots     # where the desktop's snapshot button saves
//!   media: C:/retro/disks         # base folder for relative media paths
//! profiles:
//!   mac-se-755:
//!     description: Mac SE with System 7.5.5 from SCSI
//!     machine: mac
//!     model: se
//!     roms: roms                  # ROM folder
//!     rom: { macse: roms/mac/macse.rom }   # or single ROM files by role
//!     options:                    # machine options: memory, drives, slots, port devices
//!       ram: 4m
//!     media:
//!       disks: [system.dsk, hd.hda]   # the next drive that takes each
//!       tape: game.wav
//!       cart: game.crt
//!       prg: game.prg
//!       autostart: true
//!     display: { crt: mac, scale: 2, footer: true, panel: false, deinterlace: weave }
//!     input: { joykeys: cursor, joyport: 2, speed: 100 }
//!     debug: { server: true, preview: "127.0.0.1:6580", vice: false }
//! ```
//!
//! Precedence for the ROM folder: the command line or the profile, then
//! `settings.roms`, then `EMU_ROM_DIR`.
//!
//! A profile is applied by turning it into the frontend's command line
//! ([`Profile::to_args`]); flags given on the real command line override
//! it ([`Profile::overlay`]: values replace, options and ROMs merge by key,
//! a disk list replaces the profile's). [`prepare`] does all of this for a
//! frontend, including `--list-profiles` and `--save-profile NAME`, which
//! stores the resulting setup as a profile.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

/// The file name looked for in the working and configuration folders.
pub const FILE_NAME: &str = "profiles.yaml";

/// Which frontend a command line is for (they take different flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frontend {
    Desktop,
    Headless,
    Web,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Map values as strings, whatever scalar YAML gives (`ram: 4m`,
/// `drives: 2`, `basic: off`).
fn scalar_map<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    let raw = BTreeMap::<String, serde_yaml_ng::Value>::deserialize(d)?;
    raw.into_iter()
        .map(|(k, v)| {
            let s = match v {
                serde_yaml_ng::Value::String(s) => s,
                serde_yaml_ng::Value::Number(n) => n.to_string(),
                serde_yaml_ng::Value::Bool(b) => if b { "on" } else { "off" }.to_string(),
                other => {
                    return Err(serde::de::Error::custom(format!(
                        "option {k}: a single value expected, got {other:?}"
                    )))
                }
            };
            Ok((k, s))
        })
        .collect()
}

/// A server switch: `true` (default address), `false`, or an address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Endpoint {
    On(bool),
    At(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Media {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tape: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cart: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prg: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub autostart: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Display {
    /// Monitor profile (`raw`, `1084s`, `tv`, `mac` … or a .toml file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deinterlace: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Input {
    /// Keys that play the joystick: `cursor`, `numpad`, `off`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joykeys: Option<String>,
    /// The joystick port, from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joyport: Option<u8>,
    /// Speed in percent of real time (a step of the footer's slider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Debug {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<Endpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<Endpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vice: Option<Endpoint>,
}

/// One start profile. Every field is optional so that profiles and
/// command lines can be laid over each other; a usable profile names its
/// machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// ROM folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roms: Option<String>,
    /// Single ROM files by role.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub rom: BTreeMap<String, String>,
    #[serde(
        deserialize_with = "scalar_map",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub options: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "is_default")]
    pub media: Media,
    #[serde(skip_serializing_if = "is_default")]
    pub display: Display,
    #[serde(skip_serializing_if = "is_default")]
    pub input: Input,
    #[serde(skip_serializing_if = "is_default")]
    pub debug: Debug,
}

/// `a/b/../c` as `a/c` (by the path's text, without the file system).
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn set<T: Clone>(dst: &mut Option<T>, src: &Option<T>) {
    if src.is_some() {
        dst.clone_from(src);
    }
}

impl Profile {
    /// Lay `top` over this profile: its values win, options and ROMs merge
    /// by key, a non-empty disk list replaces this one's.
    pub fn overlay(&mut self, top: &Profile) {
        set(&mut self.description, &top.description);
        set(&mut self.machine, &top.machine);
        set(&mut self.model, &top.model);
        set(&mut self.roms, &top.roms);
        self.rom
            .extend(top.rom.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.options
            .extend(top.options.iter().map(|(k, v)| (k.clone(), v.clone())));
        let (m, t) = (&mut self.media, &top.media);
        if !t.disks.is_empty() {
            m.disks.clone_from(&t.disks);
        }
        set(&mut m.tape, &t.tape);
        set(&mut m.cart, &t.cart);
        set(&mut m.prg, &t.prg);
        m.autostart |= t.autostart;
        let (d, t) = (&mut self.display, &top.display);
        set(&mut d.crt, &t.crt);
        set(&mut d.scale, &t.scale);
        set(&mut d.footer, &t.footer);
        set(&mut d.panel, &t.panel);
        set(&mut d.deinterlace, &t.deinterlace);
        let (i, t) = (&mut self.input, &top.input);
        set(&mut i.joykeys, &t.joykeys);
        set(&mut i.joyport, &t.joyport);
        set(&mut i.speed, &t.speed);
        let (g, t) = (&mut self.debug, &top.debug);
        set(&mut g.server, &t.server);
        set(&mut g.preview, &t.preview);
        set(&mut g.vice, &t.vice);
    }

    /// Make relative paths absolute against `base`.
    pub fn resolve_paths(&mut self, base: &Path) {
        let fix = |p: &mut String| {
            let path = Path::new(p.as_str());
            if path.is_relative() {
                *p = normalize(&base.join(path)).to_string_lossy().into_owned();
            }
        };
        if let Some(r) = &mut self.roms {
            fix(r);
        }
        for v in self.rom.values_mut() {
            fix(v);
        }
        for d in &mut self.media.disks {
            fix(d);
        }
        for p in [
            &mut self.media.tape,
            &mut self.media.cart,
            &mut self.media.prg,
        ]
        .into_iter()
        .flatten()
        {
            fix(p);
        }
        if let Some(c) = &mut self.display.crt {
            // A monitor profile file, not a preset name.
            if c.ends_with(".toml") {
                fix(c);
            }
        }
    }

    /// The profile's flags out of a command line; the other arguments come
    /// back unchanged, in order. Flags with an optional address (`--debug`,
    /// `--preview`, `--vice`) take the next argument when it is not a flag.
    pub fn from_args(args: &[String]) -> Result<(Profile, Vec<String>), String> {
        let mut p = Profile::default();
        let mut rest = Vec::new();
        let mut it = args.iter().peekable();
        while let Some(flag) = it.next() {
            let mut value = |n: &str| {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{n} needs a value"))
            };
            match flag.as_str() {
                "--machine" => p.machine = Some(value(flag)?),
                "--model" => p.model = Some(value(flag)?),
                "--rom-dir" => p.roms = Some(value(flag)?),
                "--rom" => {
                    let kv = value(flag)?;
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("--rom expects ROLE=FILE, got '{kv}'"))?;
                    p.rom.insert(k.into(), v.into());
                }
                "--option" => {
                    let kv = value(flag)?;
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("--option expects KEY=VALUE, got '{kv}'"))?;
                    p.options.insert(k.into(), v.into());
                }
                "--disk" => p.media.disks.push(value(flag)?),
                "--tape" => p.media.tape = Some(value(flag)?),
                "--cart" => p.media.cart = Some(value(flag)?),
                "--prg" => p.media.prg = Some(value(flag)?),
                "--autostart" => p.media.autostart = true,
                "--crt" => p.display.crt = Some(value(flag)?),
                "--scale" => {
                    let v = value(flag)?;
                    p.display.scale = Some(v.parse().map_err(|_| format!("bad scale '{v}'"))?);
                }
                "--footer" => p.display.footer = Some(true),
                "--no-panel" => p.display.panel = Some(false),
                "--deinterlace" => p.display.deinterlace = Some(value(flag)?),
                "--joykeys" => p.input.joykeys = Some(value(flag)?),
                "--joyport" => {
                    let v = value(flag)?;
                    p.input.joyport =
                        Some(v.parse().map_err(|_| format!("--joyport {v}: a number"))?);
                }
                "--speed" => {
                    let v = value(flag)?;
                    p.input.speed = Some(v.parse().map_err(|_| format!("--speed {v}: percent"))?);
                }
                "--debug" | "--preview" | "--vice" => {
                    let e = match it.peek() {
                        Some(v) if !v.starts_with("--") => {
                            Endpoint::At(it.next().cloned().unwrap_or_default())
                        }
                        _ => Endpoint::On(true),
                    };
                    let slot = match flag.as_str() {
                        "--debug" => &mut p.debug.server,
                        "--preview" => &mut p.debug.preview,
                        _ => &mut p.debug.vice,
                    };
                    *slot = Some(e);
                }
                _ => rest.push(flag.clone()),
            }
        }
        Ok((p, rest))
    }

    /// The profile as `frontend`'s command line (the headless runner has no
    /// window settings, the web frontend no servers).
    pub fn to_args(&self, frontend: Frontend) -> Vec<String> {
        let mut a = Vec::new();
        let mut push = |f: &str, v: Option<&str>| {
            a.push(f.to_string());
            if let Some(v) = v {
                a.push(v.to_string());
            }
        };
        if let Some(m) = &self.machine {
            push("--machine", Some(m));
        }
        if let Some(m) = &self.model {
            push("--model", Some(m));
        }
        if let Some(r) = &self.roms {
            push("--rom-dir", Some(r));
        }
        for (k, v) in &self.rom {
            push("--rom", Some(&format!("{k}={v}")));
        }
        for (k, v) in &self.options {
            push("--option", Some(&format!("{k}={v}")));
        }
        for d in &self.media.disks {
            push("--disk", Some(d));
        }
        if let Some(t) = &self.media.tape {
            push("--tape", Some(t));
        }
        if let Some(c) = &self.media.cart {
            push("--cart", Some(c));
        }
        if let Some(p) = &self.media.prg {
            push("--prg", Some(p));
        }
        if self.media.autostart {
            push("--autostart", None);
        }
        if let Some(c) = &self.display.crt {
            push("--crt", Some(c));
        }
        if frontend == Frontend::Desktop {
            let d = &self.display;
            if let Some(s) = d.scale {
                push("--scale", Some(&s.to_string()));
            }
            if d.footer == Some(true) {
                push("--footer", None);
            }
            if d.panel == Some(false) {
                push("--no-panel", None);
            }
            if let Some(m) = &d.deinterlace {
                push("--deinterlace", Some(m));
            }
            let i = &self.input;
            if let Some(k) = &i.joykeys {
                push("--joykeys", Some(k));
            }
            if let Some(p) = i.joyport {
                push("--joyport", Some(&p.to_string()));
            }
            if let Some(s) = i.speed {
                push("--speed", Some(&s.to_string()));
            }
        }
        if frontend != Frontend::Web {
            let g = &self.debug;
            for (flag, e) in [
                ("--debug", &g.server),
                ("--preview", &g.preview),
                ("--vice", &g.vice),
            ] {
                match e {
                    Some(Endpoint::On(true)) => push(flag, None),
                    Some(Endpoint::At(addr)) => push(flag, Some(addr)),
                    _ => {}
                }
            }
        }
        a
    }
}

/// The file's general part.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// ROM folder for profiles and command lines that name none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roms: Option<String>,
    /// Folder the desktop's snapshot button saves into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshots: Option<String>,
    /// Folder relative media paths in profiles count from (default: the
    /// file's folder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
}

/// A profile file: the settings and the profiles in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileFile {
    pub settings: Settings,
    pub profiles: Vec<(String, Profile)>,
}

impl ProfileFile {
    /// Parse YAML text (paths stay as written).
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let top: serde_yaml_ng::Mapping =
            serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
        let mut out = Self::default();
        for (k, v) in top {
            match k.as_str() {
                Some("settings") if !v.is_null() => {
                    out.settings =
                        serde_yaml_ng::from_value(v).map_err(|e| format!("settings: {e}"))?;
                }
                Some("settings") => {}
                Some("profiles") if v.is_null() => {}
                Some("profiles") => {
                    let map = v
                        .as_mapping()
                        .ok_or("profiles: a mapping from name to profile expected")?;
                    for (k, v) in map {
                        let name = k
                            .as_str()
                            .ok_or_else(|| format!("profile names are text, got {k:?}"))?
                            .to_string();
                        let p: Profile = serde_yaml_ng::from_value(v.clone())
                            .map_err(|e| format!("profile {name}: {e}"))?;
                        out.profiles.push((name, p));
                    }
                }
                _ => return Err(format!("unknown top-level key {k:?} (settings, profiles)")),
            }
        }
        Ok(out)
    }

    /// Read a file; relative paths in it count from its folder.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut f = Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let abs = |p: &mut String| {
            if Path::new(p.as_str()).is_relative() {
                *p = normalize(&base.join(p.as_str()))
                    .to_string_lossy()
                    .into_owned();
            }
        };
        let st = &mut f.settings;
        for p in [&mut st.roms, &mut st.snapshots, &mut st.media]
            .into_iter()
            .flatten()
        {
            abs(p);
        }
        let media_base = st.media.clone().map_or(base.clone(), PathBuf::from);
        for (_, p) in &mut f.profiles {
            // ROM paths count from the file, media from settings.media.
            let mut roms_only = Profile {
                roms: p.roms.take(),
                rom: std::mem::take(&mut p.rom),
                ..Profile::default()
            };
            roms_only.resolve_paths(&base);
            p.resolve_paths(&media_base);
            p.roms = roms_only.roms;
            p.rom = roms_only.rom;
        }
        Ok(f)
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p)
    }

    pub fn names(&self) -> Vec<&str> {
        self.profiles.iter().map(|(n, _)| n.as_str()).collect()
    }
}

/// YAML for one profile entry (`name:` and the profile indented below),
/// every line indented by `indent` spaces.
pub fn entry_yaml(name: &str, profile: &Profile, indent: usize) -> Result<String, String> {
    let mut map = serde_yaml_ng::Mapping::new();
    map.insert(
        serde_yaml_ng::Value::String(name.into()),
        serde_yaml_ng::to_value(profile).map_err(|e| e.to_string())?,
    );
    let text = serde_yaml_ng::to_string(&map).map_err(|e| e.to_string())?;
    let pad = " ".repeat(indent);
    Ok(text.lines().map(|l| format!("{pad}{l}\n")).collect())
}

/// The whole file as YAML.
fn file_yaml(file: &ProfileFile) -> Result<String, String> {
    let mut out = String::new();
    if file.settings != Settings::default() {
        let mut map = serde_yaml_ng::Mapping::new();
        map.insert(
            serde_yaml_ng::Value::String("settings".into()),
            serde_yaml_ng::to_value(&file.settings).map_err(|e| e.to_string())?,
        );
        out.push_str(&serde_yaml_ng::to_string(&map).map_err(|e| e.to_string())?);
    }
    out.push_str("profiles:\n");
    for (n, p) in &file.profiles {
        out.push_str(&entry_yaml(n, p, 2)?);
    }
    Ok(out)
}

/// The last top-level key of YAML text (lines starting without indent).
fn last_top_key(text: &str) -> Option<&str> {
    text.lines()
        .rfind(|l| !l.starts_with([' ', '\t', '#', '-']) && l.contains(':'))
        .and_then(|l| l.split(':').next())
        .map(str::trim)
}

/// Store `profile` as `name` in the file at `path`: appended as text when
/// the name is new and `profiles:` is the file's last part (the file's
/// comments stay), otherwise the whole file is written anew (comments are
/// lost then).
pub fn save(path: &Path, name: &str, profile: &Profile) -> Result<(), String> {
    let old = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut file = ProfileFile::parse(&old).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = if file.get(name).is_none() && last_top_key(&old) == Some("profiles") {
        let mut t = old;
        if !t.ends_with('\n') {
            t.push('\n');
        }
        t.push_str(&entry_yaml(name, profile, 2)?);
        t
    } else {
        match file.profiles.iter_mut().find(|(n, _)| n == name) {
            Some((_, p)) => *p = profile.clone(),
            None => file.profiles.push((name.into(), profile.clone())),
        }
        file_yaml(&file)?
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The user's configuration folder for mimic.
pub fn user_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("APPDATA")
        .or_else(|| var("XDG_CONFIG_HOME"))
        .or_else(|| var("HOME").map(|h| h.join(".config")))
        .map(|d| d.join("mimic"))
}

/// Where the profile file is: `explicit`, else `./profiles.yaml`, else the
/// user folder's; `None` if none of them exists.
pub fn find_file(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    let local = PathBuf::from(FILE_NAME);
    if local.is_file() {
        return Some(local);
    }
    user_dir()
        .map(|d| d.join(FILE_NAME))
        .filter(|p| p.is_file())
}

/// What [`prepare`] made of a command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prepared {
    /// Run with these arguments (the profile's flags first, then the
    /// others in their order) and the file's settings.
    Run(Vec<String>, Settings),
    /// Nothing to run (`--list-profiles`); this text is for the user.
    Done(String),
}

/// Handle the profile flags of a frontend's command line: `--profiles
/// FILE`, `--profile NAME`, `--list-profiles`, `--save-profile NAME`.
/// `messages` receives notes for the user (a saved profile).
pub fn prepare(
    args: &[String],
    frontend: Frontend,
    messages: &mut Vec<String>,
) -> Result<Prepared, String> {
    let mut file = None;
    let mut name = None;
    let mut save_as = None;
    let mut list = false;
    let mut other = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |n: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{n} needs a value"))
        };
        match a.as_str() {
            "--profiles" => file = Some(PathBuf::from(value(a)?)),
            "--profile" => name = Some(value(a)?),
            "--save-profile" => save_as = Some(value(a)?),
            "--list-profiles" => list = true,
            _ => other.push(a.clone()),
        }
    }
    let found = find_file(file.as_deref());
    let loaded = match &found {
        Some(p) if p.is_file() => ProfileFile::load(p)?,
        _ => ProfileFile::default(),
    };
    if list {
        let mut out = match &found {
            Some(p) if p.is_file() => format!("profiles in {}:\n", p.display()),
            _ => format!(
                "no profile file (./{FILE_NAME}{})\n",
                user_dir()
                    .map(|d| format!(" or {}", d.join(FILE_NAME).display()))
                    .unwrap_or_default()
            ),
        };
        for (n, p) in &loaded.profiles {
            let what = p.description.clone().unwrap_or_else(|| {
                format!(
                    "{} {}",
                    p.machine.as_deref().unwrap_or("?"),
                    p.model.as_deref().unwrap_or("")
                )
            });
            out.push_str(&format!("  {n:<20} {what}\n"));
        }
        return Ok(Prepared::Done(out));
    }
    let mut merged = match &name {
        Some(n) => loaded.get(n).cloned().ok_or_else(|| {
            let names = loaded.names();
            format!(
                "no profile '{n}'{}",
                if names.is_empty() {
                    " (no profile file found)".to_string()
                } else {
                    format!(" (there are: {})", names.join(", "))
                }
            )
        })?,
        None => Profile::default(),
    };
    let (cli, rest) = Profile::from_args(&other)?;
    merged.overlay(&cli);
    if let Some(n) = save_as {
        let mut stored = merged.clone();
        if let Ok(cwd) = std::env::current_dir() {
            stored.resolve_paths(&cwd);
        }
        // A copy under another name describes itself anew.
        if name.as_deref() != Some(n.as_str()) {
            stored.description = None;
        }
        let path = found.unwrap_or_else(|| PathBuf::from(FILE_NAME));
        save(&path, &n, &stored)?;
        messages.push(format!("saved profile '{n}' in {}", path.display()));
    }
    if merged.roms.is_none() {
        merged.roms.clone_from(&loaded.settings.roms);
    }
    let mut out = merged.to_args(frontend);
    out.extend(rest);
    Ok(Prepared::Run(out, loaded.settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
# Comment
settings:
  roms: myroms
  snapshots: shots
  media: disks
profiles:
  mac-se-755:
    description: Mac SE with System 7.5.5
    machine: mac
    model: se
    roms: roms
    options:
      ram: 4m
      drives: 2
      lc: true
    media:
      disks: [system.dsk, hd.hda]
    display: { crt: mac, scale: 3, footer: true, panel: false }
    input: { joykeys: cursor, joyport: 2, speed: 200 }
    debug: { server: true, preview: "127.0.0.1:7000" }
  c64:
    machine: c64
"#;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_a_file_in_order() {
        let f = ProfileFile::parse(EXAMPLE).unwrap();
        assert_eq!(f.names(), ["mac-se-755", "c64"]);
        let p = f.get("mac-se-755").unwrap();
        assert_eq!(p.machine.as_deref(), Some("mac"));
        assert_eq!(p.options["ram"], "4m");
        assert_eq!(p.options["drives"], "2");
        assert_eq!(p.options["lc"], "on");
        assert_eq!(p.media.disks, ["system.dsk", "hd.hda"]);
        assert_eq!(p.display.scale, Some(3));
        assert_eq!(p.debug.server, Some(Endpoint::On(true)));
        assert_eq!(f.settings.snapshots.as_deref(), Some("shots"));
        assert!(
            ProfileFile::parse("profiles:\n  x:\n    machin: c64\n").is_err(),
            "typos are errors"
        );
        assert!(ProfileFile::parse("profile:\n").is_err());
        assert!(ProfileFile::parse("").unwrap().profiles.is_empty());
    }

    #[test]
    fn command_lines_round_trip() {
        let p = ProfileFile::parse(EXAMPLE)
            .unwrap()
            .get("mac-se-755")
            .cloned()
            .unwrap();
        let a = p.to_args(Frontend::Desktop);
        let (back, rest) = Profile::from_args(&a).unwrap();
        assert!(rest.is_empty());
        let mut expect = p.clone();
        expect.description = None;
        assert_eq!(back, expect);
        // Headless: no window settings; web: no servers.
        let h = p.to_args(Frontend::Headless);
        assert!(!h.contains(&"--scale".to_string()) && h.contains(&"--debug".to_string()));
        assert!(!p.to_args(Frontend::Web).contains(&"--debug".to_string()));
    }

    #[test]
    fn the_command_line_overrides_the_profile() {
        let mut p = ProfileFile::parse(EXAMPLE)
            .unwrap()
            .get("mac-se-755")
            .cloned()
            .unwrap();
        let (cli, rest) = Profile::from_args(&args(
            "--model plus --option ram=2m --disk a.dsk --frames 10 --debug",
        ))
        .unwrap();
        assert_eq!(rest, args("--frames 10"));
        p.overlay(&cli);
        assert_eq!(p.model.as_deref(), Some("plus"));
        assert_eq!(p.options["ram"], "2m");
        assert_eq!(p.options["drives"], "2", "other options stay");
        assert_eq!(p.media.disks, ["a.dsk"], "the disk list is replaced");
        assert_eq!(p.debug.server, Some(Endpoint::On(true)));
    }

    #[test]
    fn prepare_save_and_list() {
        let dir = std::env::temp_dir().join(format!("mimic-profiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(FILE_NAME);
        std::fs::write(&file, EXAMPLE).unwrap();
        let f = file.to_string_lossy().to_string();
        let mut msgs = Vec::new();
        let run = prepare(
            &args(&format!("--profiles {f} --profile mac-se-755 --frames 5")),
            Frontend::Headless,
            &mut msgs,
        )
        .unwrap();
        let Prepared::Run(a, settings) = run else {
            panic!()
        };
        assert_eq!(&a[..4], &args("--machine mac --model se")[..]);
        assert!(a
            .iter()
            .any(|x| x.ends_with("hd.hda") && Path::new(x).is_absolute()));
        assert!(
            a.iter().any(|x| Path::new(x).ends_with("disks/hd.hda")),
            "media folder"
        );
        assert!(settings.snapshots.unwrap().ends_with("shots"));
        assert_eq!(&a[a.len() - 2..], &args("--frames 5")[..]);
        // A profile without a ROM folder takes the settings' one.
        let Prepared::Run(a, _) = prepare(
            &args(&format!("--profiles {f} --profile c64")),
            Frontend::Headless,
            &mut msgs,
        )
        .unwrap() else {
            panic!()
        };
        assert!(a.iter().any(|x| x.ends_with("myroms")));
        // Save a new profile: appended, the comment stays.
        prepare(
            &args(&format!(
                "--profiles {f} --machine vic20 --option ram=8k --save-profile vic8"
            )),
            Frontend::Desktop,
            &mut msgs,
        )
        .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("# Comment"), "appended: {text}");
        let back = ProfileFile::load(&file).unwrap();
        assert_eq!(back.names(), ["mac-se-755", "c64", "vic8"]);
        assert_eq!(back.get("vic8").unwrap().options["ram"], "8k");
        // Replace an existing one.
        prepare(
            &args(&format!(
                "--profiles {f} --machine c64 --model ntsc --save-profile c64"
            )),
            Frontend::Desktop,
            &mut msgs,
        )
        .unwrap();
        let back = ProfileFile::load(&file).unwrap();
        assert_eq!(back.get("c64").unwrap().model.as_deref(), Some("ntsc"));
        assert_eq!(back.names().len(), 3);
        assert_eq!(msgs.len(), 2);
        let Prepared::Done(list) = prepare(
            &args(&format!("--profiles {f} --list-profiles")),
            Frontend::Desktop,
            &mut msgs,
        )
        .unwrap() else {
            panic!()
        };
        assert!(list.contains("vic8") && list.contains("Mac SE with System 7.5.5"));
        assert!(prepare(
            &args(&format!("--profiles {f} --profile nope")),
            Frontend::Desktop,
            &mut msgs
        )
        .unwrap_err()
        .contains("there are"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
