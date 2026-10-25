//! The emulator as the web page drives it: files the user uploaded (ROMs,
//! disks, tapes, cartridges; nothing is bundled, golden rule 6), one
//! running machine, its picture as RGBA, its sound as float samples and
//! browser input mapped through `emu-input` (W3C `KeyboardEvent.code`
//! names, standard gamepad buttons).
//!
//! Plain Rust without any browser API, so it runs in native tests too;
//! [`crate::abi`] exports it to JavaScript.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use emu_core::input::{InputEvent, InputTarget, TimedInput};
use emu_core::media::{MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{ControlId, DebugHooks, FrameRef, Machine, ResetKind, StopReason};
use emu_input::{Axis, Button, HostEvent, Mapper, Profile};
use video_crt::field::{Deinterlace, FieldStage};

/// Longest a frame may run before `run_frame` gives up (a stuck machine
/// must not hang the page).
const MAX_FRAME_STEPS: usize = 4;

pub struct Running {
    pub id: String,
    pub(crate) machine: Box<dyn Machine>,
    mapper: Mapper,
    /// Freezes (cheats) live here, as in the debug session.
    pub(crate) hooks: DebugHooks,
    fields: FieldStage,
    /// Monitor profile for the CPU CRT path (`None`: raw pixels).
    crt: Option<video_crt::Profile>,
    /// Phosphor persistence history of the CRT path (not machine state).
    history: video_crt::Crt,
    /// A light pen/gun takes the pointer; a mouse-like device the captured mouse.
    wants_pointer: bool,
    wants_mouse: bool,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    aspect: f64,
    samples: Vec<i16>,
    audio: Vec<f32>,
}

#[derive(Default)]
pub struct WebEmu {
    /// Uploaded files by path (`roms/c64/kernal.bin`, `game.d64`).
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub running: Option<Running>,
    /// A cheat search in progress.
    pub(crate) search: Option<emu_cheats::search::Search>,
}

fn kind_name(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Disk => "disk",
        MediaKind::Tape => "tape",
        MediaKind::Cartridge => "cartridge",
        MediaKind::Program => "program",
        #[allow(unreachable_patterns)]
        _ => "other",
    }
}

fn kind_from(name: &str) -> Option<MediaKind> {
    match name {
        "disk" => Some(MediaKind::Disk),
        "tape" => Some(MediaKind::Tape),
        "cartridge" | "cart" => Some(MediaKind::Cartridge),
        "program" | "prg" => Some(MediaKind::Program),
        _ => None,
    }
}

impl WebEmu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Machines with their models and the ROMs they need.
    pub fn machines(&self) -> Value {
        let list: Vec<Value> = emu_machines::registry()
            .entries()
            .iter()
            .map(|e| {
                let roms: Vec<Value> = media_backends::roms::specs(e.id)
                    .iter()
                    .map(|s| {
                        json!({"role": s.role, "size": s.size, "optional": s.optional,
                               "names": s.prefixes})
                    })
                    .collect();
                json!({"id": e.id, "name": e.name, "models": e.models, "roms": roms})
            })
            .collect();
        json!(list)
    }

    pub fn add_file(&mut self, path: &str, data: Vec<u8>) {
        self.files.insert(path.to_string(), data);
    }

    pub fn remove_file(&mut self, path: &str) {
        self.files.remove(path);
    }

    pub fn files(&self) -> Value {
        let list: Vec<Value> = self
            .files
            .iter()
            .map(|(p, d)| {
                let kind = media_backends::image::kind_for_name(p).map(kind_name);
                json!({"path": p, "size": d.len(), "kind": kind})
            })
            .collect();
        json!(list)
    }

    fn file(&self, path: &str) -> Result<Vec<u8>, String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| format!("no uploaded file '{path}'"))
    }

    /// Start a machine: `{"machine", "model", "options": {k: v}, "crt":
    /// profile|null, "media": [{"file", "kind"}]}`. ROMs come from the
    /// uploaded files (matched by name and size as in a ROM folder) unless
    /// `options.roms` is `stub`.
    pub fn start(&mut self, cfg: &Value) -> Result<Value, String> {
        let id = cfg["machine"]
            .as_str()
            .ok_or("machine missing")?
            .to_string();
        let mut config = MachineConfig {
            model: cfg["model"]
                .as_str()
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            ..MachineConfig::default()
        };
        if let Some(opts) = cfg["options"].as_object() {
            for (k, v) in opts {
                let v = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                config.options.insert(k.clone(), v);
            }
        }
        if !config.options.contains_key("roms") {
            let files: Vec<(String, Vec<u8>)> = self
                .files
                .iter()
                .map(|(p, d)| (p.clone(), d.clone()))
                .collect();
            config.roms = media_backends::roms::match_files(&files, &id)?;
        }
        let media: Vec<(String, MediaKind)> = cfg["media"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|m| {
                        let file = m["file"].as_str()?.to_string();
                        let kind = m["kind"]
                            .as_str()
                            .and_then(kind_from)
                            .or_else(|| media_backends::image::kind_for_name(&file))?;
                        Some((file, kind))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let registry = emu_machines::registry();
        // Machines without a program slot take a program as a ROM role.
        let mut machine = registry.create(&id, &config).map_err(|e| e.to_string())?;
        for (file, kind) in &media {
            if *kind == MediaKind::Program && slot_of(machine.as_ref(), *kind, 0).is_none() {
                config.roms.insert("program".into(), self.file(file)?);
                machine = registry.create(&id, &config).map_err(|e| e.to_string())?;
            }
        }
        // A browser has no raw sockets: no network (the WiC64 and the
        // Ultimate command interface report "no network"). The UCI's DOS
        // folder is the uploaded files, in memory (writes stay there).
        if config.options.get("uci").is_some_and(|v| v == "on") {
            let mut folder = emu_core::host::MemFileHost::new();
            for (name, data) in &self.files {
                if let Ok(path) = emu_core::host::join_path("/", name) {
                    let _ = folder.insert(&path, data.clone());
                }
            }
            machine.attach_host(emu_core::host::HostServices::new(
                None,
                Some(Box::new(folder)),
            ));
        }
        let mut used: BTreeMap<u8, usize> = BTreeMap::new();
        for (file, kind) in &media {
            if *kind == MediaKind::Program && slot_of(machine.as_ref(), *kind, 0).is_none() {
                continue;
            }
            // Each disk goes into the next free drive that takes it.
            let n = used.entry(*kind as u8).or_insert(0);
            let mut last = None;
            loop {
                let Some(slot) = slot_of(machine.as_ref(), *kind, *n) else {
                    return Err(last.unwrap_or_else(|| {
                        format!("{file}: {id} has no free {} slot", kind_name(*kind))
                    }));
                };
                *n += 1;
                match insert(machine.as_mut(), slot, &id, file, self.file(file)?) {
                    Ok(()) => break,
                    Err(e) => last = Some(e),
                }
            }
        }
        let crt = match cfg["crt"].as_str() {
            None | Some("raw") | Some("") => None,
            Some("default") => Some(video_crt::Profile::default_for(&id)),
            Some(name) => {
                Some(video_crt::Profile::preset(name).ok_or(format!("unknown monitor '{name}'"))?)
            }
        };
        let profile = Profile::for_options(&id, &config.options);
        let wants_pointer = !profile.pointer.is_empty();
        let wants_mouse = !profile.mouse.is_empty();
        let mapper = Mapper::new(profile);
        self.running = Some(Running {
            id,
            machine,
            mapper,
            hooks: DebugHooks::new(),
            fields: FieldStage::new(Deinterlace::Weave),
            crt,
            history: video_crt::Crt::new(),
            wants_pointer,
            wants_mouse,
            rgba: Vec::new(),
            width: 0,
            height: 0,
            aspect: 1.0,
            samples: Vec::new(),
            audio: Vec::new(),
        });
        self.search = None;
        let r = self.running.as_mut().expect("just set");
        r.render();
        // The cheat database (uploaded cheat files) for the images.
        let mut games = Vec::new();
        for (file, _) in &media {
            let data = self.file(file)?;
            games.extend(self.cheats_for_image(&data)?);
        }
        let mut info = self.info();
        info["cheats"] = json!(games);
        Ok(info)
    }

    pub fn stop(&mut self) {
        self.running = None;
    }

    /// Slots, ports, controls, screens and the frame rate of the machine.
    pub fn info(&self) -> Value {
        let Some(r) = &self.running else {
            return json!({"running": false});
        };
        let m = r.machine.as_ref();
        let slots: Vec<Value> = m
            .media_slots()
            .iter()
            .map(|s| json!({"id": s.id.0, "name": s.name, "kind": kind_name(s.kind)}))
            .collect();
        let ports: Vec<Value> = m
            .ports()
            .iter()
            .map(|p| json!({"id": p.id.0, "name": p.name, "kind": format!("{:?}", p.kind)}))
            .collect();
        json!({
            "running": true,
            "machine": r.id,
            "slots": slots,
            "ports": ports,
            "controls": controls_json(m),
            "fps": frame_rate(&m.framebuffer()),
            "width": r.width,
            "height": r.height,
            "aspect": r.aspect,
            "pointer": r.wants_pointer,
            "mouse": r.wants_mouse,
        })
    }

    pub fn run_frame(&mut self) -> Result<(), String> {
        let r = self.running.as_mut().ok_or("no machine running")?;
        for _ in 0..MAX_FRAME_STEPS {
            match r.machine.run_frame(&mut r.hooks) {
                StopReason::FrameDone => break,
                StopReason::Fault { device, reason } => {
                    return Err(format!("{} fault: {reason}", device.0))
                }
                // A jam is the guest's own doing: the picture stays.
                _ => {}
            }
        }
        r.render();
        r.samples.clear();
        let format = r.machine.audio(&mut r.samples);
        let scale = 1.0 / 32768.0;
        if format.channels == 2 {
            r.audio
                .extend(r.samples.iter().map(|&s| f32::from(s) * scale));
        } else {
            for &s in &r.samples {
                let v = f32::from(s) * scale;
                r.audio.extend([v, v]);
            }
        }
        // Keep at most ~0.5 s if nobody takes it.
        let max = 2 * 24_000;
        if r.audio.len() > max {
            let cut = r.audio.len() - max;
            r.audio.drain(..cut);
        }
        Ok(())
    }

    pub fn frame(&self) -> Option<(&[u8], u32, u32)> {
        self.running
            .as_ref()
            .map(|r| (r.rgba.as_slice(), r.width, r.height))
    }

    /// Stereo samples (interleaved, 48 kHz) since the last call.
    pub fn take_audio(&mut self) -> Vec<f32> {
        self.running
            .as_mut()
            .map(|r| std::mem::take(&mut r.audio))
            .unwrap_or_default()
    }

    fn host(&mut self, ev: HostEvent) {
        if let Some(r) = &mut self.running {
            let now = r.machine.now();
            for e in r.mapper.handle(&ev, now) {
                r.machine.input(e);
            }
        }
    }

    pub fn key(&mut self, code: &str, down: bool) {
        self.host(HostEvent::Key {
            code: code.to_string(),
            down,
        });
    }

    pub fn button(&mut self, pad: u8, name: &str, down: bool) -> bool {
        let Some(button) = Button::from_name(name) else {
            return false;
        };
        self.host(HostEvent::Button { pad, button, down });
        true
    }

    pub fn axis(&mut self, pad: u8, name: &str, value: f32) -> bool {
        let Some(axis) = Axis::from_name(name) else {
            return false;
        };
        self.host(HostEvent::Axis { pad, axis, value });
        true
    }

    pub fn mouse(&mut self, dx: i16, dy: i16, buttons: u8) {
        self.host(HostEvent::Mouse { dx, dy, buttons });
    }

    /// The pointer over the picture for a light pen or light gun, as a
    /// fraction of the picture's width and height (`None` off the picture),
    /// so the CRT picture's other size does not matter.
    pub fn pointer(&mut self, pos: Option<(f64, f64)>, buttons: u8) {
        let Some(r) = &self.running else { return };
        let f = r.machine.framebuffer();
        let pos = pos.map(|(fx, fy)| {
            let x = (fx.clamp(0.0, 0.9999) * f64::from(f.width)) as u16;
            let y = (fy.clamp(0.0, 0.9999) * f64::from(f.height)) as u16;
            (x, y)
        });
        self.host(HostEvent::Pointer { pos, buttons });
    }

    pub fn control(&mut self, id: u16, value: u8) {
        if let Some(r) = &mut self.running {
            let at = r.machine.now();
            r.machine.input(TimedInput {
                at,
                target: InputTarget::Panel,
                event: InputEvent::Control {
                    id: ControlId(id),
                    value,
                },
            });
        }
    }

    /// Insert an uploaded file; the game of the cheats it brought, if any.
    pub fn insert(&mut self, slot: u8, path: &str) -> Result<Option<String>, String> {
        let data = self.file(path)?;
        let r = self.running.as_mut().ok_or("no machine running")?;
        insert(r.machine.as_mut(), SlotId(slot), &r.id, path, data.clone())?;
        self.cheats_for_image(&data)
    }

    pub fn eject(&mut self, slot: u8) -> Result<(), String> {
        let r = self.running.as_mut().ok_or("no machine running")?;
        r.machine
            .media_slot(SlotId(slot))
            .ok_or("no such slot")?
            .eject();
        Ok(())
    }

    pub fn reset(&mut self, hard: bool) {
        if let Some(r) = &mut self.running {
            r.machine.reset(if hard {
                ResetKind::Hard
            } else {
                ResetKind::Soft
            });
            r.history.reset();
        }
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        self.running.as_ref().map(|r| r.machine.save_state())
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        let r = self.running.as_mut().ok_or("no machine running")?;
        r.machine
            .load_state(data)
            .map_err(|e| format!("savestate rejected: {e:?}"))?;
        r.history.reset();
        r.render();
        Ok(())
    }
}

impl Running {
    /// The current picture as RGBA: woven fields, raw or through the CPU
    /// CRT path.
    fn render(&mut self) {
        let m = self.machine.as_ref();
        let frame = m.framebuffer();
        self.aspect = video_crt::cpu::pixel_aspect(&frame);
        if let Some(profile) = &self.crt {
            let img = self.history.render(&frame, profile, 2);
            self.width = img.width;
            self.height = img.height;
            self.aspect = 1.0;
            self.rgba.clear();
            self.rgba.extend(
                img.rgb
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2], 255]),
            );
            return;
        }
        let (w, h) = (frame.width, frame.height);
        self.rgba.clear();
        if let Some((fw, fh, px)) = self.fields.process(&frame) {
            self.width = fw;
            self.height = fh;
            self.rgba
                .extend(px.iter().flat_map(|c| [c.r, c.g, c.b, 255]));
            return;
        }
        self.width = w;
        self.height = h;
        self.rgba.reserve(w as usize * h as usize * 4);
        for y in 0..h {
            for x in 0..w {
                let c = frame.rgb_at(x, y).unwrap_or_default();
                self.rgba.extend([c.r, c.g, c.b, 255]);
            }
        }
    }
}

fn frame_rate(f: &FrameRef<'_>) -> f64 {
    let s = f.signal;
    let per_field = f64::from(s.total_pixels) * f64::from(s.lines_per_field);
    if s.pixel_clock_hz == 0 || per_field == 0.0 {
        return 50.0;
    }
    (f64::from(s.pixel_clock_hz) / per_field).clamp(10.0, 120.0)
}

fn controls_json(m: &dyn Machine) -> Value {
    let list: Vec<Value> = m
        .controls()
        .iter()
        .map(|c| {
            let (kind, positions): (&str, &[&str]) = match c.kind {
                emu_core::ControlKind::Button => ("button", &[]),
                emu_core::ControlKind::Switch(p) => ("switch", p),
            };
            json!({"id": c.id.0, "name": c.name, "kind": kind, "positions": positions,
                   "value": c.value})
        })
        .collect();
    json!(list)
}

/// The `n`-th slot of `kind`.
fn slot_of(m: &dyn Machine, kind: MediaKind, n: usize) -> Option<SlotId> {
    m.media_slots()
        .into_iter()
        .filter(|s| s.kind == kind)
        .nth(n)
        .map(|s| s.id)
}

fn insert(
    m: &mut dyn Machine,
    slot: SlotId,
    machine_id: &str,
    file: &str,
    data: Vec<u8>,
) -> Result<(), String> {
    let kind = m
        .media_slots()
        .into_iter()
        .find(|s| s.id == slot)
        .ok_or("no such slot")?
        .kind;
    let media = match kind {
        MediaKind::Tape => media_backends::image::tape_from_bytes(file, data, machine_id)
            .map_err(|e| format!("{file}: {e}"))?,
        k => media_backends::image::from_bytes(file, data, k),
    };
    m.media_slot(slot)
        .ok_or("no such slot")?
        .insert(media)
        .map_err(|e| format!("{file}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_runs_with_stub_roms_and_takes_input() {
        let mut emu = WebEmu::new();
        let list = emu.machines();
        assert!(list.as_array().unwrap().iter().any(|m| m["id"] == "c64"));
        let info = emu
            .start(&json!({"machine": "c64", "options": {"roms": "stub"}}))
            .unwrap();
        assert_eq!(info["machine"], "c64");
        assert!(info["slots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["kind"] == "disk"));
        for _ in 0..3 {
            emu.run_frame().unwrap();
        }
        let (rgba, w, h) = emu.frame().unwrap();
        assert_eq!((w, h), (384, 272));
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        assert!(rgba.chunks(4).all(|p| p[3] == 255));
        let audio = emu.take_audio();
        assert!(audio.len() > 1000, "stereo samples: {}", audio.len());
        assert!(emu.take_audio().is_empty());
        emu.key("KeyA", true);
        emu.key("KeyA", false);
        assert!(emu.button(0, "South", true));
        assert!(!emu.button(0, "Turbo", true));
        let state = emu.save_state().unwrap();
        emu.run_frame().unwrap();
        emu.load_state(&state).unwrap();
        assert!(emu.load_state(&[1, 2, 3]).is_err());
    }

    #[test]
    fn roms_and_media_come_from_uploaded_files() {
        let mut emu = WebEmu::new();
        let err = emu.start(&json!({"machine": "c64"})).unwrap_err();
        assert!(err.contains("kernal"), "{err}");
        // A tape from an uploaded PRG, recorded as the KERNAL saves it.
        emu.add_file("game.prg", vec![0x01, 0x08, 0x60]);
        let info = emu
            .start(&json!({"machine": "c64", "options": {"roms": "stub"},
                           "media": [{"file": "game.prg", "kind": "tape"}]}))
            .unwrap();
        assert_eq!(info["running"], true);
        assert!(emu.insert(9, "game.prg").is_err());
        assert!(emu.insert(0, "nothing.d64").is_err());
        let files = emu.files();
        assert_eq!(files[0]["kind"], "program");
        emu.remove_file("game.prg");
        assert!(emu.files().as_array().unwrap().is_empty());
    }

    #[test]
    fn crt_pictures_and_other_machines() {
        let mut emu = WebEmu::new();
        emu.start(&json!({"machine": "atari2600", "options": {"roms": "stub"}, "crt": "default"}))
            .unwrap();
        emu.run_frame().unwrap();
        let (rgba, w, h) = emu.frame().unwrap();
        assert!(w > 0 && h > 0 && rgba.len() == (w * h * 4) as usize);
        assert!(emu
            .start(&json!({"machine": "c64", "options": {"roms": "stub"}, "crt": "x"}))
            .is_err());
        assert!(emu.start(&json!({"machine": "nothing"})).is_err());
    }
}
