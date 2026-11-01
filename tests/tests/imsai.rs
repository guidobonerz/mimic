//! IMSAI 8080 through the registry, the way a frontend drives it: the
//! panel by control events, the terminal by a program in the program slot.

use emu_core::frame::ScreenId;
use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{ControlId, DebugHooks, Machine};

struct Mem(Vec<u8>);

impl MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn frames(m: &mut Box<dyn Machine>, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        m.run_frame(&mut dbg);
    }
}

fn send(m: &mut Box<dyn Machine>, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
    frames(m, 1);
}

/// Echo on the SIO-2, typed through the keyboard path: the terminal screen
/// changes, the panel keeps its size.
#[test]
fn a_loaded_program_talks_to_the_terminal_screen() {
    let program = vec![
        0xdb, 0x03, 0xe6, 0x02, 0xca, 0x00, 0x00, // wait for RxRDY
        0xdb, 0x02, 0xd3, 0x02, 0xc3, 0x00, 0x00, // echo
    ];
    let mut config = MachineConfig::default();
    config.options.insert("autostart".into(), "on".into());
    let registry = emu_machines::registry();
    let mut m = registry.create("imsai", &config).unwrap();
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == MediaKind::Program)
        .unwrap();
    m.media_slot(slot.id)
        .unwrap()
        .insert(Media {
            name: "echo.bin".into(),
            kind: MediaKind::Program,
            backend: Box::new(Mem(program)),
        })
        .unwrap();
    frames(&mut m, 2);
    let terminal = m.screens()[1].id;
    let lit = |m: &dyn Machine| {
        let f = m.screen(terminal).unwrap();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .filter(|&(x, y)| f.rgb_at(x, y).is_some_and(|c| c.g > 0x80))
            .count()
    };
    let before = lit(m.as_ref());
    for down in [true, false] {
        send(
            &mut m,
            InputTarget::Keyboard,
            InputEvent::Key {
                machine_key: MachineKey(u16::from(b'h')),
                down,
            },
        );
    }
    frames(&mut m, 3);
    assert!(lit(m.as_ref()) > before, "the letter came back");
    assert_eq!(m.screen(ScreenId::MAIN).unwrap().width, 1000);
    // STOP and PWR through the panel events.
    send(
        &mut m,
        InputTarget::Panel,
        InputEvent::Control {
            id: ControlId(0),
            value: 0,
        },
    );
    frames(&mut m, 1);
    let power = m.controls().into_iter().find(|c| c.id.0 == 0).unwrap();
    assert_eq!(power.value, 0);
}

/// Run a program file from `testdata/imsai` and answer on the terminal:
/// each pair is the text to wait for and the line to type.
fn session(file: Option<&str>, script: &[(&str, &str)]) -> Option<String> {
    let dir = std::env::var("EMU_TESTDATA_DIR").ok()?;
    let mut config = MachineConfig::default();
    config.options.insert("autostart".into(), "on".into());
    if file.is_none() {
        let Ok(roms) = std::env::var("EMU_ROM_DIR") else {
            return None;
        };
        let rom = std::fs::read(format!("{roms}/imsai/mpu-b.rom")).expect("mpu-b.rom");
        config.roms.insert("mpu-b".into(), rom);
        config.options.insert("mpu".into(), "b".into());
    }
    let mut m = machine_imsai::Imsai::new(&config).unwrap();
    if let Some(file) = file {
        let data = std::fs::read(format!("{dir}/imsai/{file}")).expect("fetch-testdata.sh");
        m.media_slot(machine_imsai::SLOT_PROGRAM)
            .unwrap()
            .insert(Media {
                name: file.into(),
                kind: MediaKind::Program,
                backend: Box::new(Mem(data)),
            })
            .unwrap();
    }
    let mut dbg = DebugHooks::new();
    let screen = |m: &machine_imsai::Imsai| {
        (0..24)
            .map(|r| m.terminal().line(r))
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (wait, line) in script {
        let mut frames = 0;
        loop {
            m.run_frame(&mut dbg);
            frames += 1;
            let text = screen(&m);
            // No text to wait for: give the program its time.
            if wait.is_empty() {
                if frames == 400 {
                    break;
                }
                continue;
            }
            // The newest line that says so: an old answer is not a prompt.
            if m.terminal().col > 0 && m.terminal().line(m.terminal().row).contains(wait)
                || *wait == "OK"
                    && m.terminal().col == 0
                    && m.terminal().row > 0
                    && m.terminal().line(m.terminal().row - 1) == "OK"
            {
                break;
            }
            assert!(frames < 1200, "no {wait:?} on\n{text}");
        }
        // As fast as a person: BASIC eats what comes while it prints.
        for b in line.bytes() {
            m.type_byte(b);
            for _ in 0..4 {
                m.run_frame(&mut dbg);
            }
        }
        m.type_byte(b'\r');
    }
    for _ in 0..120 {
        m.run_frame(&mut dbg);
    }
    let text = screen(&m);
    println!("{text}");
    Some(text)
}

/// Altair BASIC 3.2 (4K) as ported to the IMSAI's console by Bruce Hall.
#[test]
#[ignore = "needs EMU_TESTDATA_DIR"]
fn altair_basic_runs_on_the_terminal() {
    let Some(text) = session(
        Some("basic-tape.hex"),
        &[
            ("MEMORY SIZE", "32768"),
            ("TERMINAL WIDTH", ""),
            ("WANT SIN", "Y"),
            // Not all on one line: 4K BASIC's FOR looks for the end of the
            // line, not of the statement (a bug of its own).
            ("OK", "10 FOR I=1 TO 3"),
            ("", "20 PRINT I*I"),
            ("", "30 NEXT I"),
            ("", "40 PRINT 7/2"),
            ("", "RUN"),
        ],
    ) else {
        return;
    };
    assert!(
        text.contains("BYTES FREE") || text.contains("WORDS"),
        "{text}"
    );
    assert!(text.contains(" 9") && text.contains(" 3.5"), "{text}");
}

/// The MPU-B ROM's monitor: finds the console by the space bar, then its
/// D command dumps memory.
#[test]
#[ignore = "needs EMU_TESTDATA_DIR and EMU_ROM_DIR"]
fn mpu_b_monitor_answers_on_the_console() {
    let Some(text) = session(None, &[("", " "), ("?", "D 0000 000F")]) else {
        return;
    };
    assert!(text.contains("MONITOR"), "{text}");
    assert!(text.contains("0000  00"), "{text}");
}
