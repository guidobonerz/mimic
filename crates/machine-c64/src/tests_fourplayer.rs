//! Four-player adapters in the user port (`devices_input::fourplayer`),
//! read by programs the way the adapters' own routines do.

use super::*;
use devices_input::{FourPlayer, FourPlayerKind};
use emu_core::input::{Dirs, InputEvent, InputTarget, TimedInput};

const JOY3: PortId = PortId(3);
const JOY4: PortId = PortId(4);

/// A machine whose KERNAL is `code` (at $E000), with `adapter` plugged in.
fn machine(code: &[u8], adapter: &str) -> C64 {
    let mut kernal = vec![0xea; crate::memory::KERNAL_SIZE];
    kernal[..code.len()].copy_from_slice(code);
    kernal[0x100] = 0x40;
    kernal[0x1ffa..].copy_from_slice(&[0x00, 0xe1, 0x00, 0xe0, 0x00, 0xe1]);
    let mut config = MachineConfig::default();
    config.roms.insert("kernal".into(), kernal);
    config.roms.insert("basic".into(), vec![0xb0; BASIC_SIZE]);
    config
        .roms
        .insert("chargen".into(), vec![0xc0; CHARGEN_SIZE]);
    config.options.insert("drive8".into(), "none".into());
    config.options.insert("userport".into(), adapter.into());
    C64::new(&config).unwrap()
}

fn stick(m: &mut C64, port: PortId, dirs: Dirs, fire: bool) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(port),
        event: InputEvent::Digital {
            dirs,
            buttons: u8::from(fire),
        },
    });
}

fn run(m: &mut C64, cycles: u32) {
    for _ in 0..cycles {
        m.step_cycle();
    }
}

fn zp(m: &C64, addr: u32) -> u8 {
    m.peek(CPU_SPACE, addr, Size::Byte).unwrap() as u8
}

/// Protovision's way: PB7 an output; high reads joystick 3, low joystick 4.
const CGA: [u8; 28] = [
    0xa9, 0x80, 0x8d, 0x03, 0xdd, // LDA #$80 ; STA $DD03
    0xa9, 0x80, 0x8d, 0x01, 0xdd, // loop: LDA #$80 ; STA $DD01
    0xad, 0x01, 0xdd, 0x85, 0x02, // LDA $DD01 ; STA $02
    0xa9, 0x00, 0x8d, 0x01, 0xdd, // LDA #0 ; STA $DD01
    0xad, 0x01, 0xdd, 0x85, 0x03, // LDA $DD01 ; STA $03
    0x4c, 0x05, 0xe0, // JMP loop
];

#[test]
fn the_protovision_adapter_reads_two_joysticks_through_port_b() {
    let mut m = machine(&CGA, "cga");
    let names: Vec<&str> = m.ports().iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "control port 1",
            "control port 2",
            "user port",
            "joystick 3",
            "joystick 4"
        ]
    );
    run(&mut m, 200);
    assert_eq!((zp(&m, 2), zp(&m, 3)), (0xff, 0x7f), "nothing held");
    stick(&mut m, JOY3, Dirs::UP, true);
    stick(&mut m, JOY4, Dirs::LEFT | Dirs::DOWN, false);
    run(&mut m, 200);
    assert_eq!(zp(&m, 2), !0x01 & !0x10, "joystick 3: up and fire");
    assert_eq!(
        zp(&m, 3),
        0x7f & !0x06 & !0x10,
        "joystick 4: down and left; joystick 3's fire line stays"
    );
    stick(&mut m, JOY3, Dirs::NONE, false);
    stick(&mut m, JOY4, Dirs::NONE, true);
    run(&mut m, 200);
    assert_eq!(
        (zp(&m, 2), zp(&m, 3)),
        (0xdf, 0x5f),
        "joystick 4's fire: PB5"
    );
    // The control ports are not touched.
    stick(&mut m, PortId(1), Dirs::UP, true);
    run(&mut m, 200);
    assert_eq!((zp(&m, 2), zp(&m, 3)), (0xdf, 0x5f));

    // The state carries the adapter's joysticks.
    let state = m.save_state();
    let mut n = machine(&CGA, "cga");
    n.load_state(&state).unwrap();
    run(&mut n, 200);
    assert_eq!((zp(&n, 2), zp(&n, 3)), (0xdf, 0x5f));
    let mut other = machine(&CGA, "hit");
    assert!(other.load_state(&state).is_err());

    // Pulled out, the joysticks go with it; another adapter fits.
    assert!(m.disconnect(USER_PORT).is_some());
    assert_eq!(m.ports().len(), 3);
    stick(&mut m, JOY4, Dirs::UP, false);
    run(&mut m, 200);
    assert_eq!((zp(&m, 2), zp(&m, 3)), (0xff, 0x7f));
    m.connect(
        USER_PORT,
        Box::new(FourPlayer::new(FourPlayerKind::Kingsoft)),
    )
    .unwrap();
    assert_eq!(m.ports().len(), 5);
    assert!(m
        .connect(
            JOY3,
            Box::new(devices_input::Joystick::new(PortKind::CbmControl))
        )
        .is_err());
}

/// The Hitmen adapter as the VICE test program reads it: port B, PA2 as
/// an input, and a byte shifted out of CIA1 that clocks SP2 into CIA2.
fn hit_program() -> Vec<u8> {
    let mut code = Vec::new();
    for (value, addr) in [
        (0x00u8, 0xdd03u16), // port B all inputs
        (0x01, 0xdd04),
        (0x00, 0xdd05),
        (0x11, 0xdd0e), // CIA2: timer A runs, serial port shifts in
        (0x01, 0xdc04),
        (0x00, 0xdc05),
        (0x51, 0xdc0e), // CIA1: timer A runs, serial port shifts out
    ] {
        code.extend([0xa9, value, 0x8d, addr as u8, (addr >> 8) as u8]);
    }
    code.extend([0xad, 0x02, 0xdd, 0x29, 0xfb, 0x8d, 0x02, 0xdd]); // PA2 an input
    let top = 0xe000 + code.len() as u16;
    code.extend([0xad, 0x01, 0xdd, 0x85, 0x02]); // loop: LDA $DD01 ; STA $02
    code.extend([0xad, 0x00, 0xdd, 0x29, 0x04, 0x85, 0x03]); // LDA $DD00 ; AND #4 ; STA $03
    code.extend([0xa9, 0xff, 0x8d, 0x0c, 0xdc]); // LDA #$FF ; STA $DC0C
    code.extend([0xa2, 0x20, 0xca, 0xd0, 0xfd]); // LDX #$20 ; DEX ; BNE: wait
    code.extend([0xad, 0x0c, 0xdd, 0x85, 0x04]); // LDA $DD0C ; STA $04
    code.extend([0x4c, top as u8, (top >> 8) as u8]);
    code
}

#[test]
fn the_hitmen_adapter_uses_pa2_and_the_serial_ports() {
    let mut m = machine(&hit_program(), "hit");
    run(&mut m, 2000);
    assert_eq!((zp(&m, 2), zp(&m, 3), zp(&m, 4)), (0xff, 0x04, 0xff));
    stick(&mut m, JOY3, Dirs::RIGHT, true);
    stick(&mut m, JOY4, Dirs::UP | Dirs::RIGHT, false);
    run(&mut m, 2000);
    assert_eq!(zp(&m, 2), !0x08 & !0x90, "PB0–3 and PB4–7");
    assert_eq!(zp(&m, 3), 0x00, "joystick 3's fire pulls PA2 low");
    assert_eq!(zp(&m, 4), 0xff, "joystick 4's fire is not held");
    stick(&mut m, JOY3, Dirs::NONE, false);
    stick(&mut m, JOY4, Dirs::NONE, true);
    run(&mut m, 2000);
    assert_eq!((zp(&m, 2), zp(&m, 3)), (0xff, 0x04));
    assert_eq!(zp(&m, 4), 0x00, "SP2 held low shifts in zeros");
    stick(&mut m, JOY4, Dirs::NONE, false);
    run(&mut m, 2000);
    assert_eq!(zp(&m, 4), 0xff);
}

#[test]
fn the_userport_option_names_the_adapters() {
    for name in [
        "cga",
        "4player",
        "protovision",
        "hit",
        "kingsoft",
        "starbyte",
    ] {
        let m = machine(&CGA, name);
        assert_eq!(m.ports().len(), 5, "{name}");
    }
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("userport".into(), "multitap".into());
    assert!(matches!(C64::new(&config), Err(ConfigError::Invalid(_))));
    // The WiC64 has no joysticks.
    config.options.insert("userport".into(), "wic64".into());
    assert_eq!(C64::new(&config).unwrap().ports().len(), 3);
}
