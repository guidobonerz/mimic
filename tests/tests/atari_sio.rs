//! Atari 800XL SIO with real ROMs: the OS boots a disk from D1: (an ATR
//! built here, whose boot code writes and reads a sector through the OS's
//! DSKINV), and the program slot loads an XEX with INITAD and RUNAD.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test atari_sio -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};
use machine_atari8::{CPU_SPACE, SLOT_D1, SLOT_PROGRAM};
use media_backends::image::MemoryBackend;

fn machine(options: &[(&str, &str)]) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "atari8").expect("ROMs"),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    emu_machines::registry().create("atari8", &config).unwrap()
}

fn run(m: &mut dyn Machine, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn byte(m: &mut dyn Machine, addr: u32) -> u8 {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    cpu.peek(CPU_SPACE, addr, Size::Byte).unwrap() as u8
}

fn media(name: &str, kind: MediaKind, data: Vec<u8>) -> Media {
    Media {
        name: name.into(),
        kind,
        backend: Box::new(MemoryBackend { data }),
    }
}

/// Boot code: copy sector 2's marker, write sector 4 from sector 2's
/// buffer with DSKINV ('W'), read it back to $0680 ('R'); statuses to
/// $0601/$0602.
const BOOT: [u8; 66] = [
    0x00, 0x02, 0x00, 0x07, 0x41, 0x07, // 2 sectors at $0700, DOSINI $0741
    0xad, 0x80, 0x07, // LDA $0780
    0x8d, 0x00, 0x06, // STA $0600
    0xa9, 0x01, 0x8d, 0x01, 0x03, // DUNIT = 1
    0xa9, 0x57, 0x8d, 0x02, 0x03, // DCOMND = 'W'
    0xa9, 0x80, 0x8d, 0x04, 0x03, // DBUF = $0780
    0xa9, 0x07, 0x8d, 0x05, 0x03, //
    0xa9, 0x04, 0x8d, 0x0a, 0x03, // DAUX = sector 4
    0xa9, 0x00, 0x8d, 0x0b, 0x03, //
    0x20, 0x53, 0xe4, // JSR DSKINV
    0x8c, 0x01, 0x06, // STY $0601
    0xa9, 0x52, 0x8d, 0x02, 0x03, // DCOMND = 'R'
    0xa9, 0x06, 0x8d, 0x05, 0x03, // DBUF = $0680
    0x20, 0x53, 0xe4, // JSR DSKINV
    0x8c, 0x02, 0x06, // STY $0602
    0x18, 0x60, // CLC; RTS (also DOSINI at $0741)
];

fn boot_disk() -> Vec<u8> {
    let mut img = media::atr::blank(720);
    let s = |n: usize| media::atr::HEADER + (n - 1) * 128;
    img[s(1)..s(1) + BOOT.len()].copy_from_slice(&BOOT);
    for i in 0..128 {
        img[s(2) + i] = (i as u8).wrapping_mul(3) ^ 0xa5;
    }
    img
}

#[test]
#[ignore = "needs Atari XL OS and BASIC ROMs in EMU_ROM_DIR"]
fn boots_a_disk_and_writes_through_dskinv() {
    let mut m = machine(&[]);
    m.media_slot(SLOT_D1)
        .unwrap()
        .insert(media("boot.atr", MediaKind::Disk, boot_disk()))
        .unwrap();
    run(m.as_mut(), 300);
    assert_eq!(byte(m.as_mut(), 0x0600), 0xa5, "sector 2 loaded");
    assert_eq!(byte(m.as_mut(), 0x0601), 1, "write status");
    assert_eq!(byte(m.as_mut(), 0x0602), 1, "read status");
    for i in 0..128u32 {
        assert_eq!(
            byte(m.as_mut(), 0x0680 + i),
            (i as u8).wrapping_mul(3) ^ 0xa5,
            "read back {i}"
        );
    }
    // The disk image has sector 4.
    let mut disk = m.media_slot(SLOT_D1).unwrap().eject().unwrap();
    let mut s4 = [0u8; 128];
    disk.backend
        .read(media::atr::HEADER as u64 + 3 * 128, &mut s4)
        .unwrap();
    assert_eq!(s4[1], 3 ^ 0xa5);
}

#[test]
#[ignore = "needs Atari XL OS ROM in EMU_ROM_DIR"]
fn program_slot_loads_an_xex() {
    let mut m = machine(&[("drives", "0")]);
    let xex = [
        0xff, 0xff, // header
        0x00, 0x06, 0x07, 0x06, // $0600–$0607: main
        0xa9, 0x99, 0x8d, 0x10, 0x06, // LDA #$99; STA $0610
        0x4c, 0x05, 0x06, // JMP $0605
        0x20, 0x06, 0x25, 0x06, // $0620–$0625: init
        0xa9, 0x77, 0x8d, 0x11, 0x06, 0x60, // LDA #$77; STA $0611; RTS
        0xe2, 0x02, 0xe3, 0x02, 0x20, 0x06, // INITAD = $0620
        0xe0, 0x02, 0xe1, 0x02, 0x00, 0x06, // RUNAD = $0600
    ];
    m.media_slot(SLOT_PROGRAM)
        .unwrap()
        .insert(media("test.xex", MediaKind::Program, xex.to_vec()))
        .unwrap();
    run(m.as_mut(), 300);
    assert_eq!(byte(m.as_mut(), 0x0611), 0x77, "INITAD ran");
    assert_eq!(byte(m.as_mut(), 0x0610), 0x99, "RUNAD ran");
    let pc = m
        .debug_target(DeviceId("cpu"))
        .unwrap()
        .regs()
        .get("PC")
        .unwrap();
    assert!((0x0605..0x0608).contains(&pc), "in the program: {pc:04X}");
    assert_eq!(byte(m.as_mut(), 0xd301) & 0x02, 0x02, "BASIC off");
}

#[test]
#[ignore = "needs Atari XL OS and BASIC ROMs in EMU_ROM_DIR"]
fn savestate_mid_boot_continues_identically() {
    let mut a = machine(&[]);
    a.media_slot(SLOT_D1)
        .unwrap()
        .insert(media("boot.atr", MediaKind::Disk, boot_disk()))
        .unwrap();
    run(a.as_mut(), 90);
    let state = a.save_state();
    let mut b = machine(&[]);
    b.media_slot(SLOT_D1)
        .unwrap()
        .insert(media("boot.atr", MediaKind::Disk, boot_disk()))
        .unwrap();
    b.load_state(&state).unwrap();
    run(a.as_mut(), 200);
    run(b.as_mut(), 200);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(byte(b.as_mut(), 0x0602), 1);
}
