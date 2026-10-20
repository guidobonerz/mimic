//! Analog control port devices seen by BASIC with real ROMs (user-supplied
//! via `EMU_ROM_DIR`): paddles and the 1351 mouse through the SID's POTX on
//! the C64 and C128, paddles through the VIC on the VIC-20.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test input_devices -- --ignored

use emu_core::input::{InputEvent, InputTarget, TimedInput};
use emu_core::machine::Machine;
use emu_core::port::PortId;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, MemSpaceId, Size};

struct Target {
    id: &'static str,
    screen: u32,
    cols: usize,
    rows: usize,
    typing: fn(&str, u64) -> (Vec<TimedInput>, u64),
}

const C64: Target = Target {
    id: "c64",
    screen: 0x0400,
    cols: 40,
    rows: 25,
    typing: machine_c64::keyboard::typed_events,
};

const C128: Target = Target {
    id: "c128",
    screen: 0x0400,
    cols: 40,
    rows: 25,
    typing: machine_c128::keyboard::typed_events,
};

const VIC20: Target = Target {
    id: "vic20",
    screen: 0x1e00,
    cols: 22,
    rows: 23,
    typing: machine_vic20::keyboard::typed_events,
};

fn machine(t: &Target, device: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, t.id).expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("port1".into(), device.into());
    config.options.insert("drive8".into(), "none".into());
    emu_machines::registry().create(t.id, &config).unwrap()
}

fn screen_text(m: &mut dyn Machine, t: &Target) -> String {
    let space = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == "ram")
        .map_or(MemSpaceId(0), |s| s.id);
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    let bytes: Vec<u8> = (0..(t.cols * t.rows) as u32)
        .map(|i| cpu.peek(space, t.screen + i, Size::Byte).unwrap() as u8)
        .collect();
    bytes
        .chunks(t.cols)
        .map(|row| {
            row.iter()
                .map(|&c| match c & 0x7f {
                    c @ 1..=26 => (c + 0x40) as char,
                    c @ 0x20..=0x3f => c as char,
                    _ => ' ',
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(m: &mut dyn Machine, frames: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
}

fn type_text(m: &mut dyn Machine, t: &Target, text: &str) {
    let (events, end) = (t.typing)(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end {
        m.run_frame(&mut dbg);
    }
    run(m, 30);
}

fn port_event(m: &mut dyn Machine, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(PortId(0)),
        event,
    });
    run(m, 5);
}

/// The number BASIC printed last.
fn last_number(m: &mut dyn Machine, t: &Target) -> i32 {
    let text = screen_text(m, t);
    text.lines()
        .rev()
        .find_map(|l| l.trim().parse::<i32>().ok())
        .unwrap_or_else(|| panic!("no number on the screen:\n{text}"))
}

fn paddle_reads(t: &Target, peek: &str) {
    let mut m = machine(t, "paddles");
    run(m.as_mut(), 400);
    port_event(
        m.as_mut(),
        InputEvent::Analog {
            axis: 0,
            value: 0x3000,
        },
    );
    type_text(m.as_mut(), t, &format!("PRINT PEEK({peek})\n"));
    assert_eq!(
        last_number(m.as_mut(), t),
        0x30,
        "{}",
        screen_text(m.as_mut(), t)
    );
    port_event(
        m.as_mut(),
        InputEvent::Analog {
            axis: 0,
            value: 0xc000,
        },
    );
    type_text(m.as_mut(), t, &format!("PRINT PEEK({peek})\n"));
    assert_eq!(last_number(m.as_mut(), t), 0xc0);
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn c64_paddles_in_potx() {
    paddle_reads(&C64, "54297");
}

#[test]
#[ignore = "needs C128 ROMs in EMU_ROM_DIR"]
fn c128_paddles_in_potx() {
    paddle_reads(&C128, "54297");
}

#[test]
#[ignore = "needs VIC-20 ROMs in EMU_ROM_DIR"]
fn vic20_paddles_in_potx() {
    paddle_reads(&VIC20, "36872");
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn c64_1351_moves_potx_and_poty() {
    let t = &C64;
    let mut m = machine(t, "mouse1351");
    run(m.as_mut(), 400);
    type_text(m.as_mut(), t, "PRINT PEEK(54297) AND 126\n");
    let x0 = last_number(m.as_mut(), t);
    port_event(
        m.as_mut(),
        InputEvent::MouseRel {
            dx: 10,
            dy: 0,
            buttons: 0,
        },
    );
    type_text(m.as_mut(), t, "PRINT PEEK(54297) AND 126\n");
    assert_eq!(last_number(m.as_mut(), t), (x0 + 20) & 126);
}
