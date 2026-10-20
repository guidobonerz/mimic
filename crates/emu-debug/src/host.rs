//! What the debugger needs from the frontend, and input recordings.
//!
//! `emu-debug` sees only `emu-core` (golden rule 1), so creating port
//! devices (`devices-input`) and opening image files (`media-backends`) is
//! lent by the frontend through [`HostServices`].
//!
//! Recordings ([`Replay`]) hold a savestate taken when recording started
//! and every input event queued since, with its exact cycle. Loading the
//! state and queueing the events reproduces the run bit-exactly (golden
//! rule 3); `tests/replays/` drives regression tests with them.

use serde_json::{json, Value};

use emu_core::input::{Dirs, InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::media::Media;
use emu_core::port::{PortDevice, PortId, PortKind};
use emu_core::Cycle;

/// Frontend services for device and media commands.
pub trait HostServices: Send {
    /// Names of the devices [`HostServices::create_device`] can make for a
    /// port of `kind`, e.g. `["joystick"]`.
    fn device_names(&self, kind: PortKind) -> Vec<&'static str>;

    /// A new device `name` for a port of `kind`; `None` if unknown there.
    fn create_device(&self, name: &str, kind: PortKind) -> Option<Box<dyn PortDevice>>;

    /// Open an image file on this host (copy-on-write: the file stays
    /// untouched until the slot is flushed).
    fn open_media(&self, path: &str) -> Result<Media, String>;
}

/// Version of the recording format.
pub const REPLAY_VERSION: u32 = 1;
/// Upper bound on events in one recording.
pub const MAX_EVENTS: usize = 1 << 20;

/// An input recording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    pub machine: String,
    /// Machine state when recording started.
    pub state: Vec<u8>,
    /// The freeze list then (savestate form; empty: none).
    pub freezes: Vec<u8>,
    pub events: Vec<TimedInput>,
    /// Machine time when recording stopped.
    pub end: Cycle,
}

const DIR_NAMES: [(&str, Dirs); 4] = [
    ("up", Dirs::UP),
    ("down", Dirs::DOWN),
    ("left", Dirs::LEFT),
    ("right", Dirs::RIGHT),
];

pub fn target_json(t: InputTarget) -> Value {
    match t {
        InputTarget::Keyboard => json!("keyboard"),
        InputTarget::Port(p) => json!({"port": p.0}),
    }
}

pub fn parse_target(v: &Value) -> Result<InputTarget, String> {
    if v == "keyboard" {
        return Ok(InputTarget::Keyboard);
    }
    v["port"]
        .as_u64()
        .and_then(|p| u8::try_from(p).ok())
        .map(|p| InputTarget::Port(PortId(p)))
        .ok_or_else(|| format!("target must be \"keyboard\" or {{\"port\": n}}, not {v}"))
}

/// JSON form of an event: `{"type": "key", "key": 7, "down": true}`,
/// `{"type": "joystick", "dirs": ["up", "left"], "buttons": 1}`,
/// `{"type": "analog", "axis": 0, "value": 128}`,
/// `{"type": "mouse", "dx": 3, "dy": -1, "buttons": 0}`,
/// `{"type": "lightpen", "x": 100, "y": 50, "trigger": true}`.
pub fn event_json(e: &InputEvent) -> Value {
    match *e {
        InputEvent::Key { machine_key, down } => {
            json!({"type": "key", "key": machine_key.0, "down": down})
        }
        InputEvent::Digital { dirs, buttons } => {
            let names: Vec<&str> = DIR_NAMES
                .iter()
                .filter(|(_, d)| dirs.contains(*d))
                .map(|(n, _)| *n)
                .collect();
            json!({"type": "joystick", "dirs": names, "buttons": buttons})
        }
        InputEvent::Analog { axis, value } => {
            json!({"type": "analog", "axis": axis, "value": value})
        }
        InputEvent::MouseRel { dx, dy, buttons } => {
            json!({"type": "mouse", "dx": dx, "dy": dy, "buttons": buttons})
        }
        InputEvent::LightPen { x, y, trigger } => {
            json!({"type": "lightpen", "x": x, "y": y, "trigger": trigger})
        }
    }
}

fn field<T: TryFrom<i64>>(v: &Value, name: &str) -> Result<T, String> {
    v[name]
        .as_i64()
        .and_then(|n| T::try_from(n).ok())
        .ok_or_else(|| format!("'{name}' missing or out of range"))
}

fn flag(v: &Value, name: &str) -> Result<bool, String> {
    v[name]
        .as_bool()
        .ok_or_else(|| format!("'{name}' must be true or false"))
}

pub fn parse_event(v: &Value) -> Result<InputEvent, String> {
    Ok(match v["type"].as_str().unwrap_or_default() {
        "key" => InputEvent::Key {
            machine_key: MachineKey(field(v, "key")?),
            down: flag(v, "down")?,
        },
        "joystick" => {
            let mut dirs = Dirs::NONE;
            for d in v["dirs"].as_array().map(Vec::as_slice).unwrap_or_default() {
                let (_, bit) = DIR_NAMES
                    .iter()
                    .find(|(n, _)| d == n)
                    .ok_or_else(|| format!("unknown direction {d}"))?;
                dirs = dirs | *bit;
            }
            InputEvent::Digital {
                dirs,
                buttons: if v.get("buttons").is_some() {
                    field(v, "buttons")?
                } else {
                    0
                },
            }
        }
        "analog" => InputEvent::Analog {
            axis: field(v, "axis")?,
            value: field(v, "value")?,
        },
        "mouse" => InputEvent::MouseRel {
            dx: field(v, "dx")?,
            dy: field(v, "dy")?,
            buttons: field(v, "buttons")?,
        },
        "lightpen" => InputEvent::LightPen {
            x: field(v, "x")?,
            y: field(v, "y")?,
            trigger: flag(v, "trigger")?,
        },
        other => return Err(format!("unknown event type '{other}'")),
    })
}

impl Replay {
    pub fn to_json(&self) -> Value {
        let events: Vec<Value> = self
            .events
            .iter()
            .map(|e| json!({"at": e.at.0, "target": target_json(e.target), "event": event_json(&e.event)}))
            .collect();
        json!({
            "format": "polytrace-replay",
            "version": REPLAY_VERSION,
            "machine": self.machine,
            "state": crate::protocol::base64_encode(&self.state),
            "freezes": crate::protocol::base64_encode(&self.freezes),
            "end": self.end.0,
            "events": events,
        })
    }

    pub fn from_json(v: &Value) -> Result<Self, String> {
        if v["format"] != "polytrace-replay" {
            return Err("not a polytrace replay".into());
        }
        let version = v["version"].as_u64().unwrap_or(0);
        if version != u64::from(REPLAY_VERSION) {
            return Err(format!("unsupported replay version {version}"));
        }
        let list = v["events"].as_array().ok_or("events missing")?;
        if list.len() > MAX_EVENTS {
            return Err(format!("more than {MAX_EVENTS} events"));
        }
        let mut events = Vec::with_capacity(list.len());
        for e in list {
            events.push(TimedInput {
                at: Cycle(e["at"].as_u64().ok_or("event without 'at'")?),
                target: parse_target(&e["target"])?,
                event: parse_event(&e["event"])?,
            });
        }
        Ok(Self {
            machine: v["machine"].as_str().ok_or("machine missing")?.to_string(),
            state: crate::protocol::base64_decode(v["state"].as_str().ok_or("state missing")?)?,
            freezes: crate::protocol::base64_decode(v["freezes"].as_str().unwrap_or(""))?,
            events,
            end: Cycle(v["end"].as_u64().ok_or("end missing")?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_through_json() {
        let events = [
            InputEvent::Key {
                machine_key: MachineKey(63),
                down: true,
            },
            InputEvent::Digital {
                dirs: Dirs::UP | Dirs::LEFT,
                buttons: 1,
            },
            InputEvent::Analog {
                axis: 1,
                value: 200,
            },
            InputEvent::MouseRel {
                dx: -3,
                dy: 4,
                buttons: 2,
            },
            InputEvent::LightPen {
                x: 100,
                y: 50,
                trigger: false,
            },
        ];
        for e in events {
            assert_eq!(parse_event(&event_json(&e)).unwrap(), e);
        }
        let j = json!({"type": "joystick", "dirs": ["right"]});
        assert_eq!(
            parse_event(&j).unwrap(),
            InputEvent::Digital {
                dirs: Dirs::RIGHT,
                buttons: 0
            }
        );
        for bad in [
            json!({"type": "key", "key": -1, "down": true}),
            json!({"type": "key", "key": 1}),
            json!({"type": "joystick", "dirs": ["sideways"]}),
            json!({"type": "teleport"}),
        ] {
            assert!(parse_event(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn replay_round_trip() {
        let r = Replay {
            machine: "c64".into(),
            state: vec![1, 2, 3],
            freezes: vec![4, 5],
            events: vec![TimedInput {
                at: Cycle(1000),
                target: InputTarget::Port(PortId(1)),
                event: InputEvent::Digital {
                    dirs: Dirs::DOWN,
                    buttons: 0,
                },
            }],
            end: Cycle(5000),
        };
        assert_eq!(Replay::from_json(&r.to_json()).unwrap(), r);
        assert!(Replay::from_json(&json!({"format": "x"})).is_err());
        assert_eq!(
            parse_target(&json!("keyboard")).unwrap(),
            InputTarget::Keyboard
        );
        assert!(parse_target(&json!({"port": 300})).is_err());
    }
}
