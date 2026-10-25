//! Session tests against a fake machine (emu-debug must not depend on any
//! machine-* crate). End-to-end tests over TCP with a real machine live in
//! the `emu-tests` crate.

use super::*;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::media::{MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::{BeamPos, ChipInfo, Instr, MemSpace, Register, StateError};

const SPACE: MemSpaceId = MemSpaceId(0);
const CYCLES_PER_LINE: u16 = 4;
const LINES: u16 = 3;
const PALETTE: [Rgb8; 1] = [Rgb8 { r: 0, g: 0, b: 0 }];
const PIXELS: [u8; 1] = [0];

/// Every instruction is 2 cycles long and advances PC by 1.
#[derive(Default)]
struct Fake {
    pc: u32,
    half: bool,
    line: u16,
    cycle: u16,
    frame: u64,
    ram: [u8; 16],
}

impl Fake {
    /// Returns (instruction done, new field).
    fn advance(&mut self) -> (bool, bool) {
        let done = self.half;
        self.half = !self.half;
        if done {
            self.pc = (self.pc + 1) % 16;
        }
        self.cycle += 1;
        let mut field = false;
        if self.cycle == CYCLES_PER_LINE {
            self.cycle = 0;
            self.line += 1;
            if self.line == LINES {
                self.line = 0;
                self.frame += 1;
                field = true;
            }
        }
        (done, field)
    }
}

impl Machine for Fake {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        loop {
            let (done, field) = self.advance();
            if done {
                if dbg.instr_step_armed() {
                    dbg.check_instr_step();
                }
                if dbg.exec_armed(SPACE) {
                    dbg.check_exec(SPACE, self.pc);
                }
            }
            if dbg.raster_armed() {
                dbg.check_raster(self.line, self.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                return stop;
            }
            if field {
                return StopReason::FrameDone;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.advance();
        StopReason::StepDone
    }

    fn reset(&mut self, kind: ResetKind) {
        self.pc = 0;
        self.half = false;
        if kind == ResetKind::Hard {
            self.ram = [0; 16];
        }
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: 1,
            height: 1,
            pixels: PixelData::Indexed {
                pixels: &PIXELS,
                palette: &PALETTE,
            },
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 1000,
                active_pixels: 1,
                total_pixels: 10,
                lines_per_field: 2,
                standard: VideoStandard::Pal,
                output: OutputType::Rgb,
                field: FieldParity::Progressive,
            },
        }
    }

    fn audio(&mut self, _out: &mut Vec<i16>) -> emu_core::AudioFormat {
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(BeamPos {
            frame: self.frame,
            line: self.line,
            cycle: self.cycle,
            hpos: 0,
        })
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![MemSpace {
            id: SPACE,
            name: "cpu",
            device: DeviceId("cpu"),
            addr_bits: 4,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        Vec::new()
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![DeviceId("cpu")]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev.0 == "cpu").then_some(self as &mut dyn DebugTarget)
    }

    fn input(&mut self, _ev: emu_core::input::TimedInput) {}

    fn now(&self) -> emu_core::Cycle {
        emu_core::Cycle(0)
    }

    fn ports(&self) -> Vec<PortInfo> {
        Vec::new()
    }

    fn connect(&mut self, port: PortId, _dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        Err(PortError::NoSuchPort(port))
    }

    fn disconnect(&mut self, _port: PortId) -> Option<Box<dyn PortDevice>> {
        None
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        Vec::new()
    }

    fn media_slot(&mut self, _slot: SlotId) -> Option<&mut dyn MediaSlot> {
        None
    }

    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }

    fn load_state(&mut self, _s: &[u8]) -> Result<(), StateError> {
        Err(StateError::Truncated)
    }
}

impl DebugTarget for Fake {
    fn regs(&self) -> RegisterSet {
        RegisterSet(vec![Register {
            name: "PC",
            value: self.pc,
            bits: 16,
        }])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        if name.eq_ignore_ascii_case("PC") {
            self.pc = v % 16;
        }
    }

    fn peek(&self, space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
        (space == SPACE).then(|| u32::from(self.ram[addr as usize % 16]))
    }

    fn poke(&mut self, _space: MemSpaceId, _addr: u32, _size: Size, _v: u32) -> bool {
        false
    }

    fn disassemble(&self, addr: u32) -> Instr {
        Instr {
            addr,
            bytes: vec![self.ram[addr as usize % 16]],
            mnemonic: "NOP".into(),
            operand: String::new(),
            target: None,
            flow: Flow::Normal,
        }
    }
}

fn session() -> Session {
    let mut fake = Fake::default();
    fake.ram[3] = 0xab;
    Session::new(Box::new(fake), "fake")
}

fn exec_at(addr: u32) -> Command {
    Command::BreakAdd(BreakSpec {
        addr: Some(addr),
        ..BreakSpec::default()
    })
}

/// Run slices until a stopped event; panics after 50 frames.
fn run_until_stopped(s: &mut Session) -> Value {
    for _ in 0..50 {
        s.run_slice();
        if let Some(ev) = s
            .take_events()
            .into_iter()
            .find(|e| e["event"] == "stopped")
        {
            return ev;
        }
    }
    panic!("no stop");
}

fn call(s: &mut Session, cmd: Command) -> Value {
    let v = s.handle(Request { id: 1, cmd });
    assert_eq!(v["id"], 1);
    v
}

fn result(s: &mut Session, cmd: Command) -> Value {
    let v = call(s, cmd);
    assert_eq!(v["ok"], true, "{v}");
    v["result"].clone()
}

#[test]
fn info_lists_devices_and_spaces() {
    let mut s = session();
    let r = result(&mut s, Command::Info);
    assert_eq!(r["machine"], "fake");
    assert_eq!(r["devices"], json!(["cpu"]));
    assert_eq!(r["memory_spaces"][0]["name"], "cpu");
    assert_eq!(r["running"], false);
}

#[test]
fn step_instr_regs_and_set_reg() {
    let mut s = session();
    let r = result(&mut s, Command::StepInstr { count: None });
    assert_eq!(r["reason"]["kind"], "step_done");
    assert_eq!(r["pc"], 1);
    let r = result(&mut s, Command::Regs { device: None });
    assert_eq!(r["regs"][0], json!({"name": "PC", "value": 1, "bits": 16}));
    result(
        &mut s,
        Command::SetReg {
            device: None,
            name: "PC".into(),
            value: 9,
        },
    );
    assert_eq!(result(&mut s, Command::StepInstr { count: None })["pc"], 10);
    let e = call(
        &mut s,
        Command::SetReg {
            device: None,
            name: "Q".into(),
            value: 0,
        },
    );
    assert_eq!(e["ok"], false);
}

#[test]
fn step_cycle_and_step_line_move_the_beam() {
    let mut s = session();
    let r = result(&mut s, Command::StepCycle);
    assert_eq!(r["beam"]["cycle"], 1);
    let r = result(&mut s, Command::StepLine);
    assert_eq!(r["reason"]["kind"], "step_done");
    assert_eq!(
        (r["beam"]["line"].clone(), r["beam"]["cycle"].clone()),
        (json!(1), json!(0))
    );
    let r = result(&mut s, Command::StepFrame);
    assert_eq!(r["reason"]["kind"], "frame_done");
    assert_eq!(r["beam"]["frame"], 1);
}

#[test]
fn breakpoints_stop_a_running_session() {
    let mut s = session();
    let id = result(&mut s, exec_at(5))["bp"].clone();
    assert_eq!(result(&mut s, Command::BreakList)[0]["addr"], 5);
    assert_eq!(result(&mut s, Command::Resume), json!({"running": true}));
    assert!(s.is_running());
    assert_eq!(s.take_events(), [json!({"event": "resumed", "pc": 0})]);
    let ev = run_until_stopped(&mut s);
    assert_eq!(ev["reason"]["kind"], "breakpoint");
    assert_eq!(ev["reason"]["hits"], 1);
    assert_eq!(ev["pc"], 5);
    assert!(!s.is_running());
    s.run_slice();
    assert!(s.take_events().is_empty(), "paused sessions do not run");

    result(
        &mut s,
        Command::BreakRemove {
            bp: id.as_u64().unwrap() as u32,
        },
    );
    assert_eq!(call(&mut s, Command::BreakRemove { bp: 99 })["ok"], false);
}

#[test]
fn peek_and_disasm() {
    let mut s = session();
    let r = result(
        &mut s,
        Command::Peek {
            space: None,
            addr: 2,
            len: 3,
        },
    );
    assert_eq!(r["bytes"], json!([0, 0xab, 0]));
    let r = result(
        &mut s,
        Command::Disasm {
            device: None,
            addr: Some(3),
            count: Some(2),
        },
    );
    assert_eq!(r["lines"][0]["bytes"], json!([0xab]));
    assert_eq!(r["lines"][1]["addr"], 4);
}

#[test]
fn invalid_arguments_are_errors_not_panics() {
    let mut s = session();
    for cmd in [
        Command::Regs {
            device: Some("z80".into()),
        },
        Command::Peek {
            space: Some(7),
            addr: 0,
            len: 1,
        },
        Command::Peek {
            space: None,
            addr: 0,
            len: MAX_PEEK + 1,
        },
    ] {
        let v = call(&mut s, cmd);
        assert_eq!(v["ok"], false);
        assert!(v["error"].is_string());
    }
}

#[test]
fn frame_duration_follows_signal_timing() {
    // 2 lines × 10 pixels at 1 kHz = 20 ms.
    assert_eq!(session().frame_duration(), Duration::from_millis(20));
}

#[test]
fn conditions_and_ignore_counts_decide_which_hits_stop() {
    let mut s = session();
    // PC 5 comes round every 16 instructions = 8/3 frames.
    let bp = result(
        &mut s,
        Command::BreakAdd(BreakSpec {
            addr: Some(5),
            condition: Some("[3] == $AB && frame >= 3".into()),
            ignore: Some(1),
            ..BreakSpec::default()
        }),
    )["bp"]
        .as_u64()
        .unwrap();
    result(&mut s, Command::Resume);
    let ev = run_until_stopped(&mut s);
    assert_eq!(ev["pc"], 5);
    assert_eq!(
        ev["reason"],
        json!({"kind": "breakpoint", "bp": bp, "hits": 2})
    );
    // Hits before frame 3 did not count, the first counting one was ignored.
    assert!(ev["beam"]["frame"].as_u64().unwrap() >= 5, "{ev}");
    let list = result(&mut s, Command::BreakList);
    assert_eq!(list[0]["condition"], "[3] == $AB && frame >= 3");
    assert_eq!(list[0]["ignore"], 1);

    result(
        &mut s,
        Command::BreakEnable {
            bp: bp as u32,
            enabled: false,
        },
    );
    result(&mut s, Command::Resume);
    for _ in 0..20 {
        s.run_slice();
    }
    assert!(s.is_running(), "disabled breakpoints do not stop");
}

#[test]
fn bad_conditions_and_kinds_are_rejected() {
    let mut s = session();
    for spec in [
        BreakSpec {
            addr: Some(1),
            condition: Some("A ==".into()),
            ..BreakSpec::default()
        },
        BreakSpec {
            kind: Some("teleport".into()),
            addr: Some(1),
            ..BreakSpec::default()
        },
        BreakSpec {
            kind: Some("write".into()),
            ..BreakSpec::default()
        },
        BreakSpec {
            kind: Some("raster".into()),
            ..BreakSpec::default()
        },
    ] {
        let v = call(&mut s, Command::BreakAdd(spec));
        assert_eq!(v["ok"], false, "{v}");
    }
    assert_eq!(result(&mut s, Command::BreakList), json!([]));
}

#[test]
fn raster_and_interrupt_breakpoints_are_listed() {
    let mut s = session();
    result(
        &mut s,
        Command::BreakAdd(BreakSpec {
            kind: Some("raster".into()),
            line: Some(2),
            cycle: Some(1),
            ..BreakSpec::default()
        }),
    );
    result(
        &mut s,
        Command::BreakAdd(BreakSpec {
            kind: Some("irq".into()),
            ..BreakSpec::default()
        }),
    );
    let list = result(&mut s, Command::BreakList);
    assert_eq!(list[0]["kind"], "raster");
    assert_eq!(list[0]["line"], 2);
    assert_eq!(list[1]["kind"], "irq");
    result(&mut s, Command::Resume);
    let ev = run_until_stopped(&mut s);
    assert_eq!(ev["beam"]["line"], 2);
    assert_eq!(ev["beam"]["cycle"], 1);
}

#[test]
fn frame_ready_only_with_subscribers_and_pause_reports_stopped() {
    let mut s = session();
    result(&mut s, Command::Resume);
    s.take_events();
    s.run_slice();
    assert!(s.take_events().is_empty());
    s.frame_subscriber(true);
    s.run_slice();
    let evs = s.take_events();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0]["event"], "frame_ready");
    s.frame_subscriber(false);

    let state = result(&mut s, Command::Pause);
    let evs = s.take_events();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0]["event"], "stopped");
    assert_eq!(evs[0]["pc"], state["pc"]);
    result(&mut s, Command::Pause);
    assert!(
        s.take_events().is_empty(),
        "pausing a paused session is quiet"
    );
}

#[test]
fn step_over_a_plain_instruction_and_run_to_are_asynchronous() {
    let mut s = session();
    assert_eq!(result(&mut s, Command::StepOver), json!({"running": true}));
    let ev = run_until_stopped(&mut s);
    assert_eq!(ev["reason"]["kind"], "step_done");
    assert_eq!(ev["pc"], 1);

    result(
        &mut s,
        Command::SymbolsLoad {
            path: None,
            text: Some("al C:0009 .nine\n".into()),
            format: None,
            space: None,
        },
    );
    result(
        &mut s,
        Command::RunTo {
            space: None,
            addr: None,
            symbol: Some("nine".into()),
        },
    );
    let ev = run_until_stopped(&mut s);
    assert_eq!(ev["reason"]["kind"], "step_done");
    assert_eq!(ev["pc"], 9);
    let unknown = Command::RunTo {
        space: None,
        addr: None,
        symbol: Some("ten".into()),
    };
    assert_eq!(call(&mut s, unknown)["ok"], false);
    assert_eq!(
        result(&mut s, Command::BreakList),
        json!([]),
        "internal breakpoint gone"
    );
}

#[test]
fn reset_poke_and_step_counts() {
    let mut s = session();
    let r = result(&mut s, Command::StepInstr { count: Some(3) });
    assert_eq!(r["pc"], 3);
    let r = result(&mut s, Command::Reset { kind: None });
    assert_eq!(r["pc"], 0);
    assert_eq!(
        call(
            &mut s,
            Command::Reset {
                kind: Some("warm".into())
            }
        )["ok"],
        false
    );
    let e = call(
        &mut s,
        Command::Poke {
            space: None,
            addr: 0,
            bytes: vec![1],
        },
    );
    assert_eq!(e["ok"], false, "the fake machine is read-only");
    assert_eq!(
        call(&mut s, Command::StepInstr { count: Some(0) })["ok"],
        false
    );
}

#[test]
fn cheats_import_formats_and_the_database() {
    let mut s = session();
    let r = result(
        &mut s,
        Command::CheatsLoad {
            path: None,
            text: Some("10 POKE 5,7 : REM lives".into()),
            format: Some("pokes".into()),
        },
    );
    assert_eq!(r["freezes"].as_array().unwrap().len(), 1);
    let r = result(
        &mut s,
        Command::CheatsLoad {
            path: None,
            text: Some("> 8 01 02".into()),
            format: Some("vice".into()),
        },
    );
    assert_eq!(r["patches"], 1);
    let list = result(&mut s, Command::FreezeList);
    assert_eq!(
        (list[0]["addr"].clone(), list[0]["value"].clone()),
        (json!(5), json!(7))
    );
    assert_eq!(list[0]["label"], "lives");
    assert_eq!(list[0]["mode"], "reapply_per_frame");
    let bad = call(
        &mut s,
        Command::CheatsLoad {
            path: None,
            text: Some("x".into()),
            format: Some("gif".into()),
        },
    );
    assert_eq!(bad["ok"], false);
    // The database: cheats/<machine>/<crc>.toml for an image's bytes.
    let dir = std::env::temp_dir().join(format!("mimic-session-cheats-{}", std::process::id()));
    let image = b"image bytes";
    let path = emu_cheats::db_path(&dir, "fake", &emu_cheats::media_key(image));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "game = \"Fake Game\"\n[[freeze]]\nspace = \"cpu\"\naddr = 2\nvalue = 9\n",
    )
    .unwrap();
    assert_eq!(s.cheats_for_image(image).unwrap(), None, "no folder set");
    s.set_cheats_dir(Some(dir.clone()));
    assert_eq!(
        s.cheats_for_image(image).unwrap().as_deref(),
        Some("Fake Game")
    );
    assert_eq!(s.cheats_for_image(b"other").unwrap(), None);
    assert!(s
        .take_events()
        .iter()
        .any(|e| e["event"] == "cheats_loaded" && e["game"] == "Fake Game"));
    std::fs::remove_dir_all(&dir).unwrap();
}
