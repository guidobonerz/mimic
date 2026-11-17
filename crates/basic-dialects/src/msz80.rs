//! Microsoft BASIC for the Z80: the TRS-80's Level II BASIC (Model I and
//! Model III) and MSX BASIC.
//!
//! These interpreters keep their text pointer in the HL register, not in
//! memory, but at the top of the statement loop they store it and the
//! stack pointer:
//!
//! ```text
//! NEWSTT: …
//!         LD (SAVTXT),HL      ; on the ':' or the 0 before the statement
//!         LD (SAVSTK),SP      ; only FOR and GOSUB frames above it
//!         LD A,(HL) : CP ':' : JR Z,exec
//!         …                   ; a new line: its number into CURLIN
//! exec:   RST 10H             ; the stop point
//! ```
//!
//! The loop is found by these two stores (`22 lo hi` and `ED 73 lo hi`
//! with the addresses of SAVTXT and SAVSTK, in either order) before
//! `RST 10H : LD DE,NEWSTT : PUSH DE`; that `RST` is the stop point.
//! Everything the decoder reports comes
//! from SAVTXT and SAVSTK, so it also holds while the machine is halted
//! inside a statement: the statement is the one behind SAVTXT (the next
//! line's first if SAVTXT is on a line's end), and the frames start at
//! SAVSTK. Both are written at the loop's top, so place and stack belong
//! to the same moment; while GOSUB, RETURN, FOR or NEXT run they still
//! show the state at the statement's start.
//!
//! A program is running while CURLIN is not `$FFFF` and SAVTXT leads to a
//! stored line.
//!
//! Program lines: link (2), number (2), text, 0; the system variables are
//! at fixed addresses in RAM (the same in every ROM version of a machine).
//!
//! # TRS-80 Level II (Model I 1.2/1.3, Model III)
//!
//! TXTTAB `$40A4`, CURLIN `$40A2`, SAVTXT `$40E6`, SAVSTK `$40E8`, VARTAB
//! `$40F9`, ARYTAB `$40FB`, STREND `$40FD`. The keyword table (`$1650`)
//! marks each word's first character with bit 7 and ends with `$80`; it is
//! read from the ROM. The listing is LIST's (`$2B7E`): every byte from
//! `$80` is a keyword, also inside quotes; `'` is stored as `:`, REM, `'`
//! and ELSE as `:`, ELSE, and LIST hides those colons. They still begin a
//! statement for the interpreter, so `'` and ELSE are statements of their
//! own here.
//!
//! Variables (from VARTAB): type (2 integer, 3 string, 4 single, 8 double
//! precision: the bytes of the value), the name's second and first
//! character, the value: integer low/high; string length and pointer;
//! single three mantissa bytes (low first, the sign in bit 7 of the third)
//! and the exponent + `$80`; double seven mantissa bytes and the exponent.
//! Arrays (from ARYTAB): type, name, the size of what follows (2), the
//! number of dimensions, their sizes (2 each, last dimension first), the
//! elements with the first index running fastest. A name's type sign is
//! shown for integers (`%`), strings (`$`) and double precision (`#`);
//! single precision has none (DEFINT and its kin are not looked at: a
//! variable made integer by DEFINT A shows as `A%`).
//!
//! Frames on the Z80 stack, from SAVSTK up: GOSUB `$91`, CURLIN, text
//! pointer (5 bytes); FOR `$81`, the address of the variable's value, the
//! step's sign, a type byte, step and limit (4 bytes each for single
//! precision; for an integer loop four unused bytes, the step, the limit),
//! CURLIN, text pointer (17 bytes).
//!
//! # MSX BASIC (MSX1 and MSX2)
//!
//! TXTTAB `$F676`, CURLIN `$F41C`, SAVTXT `$F6AF`, SAVSTK `$F6B1`, VARTAB
//! `$F6C2`, ARYTAB `$F6C4`, STREND `$F6C6`. What differs from the TRS-80:
//!
//! - The keyword table (`$3A72`) is alphabetical: for each letter the
//!   words without it, the last character with bit 7 set, each followed
//!   by its token; LIST takes a token's first word (GOTO, not GO TO).
//!   Tokens below `$80` are functions, stored as `$FF`, token + `$80`.
//! - The tokeniser packs constants: `$0B`/`$0C` octal and hex words,
//!   `$0E` a line number, `$0D` the pointer it becomes once the line ran
//!   (to the byte before the target line), `$0F` a byte, `$11`–`$1A` the
//!   digits 0–9, `$1C` an integer, `$1D` single and `$1F` double
//!   precision. The listing writes them out as LIST does (`$5284`,
//!   `$537E`): single precision gets a `!` unless its text has a point or
//!   an exponent, double precision a `#` unless it has an exponent. Lines
//!   end where their links say, since constants hold zero bytes.
//! - LIST knows quotes, DATA and REM: bytes from `$80` are text there.
//! - Floats are decimal: sign and exponent + `$40`, then two digits a
//!   byte (6 or 14), the value 0.d1d2… × 10^exponent. They print without
//!   a leading zero, plain up to 14 digits before the point and from
//!   0.01, with an exponent otherwise (`.001` is `1E-03`).
//! - A variable's name has its first character first. The default type
//!   is double precision, which therefore has no sign; single precision
//!   shows `!`.
//! - A GOSUB frame has a word for the event traps behind its token (7
//!   bytes). A FOR frame has 16 bytes for step and limit whatever the
//!   type (25 bytes): double precision 8 and 8; single precision the step
//!   at offset 8, the limit as two register pairs with its second half
//!   below the first; an integer loop the step at offset 12 and the limit
//!   at 14.
//!
//! An event trap (ON KEY GOSUB …) pushes its frame between the two stores
//! at the loop's top, so at the handler's first statement the frames
//! still lack it.
//!
//! Sources: the TRS-80's system variables are those of the "TRS-80 Level
//! II BASIC Reference Manual" (VARPTR, appendix) and of James Farvour,
//! "Microsoft BASIC Decoded & Other Mysteries" (1981), MSX's those of the
//! "MSX2 Technical Handbook" (system work area); the statement loops
//! (`$1D1E`, MSX `$4601`), GOSUB (`$1EB1`, `$47B2`), FOR (`$1CA1`,
//! `$4524`), LIST's expansion (`$2B7E`, `$5284`) and the keyword tables
//! were read from the user's Level II ROM and MSX1 BIOS with this
//! project's Z80 disassembler; MSX's number format was compared with what
//! LIST writes on the screen. No emulator source was used.

use mimic_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use mimic_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: usize = 20_000;
const MAX_VARS: usize = 8_192;
const MAX_FRAMES: usize = 128;

/// The system variables of one machine family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sys {
    txttab: u16,
    curlin: u16,
    savtxt: u16,
    savstk: u16,
    vartab: u16,
    arytab: u16,
    strend: u16,
    /// Where the ROM with the interpreter is: first address and length.
    rom: (u16, usize),
    msx: bool,
}

const TRS80: Sys = Sys {
    txttab: 0x40a4,
    curlin: 0x40a2,
    savtxt: 0x40e6,
    savstk: 0x40e8,
    vartab: 0x40f9,
    arytab: 0x40fb,
    strend: 0x40fd,
    rom: (0, 0x3000),
    msx: false,
};

const MSX: Sys = Sys {
    txttab: 0xf676,
    curlin: 0xf41c,
    savtxt: 0xf6af,
    savstk: 0xf6b1,
    vartab: 0xf6c2,
    arytab: 0xf6c4,
    strend: 0xf6c6,
    rom: (0, 0x8000),
    msx: true,
};

/// What was found in the ROM.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The stop point: `RST 10H` of the statement loop.
    stop: u16,
    /// The three bytes there, to see whether the ROM is still mapped.
    signature: [u8; 3],
    /// The words of the tokens from `$80`.
    keywords: Vec<String>,
    /// MSX: the words of the function tokens `$FF nn`, by `nn` & `$7F`.
    functions: Vec<String>,
    t_data: u8,
    /// Tokens the decoder has to know, by their place in the table.
    t_for: u8,
    t_gosub: u8,
    t_rem: u8,
    t_else: u8,
    t_tick: u8,
}

/// The decoder a Z80 machine with a Microsoft BASIC keeps; see the module
/// text.
#[derive(Clone, Debug)]
pub struct Z80Basic {
    space: MemSpaceId,
    name: &'static str,
    sys: Sys,
    rom: Option<Rom>,
}

#[derive(Clone, Copy)]
struct Line {
    addr: u16,
    number: u16,
    /// Address of the terminating 0.
    end: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Int,
    Str,
    Single,
    Double,
}

impl VarKind {
    fn of(type_byte: u8) -> Option<Self> {
        Some(match type_byte {
            2 => VarKind::Int,
            3 => VarKind::Str,
            4 => VarKind::Single,
            8 => VarKind::Double,
            _ => return None,
        })
    }

    /// The bytes of a value, which is also the type byte.
    fn bytes(self) -> u16 {
        match self {
            VarKind::Int => 2,
            VarKind::Str => 3,
            VarKind::Single => 4,
            VarKind::Double => 8,
        }
    }

    fn ty(self) -> BasicType {
        match self {
            VarKind::Int => BasicType::Int,
            VarKind::Str => BasicType::Str,
            _ => BasicType::Float,
        }
    }

    /// The type sign shown; none for the dialect's default type.
    fn sign(self, msx: bool) -> &'static str {
        match self {
            VarKind::Int => "%",
            VarKind::Str => "$",
            VarKind::Single if msx => "!",
            VarKind::Double if !msx => "#",
            _ => "",
        }
    }
}

struct Array {
    name: String,
    kind: VarKind,
    addr: u16,
    data: u16,
    /// As stored: last dimension first.
    dims: Vec<u32>,
}

impl Array {
    fn elements(&self) -> u32 {
        self.dims.iter().fold(1u32, |n, &d| n.saturating_mul(d))
    }
}

enum RawFrame {
    Gosub {
        line: u16,
        txt: u16,
    },
    For {
        /// Address of the variable's value.
        var: u16,
        /// The bytes of step and limit (8 on the TRS-80, 16 on MSX).
        values: [u8; 16],
        line: u16,
        txt: u16,
    },
}

fn push_ascii(out: &mut String, b: u8) {
    match b {
        0x20..=0x7e => out.push(char::from(b)),
        _ => out.push_str(&format!("{{${b:02x}}}")),
    }
}

/// A float in Microsoft's binary format: the mantissa's bytes low first,
/// the sign in bit 7 of its last one, then the exponent + `$80`.
fn mbf_value(bytes: &[u8]) -> f64 {
    let (mantissa, exp) = bytes.split_at(bytes.len() - 1);
    if exp[0] == 0 {
        return 0.0;
    }
    let top = mantissa.len() - 1;
    let mut m = 0.0;
    for (i, &b) in mantissa.iter().enumerate() {
        let b = if i == top { b | 0x80 } else { b };
        m = (m + f64::from(b)) / 256.0;
    }
    let v = m * 2f64.powi(i32::from(exp[0]) - 128);
    if mantissa[top] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

/// `v` in that format with `len` bytes (4 or 8).
fn mbf_bytes(v: f64, len: usize) -> Result<Vec<u8>, BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if v == 0.0 {
        return Ok(vec![0; len]);
    }
    let bits = 8 * (len as i32 - 1);
    let scale = 2f64.powi(bits);
    let mut exp = v.abs().log2().floor() as i32 + 1;
    let mut mantissa = (v.abs() / 2f64.powi(exp) * scale).round();
    // The logarithm may be off by one at powers of two; rounding may carry.
    if mantissa >= scale {
        mantissa /= 2.0;
        exp += 1;
    } else if mantissa < scale / 2.0 {
        mantissa *= 2.0;
        exp -= 1;
    }
    if exp < -127 {
        return Ok(vec![0; len]);
    }
    if exp > 127 {
        return Err(BasicError::OutOfRange);
    }
    let mut out = (mantissa as u64).to_le_bytes()[..len - 1].to_vec();
    out[len - 2] = out[len - 2] & 0x7f | if v < 0.0 { 0x80 } else { 0 };
    out.push((exp + 128) as u8);
    Ok(out)
}

/// A float in MSX BASIC's decimal format: sign and exponent + `$40`, then
/// two digits a byte (6 or 14), the value 0.d1d2… × 10^exponent.
fn bcd_value(bytes: &[u8]) -> f64 {
    bcd_text(bytes).replace('D', "E").parse().unwrap_or(0.0)
}

/// The digits and the decimal exponent of such a float.
fn bcd_digits(bytes: &[u8]) -> (bool, Vec<u8>, i32) {
    let digits = bytes[1..].iter().flat_map(|b| [b >> 4, b & 15]).collect();
    (
        bytes[0] & 0x80 != 0,
        digits,
        i32::from(bytes[0] & 0x7f) - 0x40,
    )
}

/// Such a float as MSX BASIC prints it: without a leading zero, plain up
/// to the format's digits before the point and for values from 0.01, with
/// an exponent (`E`, double precision `D`) otherwise.
fn bcd_text(bytes: &[u8]) -> String {
    if bytes[0] & 0x7f == 0 {
        return "0".into();
    }
    let (negative, mut digits, exp) = bcd_digits(bytes);
    let double = digits.len() > 6;
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    let ascii = |d: &[u8]| -> String { d.iter().map(|&d| char::from(b'0' + d.min(9))).collect() };
    let n = digits.len() as i32;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    // Up to 14 digits before the point, whatever the precision.
    if (-1..=14).contains(&exp) {
        if exp <= 0 {
            out.push('.');
            out.push_str(&"0".repeat(-exp as usize));
            out.push_str(&ascii(&digits));
        } else if n <= exp {
            out.push_str(&ascii(&digits));
            out.push_str(&"0".repeat((exp - n) as usize));
        } else {
            out.push_str(&ascii(&digits[..exp as usize]));
            out.push('.');
            out.push_str(&ascii(&digits[exp as usize..]));
        }
    } else {
        out.push_str(&ascii(&digits[..1]));
        if n > 1 {
            out.push('.');
            out.push_str(&ascii(&digits[1..]));
        }
        let e = exp - 1;
        out.push(if double { 'D' } else { 'E' });
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", e.abs()));
    }
    out
}

/// `v` in that format with `len` bytes (4 or 8).
fn bcd_bytes(v: f64, len: usize) -> Result<Vec<u8>, BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    let places = 2 * (len - 1);
    // "d.ddddde-x": the digits and the exponent of the first one.
    let text = format!("{:.*e}", places - 1, v.abs());
    let (mantissa, e) = text.split_once('e').ok_or(BasicError::OutOfRange)?;
    let digits: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|d| d - b'0')
        .collect();
    let exp = e.parse::<i32>().map_err(|_| BasicError::OutOfRange)? + 1;
    if v == 0.0 || exp < -63 {
        return Ok(vec![0; len]);
    }
    if exp > 63 || digits.len() != places {
        return Err(BasicError::OutOfRange);
    }
    let mut out = vec![(exp + 0x40) as u8 | if v < 0.0 { 0x80 } else { 0 }];
    out.extend(digits.chunks(2).map(|d| d[0] << 4 | d[1]));
    Ok(out)
}

/// The bytes a packed constant of MSX BASIC takes with its mark.
fn constant_len(mark: u8) -> u16 {
    match mark {
        0x0b..=0x0e | 0x1c => 3,
        0x0f => 2,
        0x1d => 5,
        0x1f => 9,
        _ => 1,
    }
}

/// MSX BASIC's keyword table: for each letter from A the words without
/// that letter, the last character with bit 7 set, each followed by its
/// token, a 0 behind each letter's words; behind Z the signs, complete.
/// Tokens below `$80` are functions (stored as `$FF`, token + `$80`).
/// Returns the words of the tokens from `$80` and of the functions.
fn words_msx(mem: &[u8]) -> (Vec<String>, Vec<String>) {
    let mut keywords = vec![String::new(); 0x80];
    let mut functions = vec![String::new(); 0x80];
    let mut i = 0;
    for letter in b'A'..=b'[' {
        while let Some(&first) = mem.get(i) {
            if first == 0 {
                i += 1;
                break;
            }
            let mut word = String::new();
            if letter <= b'Z' {
                word.push(char::from(letter));
            }
            while let Some(&c) = mem.get(i) {
                i += 1;
                push_ascii(&mut word, c & 0x7f);
                if c & 0x80 != 0 {
                    break;
                }
            }
            let Some(&token) = mem.get(i) else {
                break;
            };
            i += 1;
            let list = if token < 0x80 {
                &mut functions
            } else {
                &mut keywords
            };
            // LIST takes the first word with the token (GOTO, not GO TO).
            let slot = &mut list[usize::from(token & 0x7f)];
            if slot.is_empty() {
                *slot = word;
            }
        }
    }
    (keywords, functions)
}

fn number(value: &BasicValue) -> Result<f64, BasicError> {
    match *value {
        BasicValue::Int(n) => Ok(n as f64),
        BasicValue::Float(f) => Ok(f),
        BasicValue::Str(_) => Err(BasicError::TypeMismatch),
    }
}

fn int16(value: &BasicValue) -> Result<i16, BasicError> {
    let f = number(value)?;
    if f.fract() != 0.0 || !(-32768.0..=32767.0).contains(&f) {
        return Err(BasicError::OutOfRange);
    }
    Ok(f as i16)
}

fn peek(host: &dyn DebugTarget, space: MemSpaceId, addr: u16) -> u8 {
    host.peek(space, u32::from(addr), Size::Byte)
        .map_or(0xff, |v| v as u8)
}

/// The interpreter as it is mapped now.
struct View<'a> {
    host: &'a dyn DebugTarget,
    space: MemSpaceId,
    sys: Sys,
    rom: &'a Rom,
}

impl View<'_> {
    fn b(&self, addr: u16) -> u8 {
        peek(self.host, self.space, addr)
    }

    fn w(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.b(addr), self.b(addr.wrapping_add(1))])
    }

    fn bytes<const N: usize>(&self, addr: u16) -> [u8; N] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let mut at = self.w(self.sys.txttab);
        while out.len() < MAX_LINES {
            let link = self.w(at);
            if link == 0 {
                break;
            }
            // The line ends before the next one; MSX has constants with
            // zero bytes in its text.
            let text = at.wrapping_add(4);
            let mut end = link.wrapping_sub(1);
            if link < text || link - text > 0x110 || self.b(end) != 0 {
                end = text;
                while self.b(end) != 0 && end.wrapping_sub(text) < 255 {
                    end = end.wrapping_add(1);
                }
            }
            out.push(Line {
                addr: at,
                number: self.w(at.wrapping_add(2)),
                end,
            });
            if link <= at {
                break;
            }
            at = link;
        }
        out
    }

    /// A line as `LIST` shows it, where its statements begin in that text
    /// and the addresses of the colons between them.
    fn scan(&self, line: &Line) -> (String, Vec<u32>, Vec<u16>) {
        if self.sys.msx {
            return self.scan_msx(line);
        }
        let rom = self.rom;
        let mut text = String::new();
        let mut statements = vec![0];
        let mut colons = Vec::new();
        let (mut quote, mut rem) = (false, false);
        let mut at = line.addr.wrapping_add(4);
        while at != line.end {
            let b = self.b(at);
            match b {
                0x80..=0xff => {
                    // LIST hides the colon before ELSE and the colon and
                    // REM before the tick.
                    let hidden = if b == rom.t_tick {
                        ":REM"
                    } else if b == rom.t_else {
                        ":"
                    } else {
                        ""
                    };
                    if !hidden.is_empty() && text.ends_with(hidden) {
                        text.truncate(text.len() - hidden.len());
                        if let Some(last) = statements.last_mut() {
                            *last = (*last).min(text.len() as u32);
                        }
                    }
                    rem |= (b == rom.t_rem || b == rom.t_tick) && !quote;
                    match rom.keywords.get(usize::from(b - 0x80)) {
                        Some(k) => text.push_str(k),
                        None => push_ascii(&mut text, b),
                    }
                }
                b'"' => {
                    quote = !quote;
                    text.push('"');
                }
                b':' if !quote && !rem => {
                    text.push(':');
                    statements.push(text.len() as u32);
                    colons.push(at);
                }
                _ => push_ascii(&mut text, b),
            }
            at = at.wrapping_add(1);
        }
        (text, statements, colons)
    }

    /// MSX: LIST (`$5284`) knows quotes, DATA and REM (bytes from `$80`
    /// are text there) and writes out the constants the tokeniser packed.
    fn scan_msx(&self, line: &Line) -> (String, Vec<u32>, Vec<u16>) {
        let rom = self.rom;
        let mut text = String::new();
        let mut statements = vec![0];
        let mut colons = Vec::new();
        let (mut quote, mut data, mut rem) = (false, false, false);
        let mut at = line.addr.wrapping_add(4);
        let left = |at: u16| line.end.wrapping_sub(at);
        while at != line.end {
            let b = self.b(at);
            let mut used = 1;
            match b {
                b'"' => {
                    quote = !quote;
                    text.push('"');
                }
                b':' if !quote && !rem => {
                    data = false;
                    text.push(':');
                    statements.push(text.len() as u32);
                    colons.push(at);
                }
                0x0b..=0x1f if !quote && !rem && !data => {
                    used = constant_len(b).min(left(at));
                    let bytes: Vec<u8> = (1..used).map(|i| self.b(at.wrapping_add(i))).collect();
                    if usize::from(used) == usize::from(constant_len(b)) {
                        self.constant(&mut text, b, &bytes);
                    }
                }
                0x80..=0xff if !quote && !data && (!rem || b == rom.t_tick) => {
                    let hidden = if b == rom.t_tick {
                        ":REM"
                    } else if b == rom.t_else {
                        ":"
                    } else {
                        ""
                    };
                    if !hidden.is_empty() && text.ends_with(hidden) {
                        text.truncate(text.len() - hidden.len());
                        if let Some(last) = statements.last_mut() {
                            *last = (*last).min(text.len() as u32);
                        }
                    }
                    let word = if b == 0xff && left(at) > 1 {
                        used = 2;
                        let n = self.b(at.wrapping_add(1)) & 0x7f;
                        rom.functions.get(usize::from(n))
                    } else {
                        rem |= b == rom.t_rem || b == rom.t_tick;
                        data |= b == rom.t_data;
                        rom.keywords.get(usize::from(b - 0x80))
                    };
                    match word.filter(|w| !w.is_empty()) {
                        Some(k) => text.push_str(k),
                        None => push_ascii(&mut text, b),
                    }
                }
                _ => push_ascii(&mut text, b),
            }
            at = at.wrapping_add(used);
        }
        (text, statements, colons)
    }

    /// A packed constant as LIST writes it.
    fn constant(&self, text: &mut String, kind: u8, bytes: &[u8]) {
        let word = || u16::from_le_bytes([bytes[0], bytes[1]]);
        match kind {
            0x0b => text.push_str(&format!("&O{:o}", word())),
            0x0c => text.push_str(&format!("&H{:X}", word())),
            // A line number, or once the line ran the address of its line.
            0x0d => text.push_str(&self.w(word().wrapping_add(3)).to_string()),
            0x0e => text.push_str(&word().to_string()),
            0x0f => text.push_str(&bytes[0].to_string()),
            0x11..=0x1a => text.push(char::from(b'0' + kind - 0x11)),
            0x1c => text.push_str(&(word() as i16).to_string()),
            0x1d | 0x1f => {
                let s = bcd_text(bytes);
                text.push_str(&s);
                // A sign unless the text itself shows the type.
                if kind == 0x1d && !s.contains(['.', 'E', 'D']) {
                    text.push('!');
                } else if kind == 0x1f && !s.contains(['E', 'D']) {
                    text.push('#');
                }
            }
            _ => {}
        }
    }

    /// The statement that begins behind `txt`, a pointer on a colon or on
    /// the 0 before a line.
    fn pos_behind(&self, lines: &[Line], txt: u16) -> Option<BasicPos> {
        match self.b(txt) {
            0 => lines
                .iter()
                .find(|l| l.addr == txt.wrapping_add(1))
                .map(|l| BasicPos {
                    line: u32::from(l.number),
                    statement: 0,
                }),
            b':' => {
                let l = lines.iter().find(|l| (l.addr + 4..l.end).contains(&txt))?;
                let (_, _, colons) = self.scan(l);
                let n = colons.iter().filter(|&&c| c <= txt).count();
                // A colon inside a string or behind REM is no statement's
                // end: SAVTXT is not there.
                colons.contains(&txt).then_some(BasicPos {
                    line: u32::from(l.number),
                    statement: n as u16,
                })
            }
            _ => None,
        }
    }

    /// The statement of line `number` the saved text pointer `txt` is in
    /// (FOR and GOSUB frames).
    fn pos_in(&self, lines: &[Line], number: u16, txt: u16) -> BasicPos {
        let statement = lines
            .iter()
            .find(|l| l.number == number && (l.addr..=l.end).contains(&txt))
            .map_or(0, |l| {
                let (_, _, colons) = self.scan(l);
                colons.iter().filter(|&&c| c < txt).count()
            });
        BasicPos {
            line: u32::from(number),
            statement: statement as u16,
        }
    }

    /// The frames from SAVSTK up, innermost first.
    fn frames(&self) -> Vec<RawFrame> {
        let mut out = Vec::new();
        let mut p = self.w(self.sys.savstk);
        while out.len() < MAX_FRAMES {
            let token = self.b(p);
            // MSX: a word for the event traps in the GOSUB frame, 16
            // bytes for step and limit in the FOR frame.
            let (trap, values) = if self.sys.msx { (2, 16) } else { (0, 8) };
            if token == self.rom.t_gosub {
                out.push(RawFrame::Gosub {
                    line: self.w(p.wrapping_add(1 + trap)),
                    txt: self.w(p.wrapping_add(3 + trap)),
                });
                p = p.wrapping_add(5 + trap);
            } else if token == self.rom.t_for {
                out.push(RawFrame::For {
                    var: self.w(p.wrapping_add(1)),
                    values: self.bytes(p.wrapping_add(5)),
                    line: self.w(p.wrapping_add(5 + values)),
                    txt: self.w(p.wrapping_add(7 + values)),
                });
                p = p.wrapping_add(9 + values);
            } else {
                break;
            }
        }
        out
    }

    fn name(&self, entry: u16, kind: VarKind) -> String {
        let mut name = String::new();
        // The TRS-80 stores the second character first.
        let (first, second) = if self.sys.msx { (1, 2) } else { (2, 1) };
        for at in [first, second] {
            let c = self.b(entry.wrapping_add(at));
            if c != 0 {
                push_ascii(&mut name, c);
            }
        }
        name.push_str(kind.sign(self.sys.msx));
        name
    }

    /// Plain variables: (entry address, name, kind).
    fn scalars(&self) -> Vec<(u16, String, VarKind)> {
        let (start, end) = (self.w(self.sys.vartab), self.w(self.sys.arytab));
        let mut out = Vec::new();
        let mut at = start;
        while at < end && out.len() < MAX_VARS {
            let Some(kind) = VarKind::of(self.b(at)) else {
                break;
            };
            if end - at < 3 + kind.bytes() {
                break;
            }
            out.push((at, self.name(at, kind), kind));
            at += 3 + kind.bytes();
        }
        out
    }

    fn arrays(&self) -> Vec<Array> {
        let (start, end) = (self.w(self.sys.arytab), self.w(self.sys.strend));
        let mut out = Vec::new();
        let mut at = start;
        while at < end && end - at >= 6 && out.len() < MAX_VARS {
            let Some(kind) = VarKind::of(self.b(at)) else {
                break;
            };
            // The size counts what follows it.
            let size = self.w(at + 3);
            let n = u16::from(self.b(at + 5));
            let header = 1 + 2 * n;
            if n == 0 || size < header || size > end - at - 5 {
                break;
            }
            let dims: Vec<u32> = (0..n).map(|i| u32::from(self.w(at + 6 + 2 * i))).collect();
            let array = Array {
                name: self.name(at, kind),
                kind,
                addr: at,
                data: at + 5 + header,
                dims,
            };
            let fits =
                u64::from(array.elements()) * u64::from(kind.bytes()) <= u64::from(size - header);
            if fits {
                out.push(array);
            }
            at += 5 + size;
        }
        out
    }

    fn string(&self, at: u16) -> String {
        let (len, ptr) = (self.b(at), self.w(at.wrapping_add(1)));
        let mut s = String::new();
        for i in 0..u16::from(len) {
            push_ascii(&mut s, self.b(ptr.wrapping_add(i)));
        }
        s
    }

    fn value(&self, at: u16, kind: VarKind) -> BasicValue {
        match kind {
            VarKind::Int => BasicValue::Int(i64::from(self.w(at) as i16)),
            VarKind::Str => BasicValue::Str(self.string(at)),
            VarKind::Single => BasicValue::Float(self.float(&self.bytes::<4>(at))),
            VarKind::Double => BasicValue::Float(self.float(&self.bytes::<8>(at))),
        }
    }

    /// A float of 4 or 8 bytes in the dialect's format.
    fn float(&self, bytes: &[u8]) -> f64 {
        if self.sys.msx {
            bcd_value(bytes)
        } else {
            mbf_value(bytes)
        }
    }

    fn float_bytes(&self, v: f64, len: usize) -> Result<Vec<u8>, BasicError> {
        if self.sys.msx {
            bcd_bytes(v, len)
        } else {
            mbf_bytes(v, len)
        }
    }

    /// The bytes that store `value` at `at`.
    fn plan(
        &self,
        at: u16,
        kind: VarKind,
        value: &BasicValue,
    ) -> Result<Vec<(u32, u8)>, BasicError> {
        let (target, bytes): (u16, Vec<u8>) = match kind {
            VarKind::Int => (at, int16(value)?.to_le_bytes().to_vec()),
            VarKind::Str => {
                let BasicValue::Str(s) = value else {
                    return Err(BasicError::TypeMismatch);
                };
                let bytes = s
                    .chars()
                    .map(|c| (' '..='~').contains(&c).then_some(c as u8))
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // Only in place: the same length, and not a literal in
                // the program text.
                let ptr = self.w(at.wrapping_add(1));
                if bytes.len() != usize::from(self.b(at)) || ptr < self.w(self.sys.strend) {
                    return Err(BasicError::Unsupported);
                }
                (ptr, bytes)
            }
            VarKind::Single => (at, self.float_bytes(number(value)?, 4)?),
            VarKind::Double => (at, self.float_bytes(number(value)?, 8)?),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (u32::from(target.wrapping_add(i as u16)), b))
            .collect())
    }
}

/// A keyword table whose words begin with bit 7 set and that ends with a
/// bare `$80`.
fn words(mem: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for &b in mem {
        if b == 0x80 || out.len() > 0x7f {
            break;
        }
        if b & 0x80 != 0 {
            out.push(String::new());
        }
        if let Some(word) = out.last_mut() {
            push_ascii(word, b & 0x7f);
        }
    }
    out
}

fn find(host: &dyn DebugTarget, space: MemSpaceId, sys: &Sys) -> Option<Rom> {
    let (base, len) = sys.rom;
    let mem: Vec<u8> = (0..len)
        .map(|i| peek(host, space, base.wrapping_add(i as u16)))
        .collect();
    let [tl, th] = sys.savtxt.to_le_bytes();
    let [sl, sh] = sys.savstk.to_le_bytes();
    let (text, stack) = ([0x22, tl, th], [0xed, 0x73, sl, sh]);
    // RST 10H : LD DE,NEWSTT : PUSH DE, with both stores in the loop's
    // head before it.
    let stop = (0x60..mem.len().saturating_sub(4)).find(|&i| {
        let head = &mem[i - 0x60..i];
        mem[i] == 0xd7
            && mem[i + 1] == 0x11
            && mem[i + 4] == 0xd5
            && head.windows(3).any(|w| w == text)
            && head.windows(4).any(|w| w == stack)
    })?;
    let (keywords, functions) = if sys.msx {
        // The table begins with A's words: (A)UTO, (A)ND.
        const HEAD: &[u8] = &[b'U', b'T', b'O' | 0x80, 0xa9, b'N', b'D' | 0x80];
        let table = mem.windows(HEAD.len()).position(|w| w == HEAD)?;
        words_msx(&mem[table..])
    } else {
        const HEAD: &[u8] = &[b'E' | 0x80, b'N', b'D', b'F' | 0x80, b'O', b'R'];
        let table = mem.windows(HEAD.len()).position(|w| w == HEAD)?;
        (words(&mem[table..]), Vec::new())
    };
    let token = |word: &str| {
        keywords
            .iter()
            .position(|k| k == word)
            .map(|i| 0x80 + i as u8)
    };
    Some(Rom {
        stop: base.wrapping_add(stop as u16),
        signature: [mem[stop], mem[stop + 1], mem[stop + 2]],
        t_for: token("FOR")?,
        t_gosub: token("GOSUB")?,
        t_rem: token("REM")?,
        // A token no line has, where the dialect lacks the word.
        t_else: token("ELSE").unwrap_or(0),
        t_tick: token("'").unwrap_or(0),
        t_data: token("DATA").unwrap_or(0),
        keywords,
        functions,
    })
}

impl Z80Basic {
    /// The TRS-80's Level II BASIC, named as the machine does (`space`:
    /// the CPU's view).
    pub fn trs80(space: MemSpaceId, name: &'static str) -> Self {
        Self {
            space,
            name,
            sys: TRS80,
            rom: None,
        }
    }

    /// MSX BASIC (MSX1 and MSX2), in the CPU's view `space`.
    pub fn msx(space: MemSpaceId, name: &'static str) -> Self {
        Self {
            space,
            name,
            sys: MSX,
            rom: None,
        }
    }

    /// Look at the machine again: `Some` with the decoder to keep from now
    /// on if the interpreter appeared or went away.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.rom.as_ref().is_some_and(|r| self.mapped(host, r)) {
            return None;
        }
        let rom = find(host, self.space, &self.sys);
        (rom != self.rom).then(|| Self {
            rom,
            ..self.clone()
        })
    }

    fn mapped(&self, host: &dyn DebugTarget, rom: &Rom) -> bool {
        let at = rom.stop;
        [0, 1, 2].map(|i| peek(host, self.space, at.wrapping_add(i))) == rom.signature
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let rom = self.rom.as_ref()?;
        self.mapped(host, rom).then_some(View {
            host,
            space: self.space,
            sys: self.sys,
            rom,
        })
    }
}

impl Dialect for Z80Basic {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: self.name.to_string(),
            granularity: BasicGranularity::Statement,
            space: self.space,
            name_len: 2,
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
        let txt = v.w(v.sys.savtxt);
        let lines = v.lines();
        let pos = v
            .pos_behind(&lines, txt)
            .filter(|_| v.w(v.sys.curlin) != 0xffff);
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
                    raw: (0..=l.end.wrapping_sub(l.addr))
                        .map(|i| v.b(l.addr.wrapping_add(i)))
                        .collect(),
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
        let mut vars: Vec<BasicVar> = v
            .scalars()
            .into_iter()
            .map(|(at, name, kind)| BasicVar {
                name,
                ty: kind.ty(),
                addr: u32::from(at),
                value: Some(v.value(at + 3, kind)),
                dims: Vec::new(),
            })
            .collect();
        vars.extend(v.arrays().into_iter().map(|a| BasicVar {
            name: a.name,
            ty: a.kind.ty(),
            addr: u32::from(a.addr),
            value: None,
            dims: a.dims.into_iter().rev().collect(),
        }));
        vars
    }

    fn array(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        first: u32,
        count: u32,
    ) -> Result<Vec<BasicValue>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        let a = v
            .arrays()
            .into_iter()
            .find(|a| a.name == name)
            .ok_or(BasicError::NoSuchVariable)?;
        let total = a.elements();
        if first >= total {
            return Err(BasicError::OutOfRange);
        }
        let size = u32::from(a.kind.bytes());
        Ok((first..total.min(first.saturating_add(count)))
            .map(|i| v.value(a.data.wrapping_add((i * size) as u16), a.kind))
            .collect())
    }

    fn stack(&self, host: &dyn DebugTarget) -> Vec<BasicFrame> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        let lines = v.lines();
        v.frames()
            .into_iter()
            .map(|f| match f {
                RawFrame::Gosub { line, txt } => BasicFrame::Gosub {
                    from: v.pos_in(&lines, line, txt),
                },
                RawFrame::For {
                    var,
                    values,
                    line,
                    txt,
                } => {
                    let entry = var.wrapping_sub(3);
                    let kind = VarKind::of(v.b(entry)).unwrap_or(VarKind::Single);
                    let int = |i: usize| {
                        let n = i16::from_le_bytes([values[i], values[i + 1]]);
                        BasicValue::Int(i64::from(n))
                    };
                    let float = |b: &[u8]| BasicValue::Float(v.float(b));
                    let (step, limit) = match (v.sys.msx, kind) {
                        (false, VarKind::Int) => (int(4), int(6)),
                        (false, _) => (float(&values[..4]), float(&values[4..8])),
                        (true, VarKind::Int) => (int(12), int(14)),
                        // The limit was pushed as two register pairs, its
                        // second half below the first.
                        (true, VarKind::Single) => (
                            float(&values[8..12]),
                            float(&[values[14], values[15], values[12], values[13]]),
                        ),
                        (true, _) => (float(&values[..8]), float(&values[8..])),
                    };
                    BasicFrame::For {
                        var: v.name(entry, kind),
                        limit,
                        step,
                        from: v.pos_in(&lines, line, txt),
                    }
                }
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
        match index {
            None => {
                let (at, _, kind) = v
                    .scalars()
                    .into_iter()
                    .find(|(_, n, _)| n == name)
                    .ok_or(BasicError::NoSuchVariable)?;
                v.plan(at + 3, kind, value)
            }
            Some(i) => {
                let a = v
                    .arrays()
                    .into_iter()
                    .find(|a| a.name == name)
                    .ok_or(BasicError::NoSuchVariable)?;
                if i >= a.elements() {
                    return Err(BasicError::OutOfRange);
                }
                let at = a.data.wrapping_add((i * u32::from(a.kind.bytes())) as u16);
                v.plan(at, a.kind, value)
            }
        }
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
