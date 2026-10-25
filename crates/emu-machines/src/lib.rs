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
        PortKind::CbmControl
            | PortKind::AmigaGameport
            | PortKind::AtariJoy
            | PortKind::CpcJoy
            | PortKind::ColecoControl
            | PortKind::NesControl
            | PortKind::SnesControl
            | PortKind::VectrexControl
            | PortKind::AppleGame
    )
}

/// Names of the devices [`create_device`] makes for a port of `kind`.
///
/// "lightpen" works where the machine wires the port to its video chip's
/// light-pen input: C64/C128 control port 1, Atari 8-bit joystick ports,
/// Amiga game port 0, the CPC's expansion port. In another port of the same
/// kind (or on a VIC-20, C16/Plus/4, 2600) it only reports its button.
pub fn device_names(kind: PortKind) -> Vec<&'static str> {
    match kind {
        PortKind::CbmControl => vec!["joystick", "paddles", "mouse1351", "lightpen"],
        PortKind::AtariJoy => vec!["joystick", "paddles", "keypad", "driving", "lightpen"],
        PortKind::CpcJoy => vec!["joystick", "amx"],
        PortKind::AmigaGameport => vec!["mouse", "joystick", "lightpen"],
        // The CPC's expansion port.
        PortKind::Cartridge => vec!["lightpen"],
        PortKind::ColecoControl => vec!["controller", "superaction", "driving", "roller"],
        PortKind::NesControl => vec!["controller", "zapper", "arkanoid"],
        PortKind::SnesControl => vec!["pad", "mouse", "superscope"],
        PortKind::VectrexControl => vec!["controller", "lightpen"],
        PortKind::AppleGame => vec!["joystick", "paddles"],
        PortKind::MacMouse => vec!["mouse"],
        PortKind::CbmUserPort => vec!["wic64"],
        k if joystick_port(k) => vec!["joystick"],
        _ => Vec::new(),
    }
}

/// A new port device by name for a port of `kind`; `None` if it does not
/// exist or does not fit.
pub fn create_device(name: &str, kind: PortKind) -> Option<Box<dyn PortDevice>> {
    if kind == PortKind::ColecoControl {
        use devices_input::{ColecoController, ColecoKind};
        let k = match name {
            "controller" | "joystick" => ColecoKind::Standard,
            "superaction" => ColecoKind::SuperAction,
            "driving" => ColecoKind::Driving,
            "roller" => ColecoKind::Roller,
            _ => return None,
        };
        // About 400 steps a second at the ColecoVision's 3.58 MHz.
        return Some(Box::new(ColecoController::new(k, 8949)));
    }
    if kind == PortKind::NesControl {
        use devices_input::{NesController, NesKind};
        let k = match name {
            "controller" | "joystick" => NesKind::Standard,
            "zapper" => NesKind::Zapper,
            "arkanoid" => NesKind::Arkanoid,
            _ => return None,
        };
        return Some(Box::new(NesController::new(k)));
    }
    if kind == PortKind::SnesControl {
        use devices_input::{SnesController, SnesKind};
        let k = match name {
            "pad" | "controller" | "joystick" => SnesKind::Pad,
            "mouse" => SnesKind::Mouse,
            "superscope" => SnesKind::SuperScope,
            _ => return None,
        };
        return Some(Box::new(SnesController::new(k)));
    }
    if kind == PortKind::VectrexControl {
        use devices_input::{VectrexController, VectrexKind};
        let k = match name {
            "controller" | "joystick" => VectrexKind::Controller,
            "lightpen" => VectrexKind::LightPen,
            _ => return None,
        };
        return Some(Box::new(VectrexController::new(k)));
    }
    if kind == PortKind::AppleGame {
        use devices_input::{AppleJoystick, AppleKind};
        let k = match name {
            "joystick" => AppleKind::Joystick,
            "paddles" => AppleKind::Paddles,
            _ => return None,
        };
        return Some(Box::new(AppleJoystick::new(k)));
    }
    if name == "lightpen" {
        return devices_input::LightPenKind::for_port(kind)
            .map(|k| Box::new(devices_input::LightPen::new(k)) as Box<dyn PortDevice>);
    }
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
        // The Mac samples its mouse once a line (45 µs at 7.83 MHz).
        "mouse" if kind == PortKind::MacMouse => {
            Some(Box::new(devices_input::AmigaMouse::mac(1000)))
        }
        // One step of motion per millisecond of the CPC's 4 MHz clock.
        "amx" if kind == PortKind::CpcJoy => Some(Box::new(devices_input::AmxMouse::new(4000))),
        // Its network comes from the machine's host services on connect.
        "wic64" if kind == PortKind::CbmUserPort => Some(Box::new(devices_net::Wic64::new())),
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
    r.register(machine_zx81::entry());
    r.register(machine_coleco::entry());
    r.register(machine_nes::entry());
    r.register(machine_snes::entry());
    r.register(machine_vectrex::entry());
    r.register(machine_apple2::entry());
    r.register(machine_mac::entry());
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
            ["joystick", "paddles", "mouse1351", "lightpen"]
        );
        for kind in [
            PortKind::CbmControl,
            PortKind::AtariJoy,
            PortKind::AmigaGameport,
            PortKind::Cartridge,
        ] {
            assert!(device_names(kind).contains(&"lightpen"));
            let pen = create_device("lightpen", kind).unwrap();
            assert_eq!(pen.kind(), kind);
        }
        assert!(create_device("lightpen", PortKind::CpcJoy).is_none());
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
        let pad = create_device("roller", PortKind::ColecoControl).unwrap();
        assert_eq!(pad.kind(), PortKind::ColecoControl);
        assert!(create_device("joystick", PortKind::ColecoControl).is_some());
        assert!(create_device("paddles", PortKind::ColecoControl).is_none());
        let zapper = create_device("zapper", PortKind::NesControl).unwrap();
        assert_eq!(zapper.kind(), PortKind::NesControl);
        assert!(create_device("joystick", PortKind::NesControl).is_some());
        assert!(create_device("zapper", PortKind::CbmControl).is_none());
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
