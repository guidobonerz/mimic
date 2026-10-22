//! What this frontend lends the debugger and the preview: port devices
//! (via the `emu-machines` catalogue), image files (copy-on-write), and
//! viewer input as host events, and the live preview server (started
//! with `--preview` or on request from the debugger, e.g. GDB's `monitor
//! preview`).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use emu_core::input::{InputEvent, InputTarget, TimedInput};
use emu_core::media::Media;
use emu_core::port::{PortDevice, PortKind};
use emu_core::ControlId;
use emu_debug::host::HostServices;
use emu_debug::{Control, FrameHook};
use emu_input::{Axis, Button, HostEvent, Mapper, Profile as InputProfile};
use emu_preview::server::{FrameTap, InputSink, PreviewConfig, PreviewServer, RemoteInput};

/// Where the preview server listens when the debugger starts it.
const PREVIEW_ADDR: &str = "127.0.0.1:6580";

pub struct FrontendHost {
    pub preview: Arc<Mutex<Preview>>,
}

/// The live preview server of a debug session, once started.
pub struct Preview {
    machine: String,
    /// Set once the debug server runs (viewer input goes through it).
    pub control: Option<Control>,
    pub server: Option<PreviewServer>,
    pub url: Option<String>,
}

impl Preview {
    pub fn new(machine: &str) -> Self {
        Self {
            machine: machine.into(),
            control: None,
            server: None,
            url: None,
        }
    }

    /// Start the server on `bind`, fed by `tap`.
    pub fn start(&mut self, bind: SocketAddr, tap: FrameTap) -> Result<String, String> {
        let control = self
            .control
            .clone()
            .ok_or("the debug server is not running")?;
        let sink = input_sink(&self.machine, control);
        let server = PreviewServer::start(PreviewConfig { bind }, &self.machine, tap, Some(sink))
            .map_err(|e| format!("preview server: {e}"))?;
        let url = format!("http://{}/", server.addr());
        self.server = Some(server);
        self.url = Some(url.clone());
        Ok(url)
    }
}

/// Feed the preview from the session: frames, and audio for raw streams
/// (there is no audio device here).
pub fn frame_hook(tap: FrameTap) -> FrameHook {
    let mut samples = Vec::new();
    Box::new(move |m, running| {
        tap.publish(m, running);
        samples.clear();
        let format = m.audio(&mut samples);
        tap.publish_audio(&samples, format);
    })
}

/// Viewer input is mapped like local keys and gamepads, on the emulation
/// thread; panel controls go to the machine directly.
fn input_sink(machine: &str, control: Control) -> InputSink {
    let mapper = Arc::new(Mutex::new(Mapper::new(InputProfile::default_for(machine))));
    Arc::new(move |input| {
        if let RemoteInput::Control { id, value } = input {
            control.exec(move |s| {
                let at = s.machine().now();
                s.input(TimedInput {
                    at,
                    target: InputTarget::Panel,
                    event: InputEvent::Control {
                        id: ControlId(id),
                        value,
                    },
                });
            });
            return;
        }
        let Some(ev) = host_event(input) else {
            return;
        };
        let mapper = mapper.clone();
        control.exec(move |s| {
            let now = s.machine().now();
            let events = mapper.lock().map(|mut m| m.handle(&ev, now));
            for e in events.unwrap_or_default() {
                s.input(e);
            }
        });
    })
}

impl HostServices for FrontendHost {
    fn device_names(&self, kind: PortKind) -> Vec<&'static str> {
        emu_machines::device_names(kind)
    }

    fn create_device(&self, name: &str, kind: PortKind) -> Option<Box<dyn PortDevice>> {
        emu_machines::create_device(name, kind)
    }

    fn open_media(&self, path: &str) -> Result<Media, String> {
        media_backends::image::open(Path::new(path)).map_err(|e| format!("{path}: {e}"))
    }

    fn preview(&mut self) -> Result<(String, Option<FrameHook>), String> {
        let mut p = self.preview.lock().map_err(|_| "preview state poisoned")?;
        if let Some(url) = &p.url {
            return Ok((url.clone(), None));
        }
        let tap = FrameTap::new();
        let bind: SocketAddr = PREVIEW_ADDR.parse().map_err(|_| "bad preview address")?;
        // The usual port may be taken (another session): any free one then.
        let url = match p.start(bind, tap.clone()) {
            Ok(url) => url,
            Err(_) => p.start(SocketAddr::from(([127, 0, 0, 1], 0)), tap.clone())?,
        };
        Ok((url, Some(frame_hook(tap))))
    }
}

/// A preview viewer's input as a host event; `None` for unknown names.
pub fn host_event(input: RemoteInput) -> Option<HostEvent> {
    Some(match input {
        RemoteInput::Key { code, down } => HostEvent::Key { code, down },
        RemoteInput::Button { pad, button, down } => HostEvent::Button {
            pad,
            button: Button::from_name(&button)?,
            down,
        },
        RemoteInput::Axis { pad, axis, value } => HostEvent::Axis {
            pad,
            axis: Axis::from_name(&axis)?,
            value,
        },
        // Panel controls bypass the host mapping (see the preview sinks).
        RemoteInput::Control { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_input_becomes_host_events() {
        let ev = host_event(RemoteInput::Button {
            pad: 0,
            button: "South".into(),
            down: true,
        });
        assert_eq!(
            ev,
            Some(HostEvent::Button {
                pad: 0,
                button: Button::South,
                down: true
            })
        );
        assert_eq!(
            host_event(RemoteInput::Axis {
                pad: 0,
                axis: "RightZ".into(),
                value: 0.0
            }),
            None
        );
        let host = FrontendHost {
            preview: Arc::new(Mutex::new(Preview::new("c64"))),
        };
        assert!(host.open_media("missing.d64").is_err());
        assert_eq!(
            host.device_names(PortKind::CbmControl),
            ["joystick", "paddles", "mouse1351"]
        );
    }
}
