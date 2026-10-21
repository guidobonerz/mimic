//! Machine registry population: the only crate that links every machine-*
//! crate, and the catalogue of pluggable port devices (frontends may not
//! depend on `devices-input` themselves).

#![forbid(unsafe_code)]

use emu_core::port::{PortDevice, PortKind};
use emu_core::registry::Registry;

/// Port kinds a digital joystick fits.
fn joystick_port(kind: PortKind) -> bool {
    matches!(
        kind,
        PortKind::CbmControl | PortKind::AmigaGameport | PortKind::AtariJoy | PortKind::CpcJoy
    )
}

/// Names of the devices [`create_device`] makes for a port of `kind`.
pub fn device_names(kind: PortKind) -> Vec<&'static str> {
    match kind {
        PortKind::CbmControl => vec!["joystick", "paddles", "mouse1351"],
        PortKind::AtariJoy => vec!["joystick", "paddles", "keypad", "driving"],
        PortKind::CpcJoy => vec!["joystick", "amx"],
        PortKind::AmigaGameport => vec!["mouse", "joystick"],
        k if joystick_port(k) => vec!["joystick"],
        _ => Vec::new(),
    }
}

/// A new port device by name for a port of `kind`; `None` if it does not
/// exist or does not fit.
pub fn create_device(name: &str, kind: PortKind) -> Option<Box<dyn PortDevice>> {
    match name {
        "joystick" if joystick_port(kind) => Some(Box::new(devices_input::Joystick::new(kind))),
        "paddles" if matches!(kind, PortKind::CbmControl | PortKind::AtariJoy) => {
            Some(Box::new(devices_input::Paddles::for_port(kind)))
        }
        "mouse1351" if kind == PortKind::CbmControl => {
            Some(Box::new(devices_input::Mouse1351::new()))
        }
        "keypad" if kind == PortKind::AtariJoy => Some(Box::new(devices_input::Keypad::new())),
        // About 32 code steps (two turns) a second at the 2600's 1.19 MHz.
        "driving" if kind == PortKind::AtariJoy => {
            Some(Box::new(devices_input::Driving::new(37_000)))
        }
        // At most one quadrature step per 1000 CPU clocks (141 µs), so a
        // machine sampling the pins every raster line sees every phase.
        "mouse" if kind == PortKind::AmigaGameport => {
            Some(Box::new(devices_input::AmigaMouse::new(1000)))
        }
        // One step of motion per millisecond of the CPC's 4 MHz clock.
        "amx" if kind == PortKind::CpcJoy => Some(Box::new(devices_input::AmxMouse::new(4000))),
        _ => None,
    }
}

/// Every machine this build supports. Machines are added here as they
/// become runnable.
pub fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(machine_sim6502::entry());
    r.register(machine_c64::entry());
    r.register(machine_vic20::entry());
    r.register(machine_pet::entry());
    r.register(machine_ted::c16_entry());
    r.register(machine_ted::plus4_entry());
    r.register(machine_c128::entry());
    r.register(machine_atari8::entry());
    r.register(machine_atari2600::entry());
    r.register(machine_cpc::entry());
    r.register(machine_amiga::entry());
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;
    use emu_core::{DebugHooks, StopReason};

    #[test]
    fn device_catalogue() {
        assert_eq!(
            device_names(PortKind::CbmControl),
            ["joystick", "paddles", "mouse1351"]
        );
        assert!(create_device("mouse1351", PortKind::CbmControl).is_some());
        assert_eq!(
            create_device("paddles", PortKind::AtariJoy).map(|d| d.kind()),
            Some(PortKind::AtariJoy)
        );
        assert!(create_device("paddles", PortKind::CpcJoy).is_none());
        assert!(device_names(PortKind::Iec).is_empty());
        let j = create_device("joystick", PortKind::AtariJoy).unwrap();
        assert_eq!(j.kind(), PortKind::AtariJoy);
        assert!(create_device("joystick", PortKind::Iec).is_none());
        assert!(create_device("lightgun", PortKind::CbmControl).is_none());
    }

    #[test]
    fn registered_machines_run_a_frame() {
        let r = registry();
        assert!(!r.entries().is_empty());
        // Machines that need ROMs accept built-in stubs for tests.
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        for entry in r.entries() {
            let mut m = r.create(entry.id, &config).unwrap();
            assert_eq!(m.run_frame(&mut DebugHooks::new()), StopReason::FrameDone);
            assert_eq!(m.devices().first().map(|d| d.0), Some("cpu"));
        }
    }
}
