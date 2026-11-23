//! HC-BASIC of the Robotron KC85/3, /4 and /5 (and the BASIC module M006
//! of the KC85/2): Microsoft's 8K BASIC for the 8080 with robotron's
//! additions.
//!
//! **Program.** From TXTTAB (`$035F` holds its address, `$0401`): link,
//! number, text, 0; two zero bytes end it. Keywords are tokens from `$80`,
//! the ROM's table marks each word's first character with bit 7; tokens
//! from `$D5` are words of an extension table the CAOS ROM points to
//! (`$E00C`). The listing is LIST's (`$C6F2`): the text as stored, tokens
//! written out except inside quotes.
//!
//! **Where the interpreter is.** The statement loop stores the text
//! pointer at its top (`LD (SAVTXT),HL`, `$03CF`): it stands on the colon
//! or the line's end before the statement. For a new line the loop then
//! stores the line number (CURLIN, `$0358`; `$FFFF` while a typed command
//! runs). The stop point is the call that reads the statement's first
//! character (`$C88A`), found as the call before `LD DE,loop : PUSH DE`.
//! FOR uses SAVTXT for the address of its variable until the statement's
//! end; the place is then CURLIN's line with its first statement.
//!
//! **Stack.** There is no saved stack pointer. The frames lie on the Z80's
//! stack and end below the word 0 at its top (the top's address in
//! `$0356`): GOSUB `$8C`, CURLIN, the text pointer behind the GOSUB token
//! (5 bytes); FOR `$81`, the address of the variable's value, the step's
//! sign, the step, the limit, CURLIN and the text pointer at the FOR
//! statement's end (16 bytes). At the stop point the stack pointer is on
//! the first frame; inside a statement return addresses and values lie
//! below, so the frames are the first run of frames from the stack pointer
//! up that ends exactly at the top.
//!
//! **Variables.** Plain variables from VARTAB (`$03D7`) to ARYTAB
//! (`$03D9`), six bytes each: the name's second character (0 without one;
//! bit 7 set for a string), its first, and the value: a number (three
//! mantissa bytes, low first, the sign in bit 7 of the third, and the
//! exponent with `$80` added) or a string (length, a spare byte, address).
//! Arrays from ARYTAB to STREND (`$03DB`): name, the size of what follows,
//! the number of dimensions, their sizes (last dimension first), the
//! elements with the first index running fastest. All numbers are such
//! reals; two characters of a name count.
//!
//! The addresses of the system variables are taken from the operands of
//! the ROM's own code (the statement loop, NEW, LIST).
//!
//! Sources: the KC85/3 and KC85/4 BASIC manuals (robotron) for the
//! language; the statement loop (`$C854`), NEW and CLEAR (`$C640`), LIST
//! (`$C6F2`, the keyword lookup `$C79A`), FOR (`$C7DE`) with the stack
//! search (`$C7BD`), GOSUB and RETURN (`$C9F6`, `$CA25`) and the variable
//! search (`$CF0A`) were read from the user's KC85/4 BASIC ROM with this
//! project's Z80 disassembler. No emulator source was used.

use mimic_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use mimic_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: usize = 10_000;
const MAX_VARS: usize = 4_000;
const MAX_FRAMES: usize = 64;
/// How far above the stack pointer the frames are looked for.
const MAX_SEARCH: u16 = 256;

const ROM_START: u16 = 0xc000;
const ROM_LEN: usize = 0x2000;

const FOR: u8 = 0x81;
const FOR_LEN: u16 = 16;
const GOSUB_LEN: u16 = 5;
/// The first token of the extension table.
const EXTENSION: u8 = 0xd5;

/// What was found in the ROM.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The stop point and the three bytes there.
    stop: u16,
    signature: [u8; 3],
    savtxt: u16,
    curlin: u16,
    txttab: u16,
    vartab: u16,
    /// Holds the address of the stack's top.
    stack_top: u16,
    /// Holds the address of the extension's keyword table.
    extension: Option<u16>,
    keywords: Vec<Vec<u8>>,
    t_gosub: u8,
    t_data: u8,
    /// REM and its short form `!`.
    t_rem: [u8; 2],
}

/// The decoder a KC85 keeps; see the module text.
#[derive(Clone, Debug)]
pub struct HcBasic {
    space: MemSpaceId,
    rom: Option<Rom>,
}

fn find(mem: &[u8], pattern: &[i16]) -> Option<usize> {
    mem.windows(pattern.len()).position(|w| {
        w.iter()
            .zip(pattern)
            .all(|(b, p)| *p < 0 || *p == i16::from(*b))
    })
}

/// Words whose first character has bit 7 set, up to a lone `$80`.
fn words(mem: &[u8], mut at: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while mem.get(at).is_some_and(|b| *b > 0x80) && out.len() < 128 {
        let mut word = vec![mem[at] & 0x7f];
        at += 1;
        while mem.get(at).is_some_and(|b| *b < 0x80) && word.len() < 12 {
            word.push(mem[at]);
            at += 1;
        }
        out.push(word);
    }
    out
}

fn scan(mem: &[u8]) -> Option<Rom> {
    let word = |at: usize| u16::from_le_bytes([mem[at], mem[at + 1]]);
    // LD (SAVTXT),HL : LD A,(HL) : CP ':' : JR Z : OR A : JP NZ.
    let top = find(mem, &[0x22, -1, -1, 0x7e, 0xfe, 0x3a, 0x28, -1, 0xb7, 0xc2])?;
    // … LD E,(HL) : INC HL : LD D,(HL) : LD (CURLIN),DE.
    let line = top + find(&mem[top..top + 0x40], &[0x5e, 0x23, 0x56, 0xed, 0x53])?;
    // CALL first character : LD DE,loop : PUSH DE, the loop a little
    // before its top (it begins with two calls).
    let stop = (top..top + 0x60).find(|&at| {
        let loop_start = usize::from(word(at + 4)).wrapping_sub(usize::from(ROM_START));
        mem[at] == 0xcd && mem[at + 3] == 0x11 && mem[at + 6] == 0xd5 && top - loop_start <= 8
    })?;
    // NEW: LD HL,(TXTTAB) : XOR A : LD (nn),A : LD (HL),A : INC HL :
    // LD (HL),A : INC HL : LD (VARTAB),HL.
    let new = find(
        mem,
        &[
            0x2a, -1, -1, 0xaf, 0x32, -1, -1, 0x77, 0x23, 0x77, 0x23, 0x22,
        ],
    )?;
    // CLEAR's end: POP BC : LD HL,(top) : LD SP,HL.
    let clear = find(mem, &[0xc1, 0x2a, -1, -1, 0xf9])?;
    // LIST's lookup: LD DE,table : LD B,A : LD A,(DE) : INC DE : OR A.
    let list = find(mem, &[0x11, -1, -1, 0x47, 0x1a, 0x13, 0xb7])?;
    let table = usize::from(word(list + 1)).checked_sub(usize::from(ROM_START))?;
    // Before it, for the tokens of the extension: LD DE,(pointer) : JR.
    let extension = (mem[list - 6..list - 2] == [0xed, 0x5b, mem[list - 4], mem[list - 3]]
        && mem[list - 2] == 0x18)
        .then(|| word(list - 4));
    let keywords = words(mem, table);
    let token = |w: &[u8]| keywords.iter().position(|k| k == w).map(|i| 0x80 + i as u8);
    Some(Rom {
        stop: ROM_START + stop as u16,
        signature: [mem[stop], mem[stop + 1], mem[stop + 2]],
        savtxt: word(top + 1),
        curlin: word(line + 5),
        txttab: word(new + 1),
        vartab: word(new + 12),
        stack_top: word(clear + 2),
        extension,
        t_gosub: token(b"GOSUB")?,
        t_data: token(b"DATA")?,
        t_rem: [token(b"REM")?, token(b"!").unwrap_or(0)],
        keywords,
    })
}

/// A number: three mantissa bytes, low first, the sign in bit 7 of the
/// third; the exponent with `$80` added, 0 for zero.
fn number(b: [u8; 4]) -> f64 {
    if b[3] == 0 {
        return 0.0;
    }
    let mantissa = u32::from_le_bytes([b[0], b[1], b[2] | 0x80, 0]);
    let v = f64::from(mantissa) / 16_777_216.0 * 2f64.powi(i32::from(b[3]) - 128);
    if b[2] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn number_bytes(v: f64) -> Result<[u8; 4], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if v == 0.0 {
        return Ok([0; 4]);
    }
    let mut e = v.abs().log2().floor() as i32 + 1;
    let mut m = (v.abs() / 2f64.powi(e) * 16_777_216.0).round();
    if m >= 16_777_216.0 {
        m /= 2.0;
        e += 1;
    }
    if m < 8_388_608.0 {
        m *= 2.0;
        e -= 1;
    }
    if e > 127 {
        return Err(BasicError::OutOfRange);
    }
    if e < -127 {
        return Ok([0; 4]);
    }
    let m = (m as u32).to_le_bytes();
    let sign = if v < 0.0 { 0x80 } else { 0 };
    Ok([m[0], m[1], m[2] & 0x7f | sign, (e + 128) as u8])
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
    addr: u16,
    number: u16,
    /// Address of the terminating 0.
    end: u16,
}

struct Var {
    /// With `$` for a string; without an array's parenthesis.
    name: String,
    string: bool,
    array: bool,
    /// The entry.
    addr: u16,
    /// The value; an array's dimension count.
    value: u16,
}

struct Text {
    text: String,
    offsets: Vec<u32>,
    /// Where each statement's text begins in memory.
    starts: Vec<u16>,
}

enum Raw {
    Gosub {
        line: u16,
        text: u16,
    },
    For {
        var: u16,
        at: u16,
        line: u16,
        text: u16,
    },
}

struct View<'a> {
    host: &'a dyn DebugTarget,
    space: MemSpaceId,
    rom: &'a Rom,
}

impl View<'_> {
    fn b(&self, addr: u16) -> u8 {
        self.host
            .peek(self.space, u32::from(addr), Size::Byte)
            .map_or(0xff, |v| v as u8)
    }

    fn w(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.b(addr), self.b(addr.wrapping_add(1))])
    }

    fn four(&self, addr: u16) -> [u8; 4] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let mut at = self.w(self.rom.txttab);
        let end = self.w(self.rom.vartab);
        while out.len() < MAX_LINES && at < end && self.w(at) != 0 {
            let mut e = at.wrapping_add(4);
            while self.b(e) != 0 && e.wrapping_sub(at) < 512 {
                e = e.wrapping_add(1);
            }
            out.push(Line {
                addr: at,
                number: self.w(at.wrapping_add(2)),
                end: e,
            });
            at = e.wrapping_add(1);
        }
        out
    }

    fn keyword(&self, token: u8) -> Option<Vec<u8>> {
        if token < EXTENSION {
            return self.rom.keywords.get(usize::from(token - 0x80)).cloned();
        }
        // The extension's table, word by word as LIST does.
        let mut at = self.w(self.rom.extension?);
        let mut word = Vec::new();
        for _ in 0..=token - EXTENSION {
            if self.b(at) <= 0x80 {
                return None;
            }
            word = vec![self.b(at) & 0x7f];
            at = at.wrapping_add(1);
            while self.b(at) < 0x80 && self.b(at) != 0 && word.len() < 12 {
                word.push(self.b(at));
                at = at.wrapping_add(1);
            }
        }
        Some(word)
    }

    fn text(&self, line: &Line) -> Text {
        let first = line.addr.wrapping_add(4);
        let mut t = Text {
            text: String::new(),
            offsets: vec![0],
            starts: vec![first],
        };
        let mut quoted = false;
        // DATA: to the next colon; REM: to the line's end.
        let (mut data, mut rest) = (false, false);
        let mut at = first;
        while at != line.end {
            let b = self.b(at);
            at = at.wrapping_add(1);
            if b == b'"' {
                quoted = !quoted;
            }
            if b == b':' && !quoted && !rest {
                data = false;
                t.text.push(':');
                t.offsets.push(t.text.len() as u32);
                t.starts.push(at);
                continue;
            }
            if b < 0x80 || quoted {
                push_char(&mut t.text, b);
                continue;
            }
            match self.keyword(b) {
                Some(word) => word.iter().for_each(|&c| push_char(&mut t.text, c)),
                None => t.text.push_str(&format!("{{${b:02x}}}")),
            }
            if !data && !rest {
                data = b == self.rom.t_data;
                rest = self.rom.t_rem.contains(&b);
            }
        }
        t
    }

    /// The statement of `line` the text at `p` belongs to; a statement
    /// owns the colon behind it.
    fn statement(&self, line: &Line, p: u16) -> u16 {
        let starts = self.text(line).starts;
        (starts.iter().filter(|s| **s <= p).count().max(1) - 1) as u16
    }

    fn pos(&self, lines: &[Line], number: u16, p: u16) -> BasicPos {
        let statement = lines
            .iter()
            .find(|l| l.number == number && p >= l.addr && p <= l.end)
            .map_or(0, |l| self.statement(l, p));
        BasicPos {
            line: u32::from(number),
            statement,
        }
    }

    fn sp(&self) -> u16 {
        self.host.regs().get("SP").unwrap_or(0) as u16
    }

    /// The frames from `p` if they end exactly at the stack's bottom.
    fn chain(&self, mut p: u16, bottom: u16) -> Option<Vec<Raw>> {
        let mut out = Vec::new();
        while p != bottom {
            if p > bottom || out.len() >= MAX_FRAMES {
                return None;
            }
            match self.b(p) {
                FOR => {
                    out.push(Raw::For {
                        var: self.w(p + 1),
                        at: p,
                        line: self.w(p + 12),
                        text: self.w(p + 14),
                    });
                    p += FOR_LEN;
                }
                t if t == self.rom.t_gosub => {
                    out.push(Raw::Gosub {
                        line: self.w(p + 1),
                        text: self.w(p + 3),
                    });
                    p += GOSUB_LEN;
                }
                _ => return None,
            }
        }
        Some(out)
    }

    fn raw_frames(&self) -> Vec<Raw> {
        // Below the word 0 that CLEAR pushes.
        let bottom = self.w(self.rom.stack_top).wrapping_sub(2);
        let sp = self.sp();
        if sp > bottom || bottom - sp > 0x1000 {
            return Vec::new();
        }
        (sp..=bottom)
            .take(usize::from(MAX_SEARCH))
            .find_map(|p| self.chain(p, bottom))
            .unwrap_or_default()
    }

    fn frames(&self) -> Vec<BasicFrame> {
        let lines = self.lines();
        let vars = self.vars();
        self.raw_frames()
            .into_iter()
            .map(|f| match f {
                Raw::Gosub { line, text } => BasicFrame::Gosub {
                    from: self.pos(&lines, line, text),
                },
                Raw::For {
                    var,
                    at,
                    line,
                    text,
                } => BasicFrame::For {
                    var: vars
                        .iter()
                        .find(|v| !v.array && v.value == var)
                        .map_or_else(|| format!("${var:04X}"), |v| v.name.clone()),
                    limit: BasicValue::Float(number(self.four(at + 8))),
                    step: BasicValue::Float(number(self.four(at + 4))),
                    from: self.pos(&lines, line, text),
                },
            })
            .collect()
    }

    fn state(&self) -> BasicState {
        let lines = self.lines();
        let current = self.w(self.rom.curlin);
        let p = self.w(self.rom.savtxt);
        let pos = (current != 0xffff)
            .then(|| lines.iter().find(|l| l.number == current))
            .flatten()
            .map(|l| {
                // On the colon before the statement, or on the end of the
                // line before (the line's own first statement then).
                let statement = if p >= l.addr.wrapping_add(4) && p < l.end {
                    self.statement(l, p.wrapping_add(1))
                } else {
                    0
                };
                BasicPos {
                    line: u32::from(l.number),
                    statement,
                }
            });
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos,
            text_addr: Some(u32::from(p)),
            depth: self
                .raw_frames()
                .iter()
                .filter(|f| matches!(f, Raw::Gosub { .. }))
                .count() as u32,
        }
    }

    fn name(&self, addr: u16) -> (String, bool) {
        let (second, first) = (self.b(addr), self.b(addr.wrapping_add(1)));
        let mut name = String::new();
        push_char(&mut name, first);
        if second & 0x7f != 0 {
            push_char(&mut name, second & 0x7f);
        }
        let string = second & 0x80 != 0;
        if string {
            name.push('$');
        }
        (name, string)
    }

    fn vars(&self) -> Vec<Var> {
        let vartab = self.w(self.rom.vartab);
        let arytab = self.w(self.rom.vartab + 2);
        let strend = self.w(self.rom.vartab + 4);
        let mut out = Vec::new();
        let mut at = vartab;
        while at.wrapping_add(6) <= arytab && at >= vartab && out.len() < MAX_VARS {
            let (name, string) = self.name(at);
            out.push(Var {
                name,
                string,
                array: false,
                addr: at,
                value: at + 2,
            });
            at += 6;
        }
        let mut at = arytab;
        while at.wrapping_add(5) <= strend && at >= arytab && out.len() < MAX_VARS {
            let (name, string) = self.name(at);
            out.push(Var {
                name,
                string,
                array: true,
                addr: at,
                value: at + 4,
            });
            at = at.wrapping_add(4).wrapping_add(self.w(at + 2));
        }
        out
    }

    /// An array's sizes, the first dimension first.
    fn dims(&self, var: &Var) -> Vec<u32> {
        let count = u16::from(self.b(var.value)).min(16);
        (0..count)
            .rev()
            .map(|i| u32::from(self.w(var.value + 1 + 2 * i)))
            .collect()
    }

    fn elements(&self, var: &Var) -> u16 {
        var.value + 1 + 2 * u16::from(self.b(var.value)).min(16)
    }

    fn value(&self, string: bool, at: u16) -> BasicValue {
        if !string {
            return BasicValue::Float(number(self.four(at)));
        }
        let text = self.w(at + 2);
        let mut s = String::new();
        for i in 0..u16::from(self.b(at)) {
            push_char(&mut s, self.b(text.wrapping_add(i)));
        }
        BasicValue::Str(s)
    }
}

/// A name as the debugger's user writes it: in upper case, without
/// blanks, cut to the two characters the interpreter tells apart.
fn wanted(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let string = clean.ends_with('$');
    let mut short: String = clean.trim_end_matches('$').chars().take(2).collect();
    if string {
        short.push('$');
    }
    short
}

impl HcBasic {
    /// `space` is the CPU's view.
    pub fn new(space: MemSpaceId) -> Self {
        Self { space, rom: None }
    }

    /// Look at the machine again: `Some` with the decoder to keep from now
    /// on if the interpreter appeared (the BASIC ROM was switched in).
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.rom.as_ref().is_some_and(|r| self.mapped(host, r)) {
            return None;
        }
        let mem: Vec<u8> = (0..ROM_LEN as u32)
            .map(|a| host.peek(self.space, u32::from(ROM_START) + a, Size::Byte))
            .map(|v| v.map_or(0xff, |v| v as u8))
            .collect();
        let rom = scan(&mem)?;
        (Some(&rom) != self.rom.as_ref()).then(|| Self {
            rom: Some(rom),
            ..self.clone()
        })
    }

    fn mapped(&self, host: &dyn DebugTarget, rom: &Rom) -> bool {
        rom.signature.iter().enumerate().all(|(i, &b)| {
            host.peek(self.space, u32::from(rom.stop) + i as u32, Size::Byte) == Some(u32::from(b))
        })
    }

    /// The ROM is switched in and BASIC was started (the byte before the
    /// program is 0 from then on).
    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let rom = self.rom.as_ref()?;
        let v = View {
            host,
            space: self.space,
            rom,
        };
        let txttab = v.w(rom.txttab);
        (self.mapped(host, rom) && txttab != 0 && v.b(txttab - 1) == 0).then_some(v)
    }
}

impl Dialect for HcBasic {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: "HC-BASIC".into(),
            granularity: BasicGranularity::Statement,
            space: self.space,
            name_len: 2,
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
        v.lines()
            .iter()
            .map(|l| {
                let t = v.text(l);
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.addr),
                    raw: (l.addr..=l.end).map(|a| v.b(a)).collect(),
                    text: t.text,
                    statements: t.offsets,
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
                value: (!var.array).then(|| v.value(var.string, var.value)),
                dims: if var.array { v.dims(var) } else { Vec::new() },
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
            .find(|x| x.array && x.name == name)
            .ok_or(BasicError::NoSuchVariable)?;
        let len = v.dims(var).iter().product::<u32>();
        if first > len {
            return Err(BasicError::OutOfRange);
        }
        let data = v.elements(var);
        Ok((first..len.min(first.saturating_add(count)))
            .map(|i| v.value(var.string, data.wrapping_add(4 * i as u16)))
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
            .find(|x| x.name == name && x.array == index.is_some())
            .ok_or(BasicError::NoSuchVariable)?;
        let at = match index {
            None => var.value,
            Some(i) if i < v.dims(var).iter().product::<u32>() => {
                v.elements(var).wrapping_add(4 * i as u16)
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
                // In place: the same length, and not a constant in the
                // program's text.
                let data = v.w(at + 2);
                let program = v.w(v.rom.txttab)..v.w(v.rom.vartab);
                if text.len() != usize::from(v.b(at)) || program.contains(&data) {
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
        self.view(host)
            .map(|v| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.space,
                    addr: u32::from(v.rom.stop),
                },
                granularity: BasicGranularity::Statement,
            })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests;
