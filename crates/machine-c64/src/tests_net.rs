//! The Ultimate command interface and the WiC64 in a running C64, driven
//! by small 6502 programs (test ROMs, not Commodore's), with scripted
//! host services (no real network).

use super::*;
use crate::memory::KERNAL_SIZE;
use devices_net::testing::FakeNet;
use emu_core::host::{Journal, MemFileHost};

/// A config whose KERNAL runs `code` at $E000 with `data` at $E100.
fn config(code: &[u8], data: &[u8], options: &[(&str, &str)]) -> MachineConfig {
    let mut kernal = vec![0xea; KERNAL_SIZE];
    kernal[..code.len()].copy_from_slice(code);
    kernal[0x100..0x100 + data.len()].copy_from_slice(data);
    kernal[0x1ffa..].copy_from_slice(&[0x00, 0xe2, 0x00, 0xe0, 0x00, 0xe2]);
    kernal[0x200] = 0x40; // RTI
    let mut c = MachineConfig::default();
    c.roms.insert("kernal".into(), kernal);
    c.roms.insert("basic".into(), vec![0xb0; BASIC_SIZE]);
    c.roms.insert("chargen".into(), vec![0xc0; CHARGEN_SIZE]);
    for (k, v) in options {
        c.options.insert((*k).into(), (*v).into());
    }
    c
}

fn net() -> FakeNet {
    FakeNet::new().page("http://example.org/hello", b"Hello, C64!")
}

/// Run until the CPU loops at `pc` (a `JMP *`), at most `frames` frames.
fn run_to(m: &mut C64, pc: u16, frames: u32) {
    for _ in 0..frames * 20_000 {
        if m.cpu.pc() == pc {
            // The instruction before may still be in its last cycles.
            run_cycles(m, 10);
            return;
        }
        m.step_cycle();
    }
    panic!("CPU did not reach ${pc:04x}, at ${:04x}", m.cpu.pc());
}

fn screen(m: &C64, n: usize) -> Vec<u8> {
    m.mem.ram[0x0400..0x0400 + n].to_vec()
}

/// UCI: send `$04 $01` (control target IDENTIFY), wait while busy, copy
/// the response to $0400, accept it.
const UCI_IDENTIFY: [u8; 48] = [
    0xa9, 0x04, 0x8d, 0x1d, 0xdf, // LDA #$04, STA $DF1D
    0xa9, 0x01, 0x8d, 0x1d, 0xdf, // LDA #$01, STA $DF1D
    0xa9, 0x01, 0x8d, 0x1c, 0xdf, // LDA #PUSH_CMD, STA $DF1C
    0xad, 0x1c, 0xdf, 0x29, 0x30, 0xc9, 0x10, 0xf0,
    0xf7, // busy: LDA $DF1C, AND #$30, CMP #$10, BEQ busy
    0xa2, 0x00, // LDX #0
    0xad, 0x1c, 0xdf, 0x10, 0x09, // loop: LDA $DF1C, BPL done
    0xad, 0x1e, 0xdf, 0x9d, 0x00, 0x04, 0xe8, 0xd0,
    0xf2, // LDA $DF1E, STA $0400,X, INX, BNE loop
    0xa9, 0x02, 0x8d, 0x1c, 0xdf, // done: LDA #DATA_ACC, STA $DF1C
    0x4c, 0x2d, 0xe0, // JMP *
];

#[test]
fn uci_answers_a_6502_program() {
    let mut m = C64::new(&config(&UCI_IDENTIFY, &[], &[("uci", "on")])).unwrap();
    m.attach_host(HostServices::default());
    // Identification register, side-effect free for the debugger.
    assert_eq!(m.peek(CPU_SPACE, 0xdf1d, Size::Byte), Some(0xc9));
    run_to(&mut m, 0xe02d, 2);
    assert_eq!(screen(&m, 19), b"CONTROL TARGET V1.1");
    assert_eq!(m.peek(CPU_SPACE, 0xdf1c, Size::Byte), Some(0), "idle again");
    assert!(m.chips().iter().any(|c| c.name == "uci"));
    // Without the option the registers are open I/O2.
    let mut m = C64::new(&config(&UCI_IDENTIFY, &[], &[])).unwrap();
    assert_ne!(m.peek(CPU_SPACE, 0xdf1d, Size::Byte), Some(0xc9));
    assert!(C64::new(&config(&[], &[], &[("uci", "maybe")])).is_err());
    assert!(m.chips().iter().all(|c| c.name != "uci"));
    let _ = &mut m;
}

#[test]
fn uci_dos_reads_the_host_folder_through_the_bus() {
    let mut m = C64::new(&config(&[0x4c, 0x00, 0xe0], &[], &[("uci", "on")])).unwrap();
    let mut fs = MemFileHost::new();
    fs.insert("/readme.txt", b"hi".to_vec()).unwrap();
    m.attach_host(HostServices::new(None, Some(Box::new(fs))));
    let poke = |m: &mut C64, a: u16, v: u8| {
        assert!(m.poke(CPU_SPACE, u32::from(a), Size::Byte, u32::from(v)))
    };
    let mut cmd = vec![1u8, 0x02, 0x01];
    cmd.extend(b"readme.txt");
    for b in cmd {
        poke(&mut m, 0xdf1d, b);
    }
    poke(&mut m, 0xdf1c, 1);
    run_cycles(&mut m, 200);
    assert_eq!(
        m.peek(CPU_SPACE, 0xdf1c, Size::Byte),
        Some(0x60),
        "data last, status"
    );
    assert_eq!(m.peek(CPU_SPACE, 0xdf1f, Size::Byte), Some(u32::from(b'0')));
}

/// WiC64: a protocol-2 HTTP GET of `REQUEST` (at $E100), the response
/// bytes to $0400.
const WIC64_GET: [u8; 78] = [
    0xa9,
    0xff,
    0x8d,
    0x03,
    0xdd, // LDA #$FF, STA $DD03: PB out
    0xad,
    0x02,
    0xdd,
    0x09,
    0x04,
    0x8d,
    0x02,
    0xdd, // DDRA |= PA2
    0xad,
    0x00,
    0xdd,
    0x09,
    0x04,
    0x8d,
    0x00,
    0xdd, // PA2 high: C64 sends
    0xad,
    0x0d,
    0xdd, // clear ICR
    0xa2,
    0x00, // LDX #0
    0xbd,
    0x00,
    0xe1,
    0x8d,
    0x01,
    0xdd, // send: LDA $E100,X, STA $DD01
    0xad,
    0x0d,
    0xdd,
    0x29,
    0x10,
    0xf0,
    0xf9, // w: LDA $DD0D, AND #$10, BEQ w
    0xe8,
    0xe0,
    REQUEST.len() as u8,
    0xd0,
    0xee, // INX, CPX #n, BNE send
    0xa9,
    0x00,
    0x8d,
    0x03,
    0xdd, // PB in
    0xad,
    0x00,
    0xdd,
    0x29,
    0xfb,
    0x8d,
    0x00,
    0xdd, // PA2 low: WiC64 sends
    0xa2,
    0x00, // LDX #0
    0xad,
    0x0d,
    0xdd,
    0x29,
    0x10,
    0xf0,
    0xf9, // recv: wait for FLAG2
    0xad,
    0x01,
    0xdd,
    0x9d,
    0x00,
    0x04,
    0xe8,
    0xd0,
    0xf0, // LDA $DD01, STA $0400,X, INX, BNE recv
    0x4c,
    0x4b,
    0xe0, // JMP *
];
const REQUEST: &[u8; 28] = b"R\x01\x18\x00http://example.org/hello";

const RESPONSE: &[u8] = b"\x00\x0b\x00Hello, C64!";

fn wic64_machine(options: &[(&str, &str)]) -> C64 {
    let mut o = vec![("userport", "wic64")];
    o.extend(options);
    C64::new(&config(&WIC64_GET, REQUEST, &o)).unwrap()
}

fn run_frames(m: &mut C64, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        while m.run_frame(&mut dbg) != StopReason::FrameDone {}
    }
}

#[test]
fn wic64_http_get_from_a_6502_program() {
    let mut m = wic64_machine(&[]);
    assert!(m
        .ports()
        .iter()
        .any(|p| p.id == USER_PORT && p.kind == PortKind::CbmUserPort && p.connected));
    m.attach_host(HostServices::new(Some(Box::new(net())), None));
    run_frames(&mut m, 3);
    assert_eq!(screen(&m, RESPONSE.len()), RESPONSE);

    // net=off: the WiC64 answers "no WLAN" (status 3).
    let mut m = wic64_machine(&[("net", "off")]);
    m.attach_host(HostServices::new(Some(Box::new(net())), None));
    run_frames(&mut m, 3);
    assert_eq!(screen(&m, 3), [3, 0, 0]);
}

#[test]
fn user_port_takes_only_user_port_devices() {
    let mut m = C64::new(&config(&[0x4c, 0x00, 0xe0], &[], &[])).unwrap();
    assert!(
        !m.ports()
            .iter()
            .find(|p| p.id == USER_PORT)
            .unwrap()
            .connected
    );
    assert!(matches!(
        m.connect(
            USER_PORT,
            Box::new(devices_input::Joystick::new(PortKind::CbmControl))
        ),
        Err(PortError::Incompatible { .. })
    ));
    assert!(matches!(
        m.connect(CONTROL_PORT_1, Box::new(devices_net::Wic64::new())),
        Err(PortError::Incompatible { .. })
    ));
    m.connect(USER_PORT, Box::new(devices_net::Wic64::new()))
        .unwrap();
    assert!(m.disconnect(USER_PORT).is_some());
}

#[test]
fn savestate_with_uci_and_wic64_resumes_identically() {
    let opts = [("uci", "on")];
    let mut a = wic64_machine(&opts);
    a.attach_host(HostServices::new(Some(Box::new(net())), None));
    // Stop in the middle of the request.
    run_cycles(&mut a, 600);
    let state = a.save_state();
    let mut b = wic64_machine(&opts);
    b.attach_host(HostServices::new(Some(Box::new(net())), None));
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    run_frames(&mut a, 2);
    run_frames(&mut b, 2);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(screen(&b, RESPONSE.len()), RESPONSE);
    // A machine without the devices refuses the state.
    let mut c = C64::new(&config(&WIC64_GET, REQUEST, &[])).unwrap();
    assert!(c.load_state(&state).is_err());
}

#[test]
fn journal_record_then_replay_is_exact() {
    // Record a live run (the network answers from the fake host).
    let journal = Journal::new(Some(Box::new(net())), Some(Box::new(MemFileHost::new())));
    let mut a = wic64_machine(&[("uci", "on")]);
    a.attach_host(journal.services());
    let start = a.save_state();
    journal.start_recording();
    run_frames(&mut a, 3);
    let record = journal.stop_recording();
    assert_eq!(screen(&a, RESPONSE.len()), RESPONSE);

    // Replay with no host behind the journal at all.
    let replay = Journal::new(None, None);
    let mut b = wic64_machine(&[("uci", "on")]);
    b.attach_host(replay.services());
    b.load_state(&start).unwrap();
    replay.start_replay(&record).unwrap();
    run_frames(&mut b, 3);
    assert_eq!(replay.replay_status().diverged, None);
    assert_eq!(b.cpu.regs(), a.cpu.regs());
    assert_eq!(b.now(), a.now());
    assert_eq!(b.mem.ram, a.mem.ram);
    let pixels = |m: &C64| match m.framebuffer().pixels {
        PixelData::Indexed { pixels, .. } => pixels.to_vec(),
        _ => unreachable!(),
    };
    assert_eq!(pixels(&b), pixels(&a));

    // Without the record the same run differs (no network).
    let mut c = wic64_machine(&[("uci", "on")]);
    c.attach_host(Journal::new(None, None).services());
    c.load_state(&start).unwrap();
    run_frames(&mut c, 3);
    assert_ne!(screen(&c, 3), screen(&a, 3));
}

fn run_cycles(m: &mut C64, n: u32) {
    for _ in 0..n {
        m.step_cycle();
    }
}
