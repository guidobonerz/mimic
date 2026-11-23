//! Console controls: the buttons and switches on a machine's case (power,
//! reset, the Atari 2600's console switches, the 800XL's START/SELECT/
//! OPTION, RESTORE …), as frontends show them on a control panel.
//!
//! A machine lists them with [`crate::Machine::controls`]; a frontend
//! operates one by queueing [`crate::input::InputEvent::Control`] on
//! [`crate::input::InputTarget::Panel`], which sets it to an absolute value
//! (deterministic, part of savestates and input replays like all input).
//!
//! Lamps beside the picture ([`LightGroup`], [`crate::Machine::lights`])
//! are shown on the same panel.

use crate::frame::Rgb8;

/// A control of one machine (machine-defined numbering).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControlId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// Momentary: value 1 while held, 0 released.
    Button,
    /// Stays where it is set: value = index into the position names.
    Switch(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlInfo {
    pub id: ControlId,
    /// As printed on the case ("RESET", "TV TYPE", "LEFT DIFFICULTY").
    pub name: &'static str,
    pub kind: ControlKind,
    /// Current state: a button's 0/1, a switch's position index.
    pub value: u8,
    /// The machine's picture shows this control and clicks on it work it
    /// (the IMSAI's front panel), so frontends with clicks leave it off
    /// their control strip. The debug protocol still offers it.
    pub on_picture: bool,
}

impl ControlInfo {
    pub const fn button(id: u16, name: &'static str, held: bool) -> Self {
        Self {
            id: ControlId(id),
            name,
            kind: ControlKind::Button,
            value: held as u8,
            on_picture: false,
        }
    }

    pub const fn switch(
        id: u16,
        name: &'static str,
        positions: &'static [&'static str],
        value: u8,
    ) -> Self {
        Self {
            id: ControlId(id),
            name,
            kind: ControlKind::Switch(positions),
            value,
            on_picture: false,
        }
    }

    /// See [`ControlInfo::on_picture`].
    pub const fn shown_on_picture(mut self) -> Self {
        self.on_picture = true;
        self
    }

    /// Values the control accepts.
    pub fn accepts(&self, value: u8) -> bool {
        match self.kind {
            ControlKind::Button => value <= 1,
            ControlKind::Switch(positions) => usize::from(value) < positions.len(),
        }
    }
}

/// The power button every machine offers: pressing it powers the machine
/// up again (a hard reset, see [`crate::Machine::reset`]).
pub const POWER: ControlId = ControlId(0);

/// A row of lamps on or in a machine's case that are not part of its
/// picture (an LED strip, case lighting, status LEDs), as frontends show
/// them on a control panel. A machine lists them with
/// [`crate::Machine::lights`]; they are output only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightGroup {
    /// What the lamps are ("LED STRIP", "KEYBOARD").
    pub name: &'static str,
    /// The lamps in their order on the machine, each as the colour a
    /// screen shows for it (sRGB; black: the lamp is dark).
    pub leds: Vec<Rgb8>,
}

impl LightGroup {
    /// A group from the light its lamps give: red, green, blue, 0–255,
    /// linear (an LED's duty cycle). See [`light_to_screen`].
    pub fn from_light(name: &'static str, light: &[[u8; 3]]) -> Self {
        Self {
            name,
            leds: light
                .iter()
                .map(|c| Rgb8 {
                    r: light_to_screen(c[0]),
                    g: light_to_screen(c[1]),
                    b: light_to_screen(c[2]),
                })
                .collect(),
        }
    }
}

/// The screen value for an amount of light (0–255, linear): the square
/// root, close to the sRGB curve, in whole numbers so every host shows
/// the same. A lamp at a quarter of its light is half-bright on screen,
/// as it looks to the eye.
pub fn light_to_screen(light: u8) -> u8 {
    let n = u32::from(light) * 255;
    // The largest r with r² ≤ n; r ≤ 255.
    let mut r = 0u32;
    for bit in (0..8).rev() {
        let t = r | 1 << bit;
        if t * t <= n {
            r = t;
        }
    }
    r as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_becomes_screen_colour() {
        assert_eq!(light_to_screen(0), 0);
        assert_eq!(light_to_screen(255), 255);
        assert_eq!(light_to_screen(64), 127);
        assert!((0..255u8).all(|v| light_to_screen(v) <= light_to_screen(v + 1)));
        let g = LightGroup::from_light("LED STRIP", &[[255, 0, 64], [0, 0, 0]]);
        assert_eq!(g.name, "LED STRIP");
        assert_eq!(
            g.leds[0],
            Rgb8 {
                r: 255,
                g: 0,
                b: 127
            }
        );
        assert_eq!(g.leds[1], Rgb8 { r: 0, g: 0, b: 0 });
    }
}
