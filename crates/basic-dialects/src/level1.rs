//! The TRS-80's Level I BASIC, a descendant of Palo Alto Tiny BASIC with
//! floating point.
//!
//! The program is plain text: from `$4200` to the address in `$406C`,
//! each line its number (2 bytes, low first), the text as typed and a
//! carriage return. Keywords are not tokens (and may be shortened with a
//! full stop: `P.` is PRINT), statements are separated by colons; behind
//! REM and DATA the rest of the line is skipped, as it is behind an IF
//! that fails.
//!
//! The interpreter keeps its text pointer in DE. `$409F` points to the
//! line being run (0: direct mode). A statement begins at one of two
//! places, the stop points:
//!
//! - the next line: `LD ($409F),DE : INC DE : INC DE` (`$039C`; the stop
//!   is behind the store, DE on the line);
//! - behind a colon: `RST 08H, ':', 4 : POP AF : JP …` (`$08B3`; the stop
//!   is the `POP AF`, DE behind the colon).
//!
//! A statement behind THEN joins the loop without passing either, so it
//! is part of its IF statement as in the other dialects. Halted inside a
//! statement, the line is still exact; the statement is the one DE is in,
//! which is right while DE is the text pointer (the interpreter uses it
//! for little else).
//!
//! GOSUB pushes the text pointer, `$409F` and the address of the last
//! such frame (`$40A3`, 0: none), six bytes; before that it saves the
//! running FOR loop and sets it to none. The one running loop is in
//! memory: the variable's address `$40A5` (0: none), the step `$4091` and
//! the limit `$406E` (16-bit integers: a loop counts in whole numbers),
//! the line `$4093` and the text pointer `$4095` behind the FOR statement.
//! FOR and GOSUB save the loop that was running on the CPU stack: its ten
//! bytes in that order from the variable up, or just a zero word for
//! none. The stack begins at `$4200` and holds nothing else between
//! statements; halted inside one, the saved loops are found as the first
//! chain from the stack pointer that ends there.
//!
//! Variables: A to Z, four bytes each from `$4000`, always there. A
//! number is three mantissa bytes (low first, the leading 1 left out) and
//! a byte with the sign in bit 7 and the exponent's upper seven bits
//! below; the exponent's lowest bit is bit 7 of the third mantissa byte.
//! The value is 0.1mmm… × 2^exponent with the exponent a signed byte;
//! −128 marks zero. Four zero bytes are therefore 0.5, which is what the
//! interpreter prints for memory it never wrote (seen with the user's
//! ROM: `G=.5` stores `00 00 00 00`, `H=0` stores `xx 00 00 40`). The
//! strings A$ and B$ are 16 bytes each at `$4070` and `$4080`, ended by
//! a zero byte. The array A() lies at the top of memory and is
//! not listed (it has no size: every element up to the end of memory
//! exists).
//!
//! Sources: Li-Chen Wang, "Palo Alto Tiny BASIC" (Dr. Dobb's Journal,
//! May 1976) for the interpreter's structure; RUN (`$038C`), GOSUB and
//! RETURN (`$04C4`), FOR and NEXT (`$0546`), the statement's end
//! (`$0030`, `$08B3`), the variables (`$0038`, `$0BE8`) and the command
//! table (`$0249`) were read from the user's Level I ROM with this
//! project's Z80 disassembler. No emulator source was used.

use emu_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use emu_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const TXTBGN: u16 = 0x4200;
const TXTUNF: u16 = 0x406c;
const CURRNT: u16 = 0x409f;
const STKGOS: u16 = 0x40a3;
const LOPVAR: u16 = 0x40a5;
const LOPINC: u16 = 0x4091;
const LOPLMT: u16 = 0x406e;
const LOPLN: u16 = 0x4093;
const LOPPT: u16 = 0x4095;
const VARS: u16 = 0x4000;
const STRINGS: [(u16, &str); 2] = [(0x4070, "A$"), (0x4080, "B$")];
const STRING_LEN: u16 = 16;
const STACK_TOP: u16 = 0x4200;
const ROM_LEN: usize = 0x1000;
const MAX_LINES: usize = 10_000;
const MAX_FRAMES: usize = 64;

/// The decoder a TRS-80 with the Level I ROM keeps; see the module text.
#[derive(Clone, Debug)]
pub struct Level1 {
    space: MemSpaceId,
    /// The stop points: the next line's and the one behind a colon.
    stops: Option<[u16; 2]>,
}

#[derive(Clone, Copy)]
struct Line {
    addr: u16,
    number: u16,
    /// Address of the carriage return.
    end: u16,
}

/// A loop as the interpreter keeps or saves it.
#[derive(Clone, Copy)]
struct Loop {
    var: u16,
    step: i16,
    limit: i16,
    line: u16,
    txt: u16,
}

enum RawFrame {
    For(Loop),
    Gosub { line: u16, txt: u16 },
}

/// The exponent that marks a zero.
const ZERO_EXP: i8 = -128;

/// A number as stored in a variable.
fn float_value(b: [u8; 4]) -> f64 {
    let exp = ((b[3] & 0x7f) << 1 | b[2] >> 7) as i8;
    if exp == ZERO_EXP {
        return 0.0;
    }
    let mantissa = u32::from_le_bytes([b[0], b[1], b[2] | 0x80, 0]);
    let v = f64::from(mantissa) / 16_777_216.0 * 2f64.powi(i32::from(exp));
    if b[3] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn float_bytes(v: f64) -> Result<[u8; 4], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    let zero = [0, 0, 0, (ZERO_EXP as u8) >> 1];
    if v == 0.0 {
        return Ok(zero);
    }
    let mut exp = v.abs().log2().floor() as i32 + 1;
    let mut mantissa = (v.abs() / 2f64.powi(exp) * 16_777_216.0).round();
    // The logarithm may be off by one at powers of two; rounding may carry.
    if mantissa >= 16_777_216.0 {
        mantissa /= 2.0;
        exp += 1;
    } else if mantissa < 8_388_608.0 {
        mantissa *= 2.0;
        exp -= 1;
    }
    if exp <= i32::from(ZERO_EXP) {
        return Ok(zero);
    }
    if exp > 127 {
        return Err(BasicError::OutOfRange);
    }
    let exp = exp as i8 as u8;
    let m = (mantissa as u32).to_le_bytes();
    let sign = if v < 0.0 { 0x80 } else { 0 };
    Ok([m[0], m[1], m[2] & 0x7f | (exp & 1) << 7, sign | exp >> 1])
}

fn push_ascii(out: &mut String, b: u8) {
    match b {
        0x20..=0x7e => out.push(char::from(b)),
        _ => out.push_str(&format!("{{${b:02x}}}")),
    }
}

fn peek(host: &dyn DebugTarget, space: MemSpaceId, addr: u16) -> u8 {
    host.peek(space, u32::from(addr), Size::Byte)
        .map_or(0xff, |v| v as u8)
}

struct View<'a> {
    host: &'a dyn DebugTarget,
    space: MemSpaceId,
}

impl View<'_> {
    fn b(&self, addr: u16) -> u8 {
        peek(self.host, self.space, addr)
    }

    fn w(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.b(addr), self.b(addr.wrapping_add(1))])
    }

    fn reg(&self, name: &str) -> u16 {
        self.host.regs().get(name).unwrap_or(0) as u16
    }

    fn lines(&self) -> Vec<Line> {
        let end = self.w(TXTUNF);
        let mut out = Vec::new();
        let mut at = TXTBGN;
        while at < end && end - at >= 3 && out.len() < MAX_LINES {
            let mut cr = at + 2;
            while cr < end && self.b(cr) != 0x0d {
                cr += 1;
            }
            if cr >= end {
                break;
            }
            out.push(Line {
                addr: at,
                number: self.w(at),
                end: cr,
            });
            at = cr + 1;
        }
        out
    }

    /// A line's text, where its statements begin in it and the addresses
    /// of the colons between them.
    fn scan(&self, line: &Line) -> (String, Vec<u32>, Vec<u16>) {
        let mut text = String::new();
        let mut statements = vec![0];
        let mut colons = Vec::new();
        let mut quote = false;
        // The current statement so far, to see REM and DATA.
        let mut head = String::new();
        let mut skipped = false;
        for at in line.addr + 2..line.end {
            let b = self.b(at);
            match b {
                b'"' => quote = !quote,
                b':' if !quote && !skipped => {
                    statements.push(text.len() as u32 + 1);
                    colons.push(at);
                    head.clear();
                }
                b' ' => {}
                _ if !quote && head.len() < 5 => {
                    head.push(char::from(b));
                    // The rest of the line belongs to REM and DATA
                    // (`D.` is DATA; `R.` is another word).
                    skipped = matches!(head.as_str(), "REM" | "DATA" | "D." | "DA." | "DAT.");
                    if skipped {
                        head.push_str("     ");
                    }
                }
                _ => {}
            }
            push_ascii(&mut text, b);
        }
        (text, statements, colons)
    }

    /// The statement of the line at `addr` the text pointer `txt` is in.
    fn pos(&self, lines: &[Line], addr: u16, txt: u16) -> Option<BasicPos> {
        let l = lines.iter().find(|l| l.addr == addr)?;
        let statement = if (l.addr..=l.end).contains(&txt) {
            let (_, _, colons) = self.scan(l);
            colons.iter().filter(|&&c| c < txt).count()
        } else {
            0
        };
        Some(BasicPos {
            line: u32::from(l.number),
            statement: statement as u16,
        })
    }

    fn loop_at(&self, var: u16, at: u16) -> Loop {
        Loop {
            var,
            step: self.w(at) as i16,
            limit: self.w(at.wrapping_add(2)) as i16,
            line: self.w(at.wrapping_add(4)),
            txt: self.w(at.wrapping_add(6)),
        }
    }

    /// The saved loops and the GOSUB frames from `start` up, if that is a
    /// chain that ends at the stack's top.
    fn chain(&self, start: u16) -> Option<Vec<RawFrame>> {
        let mut out = Vec::new();
        let mut gosub = self.w(STKGOS);
        let mut p = start;
        while p != STACK_TOP {
            if p > STACK_TOP - 2 || out.len() > MAX_FRAMES {
                return None;
            }
            if p == gosub {
                if p > STACK_TOP - 6 {
                    return None;
                }
                out.push(RawFrame::Gosub {
                    line: self.w(p + 2),
                    txt: self.w(p + 4),
                });
                gosub = self.w(p);
                p += 6;
                continue;
            }
            match self.w(p) {
                0 => p += 2,
                var if (VARS..VARS + 4 * 26).contains(&var) && var % 4 == 0 => {
                    if p > STACK_TOP - 10 {
                        return None;
                    }
                    out.push(RawFrame::For(self.loop_at(var, p + 2)));
                    p += 10;
                }
                _ => return None,
            }
        }
        (gosub == 0).then_some(out)
    }

    /// The running loop, the saved ones and the GOSUB frames, innermost
    /// first.
    fn frames(&self) -> Vec<RawFrame> {
        let mut out = Vec::new();
        let var = self.w(LOPVAR);
        if var != 0 {
            out.push(RawFrame::For(Loop {
                var,
                step: self.w(LOPINC) as i16,
                limit: self.w(LOPLMT) as i16,
                line: self.w(LOPLN),
                txt: self.w(LOPPT),
            }));
        }
        let sp = self.reg("SP");
        let saved = (0..64)
            .map(|i| sp.wrapping_add(i))
            .take_while(|&p| p <= STACK_TOP)
            .find_map(|p| self.chain(p));
        match saved {
            Some(frames) => out.extend(frames),
            // No chain: at least the GOSUB frames, by their links.
            None => {
                let mut gosub = self.w(STKGOS);
                while gosub != 0 && gosub <= STACK_TOP - 6 && out.len() < MAX_FRAMES {
                    out.push(RawFrame::Gosub {
                        line: self.w(gosub + 2),
                        txt: self.w(gosub + 4),
                    });
                    gosub = self.w(gosub);
                }
            }
        }
        out
    }

    fn string(&self, at: u16) -> String {
        let mut s = String::new();
        for i in 0..STRING_LEN {
            let b = self.b(at + i);
            if b == 0x0d || b == 0 {
                break;
            }
            push_ascii(&mut s, b);
        }
        s
    }
}

impl Level1 {
    pub fn new(space: MemSpaceId) -> Self {
        Self { space, stops: None }
    }

    /// Look at the machine again: `Some` with the decoder to keep from now
    /// on if the interpreter appeared or went away.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        let mem: Vec<u8> = (0..ROM_LEN)
            .map(|a| peek(host, self.space, a as u16))
            .collect();
        let [lo, hi] = CURRNT.to_le_bytes();
        let line = mem
            .windows(6)
            .position(|w| w == [0xed, 0x53, lo, hi, 0x13, 0x13]);
        let colon = mem
            .windows(5)
            .position(|w| w == [0xcf, b':', 0x04, 0xf1, 0xc3]);
        let stops = line.zip(colon).map(|(l, c)| [l as u16 + 4, c as u16 + 3]);
        (stops != self.stops).then(|| Self {
            stops,
            ..self.clone()
        })
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        self.stops.map(|_| View {
            host,
            space: self.space,
        })
    }

    /// The variable `name`: its address and whether it is a string.
    fn variable(name: &str) -> Option<(u16, bool)> {
        if let Some((at, _)) = STRINGS.iter().find(|(_, n)| *n == name) {
            return Some((*at, true));
        }
        match name.as_bytes() {
            [c @ b'A'..=b'Z'] => Some((VARS + 4 * u16::from(c - b'A'), false)),
            _ => None,
        }
    }
}

fn var_name(addr: u16) -> String {
    char::from(b'A' + ((addr.wrapping_sub(VARS) / 4) as u8).min(25)).to_string()
}

impl Dialect for Level1 {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: "TRS-80 Level I BASIC".into(),
            granularity: BasicGranularity::Statement,
            space: self.space,
            name_len: 1,
        }
    }

    fn state(&self, host: &dyn DebugTarget) -> BasicState {
        let Some(v) = self.view(host) else {
            return BasicState {
                mode: BasicMode::Inactive,
                pos: None,
                text_addr: None,
                depth: 0,
            };
        };
        let txt = v.reg("D") << 8 | v.reg("E");
        let lines = v.lines();
        let pos = v.pos(&lines, v.w(CURRNT), txt);
        let depth = v
            .frames()
            .iter()
            .filter(|f| matches!(f, RawFrame::Gosub { .. }))
            .count();
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos,
            text_addr: Some(u32::from(txt)),
            depth: depth as u32,
        }
    }

    fn program(&self, host: &dyn DebugTarget) -> Vec<BasicLine> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        v.lines()
            .iter()
            .map(|l| {
                let (text, statements, _) = v.scan(l);
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.addr),
                    raw: (l.addr..=l.end).map(|a| v.b(a)).collect(),
                    text,
                    statements,
                }
            })
            .collect()
    }

    fn variables(&self, host: &dyn DebugTarget) -> Vec<BasicVar> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        let mut vars: Vec<BasicVar> = (0..26u16)
            .map(|i| {
                let at = VARS + 4 * i;
                BasicVar {
                    name: var_name(at),
                    ty: BasicType::Float,
                    addr: u32::from(at),
                    value: Some(BasicValue::Float(float_value(std::array::from_fn(|n| {
                        v.b(at + n as u16)
                    })))),
                    dims: Vec::new(),
                }
            })
            .collect();
        vars.extend(STRINGS.iter().map(|(at, name)| BasicVar {
            name: (*name).into(),
            ty: BasicType::Str,
            addr: u32::from(*at),
            value: Some(BasicValue::Str(v.string(*at))),
            dims: Vec::new(),
        }));
        vars
    }

    fn array(
        &self,
        host: &dyn DebugTarget,
        _name: &str,
        _first: u32,
        _count: u32,
    ) -> Result<Vec<BasicValue>, BasicError> {
        self.view(host).ok_or(BasicError::Inactive)?;
        Err(BasicError::NoSuchVariable)
    }

    fn stack(&self, host: &dyn DebugTarget) -> Vec<BasicFrame> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        let lines = v.lines();
        let from = |line: u16, txt: u16| {
            v.pos(&lines, line, txt).unwrap_or(BasicPos {
                line: 0,
                statement: 0,
            })
        };
        v.frames()
            .into_iter()
            .map(|f| match f {
                RawFrame::Gosub { line, txt } => BasicFrame::Gosub {
                    from: from(line, txt),
                },
                RawFrame::For(l) => BasicFrame::For {
                    var: var_name(l.var),
                    limit: BasicValue::Int(i64::from(l.limit)),
                    step: BasicValue::Int(i64::from(l.step)),
                    from: from(l.line, l.txt),
                },
            })
            .collect()
    }

    fn plan_set(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        index: Option<u32>,
        value: &BasicValue,
    ) -> Result<Vec<(u32, u8)>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        let (at, string) = Self::variable(name)
            .filter(|_| index.is_none())
            .ok_or(BasicError::NoSuchVariable)?;
        let bytes: Vec<u8> = match (string, value) {
            (false, BasicValue::Int(n)) => float_bytes(*n as f64)?.to_vec(),
            (false, BasicValue::Float(f)) => float_bytes(*f)?.to_vec(),
            (true, BasicValue::Str(s)) => {
                let bytes = s
                    .chars()
                    .map(|c| (' '..='~').contains(&c).then_some(c as u8))
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // Only in place: the same length.
                if bytes.len() != v.string(at).len() {
                    return Err(BasicError::Unsupported);
                }
                bytes
            }
            _ => return Err(BasicError::TypeMismatch),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (u32::from(at) + i as u32, b))
            .collect())
    }

    fn stop_points(&self, _host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        self.stops
            .into_iter()
            .flatten()
            .map(|addr| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.space,
                    addr: u32::from(addr),
                },
                granularity: BasicGranularity::Statement,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
