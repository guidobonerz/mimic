//! Turns host events into machine input events according to a [`Profile`].
//!
//! The frontend feeds every host event together with `machine.now()` and
//! passes the returned [`TimedInput`]s to `Machine::input`. Recording those
//! is all an input replay needs.

use std::collections::{BTreeMap, BTreeSet};

use emu_core::input::{Dirs, InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::port::PortId;
use emu_core::Cycle;

use crate::keymap::{self, Keymap};
use crate::profile::{JoyKeys, JoystickMap, JoystickSource, KeyboardMode, Profile};
use crate::{Axis, Button, HostEvent};

#[derive(Debug, Default)]
struct JoyState {
    key_dirs: Dirs,
    key_fire: bool,
    pad_buttons: Dirs,
    pad_axes: Dirs,
    pad_fire: bool,
    /// Further gamepad buttons as event button bits: East bit 1 (a second
    /// fire button; the NES's B), Select bit 2, Start bit 3.
    pad_extra: u8,
    /// Last state sent, to emit only changes.
    sent: (Dirs, u8),
}

impl JoyState {
    fn current(&self) -> (Dirs, u8) {
        let dirs = self.key_dirs | self.pad_buttons | self.pad_axes;
        (
            dirs,
            u8::from(self.key_fire || self.pad_fire) | self.pad_extra,
        )
    }
}

pub struct Mapper {
    keymap: Option<&'static Keymap>,
    profile: Profile,
    joys: Vec<JoyState>,
    /// How many host keys hold each machine key (cursor keys share SHIFT).
    held: BTreeMap<MachineKey, u32>,
    /// Host keys currently down, to ignore auto-repeat.
    host_down: BTreeSet<String>,
    /// Left stick per pad: (x, y).
    sticks: BTreeMap<u8, (f32, f32)>,
    /// Paddle buttons held per pad (bit 0 South, bit 1 East).
    paddle_fire: BTreeMap<u8, u8>,
}

fn set(d: &mut Dirs, dir: Dirs, on: bool) {
    if on {
        d.0 |= dir.0;
    } else {
        d.0 &= !dir.0;
    }
}

impl Mapper {
    pub fn new(profile: Profile) -> Self {
        let keymap = match profile.keyboard {
            KeyboardMode::Positional => keymap::positional(&profile.machine),
            KeyboardMode::None => None,
        };
        let joys = profile
            .joystick
            .iter()
            .map(|_| JoyState::default())
            .collect();
        Self {
            keymap,
            profile,
            joys,
            held: BTreeMap::new(),
            host_down: BTreeSet::new(),
            sticks: BTreeMap::new(),
            paddle_fire: BTreeMap::new(),
        }
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The keys playing the keyboard joystick now.
    pub fn joy_keys(&self) -> JoyKeys {
        let keys = self
            .profile
            .joystick
            .iter()
            .find(|j| matches!(j.source, JoystickSource::Keys { .. }));
        match keys {
            None => JoyKeys::Off,
            Some(j) => JoyKeys::ALL
                .into_iter()
                .find(|k| k.source().as_ref() == Some(&j.source))
                .unwrap_or(JoyKeys::Numpad),
        }
    }

    /// The port the joysticks (keyboard and gamepad) play; `None` without
    /// any.
    pub fn joy_port(&self) -> Option<u8> {
        self.profile.joystick.first().map(|j| j.port)
    }

    /// Move every joystick (keyboard and gamepad) to `port`. The old port
    /// is released; returns the events for that and for the new port's
    /// state, at `now`.
    pub fn set_joy_port(&mut self, port: u8, now: Cycle) -> Vec<TimedInput> {
        let mut out = Vec::new();
        let old: BTreeSet<u8> = self.profile.joystick.iter().map(|j| j.port).collect();
        if old.is_empty() || old.iter().all(|&p| p == port) {
            return out;
        }
        for p in old.into_iter().filter(|&p| p != port) {
            out.push(TimedInput {
                at: now,
                target: InputTarget::Port(PortId(p)),
                event: InputEvent::Digital {
                    dirs: Dirs::NONE,
                    buttons: 0,
                },
            });
        }
        for (map, joy) in self.profile.joystick.iter_mut().zip(&mut self.joys) {
            map.port = port;
            // The new port has not heard from this joystick yet.
            joy.sent = (Dirs(0xff), 0xff);
        }
        self.flush_joysticks(now, &mut out);
        out
    }

    /// Let other keys play the keyboard joystick (on the port of the
    /// profile's first joystick). Directions and fire held on the old keys
    /// are released; returns the events for that, at `now`.
    pub fn set_joy_keys(&mut self, keys: JoyKeys, now: Cycle) -> Vec<TimedInput> {
        let mut out = Vec::new();
        let Some(port) = self.profile.joystick.first().map(|j| j.port) else {
            return out;
        };
        let mut i = 0;
        while i < self.profile.joystick.len() {
            if matches!(self.profile.joystick[i].source, JoystickSource::Keys { .. }) {
                self.profile.joystick.remove(i);
                self.joys.remove(i);
            } else {
                i += 1;
            }
        }
        if let Some(source) = keys.source() {
            self.profile
                .joystick
                .insert(0, JoystickMap { port, source });
            self.joys.insert(0, JoyState::default());
        }
        self.flush_joysticks(now, &mut out);
        out
    }

    /// Map one host event; returns the machine events to queue at `now`.
    pub fn handle(&mut self, ev: &HostEvent, now: Cycle) -> Vec<TimedInput> {
        let mut out = Vec::new();
        match ev {
            HostEvent::Key { code, down } => {
                let repeat = *down && self.host_down.contains(code);
                if *down {
                    self.host_down.insert(code.clone());
                } else {
                    self.host_down.remove(code);
                }
                if repeat {
                    return out;
                }
                if self.joystick_key(code, *down) {
                    self.flush_joysticks(now, &mut out);
                } else {
                    self.keyboard_key(code, *down, now, &mut out);
                }
            }
            HostEvent::Button { pad, button, down } => {
                out.extend(self.paddle_buttons(*pad, *button, *down, now));
                for (map, joy) in self.profile.joystick.iter().zip(&mut self.joys) {
                    if let JoystickSource::Gamepad { pad: p, fire, .. } = &map.source {
                        if p != pad {
                            continue;
                        }
                        match button {
                            Button::DPadUp => set(&mut joy.pad_buttons, Dirs::UP, *down),
                            Button::DPadDown => set(&mut joy.pad_buttons, Dirs::DOWN, *down),
                            Button::DPadLeft => set(&mut joy.pad_buttons, Dirs::LEFT, *down),
                            Button::DPadRight => set(&mut joy.pad_buttons, Dirs::RIGHT, *down),
                            b if fire.contains(b) => joy.pad_fire = *down,
                            Button::East | Button::Select | Button::Start => {
                                let bit = match button {
                                    Button::East => 2,
                                    Button::Select => 4,
                                    _ => 8,
                                };
                                if *down {
                                    joy.pad_extra |= bit;
                                } else {
                                    joy.pad_extra &= !bit;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                self.flush_joysticks(now, &mut out);
            }
            HostEvent::Axis { pad, axis, value } => {
                let stick = self.sticks.entry(*pad).or_default();
                match axis {
                    Axis::LeftX => stick.0 = *value,
                    Axis::LeftY => stick.1 = *value,
                }
                let (x, y) = *stick;
                for (map, joy) in self.profile.joystick.iter().zip(&mut self.joys) {
                    if let JoystickSource::Gamepad {
                        pad: p, deadzone, ..
                    } = &map.source
                    {
                        if p != pad {
                            continue;
                        }
                        let mut d = Dirs::NONE;
                        // Host convention: y grows downwards (gilrs: up is +).
                        set(&mut d, Dirs::LEFT, x < -deadzone);
                        set(&mut d, Dirs::RIGHT, x > *deadzone);
                        set(&mut d, Dirs::UP, y < -deadzone);
                        set(&mut d, Dirs::DOWN, y > *deadzone);
                        joy.pad_axes = d;
                    }
                }
                self.flush_joysticks(now, &mut out);
                self.paddle_axes(*pad, now, &mut out);
            }
            HostEvent::Mouse { dx, dy, buttons } => {
                for map in &self.profile.mouse {
                    out.push(TimedInput {
                        at: now,
                        target: InputTarget::Port(PortId(map.port)),
                        event: InputEvent::MouseRel {
                            dx: *dx,
                            dy: *dy,
                            buttons: *buttons,
                        },
                    });
                }
            }
        }
        out
    }

    /// Stick X/Y of `pad` as the positions of the paddles it turns.
    fn paddle_axes(&self, pad: u8, now: Cycle, out: &mut Vec<TimedInput>) {
        let (x, y) = self.sticks.get(&pad).copied().unwrap_or_default();
        let pos = |v: f32| ((v.clamp(-1.0, 1.0) + 1.0) * 32767.5) as u16;
        for map in self.profile.paddles.iter().filter(|m| m.pad == pad) {
            for (axis, v) in [(0, x), (1, y)] {
                out.push(TimedInput {
                    at: now,
                    target: InputTarget::Port(PortId(map.port)),
                    event: InputEvent::Analog {
                        axis,
                        value: pos(v),
                    },
                });
            }
        }
    }

    /// South/East of `pad` as the paddles' buttons.
    fn paddle_buttons(
        &mut self,
        pad: u8,
        button: Button,
        down: bool,
        now: Cycle,
    ) -> Vec<TimedInput> {
        let mut out = Vec::new();
        let bit = match button {
            Button::South => 1,
            Button::East => 2,
            _ => return out,
        };
        let state = self.paddle_fire.entry(pad).or_default();
        if down {
            *state |= bit;
        } else {
            *state &= !bit;
        }
        let buttons = *state;
        for map in self.profile.paddles.iter().filter(|m| m.pad == pad) {
            out.push(TimedInput {
                at: now,
                target: InputTarget::Port(PortId(map.port)),
                event: InputEvent::Digital {
                    dirs: Dirs::NONE,
                    buttons,
                },
            });
        }
        out
    }

    /// Returns true if `code` drives a keyboard joystick.
    fn joystick_key(&mut self, code: &str, down: bool) -> bool {
        let mut used = false;
        for (map, joy) in self.profile.joystick.iter().zip(&mut self.joys) {
            if let JoystickSource::Keys {
                up,
                down: dn,
                left,
                right,
                fire,
            } = &map.source
            {
                for (name, dir) in [
                    (up, Dirs::UP),
                    (dn, Dirs::DOWN),
                    (left, Dirs::LEFT),
                    (right, Dirs::RIGHT),
                ] {
                    if name == code {
                        set(&mut joy.key_dirs, dir, down);
                        used = true;
                    }
                }
                if fire.iter().any(|f| f == code) {
                    joy.key_fire = down;
                    used = true;
                }
            }
        }
        used
    }

    fn flush_joysticks(&mut self, now: Cycle, out: &mut Vec<TimedInput>) {
        // Several sources may feed one port: merge per port.
        let mut ports: BTreeMap<u8, (Dirs, u8)> = BTreeMap::new();
        for (map, joy) in self.profile.joystick.iter().zip(&self.joys) {
            let (d, f) = joy.current();
            let e = ports.entry(map.port).or_default();
            e.0 = e.0 | d;
            e.1 |= f;
        }
        for (map, joy) in self.profile.joystick.iter().zip(&mut self.joys) {
            let state = ports[&map.port];
            if joy.sent != state {
                joy.sent = state;
                if !out
                    .iter()
                    .any(|e| e.target == InputTarget::Port(PortId(map.port)))
                {
                    out.push(TimedInput {
                        at: now,
                        target: InputTarget::Port(PortId(map.port)),
                        event: InputEvent::Digital {
                            dirs: state.0,
                            buttons: state.1,
                        },
                    });
                }
            }
        }
    }

    fn keyboard_key(&mut self, code: &str, down: bool, now: Cycle, out: &mut Vec<TimedInput>) {
        let Some(map) = self.keymap else { return };
        let Some(keys) = map.lookup(code) else { return };
        for k in keys {
            let n = self.held.entry(k).or_insert(0);
            let before = *n;
            if down {
                *n += 1;
            } else {
                *n = n.saturating_sub(1);
            }
            if (before == 0) != (*n == 0) {
                out.push(TimedInput {
                    at: now,
                    target: InputTarget::Keyboard,
                    event: InputEvent::Key {
                        machine_key: k,
                        down,
                    },
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: &str, down: bool) -> HostEvent {
        HostEvent::Key {
            code: code.into(),
            down,
        }
    }

    fn keys_of(ev: &[TimedInput]) -> Vec<(u16, bool)> {
        ev.iter()
            .filter_map(|e| match e.event {
                InputEvent::Key { machine_key, down } => Some((machine_key.0, down)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn joystick_keys_switch_between_numpad_cursor_and_off() {
        let mut m = Mapper::new(Profile::default_for("atari2600"));
        assert_eq!(m.joy_keys(), JoyKeys::Cursor, "no keyboard: cursor keys");
        let t = Cycle(1);
        let ev = m.handle(&key("ArrowLeft", true), t);
        assert_eq!(
            ev[0].event,
            InputEvent::Digital {
                dirs: Dirs::LEFT,
                buttons: 0
            }
        );
        let ev = m.handle(&key("Space", true), t);
        assert_eq!(
            ev[0].event,
            InputEvent::Digital {
                dirs: Dirs::LEFT,
                buttons: 1
            }
        );
        // Switching releases what the old keys held.
        let ev = m.set_joy_keys(JoyKeys::Numpad, t);
        assert_eq!(
            ev[0].event,
            InputEvent::Digital {
                dirs: Dirs::NONE,
                buttons: 0
            }
        );
        assert_eq!(m.joy_keys(), JoyKeys::Numpad);
        assert!(
            m.handle(&key("ArrowUp", true), t).is_empty(),
            "the 2600 has no keyboard"
        );
        let ev = m.handle(&key("Numpad8", true), t);
        assert_eq!(
            ev[0].event,
            InputEvent::Digital {
                dirs: Dirs::UP,
                buttons: 0
            }
        );
        m.set_joy_keys(JoyKeys::Off, t);
        assert_eq!(m.joy_keys(), JoyKeys::Off);
        assert!(m.handle(&key("Numpad2", true), t).is_empty());
        // On a computer the cursor keys type until they play the joystick.
        let mut c64 = Mapper::new(Profile::default_for("c64"));
        assert_eq!(c64.joy_keys(), JoyKeys::Numpad);
        assert!(!keys_of(&c64.handle(&key("ArrowDown", true), t)).is_empty());
        c64.handle(&key("ArrowDown", false), t);
        c64.set_joy_keys(JoyKeys::Cursor, t);
        let ev = c64.handle(&key("ArrowDown", true), t);
        assert_eq!(ev[0].target, InputTarget::Port(PortId(1)), "control port 2");
        assert_eq!(
            Mapper::new(Profile::default_for("cpc")).joy_keys(),
            JoyKeys::Off
        );
    }

    #[test]
    fn joysticks_move_to_another_port() {
        let mut m = Mapper::new(Profile::default_for("c64"));
        assert_eq!(m.joy_port(), Some(1), "control port 2");
        let t = Cycle(5);
        m.handle(&key("Numpad8", true), t);
        let ev = m.set_joy_port(0, t);
        let released = TimedInput {
            at: t,
            target: InputTarget::Port(PortId(1)),
            event: InputEvent::Digital {
                dirs: Dirs::NONE,
                buttons: 0,
            },
        };
        assert_eq!(ev[0], released, "the old port is let go");
        assert_eq!(ev[1].target, InputTarget::Port(PortId(0)));
        assert_eq!(
            ev[1].event,
            InputEvent::Digital {
                dirs: Dirs::UP,
                buttons: 0
            },
            "still held"
        );
        assert_eq!(m.joy_port(), Some(0));
        assert!(m.set_joy_port(0, t).is_empty(), "already there");
        let ev = m.handle(
            &HostEvent::Button {
                pad: 0,
                button: Button::South,
                down: true,
            },
            t,
        );
        assert_eq!(
            ev[0].target,
            InputTarget::Port(PortId(0)),
            "the gamepad moved too"
        );
    }

    #[test]
    fn positional_keys_with_shared_shift() {
        let mut m = Mapper::new(Profile::default_for("c64"));
        let t = Cycle(7);
        assert_eq!(keys_of(&m.handle(&key("KeyA", true), t)), [(10, true)]);
        assert!(m.handle(&key("KeyA", true), t).is_empty(), "auto-repeat");
        assert_eq!(
            keys_of(&m.handle(&key("ShiftRight", true), t)),
            [(52, true)]
        );
        // Cursor up adds CRSR DOWN; SHIFT is already held.
        assert_eq!(keys_of(&m.handle(&key("ArrowUp", true), t)), [(7, true)]);
        assert_eq!(keys_of(&m.handle(&key("ArrowUp", false), t)), [(7, false)]);
        assert_eq!(
            keys_of(&m.handle(&key("ShiftRight", false), t)),
            [(52, false)]
        );
        assert_eq!(m.handle(&key("KeyA", false), t)[0].at, t);
    }

    #[test]
    fn keypad_joystick_goes_to_port_2() {
        let mut m = Mapper::new(Profile::default_for("c64"));
        let ev = m.handle(&key("Numpad8", true), Cycle(1));
        assert_eq!(
            ev,
            [TimedInput {
                at: Cycle(1),
                target: InputTarget::Port(PortId(1)),
                event: InputEvent::Digital {
                    dirs: Dirs::UP,
                    buttons: 0
                },
            }]
        );
        let ev = m.handle(&key("Numpad0", true), Cycle(2));
        assert!(matches!(
            ev[0].event,
            InputEvent::Digital {
                dirs: Dirs::UP,
                buttons: 1
            }
        ));
        assert!(
            keys_of(&ev).is_empty(),
            "joystick keys never reach the keyboard"
        );
    }

    #[test]
    fn gamepad_east_select_start_are_button_bits() {
        let mut m = Mapper::new(Profile::default_for("nes"));
        let press = |m: &mut Mapper, button, down| {
            m.handle(
                &HostEvent::Button {
                    pad: 0,
                    button,
                    down,
                },
                Cycle(0),
            )
        };
        press(&mut m, Button::South, true);
        press(&mut m, Button::East, true);
        let ev = press(&mut m, Button::Start, true);
        assert_eq!(
            ev[0].event,
            InputEvent::Digital {
                dirs: Dirs::NONE,
                buttons: 1 | 2 | 8
            }
        );
        let ev = press(&mut m, Button::East, false);
        assert_eq!(
            ev[0].event,
            InputEvent::Digital {
                dirs: Dirs::NONE,
                buttons: 1 | 8
            }
        );
    }

    #[test]
    fn gamepad_stick_and_dpad_merge_with_keys() {
        let mut m = Mapper::new(Profile::default_for("c64"));
        m.handle(
            &HostEvent::Axis {
                pad: 0,
                axis: Axis::LeftX,
                value: 0.9,
            },
            Cycle(0),
        );
        let ev = m.handle(&key("Numpad8", true), Cycle(0));
        assert!(matches!(
            ev[0].event,
            InputEvent::Digital { dirs, .. } if dirs == Dirs::UP | Dirs::RIGHT
        ));
        let ev = m.handle(
            &HostEvent::Axis {
                pad: 0,
                axis: Axis::LeftX,
                value: 0.2,
            },
            Cycle(0),
        );
        assert!(matches!(
            ev[0].event,
            InputEvent::Digital { dirs: Dirs::UP, .. }
        ));
        // Another pad is not mapped.
        let ev = m.handle(
            &HostEvent::Button {
                pad: 3,
                button: Button::South,
                down: true,
            },
            Cycle(0),
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn mouse_and_paddles() {
        let mut p = Profile::default_for("c64");
        p.mouse.push(crate::profile::MouseMap { port: 0 });
        p.paddles
            .push(crate::profile::PaddleMap { port: 0, pad: 1 });
        let mut m = Mapper::new(p);
        let out = m.handle(
            &HostEvent::Mouse {
                dx: 3,
                dy: -4,
                buttons: 1,
            },
            Cycle(7),
        );
        assert_eq!(
            out,
            [TimedInput {
                at: Cycle(7),
                target: InputTarget::Port(PortId(0)),
                event: InputEvent::MouseRel {
                    dx: 3,
                    dy: -4,
                    buttons: 1
                },
            }]
        );
        let out = m.handle(
            &HostEvent::Axis {
                pad: 1,
                axis: Axis::LeftX,
                value: 1.0,
            },
            Cycle(8),
        );
        assert!(out.iter().any(|e| e.event
            == InputEvent::Analog {
                axis: 0,
                value: 65535
            }));
        let out = m.handle(
            &HostEvent::Button {
                pad: 1,
                button: Button::East,
                down: true,
            },
            Cycle(9),
        );
        assert!(out.iter().any(|e| e.event
            == InputEvent::Digital {
                dirs: Dirs::NONE,
                buttons: 2
            }));
    }
}
