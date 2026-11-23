//! TI BASIC of the TI-99/4A. The interpreter is written in GPL (in the
//! console GROMs) with its statement loop and the common statements in
//! machine code in the console ROM; the program, the variables and the
//! stack lie in the video RAM, the interpreter's own variables in the
//! scratch pad RAM (`>8300`).
//!
//! **Program.** A table of the lines from STLN (`>8330`) to ENLN (`>8332`,
//! the table's last byte; both the same without a program), four bytes a line: its number and the address
//! of its text; the highest number comes first. A line's text is tokens
//! up to a zero byte, its length in the byte before it. A token is a byte
//! from `>80`; `>C7` and `>C8` are a quoted and an unquoted string (a
//! length and the characters: names of subprograms and all numbers are
//! such strings), `>C9` a line number in two bytes; everything below
//! `>80` is a variable's name. There is one statement to a line.
//!
//! **Where the interpreter is.** EXTRAM (`>832E`) points into the line
//! table, at the text address of the line being run; PRGFLG (`>8344`) is 0
//! while a command typed in runs. The statement loop in the console ROM
//! begins every line of a program at one place (`>1976`: `LIMI 3 : LIMI 0
//! : CLR @>83D6`, behind the test of PRGFLG): GOTO and GOSUB branch there,
//! and the loop itself for the next line. That is the stop point; EXTRAM
//! is set when it is reached.
//!
//! **Stack.** The value stack in the video RAM, eight bytes an entry, from
//! the base (`>8324`) up to the entry VSPTR (`>836E`) points to. Numbers
//! of an expression under way lie there too; an entry of another kind has
//! a byte above 99 as its third, which no number has: `>66` GOSUB (the
//! caller's EXTRAM in its first word), `>67` FOR (the variable's entry in
//! the symbol table, the address of its value, the FOR line's EXTRAM),
//! with the limit and the step as the two entries below it.
//!
//! **Variables.** A chain from SYMTAB (`>833E`), the newest first: a byte
//! with the string flag (bit 7) and the number of dimensions (bits 0–2),
//! the name's length, the next entry's address, the address of the name
//! (in the program's text; a `$` is part of it), then the value: a number
//! (eight bytes), or the address of a string's text (its length in the
//! byte before it; 0 for an empty string); for an array the highest index
//! of each dimension and then the elements, the last index running
//! fastest. A number is an exponent of 100 with `>40` added and seven
//! digits of base 100; a negative number has its first word negated.
//!
//! **Listing.** As LIST writes it: the words from the GROM's own table
//! (grouped by length, each word followed by its token), a blank behind
//! every word and before it unless one is there, strings and numbers as
//! stored.
//!
//! Sources: *TI-99/4A Console and Peripheral Expansion System Technical
//! Data* and the *Editor/Assembler* manual (Texas Instruments) for the
//! scratch pad's names and the number format; Heiner Martin, *TI-99/4A
//! intern* (1985) for the names EXTRAM, PRGFLG, VSPTR, STLN and ENLN. The
//! statement loop (`>1968`–`>19E0`), GOTO/GOSUB (`>1B58`), RETURN
//! (`>1B84`) and NEXT (`>1C88`) were read from the user's console ROM with
//! this project's disassembler; the line table, the symbol table and the
//! stack entries were read from the video RAM of a running program
//! (`ti99_boot`); the keyword table from the user's console GROM. No
//! emulator source was used.

use mimic_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use mimic_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: u16 = 4_000;
const MAX_VARS: usize = 1_000;
const MAX_FRAMES: u16 = 512;

const STACK_BASE: u16 = 0x8324;
const EXTRAM: u16 = 0x832e;
const STLN: u16 = 0x8330;
const ENLN: u16 = 0x8332;
const SYMTAB: u16 = 0x833e;
const PRGFLG: u16 = 0x8344;
const VSPTR: u16 = 0x836e;
const VRAM_LEN: u16 = 0x4000;

const T_QUOTED: u8 = 0xc7;
const T_UNQUOTED: u8 = 0xc8;
const T_LINE: u8 = 0xc9;
const T_REM: u8 = 0x9a;
/// TAB, written like a function.
const T_TAB: u8 = 0xfc;
/// The functions: no blanks around them.
const T_FUNCTIONS: std::ops::RangeInclusive<u8> = 0xca..=0xe1;
const ID_GOSUB: u8 = 0x66;
const ID_FOR: u8 = 0x67;

/// LIMI 3 : LIMI 0 : CLR @>83D6 — the top of the statement loop.
const LOOP: [u8; 12] = [
    0x03, 0x00, 0x00, 0x03, 0x03, 0x00, 0x00, 0x00, 0x04, 0xe0, 0x83, 0xd6,
];

/// What was found in the ROM and the GROM.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    stop: u16,
    /// Words by token minus `>80`.
    keywords: Vec<Vec<u8>>,
}

/// The decoder a TI-99/4A keeps; see the module text.
#[derive(Clone, Debug)]
pub struct TiBasic {
    cpu: MemSpaceId,
    vram: MemSpaceId,
    grom: MemSpaceId,
    rom: Option<Rom>,
}

/// The keyword table: groups of words of one length, each word followed
/// by its token, each group ended by `>FF`; the first group holds the
/// signs.
fn keywords(grom: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut at = grom.windows(4).position(|w| w == b")\xb6(\xb7")?;
    let mut out = vec![Vec::new(); 0x80];
    for len in 1..=10 {
        while *grom.get(at)? != 0xff {
            let word = grom.get(at..at + len)?;
            let token = *grom.get(at + len)?;
            // Tokens below >80 are the commands (NEW, LIST …).
            if token >= 0x80 && out[usize::from(token - 0x80)].is_empty() {
                out[usize::from(token - 0x80)] = word.to_vec();
            }
            at += len + 1;
        }
        at += 1;
    }
    Some(out)
}

/// A number: the exponent of 100 with `>40` added, seven digits of base
/// 100; negative with the first word negated; 0 with a zero first word.
fn number(b: [u8; 8]) -> f64 {
    let first = i16::from_be_bytes([b[0], b[1]]);
    if first == 0 {
        return 0.0;
    }
    let [e, d0] = first.unsigned_abs().to_be_bytes();
    let mut digits = [d0, b[2], b[3], b[4], b[5], b[6], b[7]]
        .iter()
        .map(|d| format!("{:02}", d.min(&99)))
        .collect::<String>();
    digits.insert(2, '.');
    let v: f64 = format!("{digits}e{}", 2 * (i32::from(e) - 0x40))
        .parse()
        .unwrap_or(0.0);
    if first < 0 {
        -v
    } else {
        v
    }
}

fn number_bytes(v: f64) -> Result<[u8; 8], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if v == 0.0 {
        return Ok([0; 8]);
    }
    // Fourteen digits and the exponent of ten.
    let text = format!("{:.13e}", v.abs());
    let (mantissa, e10) = text.split_once('e').ok_or(BasicError::OutOfRange)?;
    let e10: i32 = e10.parse().map_err(|_| BasicError::OutOfRange)?;
    let mut digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    // An even exponent of ten: the first digit of base 100 has one digit.
    if e10.rem_euclid(2) == 0 {
        digits.insert(0, '0');
    }
    let e100 = e10.div_euclid(2);
    if e100 > 63 {
        return Err(BasicError::OutOfRange);
    }
    if e100 < -64 {
        return Ok([0; 8]);
    }
    let d = digits.as_bytes();
    let mut out = [0u8; 8];
    out[0] = (e100 + 0x40) as u8;
    for (i, o) in out[1..].iter_mut().enumerate() {
        *o = (d[2 * i] - b'0') * 10 + (d[2 * i + 1] - b'0');
    }
    if v < 0.0 {
        let first = (-i16::from_be_bytes([out[0], out[1]])).to_be_bytes();
        out[..2].copy_from_slice(&first);
    }
    Ok(out)
}

fn ascii(b: u8) -> Option<char> {
    (0x20..=0x7e).contains(&b).then(|| char::from(b))
}

fn push_char(out: &mut String, b: u8) {
    match ascii(b) {
        Some(c) => out.push(c),
        None => out.push_str(&format!("{{${b:02x}}}")),
    }
}

#[derive(Clone, Copy, Debug)]
struct Line {
    number: u16,
    /// Address of the text (the length is in the byte before it).
    text: u16,
}

struct Var {
    name: String,
    string: bool,
    /// The highest index of each dimension.
    dims: Vec<u16>,
    /// The entry.
    addr: u16,
    /// The value; an array's first element.
    value: u16,
}

enum Raw {
    Gosub { extram: u16 },
    For { symbol: u16, at: u16, extram: u16 },
}

struct View<'a> {
    host: &'a dyn DebugTarget,
    cpu: MemSpaceId,
    vram: MemSpaceId,
    rom: &'a Rom,
}

impl View<'_> {
    /// A byte of the video RAM.
    fn b(&self, addr: u16) -> u8 {
        self.host
            .peek(self.vram, u32::from(addr % VRAM_LEN), Size::Byte)
            .map_or(0xff, |v| v as u8)
    }

    fn w(&self, addr: u16) -> u16 {
        u16::from_be_bytes([self.b(addr), self.b(addr.wrapping_add(1))])
    }

    fn eight(&self, addr: u16) -> [u8; 8] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    /// A byte and a word of the scratch pad.
    fn pad_b(&self, addr: u16) -> u8 {
        self.host
            .peek(self.cpu, u32::from(addr), Size::Byte)
            .map_or(0xff, |v| v as u8)
    }

    fn pad(&self, addr: u16) -> u16 {
        u16::from_be_bytes([self.pad_b(addr), self.pad_b(addr + 1)])
    }

    /// The line table is one: BASIC has been started.
    fn active(&self) -> bool {
        let (first, last) = (self.pad(STLN), self.pad(ENLN));
        let (base, top) = (self.pad(STACK_BASE), self.pad(VSPTR));
        (0x0800..VRAM_LEN).contains(&first)
            && last < VRAM_LEN
            && last >= first
            // Without a program both point to the same byte.
            && (last == first || (last + 1 - first).is_multiple_of(4))
            && base < VRAM_LEN
            && top >= base
            && top < VRAM_LEN
            && (top - base).is_multiple_of(8)
    }

    /// The lines in the table's order: the highest number first.
    fn table(&self) -> Vec<Line> {
        let (first, last) = (self.pad(STLN), self.pad(ENLN));
        let count = if last == first {
            0
        } else {
            (last + 1 - first) / 4
        };
        (0..count.min(MAX_LINES))
            .map(|i| Line {
                number: self.w(first + 4 * i) & 0x7fff,
                text: self.w(first + 4 * i + 2),
            })
            .collect()
    }

    /// The number of the line whose table entry EXTRAM `p` points into.
    fn line_of(&self, p: u16) -> Option<u16> {
        let (first, last) = (self.pad(STLN), self.pad(ENLN));
        (p >= first + 2 && p < last && (p - first) % 4 == 2).then(|| self.w(p - 2) & 0x7fff)
    }

    fn pos_of(&self, p: u16) -> BasicPos {
        BasicPos {
            line: u32::from(self.line_of(p).unwrap_or(0)),
            statement: 0,
        }
    }

    fn text(&self, line: &Line) -> String {
        let mut out = String::new();
        let mut at = line.text;
        let end = line
            .text
            .wrapping_add(u16::from(self.b(line.text.wrapping_sub(1))));
        let next = |at: &mut u16| {
            let b = self.b(*at);
            *at = at.wrapping_add(1);
            b
        };
        while at < end {
            let b = next(&mut at);
            match b {
                0 => break,
                T_QUOTED | T_UNQUOTED => {
                    let len = next(&mut at);
                    if b == T_QUOTED {
                        out.push('"');
                    }
                    for _ in 0..len {
                        let c = next(&mut at);
                        push_char(&mut out, c);
                        if b == T_QUOTED && c == b'"' {
                            out.push('"');
                        }
                    }
                    if b == T_QUOTED {
                        out.push('"');
                    }
                }
                T_LINE => {
                    let high = next(&mut at);
                    let n = u16::from(high) << 8 | u16::from(next(&mut at));
                    out.push_str(&n.to_string());
                }
                T_REM => {
                    out.push_str("REM");
                    // The remark's text is stored as it was typed.
                    while at < end && self.b(at) != 0 {
                        let c = next(&mut at);
                        push_char(&mut out, c);
                    }
                }
                0x80..=0xff => {
                    let word = self
                        .rom
                        .keywords
                        .get(usize::from(b - 0x80))
                        .filter(|w| !w.is_empty());
                    let Some(word) = word else {
                        out.push_str(&format!("{{${b:02x}}}"));
                        continue;
                    };
                    let spaced =
                        word[0].is_ascii_alphabetic() && !T_FUNCTIONS.contains(&b) && b != T_TAB;
                    if spaced && !out.is_empty() && !out.ends_with(' ') {
                        out.push(' ');
                    }
                    word.iter().for_each(|&c| out.push(char::from(c)));
                    if spaced {
                        out.push(' ');
                    }
                }
                _ => push_char(&mut out, b),
            }
        }
        out.trim_end().to_string()
    }

    /// The entries of the value stack that are frames, innermost first.
    fn raw_frames(&self) -> Vec<Raw> {
        let (base, top) = (self.pad(STACK_BASE), self.pad(VSPTR));
        let count = ((top - base) / 8).min(MAX_FRAMES);
        (1..=count)
            .rev()
            .map(|i| base + 8 * i)
            .filter_map(|at| match self.b(at + 2) {
                ID_GOSUB => Some(Raw::Gosub { extram: self.w(at) }),
                ID_FOR => Some(Raw::For {
                    symbol: self.w(at),
                    at,
                    extram: self.w(at + 6),
                }),
                _ => None,
            })
            .collect()
    }

    fn name(&self, entry: u16) -> String {
        let at = self.w(entry + 4);
        let mut name = String::new();
        for i in 0..u16::from(self.b(entry + 1).min(15)) {
            push_char(&mut name, self.b(at.wrapping_add(i)));
        }
        name
    }

    fn frames(&self) -> Vec<BasicFrame> {
        self.raw_frames()
            .into_iter()
            .map(|f| match f {
                Raw::Gosub { extram } => BasicFrame::Gosub {
                    from: self.pos_of(extram),
                },
                Raw::For { symbol, at, extram } => BasicFrame::For {
                    var: self.name(symbol),
                    limit: BasicValue::Float(number(self.eight(at - 16))),
                    step: BasicValue::Float(number(self.eight(at - 8))),
                    from: self.pos_of(extram),
                },
            })
            .collect()
    }

    fn state(&self) -> BasicState {
        let p = self.pad(EXTRAM);
        let pos = (self.pad_b(PRGFLG) != 0)
            .then(|| self.line_of(p))
            .flatten()
            .map(|line| BasicPos {
                line: u32::from(line),
                statement: 0,
            });
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos,
            text_addr: pos.map(|_| u32::from(self.w(p))),
            depth: self
                .raw_frames()
                .iter()
                .filter(|f| matches!(f, Raw::Gosub { .. }))
                .count() as u32,
        }
    }

    fn vars(&self) -> Vec<Var> {
        let mut out = Vec::new();
        let mut entry = self.pad(SYMTAB);
        while (0x0800..VRAM_LEN).contains(&entry) && out.len() < MAX_VARS {
            let flags = self.b(entry);
            // Bit 6: a function of DEF, which has no value.
            if flags & 0x40 == 0 {
                let count = u16::from(flags & 7);
                out.push(Var {
                    name: self.name(entry),
                    string: flags & 0x80 != 0,
                    dims: (0..count).map(|i| self.w(entry + 6 + 2 * i)).collect(),
                    addr: entry,
                    value: entry + 6 + 2 * count,
                });
            }
            entry = self.w(entry + 2);
        }
        out
    }

    /// An array's sizes (OPTION BASE 0: one more than the highest index).
    fn sizes(&self, var: &Var) -> Vec<u32> {
        var.dims.iter().map(|d| u32::from(*d) + 1).collect()
    }

    fn value(&self, string: bool, at: u16) -> BasicValue {
        if !string {
            return BasicValue::Float(number(self.eight(at)));
        }
        let text = self.w(at);
        let mut s = String::new();
        if text != 0 {
            for i in 0..u16::from(self.b(text.wrapping_sub(1))) {
                push_char(&mut s, self.b(text.wrapping_add(i)));
            }
        }
        BasicValue::Str(s)
    }
}

/// A name as the debugger's user writes it: in upper case, without blanks.
fn wanted(name: &str) -> String {
    name.chars()
        .filter(|c| *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

impl TiBasic {
    /// The CPU's view (for the scratch pad and the console ROM), the video
    /// RAM and the GROMs.
    pub fn new(cpu: MemSpaceId, vram: MemSpaceId, grom: MemSpaceId) -> Self {
        Self {
            cpu,
            vram,
            grom,
            rom: None,
        }
    }

    /// Look at the ROMs once: `Some` with the decoder to keep.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.rom.is_some() {
            return None;
        }
        let read = |space, from: u32, len: u32| -> Vec<u8> {
            (from..from + len)
                .map(|a| host.peek(space, a, Size::Byte).map_or(0xff, |v| v as u8))
                .collect()
        };
        let rom = read(self.cpu, 0, 0x2000);
        let stop = rom.windows(LOOP.len()).position(|w| w == LOOP)? as u16;
        let keywords = keywords(&read(self.grom, 0x2000, 0x4000))?;
        Some(Self {
            rom: Some(Rom { stop, keywords }),
            ..self.clone()
        })
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let v = View {
            host,
            cpu: self.cpu,
            vram: self.vram,
            rom: self.rom.as_ref()?,
        };
        v.active().then_some(v)
    }
}

impl Dialect for TiBasic {
    fn space(&self) -> MemSpaceId {
        self.vram
    }

    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: "TI BASIC".into(),
            // One statement to a line.
            granularity: BasicGranularity::Statement,
            space: self.vram,
            name_len: 0,
        }
    }

    fn state(&self, host: &dyn DebugTarget) -> BasicState {
        match self.view(host) {
            Some(v) => v.state(),
            None => BasicState {
                mode: BasicMode::Inactive,
                pos: None,
                text_addr: None,
                depth: 0,
            },
        }
    }

    fn program(&self, host: &dyn DebugTarget) -> Vec<BasicLine> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        v.table()
            .iter()
            .rev()
            .map(|l| {
                let len = u16::from(v.b(l.text.wrapping_sub(1)));
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.text),
                    raw: (0..len).map(|i| v.b(l.text.wrapping_add(i))).collect(),
                    text: v.text(l),
                    statements: vec![0],
                }
            })
            .collect()
    }

    fn variables(&self, host: &dyn DebugTarget) -> Vec<BasicVar> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        v.vars()
            .iter()
            .map(|var| BasicVar {
                name: var.name.clone(),
                ty: if var.string {
                    BasicType::Str
                } else {
                    BasicType::Float
                },
                addr: u32::from(var.addr),
                value: var.dims.is_empty().then(|| v.value(var.string, var.value)),
                dims: v.sizes(var),
            })
            .collect()
    }

    fn array(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        first: u32,
        count: u32,
    ) -> Result<Vec<BasicValue>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        let name = wanted(name);
        let vars = v.vars();
        let var = vars
            .iter()
            .find(|x| !x.dims.is_empty() && x.name == name)
            .ok_or(BasicError::NoSuchVariable)?;
        let len = v.sizes(var).iter().product::<u32>();
        if first > len {
            return Err(BasicError::OutOfRange);
        }
        let size = if var.string { 2 } else { 8 };
        Ok((first..len.min(first.saturating_add(count)))
            .map(|i| v.value(var.string, var.value.wrapping_add((i * size) as u16)))
            .collect())
    }

    fn stack(&self, host: &dyn DebugTarget) -> Vec<BasicFrame> {
        self.view(host).map_or_else(Vec::new, |v| v.frames())
    }

    fn plan_set(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        index: Option<u32>,
        value: &BasicValue,
    ) -> Result<Vec<(u32, u8)>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        let name = wanted(name);
        let vars = v.vars();
        let var = vars
            .iter()
            .find(|x| x.name == name && x.dims.is_empty() == index.is_none())
            .ok_or(BasicError::NoSuchVariable)?;
        let size = if var.string { 2 } else { 8 };
        let at = match index {
            None => var.value,
            Some(i) if i < v.sizes(var).iter().product::<u32>() => {
                var.value.wrapping_add((i * size) as u16)
            }
            Some(_) => return Err(BasicError::OutOfRange),
        };
        let (at, bytes): (u16, Vec<u8>) = match (var.string, value) {
            (false, BasicValue::Int(n)) => (at, number_bytes(*n as f64)?.to_vec()),
            (false, BasicValue::Float(f)) => (at, number_bytes(*f)?.to_vec()),
            (true, BasicValue::Str(s)) => {
                let text = s
                    .chars()
                    .map(|c| u8::try_from(c).ok().filter(|b| ascii(*b).is_some()))
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // In place: the same length.
                let data = v.w(at);
                let len = if data == 0 { 0 } else { v.b(data - 1) };
                if text.len() != usize::from(len) {
                    return Err(BasicError::Unsupported);
                }
                (data, text)
            }
            _ => return Err(BasicError::TypeMismatch),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (u32::from(at) + i as u32, b))
            .collect())
    }

    fn stop_points(&self, host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        let _ = host;
        self.rom
            .iter()
            .map(|rom| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.cpu,
                    addr: u32::from(rom.stop),
                },
                granularity: BasicGranularity::Statement,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
