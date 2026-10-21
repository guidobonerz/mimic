//! Console controls: the buttons and switches on a machine's case (power,
//! reset, the Atari 2600's console switches, the 800XL's START/SELECT/
//! OPTION, RESTORE …), as frontends show them on a control panel.
//!
//! A machine lists them with [`crate::Machine::controls`]; a frontend
//! operates one by queueing [`crate::input::InputEvent::Control`] on
//! [`crate::input::InputTarget::Panel`], which sets it to an absolute value
//! (deterministic, part of savestates and input replays like all input).

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
}

impl ControlInfo {
    pub const fn button(id: u16, name: &'static str, held: bool) -> Self {
        Self {
            id: ControlId(id),
            name,
            kind: ControlKind::Button,
            value: held as u8,
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
        }
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
