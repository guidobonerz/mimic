//! A toy machine with a toy BASIC for tests of the BASIC debugger
//! ([`crate::basic`]): no CPU, the interpreter is Rust code that keeps all
//! its state in the machine's 64 KB of RAM, the way a ROM BASIC does, and
//! executes one statement every [`STATEMENT_CYCLES`] cycles. Before each
//! statement it "fetches" at [`DISPATCH`], the one stop point it offers.
//! Not real hardware and not a dialect of any.
//!
//! Memory (little-endian):
//!
//! | Address | Content |
//! | --- | --- |
//! | `$0010` | mode: 0 direct, 1 running |
//! | `$0011` | current line number (`$FFFF` in direct mode) |
//! | `$0013` | statement in the line |
//! | `$0014` | text pointer: the next statement |
//! | `$0016` | address of the current line |
//! | `$0018` | stack frames in use |
//! | `$0100` | stack, 16 frames of 11 bytes: kind (1 GOSUB, 2 FOR), variable, limit, line, statement, return text pointer, return line address |
//! | `$0200` | variables `A`–`Z`, 16-bit signed |
//! | `$0238` | which of them exist (one bit each) |
//! | `$0240` | array `@(0)`–`@(7)`, 16-bit signed |
//! | `$0260` | string `S$`: length, then up to 15 ASCII characters |
//! | `$0270` | float `X!`, 32-bit IEEE |
//! | `$1000` | program: lines of length (0 ends), number, statements |
//!
//! Statements: `01 v n` LET, `02 v n` add, `03 l` GOTO, `04 l` GOSUB,
//! `05` RETURN, `06 v a b` FOR, `07` NEXT, `08` END (`v`: 0–25, `n`, `a`,
//! `b`, `l`: 16 bits).

use crate::audio::{AudioFormat, SAMPLE_RATE};
use crate::basic::{
    BasicDebug, BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode,
    BasicPos, BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use crate::beam::BeamPos;
use crate::bus::Size;
use crate::cpu::{Flow, Instr, Register, RegisterSet};
use crate::cycle::Cycle;
use crate::debug::{BreakpointKind, DebugHooks};
use crate::frame::{FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard};
use crate::input::TimedInput;
use crate::machine::{
    ChipInfo, DebugTarget, DeviceId, Machine, MemSpace, MemSpaceId, ResetKind, StopReason,
};
use crate::media::{MediaSlot, MediaSlotInfo, SlotId};
use crate::port::{PortDevice, PortError, PortId, PortInfo};
use crate::savestate::{StateError, StateReader, StateWriter};

pub const SPACE: MemSpaceId = MemSpaceId(0);
/// Where the interpreter "fetches" before each statement.
pub const DISPATCH: u32 = 0xe000;
pub const STATEMENT_CYCLES: u8 = 8;
pub const CYCLES_PER_LINE: u16 = 64;
pub const LINES: u16 = 16;

pub const MODE: u16 = 0x10;
pub const CURLIN: u16 = 0x11;
pub const STMT: u16 = 0x13;
pub const TXTPTR: u16 = 0x14;
pub const LINEPTR: u16 = 0x16;
pub const DEPTH: u16 = 0x18;
pub const STACK: u16 = 0x0100;
pub const VARS: u16 = 0x0200;
pub const DEFINED: u16 = 0x0238;
pub const ARRAY: u16 = 0x0240;
pub const STRING: u16 = 0x0260;
pub const FLOAT: u16 = 0x0270;
pub const PROGRAM: u16 = 0x1000;

const FRAME_BYTES: u16 = 11;
const MAX_FRAMES: u8 = 16;
const ARRAY_LEN: u32 = 8;
const STRING_MAX: usize = 15;
const KIND_GOSUB: u8 = 1;
const KIND_FOR: u8 = 2;

const STATE_TAG: [u8; 4] = *b"TOYB";
const PALETTE: [Rgb8; 1] = [Rgb8 { r: 0, g: 0, b: 0 }];
const PIXELS: [u8; 1] = [0];

/// A statement of the toy BASIC. Variables are `'A'..='Z'`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToyStmt {
    Let(char, i16),
    Add(char, i16),
    Goto(u16),
    Gosub(u16),
    Return,
    For(char, i16, i16),
    Next,
    End,
}

impl ToyStmt {
    fn encode(self, out: &mut Vec<u8>) {
        let var = |c: char| (c as u8).wrapping_sub(b'A');
        match self {
            ToyStmt::Let(v, n) => {
                out.extend([1, var(v)]);
                out.extend(n.to_le_bytes());
            }
            ToyStmt::Add(v, n) => {
                out.extend([2, var(v)]);
                out.extend(n.to_le_bytes());
            }
            ToyStmt::Goto(l) => {
                out.push(3);
                out.extend(l.to_le_bytes());
            }
            ToyStmt::Gosub(l) => {
                out.push(4);
                out.extend(l.to_le_bytes());
            }
            ToyStmt::Return => out.push(5),
            ToyStmt::For(v, a, b) => {
                out.extend([6, var(v)]);
                out.extend(a.to_le_bytes());
                out.extend(b.to_le_bytes());
            }
            ToyStmt::Next => out.push(7),
            ToyStmt::End => out.push(8),
        }
    }
}

/// One frame of the stack as stored.
#[derive(Clone, Copy)]
struct Frame {
    kind: u8,
    var: u8,
    limit: u16,
    line: u16,
    stmt: u8,
    ret_txt: u16,
    ret_line: u16,
}

pub struct ToyBasic {
    ram: Vec<u8>,
    /// Cycle within the current statement.
    phase: u8,
    /// The dispatch fetch of the current statement has been reported.
    checked: bool,
    cycle: u64,
    line_stops: bool,
}

impl ToyBasic {
    /// A machine with `lines` stored and running from the first.
    pub fn new(lines: &[(u16, &[ToyStmt])]) -> Self {
        let mut m = Self {
            ram: vec![0; 0x10000],
            phase: 0,
            checked: false,
            cycle: 0,
            line_stops: false,
        };
        let mut at = PROGRAM;
        for (number, stmts) in lines {
            let mut line = vec![0];
            line.extend(number.to_le_bytes());
            for s in *stmts {
                s.encode(&mut line);
            }
            line[0] = line.len() as u8;
            for b in line {
                m.set_b(at, b);
                at = at.wrapping_add(1);
            }
        }
        m.run();
        m
    }

    /// Offer the stop point only at the first statement of a line, as a
    /// dialect that knows lines but not statements.
    pub fn with_line_stops(mut self) -> Self {
        self.line_stops = true;
        self
    }

    fn b(&self, a: u16) -> u8 {
        self.ram[usize::from(a)]
    }

    fn set_b(&mut self, a: u16, v: u8) {
        self.ram[usize::from(a)] = v;
    }

    fn w(&self, a: u16) -> u16 {
        u16::from_le_bytes([self.b(a), self.b(a.wrapping_add(1))])
    }

    fn set_w(&mut self, a: u16, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.set_b(a, lo);
        self.set_b(a.wrapping_add(1), hi);
    }

    fn running(&self) -> bool {
        self.b(MODE) == 1
    }

    /// RUN: clear variables and stack, start at the first line.
    fn run(&mut self) {
        for a in VARS..FLOAT + 4 {
            self.set_b(a, 0);
        }
        self.set_b(DEPTH, 0);
        self.set_b(MODE, 1);
        self.phase = 0;
        self.checked = false;
        self.enter_line(PROGRAM);
    }

    fn stop_program(&mut self) {
        self.set_b(MODE, 0);
        self.set_w(CURLIN, 0xffff);
    }

    /// Addresses of the stored lines.
    fn line_addrs(&self) -> Vec<u16> {
        let mut out = Vec::new();
        let mut at = PROGRAM;
        loop {
            let len = self.b(at);
            let Some(next) = at.checked_add(u16::from(len)) else {
                return out;
            };
            if len < 4 {
                return out;
            }
            out.push(at);
            at = next;
        }
    }

    fn find_line(&self, number: u16) -> Option<u16> {
        self.line_addrs()
            .into_iter()
            .find(|&a| self.w(a.wrapping_add(1)) == number)
    }

    /// Begin the line stored at `at`; the program ends if there is none.
    fn enter_line(&mut self, at: u16) -> bool {
        if self.b(at) < 4 {
            self.stop_program();
            return false;
        }
        self.set_w(LINEPTR, at);
        self.set_w(CURLIN, self.w(at.wrapping_add(1)));
        self.set_b(STMT, 0);
        self.set_w(TXTPTR, at.wrapping_add(3));
        true
    }

    /// After a statement: on to the next line at the end of this one.
    fn normalise(&mut self) {
        let line = self.w(LINEPTR);
        let len = u16::from(self.b(line));
        if self.w(TXTPTR).wrapping_sub(line) >= len {
            self.enter_line(line.wrapping_add(len));
        }
    }

    fn advance(&mut self, txt: u16) {
        self.set_w(TXTPTR, txt);
        self.set_b(STMT, self.b(STMT).wrapping_add(1));
        self.normalise();
    }

    fn frame_addr(i: u8) -> u16 {
        STACK + FRAME_BYTES * u16::from(i)
    }

    fn frame(&self, i: u8) -> Frame {
        let a = Self::frame_addr(i);
        Frame {
            kind: self.b(a),
            var: self.b(a + 1),
            limit: self.w(a + 2),
            line: self.w(a + 4),
            stmt: self.b(a + 6),
            ret_txt: self.w(a + 7),
            ret_line: self.w(a + 9),
        }
    }

    fn frames(&self) -> u8 {
        self.b(DEPTH).min(MAX_FRAMES)
    }

    fn push(&mut self, kind: u8, var: u8, limit: u16, ret_txt: u16) -> bool {
        let n = self.frames();
        if n == MAX_FRAMES {
            return false;
        }
        let a = Self::frame_addr(n);
        self.set_b(a, kind);
        self.set_b(a + 1, var);
        self.set_w(a + 2, limit);
        self.set_w(a + 4, self.w(CURLIN));
        self.set_b(a + 6, self.b(STMT));
        self.set_w(a + 7, ret_txt);
        self.set_w(a + 9, self.w(LINEPTR));
        self.set_b(DEPTH, n + 1);
        true
    }

    /// Continue after the statement that pushed `f`.
    fn resume_after(&mut self, f: Frame) {
        self.set_w(LINEPTR, f.ret_line);
        self.set_w(CURLIN, f.line);
        self.set_b(STMT, f.stmt);
        self.advance(f.ret_txt);
    }

    fn var_addr(v: u8) -> Option<u16> {
        (v < 26).then(|| VARS + 2 * u16::from(v))
    }

    fn define(&mut self, v: u8) {
        let a = DEFINED + u16::from(v / 8);
        self.set_b(a, self.b(a) | 1 << (v % 8));
    }

    fn defined(&self, v: u8) -> bool {
        self.b(DEFINED + u16::from(v / 8)) & 1 << (v % 8) != 0
    }

    fn set_var(&mut self, v: u8, n: u16) -> bool {
        let Some(a) = Self::var_addr(v) else {
            return false;
        };
        self.set_w(a, n);
        self.define(v);
        true
    }

    /// Execute the statement at the text pointer. Errors end the program.
    fn exec_statement(&mut self) {
        let p = self.w(TXTPTR);
        let at = |i: u16| p.wrapping_add(i);
        let ok = match self.b(p) {
            1 => {
                let ok = self.set_var(self.b(at(1)), self.w(at(2)));
                self.advance(at(4));
                ok
            }
            2 => {
                let v = self.b(at(1));
                let old = Self::var_addr(v).map_or(0, |a| self.w(a));
                let ok = self.set_var(v, old.wrapping_add(self.w(at(2))));
                self.advance(at(4));
                ok
            }
            3 => self
                .find_line(self.w(at(1)))
                .is_some_and(|l| self.enter_line(l)),
            4 => {
                self.push(KIND_GOSUB, 0, 0, at(3))
                    && self
                        .find_line(self.w(at(1)))
                        .is_some_and(|l| self.enter_line(l))
            }
            5 => loop {
                let n = self.frames();
                if n == 0 {
                    break false;
                }
                self.set_b(DEPTH, n - 1);
                let f = self.frame(n - 1);
                if f.kind == KIND_GOSUB {
                    self.resume_after(f);
                    break true;
                }
            },
            6 => {
                let v = self.b(at(1));
                let ok =
                    self.set_var(v, self.w(at(2))) && self.push(KIND_FOR, v, self.w(at(4)), at(6));
                self.advance(at(6));
                ok
            }
            7 => match self.frames() {
                0 => false,
                n => {
                    let f = self.frame(n - 1);
                    match Self::var_addr(f.var) {
                        Some(a) if f.kind == KIND_FOR => {
                            let next = (self.w(a) as i16).wrapping_add(1);
                            self.set_w(a, next as u16);
                            if next <= f.limit as i16 {
                                self.resume_after(f);
                            } else {
                                self.set_b(DEPTH, n - 1);
                                self.advance(at(1));
                            }
                            true
                        }
                        _ => false,
                    }
                }
            },
            _ => false,
        };
        if !ok {
            self.stop_program();
        }
    }

    /// One cycle. Returns (a statement slot ended, the frame ended).
    fn tick(&mut self) -> (bool, bool) {
        self.phase += 1;
        let done = self.phase >= STATEMENT_CYCLES;
        if done {
            self.phase = 0;
            self.checked = false;
            if self.running() {
                self.exec_statement();
            }
        }
        self.cycle += 1;
        let frame = u64::from(CYCLES_PER_LINE) * u64::from(LINES);
        (done, self.cycle.is_multiple_of(frame))
    }

    fn beam_now(&self) -> BeamPos {
        let per_line = u64::from(CYCLES_PER_LINE);
        let frame = per_line * u64::from(LINES);
        let in_frame = self.cycle % frame;
        BeamPos {
            frame: self.cycle / frame,
            line: (in_frame / per_line) as u16,
            cycle: (in_frame % per_line) as u16,
            hpos: 0,
        }
    }

    fn statement_text(&self, p: u16) -> Option<(String, u16)> {
        let at = |i: u16| p.wrapping_add(i);
        let var = |v: u8| char::from(b'A' + v % 26);
        let n = |i: u16| self.w(at(i)) as i16;
        Some(match self.b(p) {
            1 => (format!("{}={}", var(self.b(at(1))), n(2)), 4),
            2 => {
                let v = var(self.b(at(1)));
                match n(2) {
                    d if d < 0 => (format!("{v}={v}-{}", -i32::from(d)), 4),
                    d => (format!("{v}={v}+{d}"), 4),
                }
            }
            3 => (format!("GOTO {}", self.w(at(1))), 3),
            4 => (format!("GOSUB {}", self.w(at(1))), 3),
            5 => ("RETURN".into(), 1),
            6 => (
                format!("FOR {}={} TO {}", var(self.b(at(1))), n(2), n(4)),
                6,
            ),
            7 => ("NEXT".into(), 1),
            8 => ("END".into(), 1),
            _ => return None,
        })
    }

    fn float(&self) -> f32 {
        f32::from_le_bytes([
            self.b(FLOAT),
            self.b(FLOAT + 1),
            self.b(FLOAT + 2),
            self.b(FLOAT + 3),
        ])
    }

    fn string(&self) -> String {
        let len = usize::from(self.b(STRING)).min(STRING_MAX);
        (0..len as u16)
            .map(|i| match self.b(STRING + 1 + i) {
                c @ 0x20..=0x7e => char::from(c),
                _ => '\u{fffd}',
            })
            .collect()
    }
}

/// A number for a 16-bit variable.
fn int16(value: &BasicValue) -> Result<u16, BasicError> {
    let n = match *value {
        BasicValue::Int(n) => n,
        BasicValue::Float(f) if f.fract() == 0.0 && f.abs() < 1e9 => f as i64,
        BasicValue::Float(_) => return Err(BasicError::OutOfRange),
        BasicValue::Str(_) => return Err(BasicError::TypeMismatch),
    };
    i16::try_from(n)
        .map(|n| n as u16)
        .map_err(|_| BasicError::OutOfRange)
}

impl BasicDebug for ToyBasic {
    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: "Toy BASIC".into(),
            granularity: if self.line_stops {
                BasicGranularity::Line
            } else {
                BasicGranularity::Statement
            },
            space: SPACE,
        }
    }

    fn state(&self) -> BasicState {
        let running = self.running();
        let gosubs = (0..self.frames())
            .filter(|&i| self.frame(i).kind == KIND_GOSUB)
            .count();
        BasicState {
            mode: if running {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos: running.then(|| BasicPos {
                line: u32::from(self.w(CURLIN)),
                statement: u16::from(self.b(STMT)),
            }),
            text_addr: running.then(|| u32::from(self.w(TXTPTR))),
            depth: gosubs as u32,
        }
    }

    fn program(&self) -> Vec<BasicLine> {
        self.line_addrs()
            .into_iter()
            .map(|addr| {
                let len = u16::from(self.b(addr));
                let mut text = String::new();
                let mut statements = Vec::new();
                let mut p = addr + 3;
                while p - addr < len {
                    if !statements.is_empty() {
                        text.push(':');
                    }
                    statements.push(text.len() as u32);
                    match self.statement_text(p) {
                        Some((t, n)) => {
                            text.push_str(&t);
                            p = p.saturating_add(n);
                        }
                        None => {
                            text.push('?');
                            break;
                        }
                    }
                }
                BasicLine {
                    number: u32::from(self.w(addr + 1)),
                    addr: u32::from(addr),
                    raw: (addr..addr + len).map(|a| self.b(a)).collect(),
                    text,
                    statements,
                }
            })
            .collect()
    }

    fn variables(&self) -> Vec<BasicVar> {
        let mut vars: Vec<BasicVar> = (0..26u8)
            .filter(|&v| self.defined(v))
            .map(|v| {
                let addr = VARS + 2 * u16::from(v);
                BasicVar {
                    name: char::from(b'A' + v).to_string(),
                    ty: BasicType::Int,
                    addr: u32::from(addr),
                    value: Some(BasicValue::Int(i64::from(self.w(addr) as i16))),
                    dims: Vec::new(),
                }
            })
            .collect();
        vars.push(BasicVar {
            name: "S$".into(),
            ty: BasicType::Str,
            addr: u32::from(STRING),
            value: Some(BasicValue::Str(self.string())),
            dims: Vec::new(),
        });
        vars.push(BasicVar {
            name: "X!".into(),
            ty: BasicType::Float,
            addr: u32::from(FLOAT),
            value: Some(BasicValue::Float(f64::from(self.float()))),
            dims: Vec::new(),
        });
        vars.push(BasicVar {
            name: "@".into(),
            ty: BasicType::Int,
            addr: u32::from(ARRAY),
            value: None,
            dims: vec![ARRAY_LEN],
        });
        vars
    }

    fn array(&self, name: &str, first: u32, count: u32) -> Result<Vec<BasicValue>, BasicError> {
        if name != "@" {
            return Err(BasicError::NoSuchVariable);
        }
        if first >= ARRAY_LEN {
            return Err(BasicError::OutOfRange);
        }
        Ok((first..ARRAY_LEN.min(first.saturating_add(count)))
            .map(|i| BasicValue::Int(i64::from(self.w(ARRAY + 2 * i as u16) as i16)))
            .collect())
    }

    fn stack(&self) -> Vec<BasicFrame> {
        (0..self.frames())
            .rev()
            .map(|i| {
                let f = self.frame(i);
                let from = BasicPos {
                    line: u32::from(f.line),
                    statement: u16::from(f.stmt),
                };
                if f.kind == KIND_FOR {
                    BasicFrame::For {
                        var: char::from(b'A' + f.var % 26).to_string(),
                        limit: BasicValue::Int(i64::from(f.limit as i16)),
                        step: BasicValue::Int(1),
                        from,
                    }
                } else {
                    BasicFrame::Gosub { from }
                }
            })
            .collect()
    }

    fn set_variable(
        &mut self,
        name: &str,
        index: Option<u32>,
        value: &BasicValue,
    ) -> Result<(), BasicError> {
        match (name, index) {
            ("@", Some(i)) if i < ARRAY_LEN => {
                self.set_w(ARRAY + 2 * i as u16, int16(value)?);
                Ok(())
            }
            ("@", _) => Err(BasicError::OutOfRange),
            (_, Some(_)) => Err(BasicError::NoSuchVariable),
            ("S$", None) => {
                let BasicValue::Str(s) = value else {
                    return Err(BasicError::TypeMismatch);
                };
                if s.len() > STRING_MAX || !s.bytes().all(|c| (0x20..=0x7e).contains(&c)) {
                    return Err(BasicError::OutOfRange);
                }
                self.set_b(STRING, s.len() as u8);
                for (i, c) in s.bytes().enumerate() {
                    self.set_b(STRING + 1 + i as u16, c);
                }
                Ok(())
            }
            ("X!", None) => {
                let f = match *value {
                    BasicValue::Int(n) => n as f32,
                    BasicValue::Float(f) => f as f32,
                    BasicValue::Str(_) => return Err(BasicError::TypeMismatch),
                };
                if !f.is_finite() {
                    return Err(BasicError::OutOfRange);
                }
                for (i, b) in f.to_le_bytes().into_iter().enumerate() {
                    self.set_b(FLOAT + i as u16, b);
                }
                Ok(())
            }
            (name, None) => match name.as_bytes() {
                [c @ b'A'..=b'Z'] => {
                    let n = int16(value)?;
                    self.set_var(c - b'A', n);
                    Ok(())
                }
                _ => Err(BasicError::NoSuchVariable),
            },
        }
    }

    fn stop_points(&self) -> Vec<BasicStopPoint> {
        vec![BasicStopPoint {
            kind: BreakpointKind::Exec {
                space: SPACE,
                addr: DISPATCH,
            },
            granularity: self.info().granularity,
        }]
    }
}

impl Machine for ToyBasic {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        loop {
            if self.phase == 0 && !self.checked {
                self.checked = true;
                let here = self.running() && !(self.line_stops && self.b(STMT) != 0);
                if here && dbg.exec_armed(SPACE) && dbg.check_exec(SPACE, DISPATCH) {
                    if let Some(stop) = dbg.take_stop() {
                        return stop;
                    }
                }
            }
            let (done, frame) = self.tick();
            if done && dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.raster_armed() {
                let b = self.beam_now();
                dbg.check_raster(b.line, b.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                return stop;
            }
            if frame {
                return StopReason::FrameDone;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.tick();
        StopReason::StepDone
    }

    fn reset(&mut self, _kind: ResetKind) {
        self.run();
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
                pixel_clock_hz: 51_200,
                active_pixels: 1,
                total_pixels: u32::from(CYCLES_PER_LINE),
                lines_per_field: u32::from(LINES),
                standard: VideoStandard::Pal,
                output: OutputType::Rgb,
                field: FieldParity::Progressive,
            },
        }
    }

    fn basic(&mut self) -> Option<&mut dyn BasicDebug> {
        Some(self)
    }

    fn audio(&mut self, _out: &mut Vec<i16>) -> AudioFormat {
        AudioFormat {
            sample_rate: SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_now())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![MemSpace {
            id: SPACE,
            name: "cpu",
            device: DeviceId("cpu"),
            addr_bits: 16,
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

    fn input(&mut self, _ev: TimedInput) {}

    fn now(&self) -> Cycle {
        Cycle(self.cycle)
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
        let mut w = StateWriter::with_capacity(self.ram.len() + 32);
        w.chunk(STATE_TAG, 1, |w| {
            w.raw(&self.ram);
            w.u8(self.phase);
            w.bool(self.checked);
            w.u64(self.cycle);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let (_, mut r) = StateReader::new(s).chunk(STATE_TAG, 1)?;
        let ram = r.raw(self.ram.len())?.to_vec();
        let phase = r.u8()?;
        if phase >= STATEMENT_CYCLES {
            return Err(StateError::Invalid("statement phase"));
        }
        let checked = r.bool()?;
        let cycle = r.u64()?;
        self.ram = ram;
        self.phase = phase;
        self.checked = checked;
        self.cycle = cycle;
        Ok(())
    }
}

impl DebugTarget for ToyBasic {
    fn regs(&self) -> RegisterSet {
        RegisterSet(vec![
            Register {
                name: "PC",
                value: u32::from(self.w(TXTPTR)),
                bits: 16,
            },
            Register {
                name: "SP",
                value: u32::from(self.b(DEPTH)),
                bits: 8,
            },
        ])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        if name.eq_ignore_ascii_case("PC") {
            self.set_w(TXTPTR, v as u16);
        }
    }

    fn peek(&self, space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
        (space == SPACE).then(|| u32::from(self.b(addr as u16)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, _size: Size, v: u32) -> bool {
        if space == SPACE {
            self.set_b(addr as u16, v as u8);
        }
        space == SPACE
    }

    fn disassemble(&self, addr: u32) -> Instr {
        let (text, len) = self
            .statement_text(addr as u16)
            .unwrap_or_else(|| ("?".into(), 1));
        Instr {
            addr,
            bytes: (0..len)
                .map(|i| self.b((addr as u16).wrapping_add(i)))
                .collect(),
            mnemonic: text,
            operand: String::new(),
            target: None,
            flow: Flow::Normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ToyStmt::*;

    fn machine() -> ToyBasic {
        ToyBasic::new(&[
            (10, &[Let('A', 5), Gosub(100), Add('A', -2)]),
            (20, &[For('I', 1, 3), Add('B', 10), Next]),
            (30, &[End]),
            (100, &[Add('A', 1), Return]),
        ])
    }

    fn var(m: &ToyBasic, name: &str) -> Option<BasicValue> {
        m.variables()
            .into_iter()
            .find(|v| v.name == name)
            .and_then(|v| v.value)
    }

    #[test]
    fn lists_the_program_with_statement_offsets() {
        let m = machine();
        let p = m.program();
        let numbers: Vec<u32> = p.iter().map(|l| l.number).collect();
        assert_eq!(numbers, [10, 20, 30, 100]);
        assert_eq!(p[0].text, "A=5:GOSUB 100:A=A-2");
        assert_eq!(p[0].statements, [0, 4, 14]);
        assert_eq!(p[0].addr, u32::from(PROGRAM));
        assert_eq!(p[0].raw.len(), usize::from(p[0].raw[0]));
        assert_eq!(p[1].text, "FOR I=1 TO 3:B=B+10:NEXT");
    }

    #[test]
    fn runs_to_the_end() {
        let mut m = machine();
        let mut dbg = DebugHooks::new();
        assert_eq!(m.state().mode, BasicMode::Running);
        assert_eq!(
            m.state().pos,
            Some(BasicPos {
                line: 10,
                statement: 0
            })
        );
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
        assert_eq!(m.state().mode, BasicMode::Direct);
        assert_eq!(m.state().pos, None);
        assert_eq!(var(&m, "A"), Some(BasicValue::Int(4)));
        assert_eq!(var(&m, "B"), Some(BasicValue::Int(30)));
        assert_eq!(var(&m, "I"), Some(BasicValue::Int(4)));
        assert!(m.stack().is_empty());
    }

    #[test]
    fn the_stop_point_fires_once_per_statement() {
        let mut m = machine();
        let mut dbg = DebugHooks::new();
        dbg.add_breakpoint(m.stop_points()[0].kind);
        let mut seen = Vec::new();
        while let StopReason::Breakpoint(_) = m.run_frame(&mut dbg) {
            let s = m.state();
            let pos = s.pos.unwrap();
            seen.push((pos.line, pos.statement, s.depth));
        }
        assert_eq!(
            &seen[..6],
            [
                (10, 0, 0),
                (10, 1, 0),
                (100, 0, 1),
                (100, 1, 1),
                (10, 2, 0),
                (20, 0, 0)
            ]
        );
        // FOR once, the body three times, END.
        assert_eq!(seen.len(), 5 + 1 + 3 * 2 + 1);
        assert_eq!(seen.last(), Some(&(30, 0, 0)));
    }

    #[test]
    fn stack_and_variables_inside_the_subroutine_and_the_loop() {
        let mut m = machine();
        let mut dbg = DebugHooks::new();
        dbg.add_breakpoint(m.stop_points()[0].kind);
        for _ in 0..3 {
            m.run_frame(&mut dbg);
        }
        assert_eq!(
            m.stack(),
            [BasicFrame::Gosub {
                from: BasicPos {
                    line: 10,
                    statement: 1
                }
            }]
        );
        m.set_variable("A", None, &BasicValue::Int(40)).unwrap();
        for _ in 0..4 {
            m.run_frame(&mut dbg);
        }
        // At 20:1, inside the loop; A = 40 + 1 - 2.
        assert_eq!(var(&m, "A"), Some(BasicValue::Int(39)));
        assert!(matches!(&m.stack()[0], BasicFrame::For { var, .. } if var == "I"));
    }

    #[test]
    fn other_variable_kinds() {
        let mut m = machine();
        m.set_variable("S$", None, &BasicValue::Str("HELLO".into()))
            .unwrap();
        m.set_variable("X!", None, &BasicValue::Float(1.5)).unwrap();
        m.set_variable("@", Some(7), &BasicValue::Int(-3)).unwrap();
        assert_eq!(var(&m, "S$"), Some(BasicValue::Str("HELLO".into())));
        assert_eq!(var(&m, "X!"), Some(BasicValue::Float(1.5)));
        assert_eq!(
            m.array("@", 6, 10).unwrap(),
            [BasicValue::Int(0), BasicValue::Int(-3)]
        );
        assert_eq!(
            m.set_variable("A", None, &BasicValue::Int(40_000)),
            Err(BasicError::OutOfRange)
        );
        assert_eq!(
            m.set_variable("A", None, &BasicValue::Str("1".into())),
            Err(BasicError::TypeMismatch)
        );
        assert_eq!(
            m.set_variable("@", Some(8), &BasicValue::Int(1)),
            Err(BasicError::OutOfRange)
        );
        assert_eq!(
            m.set_variable("QQ", None, &BasicValue::Int(1)),
            Err(BasicError::NoSuchVariable)
        );
        assert_eq!(m.array("A", 0, 1), Err(BasicError::NoSuchVariable));
    }

    #[test]
    fn savestates_resume_identically_and_reject_garbage() {
        let mut a = machine();
        let mut dbg = DebugHooks::new();
        for _ in 0..37 {
            a.step_cycle();
        }
        let state = a.save_state();
        let mut b = machine();
        b.load_state(&state).unwrap();
        a.run_frame(&mut dbg);
        b.run_frame(&mut dbg);
        assert_eq!(a.save_state(), b.save_state());
        assert!(b.load_state(&state[..100]).is_err());
        assert_eq!(a.save_state(), b.save_state());
    }

    #[test]
    fn broken_programs_end_without_panicking() {
        let mut m = ToyBasic::new(&[(10, &[Goto(99)])]);
        let mut dbg = DebugHooks::new();
        m.run_frame(&mut dbg);
        assert_eq!(m.state().mode, BasicMode::Direct);
        // Garbage everywhere the interpreter looks.
        let mut m = machine();
        for a in 0..0x2000u32 {
            m.poke(SPACE, a, Size::Byte, a.wrapping_mul(37) >> 2);
        }
        m.set_b(MODE, 1);
        for _ in 0..4 {
            m.run_frame(&mut dbg);
        }
        m.program();
        m.variables();
        m.stack();
        m.state();
    }
}
