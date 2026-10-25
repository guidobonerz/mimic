//! Recordings carry the host-service journal: a C64 with a WiC64 fetches
//! a page while `record_start` runs; `replay_input` reproduces the run on
//! a session whose journal has no network behind it.

use emu_core::host::{Journal, JournalMode};
use emu_core::registry::MachineConfig;
use emu_core::{DeviceId, MemSpaceId, Size};
use emu_debug::protocol::Command;
use emu_debug::Session;

/// Protocol-2 HTTP GET of the request at $E100, response bytes to $0400
/// (see machine-c64's tests_net.rs for the annotated listing).
const PROGRAM: [u8; 78] = [
    0xa9, 0xff, 0x8d, 0x03, 0xdd, 0xad, 0x02, 0xdd, 0x09, 0x04, 0x8d, 0x02, 0xdd, 0xad, 0x00, 0xdd,
    0x09, 0x04, 0x8d, 0x00, 0xdd, 0xad, 0x0d, 0xdd, 0xa2, 0x00, 0xbd, 0x00, 0xe1, 0x8d, 0x01, 0xdd,
    0xad, 0x0d, 0xdd, 0x29, 0x10, 0xf0, 0xf9, 0xe8, 0xe0, 28, 0xd0, 0xee, 0xa9, 0x00, 0x8d, 0x03,
    0xdd, 0xad, 0x00, 0xdd, 0x29, 0xfb, 0x8d, 0x00, 0xdd, 0xa2, 0x00, 0xad, 0x0d, 0xdd, 0x29, 0x10,
    0xf0, 0xf9, 0xad, 0x01, 0xdd, 0x9d, 0x00, 0x04, 0xe8, 0xd0, 0xf0, 0x4c, 0x4b, 0xe0,
];
const REQUEST: &[u8; 28] = b"R\x01\x18\x00http://example.org/hello";

fn session(journal: &Journal) -> Session {
    let mut kernal = vec![0xea; 0x2000];
    kernal[..PROGRAM.len()].copy_from_slice(&PROGRAM);
    kernal[0x100..0x100 + REQUEST.len()].copy_from_slice(REQUEST);
    kernal[0x200] = 0x40;
    kernal[0x1ffa..].copy_from_slice(&[0x00, 0xe2, 0x00, 0xe0, 0x00, 0xe2]);
    let mut config = MachineConfig::default();
    config.roms.insert("kernal".into(), kernal);
    config.roms.insert("basic".into(), vec![0; 0x2000]);
    config.roms.insert("chargen".into(), vec![0; 0x1000]);
    config.options.insert("userport".into(), "wic64".into());
    config.options.insert("uci".into(), "on".into());
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    m.attach_host(journal.services());
    let mut s = Session::new(m, "c64");
    s.set_host_journal(journal.clone());
    s
}

fn screen(s: &mut Session) -> Vec<u8> {
    let t = s.machine_mut().debug_target(DeviceId("cpu")).unwrap();
    (0..14)
        .map(|i| t.peek(MemSpaceId(1), 0x0400 + i, Size::Byte).unwrap() as u8)
        .collect()
}

fn frames(s: &mut Session, n: usize) {
    for _ in 0..n {
        s.command(Command::StepFrame).unwrap();
    }
}

#[test]
fn replay_answers_host_calls_from_the_recording() {
    let net = devices_net::testing::FakeNet::new().page("http://example.org/hello", b"Hello, C64!");
    let live = Journal::new(Some(Box::new(net)), None);
    let mut a = session(&live);
    a.command(Command::RecordStart).unwrap();
    frames(&mut a, 3);
    let replay = a.command(Command::RecordStop).unwrap();
    assert_eq!(screen(&mut a), b"\x00\x0b\x00Hello, C64!");
    assert!(
        !replay["host"].as_str().unwrap().is_empty(),
        "journal recorded"
    );
    let end = a.machine().save_state();

    // No network at all behind this journal: only the recording answers.
    let offline = Journal::new(None, None);
    let mut b = session(&offline);
    b.command(Command::ReplayInput { replay }).unwrap();
    assert_eq!(offline.mode(), JournalMode::Replay);
    frames(&mut b, 3);
    assert_eq!(screen(&mut b), b"\x00\x0b\x00Hello, C64!");
    assert_eq!(offline.replay_status().diverged, None);
    assert_eq!(b.machine().save_state(), end);
}
