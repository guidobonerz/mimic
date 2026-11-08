//! The IBM 029 card punch through the registry, with a real deck file:
//! typing a FORTRAN program into the stacker's `.crd` file, committing it
//! and reading it back; the picture is saved to `target/tmp/keypunch.png`.

use emu_core::controls::ControlId;
use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Machine};
use machine_keypunch::{ctl, keys, SLOT_STACKER};

fn send(m: &mut dyn Machine, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
    m.run_frame(&mut DebugHooks::new());
}

fn key(m: &mut dyn Machine, k: u16, down: bool) {
    send(
        m,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(k),
            down,
        },
    );
}

fn typ(m: &mut dyn Machine, s: &str) {
    for b in s.bytes() {
        let (k, shift) = match b {
            b'*' => (b'8', true),
            b'(' => (b'9', true),
            b')' => (b'0', true),
            b'+' => (b'=', true),
            b'\'' => (b'\'', false),
            b'\n' => (keys::ENTER as u8, false),
            b'\t' => (keys::TAB as u8, false),
            other => (other.to_ascii_lowercase(), false),
        };
        if shift {
            key(m, keys::SHIFT, true);
        }
        key(m, u16::from(k), true);
        key(m, u16::from(k), false);
        if shift {
            key(m, keys::SHIFT, false);
        }
    }
}

#[test]
fn a_fortran_deck_into_a_text_file() {
    let registry = emu_machines::registry();
    let mut m = registry
        .create("keypunch", &MachineConfig::default())
        .unwrap();
    // PROG switch to FORTRAN, SEQ on.
    for (id, value) in [(ctl::PROGRAM, 1), (ctl::SEQ, 1)] {
        send(
            m.as_mut(),
            InputTarget::Panel,
            InputEvent::Control {
                id: ControlId(id),
                value,
            },
        );
    }
    let dir = std::env::temp_dir().join(format!("mimic-keypunch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hello.crd");
    let _ = std::fs::remove_file(&path);
    let media = media_backends::image::open_cards(&path).unwrap();
    assert!(path.exists(), "the stacker's file is created");
    m.media_slot(SLOT_STACKER).unwrap().insert(media).unwrap();

    typ(m.as_mut(), "C HELLO FROM THE 029\n");
    typ(m.as_mut(), "\t\tPRINT *, 'HELLO (1+2)'\n");
    typ(m.as_mut(), "\t\tEND");
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tmp");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("keypunch.png"), png).unwrap();
    typ(m.as_mut(), "\n");

    let slot = m.media_slot(SLOT_STACKER).unwrap();
    slot.flush().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let pad = |s: &str, n: u32| format!("{s:<72}{n:08}");
    assert_eq!(
        text,
        format!(
            "{}\n{}\n{}\n",
            pad("C HELLO FROM THE 029", 10),
            pad("      PRINT *, 'HELLO (1+2)'", 20),
            pad("      END", 30)
        )
    );
    let _ = std::fs::remove_dir_all(&dir);
}
