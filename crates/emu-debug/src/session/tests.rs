//! Session tests against a fake machine (emu-debug must not depend on any
//! machine-* crate). End-to-end tests over TCP with a real machine live in
//! the `emu-tests` crate.

use super::*;
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::media::{MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::{
    BeamPos, ChipInfo, DebugTarget, Instr, MemSpace, Register, RegisterSet, StateError,
};

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
        }
    }
}

fn session() -> Session {
    let mut fake = Fake::default();
    fake.ram[3] = 0xab;
    Session::new(Box::new(fake), "fake")
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
    let r = result(&mut s, Command::StepInstr);
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
    assert_eq!(result(&mut s, Command::StepInstr)["pc"], 10);
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
    let id = result(
        &mut s,
        Command::BreakAdd {
            space: None,
            addr: 5,
        },
    )["bp"]
        .clone();
    assert_eq!(result(&mut s, Command::BreakList)[0]["addr"], 5);
    result(&mut s, Command::Resume);
    assert!(s.is_running());
    let mut event = None;
    for _ in 0..10 {
        if let Some(ev) = s.run_slice() {
            event = Some(ev);
            break;
        }
    }
    let ev = event.expect("breakpoint hit");
    assert_eq!(ev["event"], "stopped");
    assert_eq!(ev["reason"]["kind"], "breakpoint");
    assert_eq!(ev["pc"], 5);
    assert!(!s.is_running());
    assert_eq!(s.run_slice(), None, "paused sessions do not run");

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
