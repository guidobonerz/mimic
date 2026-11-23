//! BBC BASIC for the 6502: BASIC II on the BBC Micro Model B and B+,
//! BASIC IV on the Master.
//!
//! **Program.** From PAGE (`&18` holds its high byte) to TOP (`&12`). A
//! line is `&0D`, its number (high byte first), its length and its text;
//! `&0D &FF` ends the program. Keywords are tokens from `&80`; a line
//! number behind GOTO, GOSUB, THEN, ELSE or RESTORE is `&8D` and three
//! bytes. The listing is LIST's with `LISTO 0`: the text as stored, the
//! tokens written out with the ROM's own keyword table, nothing expanded
//! inside quotes.
//!
//! **Where the interpreter is.** There is no "current line" variable. The
//! text pointer is a base (`&0B`, `&0C`) and an offset (`&0A`); every
//! statement's end moves the base to the statement's delimiter and sets
//! the offset to 1. The statement loop's entry skips blanks and looks at
//! the first character (`LDY &0A : INC &0A : LDA (&0B),Y : CMP #&20 :
//! BEQ : CMP #&CF`); the compare with `&CF` is the stop point, where the
//! base plus the offset less 1 is the statement's first character. The line is the one
//! the pointer lies in. A command typed in runs from the buffer at
//! `&0700`: a base in page 7 is the direct mode, as for the ROM itself
//! (`LDA &0C : CMP #&07`).
//!
//! **Statements.** A colon begins one. So do THEN and ELSE unless a line
//! number follows, and what stands behind REPEAT: the interpreter enters
//! its loop for each of these. REM, DATA and a `*` command take the rest
//! of the line. An empty statement (`::`, or `REPEAT:`) passes the loop
//! too and is counted. Entering a procedure the loop is passed at the end
//! of the `DEF` statement; that is reported as the `DEF` statement.
//!
//! **Stacks.** GOSUB keeps return pointers in two tables (`&05CC` low
//! bytes, `&05E6` high bytes, the count in `&25`), REPEAT likewise
//! (`&05A4`, `&05B8`, `&24`); the pointer is the base at the time, the
//! delimiter behind the GOSUB statement or the REPEAT token. FOR has
//! entries of 15 bytes from `&0500` (`&26` is the offset behind the last):
//! the variable's address and type (4 integer, 5 real), step and limit in
//! the variable's format, and the base behind the FOR statement. PROC and
//! FN move the 6502's stack to BASIC's own stack (`&04`: the old stack
//! pointer, then the bytes above it), set the stack pointer to `&FF` and
//! push the token (`&F2` PROC, `&A4` FN), the caller's offset and base and
//! the number of variables saved for parameters and LOCAL, which lie on
//! BASIC's stack below the saved 6502 stack (four bytes naming the
//! variable, then its value). So `&01FF` holds the token while a procedure
//! runs, and the callers are found by walking BASIC's stack. A call is
//! reported as a GOSUB frame and counts for the depth.
//!
//! **Variables.** `@%` and `A%` to `Z%` are four bytes each from `&0400`
//! (listed while they are not 0). All others hang in chains by their first
//! character, the heads at `&0400 + 2 × character`: the next entry's
//! address, the rest of the name with its `%`, `$` or `(`, a zero byte and
//! the value. An integer is four bytes; a real five (the exponent with 128
//! added, then the mantissa with the sign in its top bit); a string four
//! (address, room, length), so a string can be written up to its room. An
//! array has the offset of its elements (2 × dimensions + 1), the sizes
//! and the elements. Names keep their case.
//!
//! BASIC IV (65C02 code) has the same statement loop, zero page, GOSUB
//! tables, variables and procedure calls, but its REPEAT tables are at
//! `&0500` and `&0514` and its FOR entries begin at `&0528`. The tables'
//! addresses are therefore taken from the operands of the ROM's own
//! GOSUB, REPEAT and FOR code.
//!
//! Sources: the BBC Microcomputer User Guide (Acorn, 1982) and *The BBC
//! Micro Advanced User Guide* (Bray, Dickens, Holmes, 1983) for the
//! program format, the tokens and the variable formats; the statement
//! loop (`&8B9B`, BASIC IV `&900B`), GOSUB/RETURN (`&B888`), FOR/NEXT
//! (`&B7C4`, `&B695`), REPEAT/UNTIL (`&BBE4`, `&BBB1`), PROC/FN and LOCAL
//! (`&B197`, `&9323`), the variable search (`&945B`) and the keyword table
//! (`&8071`) were read from the user's BASIC II and BASIC IV ROMs with a
//! disassembler. No emulator source was used.

use mimic_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use mimic_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: usize = 10_000;
const MAX_VARS: usize = 2_000;
const MAX_CALLS: usize = 64;

const STACK: u16 = 0x04;
const HIMEM: u16 = 0x06;
const OFFSET: u16 = 0x0a;
const BASE: u16 = 0x0b;
const PAGE_HIGH: u16 = 0x18;
const REPEATS: u16 = 0x24;
const GOSUBS: u16 = 0x25;
const FORS: u16 = 0x26;
const RESIDENT: u16 = 0x0400;
const FOR_ENTRY: u16 = 15;
/// The page of the buffer a typed line runs from.
const DIRECT_PAGE: u8 = 0x07;

const ROM_START: u16 = 0x8000;
const ROM_LEN: usize = 0x4000;

const T_LINE: u8 = 0x8d;
const T_ELSE: u8 = 0x8b;
const T_THEN: u8 = 0x8c;
const T_FN: u8 = 0xa4;
const T_DATA: u8 = 0xdc;
const T_PROC: u8 = 0xf2;
const T_REM: u8 = 0xf4;
const T_REPEAT: u8 = 0xf5;

/// LDY &0A : INC &0A : LDA (&0B),Y : CMP #&20 : BEQ : CMP #&CF.
const LOOP: [u8; 12] = [
    0xa4, 0x0a, 0xe6, 0x0a, 0xb1, 0x0b, 0xc9, 0x20, 0xf0, 0xf6, 0xc9, 0xcf,
];
/// The stop point in it: the compare with `&CF`.
const LOOP_STOP: u16 = 10;

/// What was found in the ROM.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    stop: u16,
    name: &'static str,
    /// Keywords by token minus `&80`.
    keywords: Vec<Vec<u8>>,
    /// The FOR entries' start.
    fors: u16,
    /// The tables of low and high bytes of REPEAT's and GOSUB's pointers.
    repeats: (u16, u16),
    gosubs: (u16, u16),
    /// The statement's end moves the base to the delimiter and then sets
    /// the offset to 1: the addresses from behind the base's store to the
    /// offset's, between which the offset is the old one.
    moving: (u16, u16),
}

/// CLC : TYA : ADC &0B : STA &0B : BCC : INC &0C : LDY #1 : STY &0A.
const MOVE: [u8; 14] = [
    0x18, 0x98, 0x65, 0x0b, 0x85, 0x0b, 0x90, 0x02, 0xe6, 0x0c, 0xa0, 0x01, 0x84, 0x0a,
];

/// Two table addresses from `LDA &0B : STA lo,i : LDA &0C : STA hi,i :
/// INC count` (`store` is the STA's opcode).
fn tables(mem: &[u8], store: u8, count: u8) -> Option<(u16, u16)> {
    let at = mem.windows(12).position(|w| {
        w[..3] == [0xa5, 0x0b, store] && w[5..8] == [0xa5, 0x0c, store] && w[10..] == [0xe6, count]
    })?;
    let word = |i: usize| u16::from_le_bytes([mem[at + i], mem[at + i + 1]]);
    Some((word(3), word(8)))
}

/// The decoder a BBC Micro keeps; see the module text.
#[derive(Clone, Debug)]
pub struct BbcBasic {
    space: MemSpaceId,
    rom: Option<Rom>,
}

fn scan(mem: &[u8]) -> Option<Rom> {
    // A language ROM called BASIC.
    if mem.get(9..14) != Some(b"BASIC") {
        return None;
    }
    let stop = mem.windows(LOOP.len()).position(|w| w == LOOP)? as u16 + LOOP_STOP;
    // The keyword table: the word, its token and a flag byte.
    let table = mem.windows(8).position(|w| w == b"AND\x80\x00ABS")?;
    let mut keywords = vec![Vec::new(); 0x80];
    let mut at = table;
    while let Some(len) = mem[at..].iter().take(12).position(|b| *b >= 0x80) {
        if len == 0 {
            break;
        }
        let token = usize::from(mem[at + len] - 0x80);
        // The first word of a token counts (the statement forms of PAGE,
        // PTR, TIME, LOMEM and HIMEM have tokens of their own).
        if keywords[token].is_empty() {
            keywords[token] = mem[at..at + len].to_vec();
        }
        at += len + 2;
        if at + 12 >= mem.len() {
            break;
        }
    }
    let year = mem.get(0x12..0x16).unwrap_or(b"");
    let name = match year {
        b"1981" => "BBC BASIC I",
        b"1982" => "BBC BASIC II",
        b"1983" => "BBC BASIC III",
        _ => "BBC BASIC IV",
    };
    // FOR: LDA &37 : STA entries,Y : LDA &38 : STA entries+1,Y.
    let fors = mem
        .windows(8)
        .position(|w| w[..3] == [0xa5, 0x37, 0x99] && w[5..] == [0xa5, 0x38, 0x99])
        .map_or(0x0500, |at| u16::from_le_bytes([mem[at + 3], mem[at + 4]]));
    let moving = mem
        .windows(MOVE.len())
        .position(|w| w == MOVE)
        .map_or((0, 0), |at| {
            let at = ROM_START + at as u16;
            (at + 6, at + 12)
        });
    Some(Rom {
        moving,
        stop: ROM_START + stop,
        name,
        keywords,
        fors,
        repeats: tables(mem, 0x9d, 0x24).unwrap_or((0x05a4, 0x05b8)),
        gosubs: tables(mem, 0x99, 0x25).unwrap_or((0x05cc, 0x05e6)),
    })
}

/// The BBC's characters in Unicode; `None` for control codes and tokens.
fn bbc_char(b: u8) -> Option<char> {
    match b {
        0x60 => Some('£'),
        0x20..=0x7e => Some(char::from(b)),
        _ => None,
    }
}

fn bbc_code(c: char) -> Option<u8> {
    match c {
        '£' => Some(0x60),
        '`' => None,
        _ => u8::try_from(c).ok().filter(|b| (0x20..=0x7e).contains(b)),
    }
}

fn push_char(out: &mut String, b: u8) {
    match bbc_char(b) {
        Some(c) => out.push(c),
        None => out.push_str(&format!("{{${b:02x}}}")),
    }
}

/// A real: the exponent with 128 added, the mantissa's four bytes with the
/// sign in place of its top bit.
fn real(b: [u8; 5]) -> f64 {
    if b == [0; 5] {
        return 0.0;
    }
    let mantissa = u32::from_be_bytes([b[1] | 0x80, b[2], b[3], b[4]]);
    let v = f64::from(mantissa) / 4_294_967_296.0 * 2f64.powi(i32::from(b[0]) - 128);
    if b[1] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn real_bytes(v: f64) -> Result<[u8; 5], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if v == 0.0 {
        return Ok([0; 5]);
    }
    // v = m × 2^e with 0.5 <= m < 1.
    let mut e = v.abs().log2().floor() as i32 + 1;
    let mut m = (v.abs() / 2f64.powi(e) * 4_294_967_296.0).round();
    if m >= 4_294_967_296.0 {
        m /= 2.0;
        e += 1;
    }
    if m < 2_147_483_648.0 {
        m *= 2.0;
        e -= 1;
    }
    if e > 127 {
        return Err(BasicError::OutOfRange);
    }
    if e < -127 {
        return Ok([0; 5]);
    }
    let m = (m as u32).to_be_bytes();
    let sign = if v < 0.0 { 0x80 } else { 0 };
    Ok([(e + 128) as u8, m[0] & 0x7f | sign, m[1], m[2], m[3]])
}

#[derive(Clone, Copy, Debug)]
struct Line {
    addr: u16,
    number: u16,
    len: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Int,
    Real,
    Str,
}

impl Kind {
    fn size(self) -> u16 {
        match self {
            Kind::Int | Kind::Str => 4,
            Kind::Real => 5,
        }
    }

    fn ty(self) -> BasicType {
        match self {
            Kind::Int => BasicType::Int,
            Kind::Real => BasicType::Float,
            Kind::Str => BasicType::Str,
        }
    }
}

struct Var {
    /// Without an array's parenthesis.
    name: String,
    kind: Kind,
    array: bool,
    /// The entry in its chain (a resident integer: its value).
    entry: u16,
    /// The value; an array's header.
    value: u16,
}

/// A line's text and its statements: where each begins in the text and in
/// memory.
struct Text {
    text: String,
    offsets: Vec<u32>,
    starts: Vec<u16>,
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

    fn five(&self, addr: u16) -> [u8; 5] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    fn int(&self, addr: u16) -> i64 {
        let b: [u8; 4] = std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)));
        i64::from(i32::from_le_bytes(b))
    }

    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let mut at = u32::from(self.b(PAGE_HIGH)) << 8;
        while at + 4 < 0x8000 && out.len() < MAX_LINES {
            let a = at as u16;
            let high = self.b(a + 1);
            let len = self.b(a + 3);
            if self.b(a) != 0x0d || high >= 0x80 || len < 4 {
                break;
            }
            out.push(Line {
                addr: a,
                number: u16::from(high) << 8 | u16::from(self.b(a + 2)),
                len,
            });
            at += u32::from(len);
        }
        out
    }

    fn text(&self, line: &Line) -> Text {
        let end = line.addr + u16::from(line.len);
        let mut t = Text {
            text: String::new(),
            offsets: vec![0],
            starts: vec![line.addr + 4],
        };
        let mut at = line.addr + 4;
        let mut quoted = false;
        // The rest of the line is text (REM, DATA, a `*` command).
        let mut rest = false;
        // Nothing but blanks since the statement began.
        let mut first = true;
        while at < end {
            let b = self.b(at);
            at += 1;
            let begins = first && b != b' ';
            if b != b' ' {
                first = false;
            }
            if rest || quoted {
                quoted &= b != b'"';
                push_char(&mut t.text, b);
                continue;
            }
            let begin = |t: &mut Text, at: u16| {
                t.offsets.push(t.text.len() as u32);
                t.starts.push(at);
            };
            match b {
                b'"' => {
                    quoted = true;
                    t.text.push('"');
                }
                b':' => {
                    t.text.push(':');
                    begin(&mut t, at);
                    first = true;
                }
                b'*' if begins => {
                    rest = true;
                    t.text.push('*');
                }
                T_LINE => {
                    let (a, b, c) = (self.b(at), self.b(at + 1), self.b(at + 2));
                    at += 3;
                    let low = (a << 2 & 0xc0) ^ b;
                    let high = (a << 4 & 0xc0) ^ c;
                    t.text
                        .push_str(&(u16::from(high) << 8 | u16::from(low)).to_string());
                }
                0x80..=0xff => {
                    match self.rom.keywords.get(usize::from(b - 0x80)) {
                        Some(word) if !word.is_empty() => {
                            word.iter().for_each(|&c| t.text.push(char::from(c)));
                        }
                        _ => t.text.push_str(&format!("{{${b:02x}}}")),
                    }
                    match b {
                        T_REM | T_DATA => rest = true,
                        T_REPEAT => {
                            begin(&mut t, at);
                            first = true;
                        }
                        T_THEN | T_ELSE => {
                            // A statement, unless a line number follows.
                            let mut next = at;
                            while next < end && self.b(next) == b' ' {
                                next += 1;
                            }
                            if next < end && self.b(next) != T_LINE {
                                begin(&mut t, at);
                                first = true;
                            }
                        }
                        _ => {}
                    }
                }
                _ => push_char(&mut t.text, b),
            }
        }
        t
    }

    /// The line and statement the text at `p` belongs to. A line owns the
    /// bytes behind its `&0D` up to the next line's `&0D`, a statement its
    /// delimiter.
    fn place(&self, lines: &[Line], p: u16) -> Option<BasicPos> {
        let line = lines
            .iter()
            .find(|l| p > l.addr && u32::from(p) <= u32::from(l.addr) + u32::from(l.len))?;
        let starts = self.text(line).starts;
        let statement = starts.iter().filter(|s| **s <= p).count().max(1) - 1;
        Some(BasicPos {
            line: u32::from(line.number),
            statement: statement as u16,
        })
    }

    fn place_or_none(&self, lines: &[Line], p: u16) -> BasicPos {
        self.place(lines, p).unwrap_or(BasicPos {
            line: 0,
            statement: 0,
        })
    }

    /// The text pointer: the statement's first character at the stop
    /// point, the character read last inside a statement. A statement's
    /// end leaves it on the delimiter (offset 1), which still belongs to
    /// that statement: RETURN, GOSUB and NEXT do that before they act.
    fn pointer(&self) -> u16 {
        let pc = self.host.regs().get("PC").unwrap_or(0) as u16;
        let (from, to) = self.rom.moving;
        if (from..=to).contains(&pc) {
            // The base is on the delimiter already, the offset not yet 1.
            return self.w(BASE);
        }
        self.w(BASE)
            .wrapping_add(u16::from(self.b(OFFSET).max(1)) - 1)
    }

    fn sp(&self) -> u16 {
        let regs = self.host.regs();
        let s = regs.get("SP").or_else(|| regs.get("S")).unwrap_or(0xff);
        0x0100 | (s as u16 & 0xff)
    }

    /// The size of an entry on BASIC's stack that saves a variable.
    fn saved(&self, at: u16) -> u16 {
        match self.b(at + 2) {
            0x80..=0xff => 5 + u16::from(self.b(at + 4)),
            5 => 9,
            _ => 8,
        }
    }

    /// A saved 6502 stack at `at` on BASIC's stack: the address behind it
    /// and the bytes that were at `&01FB`…`&01FF`, if it is one.
    fn saved_stack(&self, at: u16, himem: u16) -> Option<(u16, [u8; 5])> {
        let s = self.b(at);
        let end = u32::from(at) + 256 - u32::from(s);
        if s < 0x80 || end > u32::from(himem) {
            return None;
        }
        let end = end as u16;
        let top: [u8; 5] = std::array::from_fn(|i| self.b(end.wrapping_sub(5 - i as u16)));
        let nested = s < 0xfb && matches!(top[4], T_PROC | T_FN);
        // Else the main level's: a return address into the ROM on top.
        (nested || (0x80..0xc0).contains(&top[4])).then_some((end, top))
    }

    /// The statements that called the procedures and functions running,
    /// innermost first.
    fn calls(&self, lines: &[Line]) -> Vec<BasicPos> {
        let mut out = Vec::new();
        if self.sp() >= 0x01fc {
            return out;
        }
        let himem = self.w(HIMEM);
        // What the 6502's stack holds from `&01FB`: the count of saved
        // variables, the caller's base (high, low) and offset, the token.
        let mut top: [u8; 5] = std::array::from_fn(|i| self.b(0x01fb + i as u16));
        let mut at = self.w(STACK);
        while matches!(top[4], T_PROC | T_FN) && out.len() < MAX_CALLS {
            let base = u16::from(top[1]) << 8 | u16::from(top[2]);
            let p = base.wrapping_add(u16::from(top[3].max(1)) - 1);
            out.push(self.place_or_none(lines, p));
            // Behind the variables saved: the caller's 6502 stack.
            let mut exact = at;
            for _ in 0..top[0] {
                exact = exact.wrapping_add(self.saved(exact));
            }
            // Inside a statement values of the expression under way may
            // lie in between: then the next thing that looks like one.
            let found = self.saved_stack(exact, himem).or_else(|| {
                (at..himem)
                    .take(1024)
                    .find_map(|a| self.saved_stack(a, himem))
            });
            match found {
                Some((end, saved)) => {
                    at = end;
                    top = saved;
                }
                None => break,
            }
        }
        out
    }

    fn frames(&self) -> Vec<BasicFrame> {
        let lines = self.lines();
        let vars = self.vars();
        let mut out = Vec::new();
        // Loops first, then subroutines; the order between the kinds is
        // not recorded (each has a stack of its own).
        let fors = self.b(FORS).min(150) / FOR_ENTRY as u8;
        for i in (0..u16::from(fors)).rev() {
            let e = self.rom.fors + i * FOR_ENTRY;
            let addr = self.w(e);
            let integer = self.b(e + 2) != 5;
            let value = |at: u16| {
                if integer {
                    BasicValue::Int(self.int(at))
                } else {
                    BasicValue::Float(real(self.five(at)))
                }
            };
            let resident = (RESIDENT..RESIDENT + 4 * 27).contains(&addr) && addr.is_multiple_of(4);
            let var = if resident {
                format!("{}%", char::from(b'@' + ((addr - RESIDENT) / 4) as u8))
            } else {
                vars.iter()
                    .find(|v| !v.array && v.value == addr)
                    .map_or_else(|| format!("&{addr:04X}"), |v| v.name.clone())
            };
            // The base behind the FOR statement: its colon, or the next
            // line's length byte.
            let next = self.w(e + 13);
            let delimiter = if self.b(next) == b':' {
                next
            } else {
                next.wrapping_sub(3)
            };
            out.push(BasicFrame::For {
                var,
                limit: value(e + 8),
                step: value(e + 3),
                from: self.place_or_none(&lines, delimiter),
            });
        }
        for i in (0..u16::from(self.b(REPEATS).min(20))).rev() {
            let (low, high) = self.rom.repeats;
            let at = u16::from(self.b(high + i)) << 8 | u16::from(self.b(low + i));
            out.push(BasicFrame::Loop {
                word: "REPEAT".into(),
                from: self.place_or_none(&lines, at),
            });
        }
        for i in (0..u16::from(self.b(GOSUBS).min(26))).rev() {
            let (low, high) = self.rom.gosubs;
            let at = u16::from(self.b(high + i)) << 8 | u16::from(self.b(low + i));
            out.push(BasicFrame::Gosub {
                from: self.place_or_none(&lines, at),
            });
        }
        out.extend(
            self.calls(&lines)
                .into_iter()
                .map(|from| BasicFrame::Gosub { from }),
        );
        out
    }

    fn state(&self) -> BasicState {
        let lines = self.lines();
        let p = self.pointer();
        let pos = (self.b(BASE + 1) != DIRECT_PAGE)
            .then(|| self.place(&lines, p))
            .flatten();
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos,
            text_addr: Some(u32::from(p)),
            depth: u32::from(self.b(GOSUBS).min(26)) + self.calls(&lines).len() as u32,
        }
    }

    fn vars(&self) -> Vec<Var> {
        let mut out = Vec::new();
        for c in b'@'..=b'Z' {
            let at = RESIDENT + 4 * u16::from(c - b'@');
            if self.int(at) != 0 {
                out.push(Var {
                    name: format!("{}%", char::from(c)),
                    kind: Kind::Int,
                    array: false,
                    entry: at,
                    value: at,
                });
            }
        }
        for c in b'@'..=b'z' {
            let mut entry = self.w(RESIDENT + 2 * u16::from(c));
            while entry >> 8 != 0 && out.len() < MAX_VARS {
                let mut name = vec![c];
                let mut at = entry.wrapping_add(2);
                while self.b(at) != 0 && name.len() < 64 {
                    name.push(self.b(at));
                    at = at.wrapping_add(1);
                }
                let array = name.last() == Some(&b'(');
                if array {
                    name.pop();
                }
                let kind = match name.last() {
                    Some(b'%') => Kind::Int,
                    Some(b'$') => Kind::Str,
                    _ => Kind::Real,
                };
                let mut text = String::new();
                name.iter().for_each(|&b| push_char(&mut text, b));
                out.push(Var {
                    name: text,
                    kind,
                    array,
                    entry,
                    value: at.wrapping_add(1),
                });
                entry = self.w(entry);
            }
        }
        out
    }

    fn dims(&self, var: &Var) -> Vec<u32> {
        let count = u16::from(self.b(var.value) / 2);
        (0..count)
            .map(|i| u32::from(self.w(var.value + 1 + 2 * i)))
            .collect()
    }

    /// Where an array's elements begin.
    fn elements(&self, var: &Var) -> u16 {
        var.value.wrapping_add(u16::from(self.b(var.value)))
    }

    fn value(&self, kind: Kind, at: u16) -> BasicValue {
        match kind {
            Kind::Int => BasicValue::Int(self.int(at)),
            Kind::Real => BasicValue::Float(real(self.five(at))),
            Kind::Str => {
                let text = self.w(at);
                let mut s = String::new();
                for i in 0..u16::from(self.b(at + 3)) {
                    push_char(&mut s, self.b(text.wrapping_add(i)));
                }
                BasicValue::Str(s)
            }
        }
    }
}

/// A name as the debugger's user writes it: without blanks. BBC BASIC
/// tells upper and lower case apart.
fn wanted(name: &str) -> String {
    name.chars().filter(|c| *c != ' ').collect()
}

impl BbcBasic {
    /// `space` is the CPU's view.
    pub fn new(space: MemSpaceId) -> Self {
        Self { space, rom: None }
    }

    /// Look at the machine again: `Some` with the decoder to keep from now
    /// on if the interpreter appeared or went away (another sideways ROM
    /// is switched in).
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.rom.as_ref().is_some_and(|r| self.mapped(host, r)) {
            return None;
        }
        let mem: Vec<u8> = (0..ROM_LEN as u32)
            .map(|a| host.peek(self.space, u32::from(ROM_START) + a, Size::Byte))
            .map(|v| v.map_or(0xff, |v| v as u8))
            .collect();
        // While another ROM is switched in the BASIC found before stays:
        // it reports as inactive and its stop point is kept.
        let rom = scan(&mem)?;
        (Some(&rom) != self.rom.as_ref()).then(|| Self {
            rom: Some(rom),
            ..self.clone()
        })
    }

    /// The ROM is switched in.
    fn mapped(&self, host: &dyn DebugTarget, rom: &Rom) -> bool {
        let first = u32::from(rom.stop - LOOP_STOP);
        LOOP.iter().enumerate().all(|(i, &b)| {
            host.peek(self.space, first + i as u32, Size::Byte) == Some(u32::from(b))
        })
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let rom = self.rom.as_ref()?;
        self.mapped(host, rom).then_some(View {
            host,
            space: self.space,
            rom,
        })
    }
}

impl Dialect for BbcBasic {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: self.rom.as_ref().map_or("BBC BASIC", |r| r.name).into(),
            granularity: BasicGranularity::Statement,
            space: self.space,
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
        v.lines()
            .iter()
            .map(|l| {
                let t = v.text(l);
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.addr),
                    raw: (0..u16::from(l.len)).map(|i| v.b(l.addr + i)).collect(),
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
                ty: var.kind.ty(),
                addr: u32::from(var.entry),
                value: (!var.array).then(|| v.value(var.kind, var.value)),
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
            .map(|i| v.value(var.kind, data.wrapping_add(i as u16 * var.kind.size())))
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
        let mut vars = v.vars();
        // A resident integer that is 0 is not listed, but it is there.
        if let [c @ b'@'..=b'Z', b'%'] = name.as_bytes() {
            let at = RESIDENT + 4 * u16::from(c - b'@');
            vars.push(Var {
                name: name.clone(),
                kind: Kind::Int,
                array: false,
                entry: at,
                value: at,
            });
        }
        let var = vars
            .iter()
            .find(|x| x.name == name && x.array == index.is_some())
            .ok_or(BasicError::NoSuchVariable)?;
        let at = match index {
            None => var.value,
            Some(i) if i < v.dims(var).iter().product::<u32>() => {
                v.elements(var).wrapping_add(i as u16 * var.kind.size())
            }
            Some(_) => return Err(BasicError::OutOfRange),
        };
        let number = match value {
            BasicValue::Int(n) => Some(*n as f64),
            BasicValue::Float(f) => Some(*f),
            BasicValue::Str(_) => None,
        };
        let (at, bytes): (u16, Vec<u8>) = match (var.kind, value, number) {
            (Kind::Int, _, Some(n)) => {
                if n.fract() != 0.0 || n < f64::from(i32::MIN) || n > f64::from(i32::MAX) {
                    return Err(BasicError::OutOfRange);
                }
                (at, (n as i32).to_le_bytes().to_vec())
            }
            (Kind::Real, _, Some(n)) => (at, real_bytes(n)?.to_vec()),
            (Kind::Str, BasicValue::Str(s), _) => {
                let text = s
                    .chars()
                    .map(bbc_code)
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // In place, up to the room the string has.
                if text.len() > usize::from(v.b(at + 2)) {
                    return Err(BasicError::Unsupported);
                }
                let data = u32::from(v.w(at));
                let mut writes: Vec<(u32, u8)> = text
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (data + i as u32, *b))
                    .collect();
                writes.push((u32::from(at) + 3, text.len() as u8));
                return Ok(writes);
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
        // Kept while another sideways ROM is switched in for a moment
        // (the filing system during a statement): the breakpoint then
        // meets other code at times, which reports as inactive.
        let _ = host;
        self.rom
            .iter()
            .map(|rom| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.space,
                    addr: u32::from(rom.stop),
                },
                granularity: BasicGranularity::Statement,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
