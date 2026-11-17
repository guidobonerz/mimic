//! What this frontend lends the debugger and the preview: port devices
//! (via the `mimic-machines` catalogue), image files (copy-on-write), and
//! viewer input as host events.

use std::path::Path;

use mimic_core::media::Media;
use mimic_core::port::{PortDevice, PortKind};
use mimic_debug::host::HostServices;
use mimic_input::{Axis, Button, HostEvent};
use mimic_preview::server::RemoteInput;

pub struct FrontendHost;

impl HostServices for FrontendHost {
    fn device_names(&self, kind: PortKind) -> Vec<&'static str> {
        mimic_machines::device_names(kind)
    }

    fn create_device(&self, name: &str, kind: PortKind) -> Option<Box<dyn PortDevice>> {
        mimic_machines::create_device(name, kind)
    }

    fn open_media(&self, path: &str) -> Result<Media, String> {
        media_backends::image::open(Path::new(path)).map_err(|e| format!("{path}: {e}"))
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
        assert!(FrontendHost.open_media("missing.d64").is_err());
        assert_eq!(
            FrontendHost.device_names(PortKind::CbmControl),
            ["joystick", "paddles", "mouse1351", "lightpen"]
        );
    }
}
