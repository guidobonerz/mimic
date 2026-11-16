//! Sinclair BASIC: the ZX81 (and TS1000) and the ZX Spectrum 48K, 128K
//! and +2. One interpreter by descent, so one decoder with two layouts.
//!
//! **Program.** From `$407D` to D_FILE on the ZX81, from PROG to VARS on
//! the Spectrum. A line is its number (high byte first), its length (low
//! byte first), the text and a NEWLINE (`$76`) or ENTER (`$0D`). Behind
//! every number in the text stands a marker (`$7E`, `$0E`) and the five
//! bytes of its value, which LIST skips. Keywords are tokens; the listing
//! writes them as the ROM's LIST does, with the blanks it puts before and
//! behind them, and takes the words from the ROM's own table.
//!
//! **Where the interpreter is.** PPC holds the line being run, on the
//! Spectrum SUBPPC the statement in it (1 for the first); a command typed
//! in runs as line −2. The Spectrum's statement loop counts SUBPPC up and
//! then looks at the statement's first character; the stop point is
//! behind that, where a statement is known to follow (`LD HL,STMT-RET`).
//! The loop also checks the syntax of a line typed in, with FLAGS bit 7
//! clear. Colons and THEN both begin a statement: the one behind THEN has
//! a number of its own, unlike in Microsoft's BASICs. The 128K and the +2
//! have a second copy of the loop in their editor ROM, which runs
//! programs in 128 BASIC; the stop points are therefore addresses in the
//! ROM space, one per ROM, so that code of the other ROM at the same
//! address does not count. The ZX81 has one statement per line; its stop
//! point is behind the store of PPC in the line loop.
//!
//! **Running or not.** While a line runs on the Spectrum, ERR_SP points
//! to the return address into the main loop: the one behind
//! `CALL LINE-RUN`, in 128 BASIC `$5B1D`. The editor puts an error
//! handler of its own there and the old ERR_SP above it, which is
//! followed (INPUT uses the editor too: FLAGX bit 5 then tells it from
//! the editor proper). The ZX81's line loop writes `$C0` into the high
//! byte of X_PTR after every statement, and its editor clears X_PTR.
//!
//! **GOSUB.** The frames lie above the word ERR_SP points to, up to an
//! end marker whose high byte is `$3E`: on the Spectrum the line (low
//! byte first) and the number of the statement to return to, three
//! bytes; on the ZX81 the line number plus one.
//!
//! **FOR.** There is no stack of loops: FOR turns its variable into a
//! control variable that holds value, limit, step, the line to loop to
//! (ZX81: the FOR line plus one) and on the Spectrum the statement. It
//! stays one after the loop has ended. A loop is reported as open while
//! its value has not passed the limit, the latest in the program first,
//! behind the GOSUB frames; the true nesting is not recorded anywhere.
//!
//! **Variables.** From VARS to an end marker `$80`. The first byte's top
//! three bits are the kind, its low five the letter: `011` a number with
//! a one-letter name, `101` one with a longer name (the last character
//! has bit 7 set), `111` a FOR control variable, `010` a string (length,
//! text), `100` an array of numbers and `110` one of characters (total
//! length, number of dimensions, the dimensions, the elements). Numbers
//! are five bytes: exponent + 128, then 32 bits of mantissa with the
//! sign in place of the leading 1; exponent 0 is zero on the ZX81 and on
//! the Spectrum a small integer (sign byte, 16 bits low byte first,
//! negative ones in two's complement). Names are told apart by all their
//! characters but not by case; they are reported in upper case.
//!
//! Sources: Ian Logan and Frank O'Hara, *The Complete Spectrum ROM
//! Disassembly* and *The Complete ZX81 ROM Disassembly* (Melbourne House,
//! 1983) for the system variables, the variable formats and the names of
//! the routines; Steven Vickers, *ZX81 BASIC Programming* (chapter 27)
//! and *ZX Spectrum BASIC Programming* (chapter 24) for the memory
//! layout. The statement loops (48K `$1B28`, 128K ROM 0 `$17C0`, +2
//! `$17DF`), GO SUB and RETURN (`$1EED`, `$1F23`; 128K `$1A53`), the
//! start of a run (`$12F4`, 128K `$0304`), the ZX81's line loop
//! (`$066C`), its GOSUB (`$0EB5`), FOR (`$0DB9`) and its keyword printing
//! (`$094B`) were read from the user's ROMs with this project's Z80
//! disassembler. No emulator source was used.

use emu_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use emu_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: usize = 10_000;
const MAX_VARS: usize = 2_000;
const MAX_FRAMES: usize = 256;
/// The high byte of the GOSUB stack's end marker.
const END_MARKER: u8 = 0x3e;
const ROM_BANK: usize = 0x4000;

const T_REM: u8 = 0xea;
/// THEN on the Spectrum.
const T_THEN: u8 = 0xcb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Zx81,
    Spectrum,
}

/// Addresses of the system variables.
#[derive(Clone, Copy, Debug)]
struct Sys {
    flags: u16,
    err_sp: u16,
    ramtop: u16,
    ppc: u16,
    ch_add: u16,
    vars: u16,
    e_line: u16,
    flagx: u16,
}

const ZX81: Sys = Sys {
    flags: 0x4001,
    err_sp: 0x4002,
    ramtop: 0x4004,
    ppc: 0x4007,
    ch_add: 0x4016,
    vars: 0x4010,
    e_line: 0x4014,
    flagx: 0x402d,
};
const ZX81_PROG: u16 = 0x407d;
const ZX81_D_FILE: u16 = 0x400c;
const ZX81_X_PTR: u16 = 0x4018;

const SPECTRUM: Sys = Sys {
    flags: 0x5c3b,
    err_sp: 0x5c3d,
    ramtop: 0x5cb2,
    ppc: 0x5c45,
    ch_add: 0x5c5d,
    vars: 0x5c4b,
    e_line: 0x5c59,
    flagx: 0x5c71,
};
const SPECTRUM_SUBPPC: u16 = 0x5c47;
const SPECTRUM_PROG: u16 = 0x5c53;

/// What was found in the ROMs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The stop points, as addresses in the space they are armed in.
    stops: Vec<u32>,
    /// What ERR_SP points to while a line runs.
    bases: Vec<u16>,
    /// The keyword table's words in the machine's character codes.
    words: Vec<Vec<u8>>,
    /// The 128K's editor ROM is there.
    is128: bool,
}

/// The decoder a ZX81 or a Spectrum keeps; see the module text.
#[derive(Clone, Debug)]
pub struct Sinclair {
    family: Family,
    space: MemSpaceId,
    /// The space of all ROMs one behind the other (Spectrum); the stop
    /// points are armed there.
    rom_space: Option<MemSpaceId>,
    scanned: bool,
    rom: Option<Rom>,
}

/// Find `pattern` in `mem`; `None` in the pattern matches any byte.
fn find(mem: &[u8], pattern: &[Option<u8>]) -> Option<usize> {
    mem.windows(pattern.len()).position(|w| {
        w.iter()
            .zip(pattern)
            .all(|(b, p)| p.is_none_or(|p| p == *b))
    })
}

fn pattern(bytes: &[i16]) -> Vec<Option<u8>> {
    bytes.iter().map(|&b| u8::try_from(b).ok()).collect()
}

/// `count` words of a keyword table: characters up to one with bit 7.
fn words(mem: &[u8], mut at: usize, count: usize, mask: u8) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while out.len() < count {
        let mut word = Vec::new();
        loop {
            let Some(&b) = mem.get(at) else {
                return out;
            };
            at += 1;
            word.push(b & mask);
            if b & 0x80 != 0 || word.len() > 16 {
                break;
            }
        }
        out.push(word);
    }
    out
}

fn scan_zx81(mem: &[u8]) -> Option<Rom> {
    // LD (PPC),DE in the line loop.
    let store = find(mem, &pattern(&[0xed, 0x53, 0x07, 0x40]))?;
    // TOKEN-ADD: PUSH HL : LD HL,table : BIT 7,A : JR Z : AND $3F : CP $43.
    let table = find(
        mem,
        &pattern(&[
            0xe5, 0x21, -1, -1, 0xcb, 0x7f, 0x28, 0x02, 0xe6, 0x3f, 0xfe, 0x43,
        ]),
    )
    .map(|at| usize::from(u16::from_le_bytes([mem[at + 2], mem[at + 3]])));
    Some(Rom {
        stops: vec![store as u32 + 4],
        bases: Vec::new(),
        words: table.map_or_else(Vec::new, |t| words(mem, t, 0x44, 0x3f)),
        is128: false,
    })
}

fn scan_spectrum(mem: &[u8]) -> Option<Rom> {
    let mut rom = Rom {
        stops: Vec::new(),
        bases: Vec::new(),
        words: Vec::new(),
        is128: false,
    };
    // INC (IY+SUBPPC) : JP M : RST 18H : LD B,0 : CP 0DH, then JR Z or
    // JP Z to the line's end, CP ':' : JR Z : LD HL,STMT-RET.
    let head = pattern(&[0xfd, 0x34, 0x0d, 0xfa, -1, -1, 0xdf, 0x06, 0x00, 0xfe, 0x0d]);
    let tail = [0xfe, 0x3a, 0x28];
    for (n, bank) in mem.chunks(ROM_BANK).enumerate() {
        let mut from = 0;
        while let Some(at) = find(&bank[from..], &head).map(|p| p + from) {
            from = at + 1;
            let next = at + head.len();
            let skip = match bank.get(next) {
                Some(0x28) => 2,
                Some(0xca) => 3,
                _ => continue,
            };
            let t = next + skip;
            if bank.get(t..t + 3) == Some(&tail[..]) && bank.get(t + 4) == Some(&0x21) {
                rom.stops.push((n * ROM_BANK + t + 4) as u32);
            }
        }
        // LD (IY+NSPPC),1 : CALL LINE-RUN, and the report's code behind.
        if let Some(at) = find(bank, &pattern(&[0xfd, 0x36, 0x0a, 0x01, 0xcd, -1, -1])) {
            rom.bases.push(at as u16 + 7);
        }
        // 128 BASIC: … LD HL,$3E00 : PUSH HL : LD HL,handler : PUSH HL.
        let run128 = pattern(&[
            0xfd, 0x36, 0x0a, 0x01, 0x21, 0x00, 0x3e, 0xe5, 0x21, -1, -1, 0xe5,
        ]);
        if let Some(at) = find(bank, &run128) {
            rom.bases
                .push(u16::from_le_bytes([bank[at + 9], bank[at + 10]]));
            rom.is128 = true;
        }
        if rom.words.is_empty() {
            if let Some(at) = find(bank, &pattern(&[0x52, 0x4e, 0xc4, 0x49, 0x4e, 0x4b])) {
                rom.words = words(bank, at, 0x5b, 0x7f);
            }
        }
    }
    (!rom.stops.is_empty() && !rom.bases.is_empty()).then_some(rom)
}

/// The Spectrum's block graphics `$80`–`$8F`: bit 0 the upper right
/// quarter, bit 1 the upper left, bit 2 the lower right, bit 3 the lower
/// left.
const SPECTRUM_BLOCKS: [char; 16] = [
    '\u{a0}', '▝', '▘', '▀', '▗', '▐', '▚', '▜', '▖', '▞', '▌', '▛', '▄', '▟', '▙', '█',
];

fn spectrum_char(b: u8) -> Option<char> {
    Some(match b {
        0x5e => '↑',
        0x60 => '£',
        0x7f => '©',
        0x20..=0x7e => char::from(b),
        0x80..=0x8f => SPECTRUM_BLOCKS[usize::from(b & 15)],
        _ => return None,
    })
}

fn spectrum_code(c: char) -> Option<u8> {
    Some(match c {
        '↑' => 0x5e,
        '£' => 0x60,
        '©' => 0x7f,
        '`' => return None,
        ' '..='~' => c as u8,
        _ => 0x80 + SPECTRUM_BLOCKS.iter().position(|&b| b == c)? as u8,
    })
}

/// The ZX81's 64 characters; `\0` where Unicode has none.
const ZX81_CHARS: [char; 64] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▒', '\0', '\0', '"', '£', '$', ':', '?', '(', ')',
    '>', '<', '=', '+', '-', '*', '/', ';', ',', '.', '0', '1', '2', '3', '4', '5', '6', '7', '8',
    '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
    'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

fn zx81_char(b: u8) -> Option<char> {
    ZX81_CHARS
        .get(usize::from(b))
        .copied()
        .filter(|&c| c != '\0')
}

fn zx81_code(c: char) -> Option<u8> {
    ZX81_CHARS
        .iter()
        .position(|&x| x == c && c != '\0')
        .map(|p| p as u8)
}

fn push_code(out: &mut String, b: u8) {
    out.push_str(&format!("{{${b:02x}}}"));
}

/// A number as stored; `small`: exponent 0 holds a 16-bit integer.
fn number(b: [u8; 5], small: bool) -> f64 {
    if b[0] == 0 {
        if !small {
            return 0.0;
        }
        let n = f64::from(u16::from_le_bytes([b[2], b[3]]));
        return if b[1] == 0xff { n - 65_536.0 } else { n };
    }
    let mantissa = u32::from_be_bytes([b[1] | 0x80, b[2], b[3], b[4]]);
    let v = f64::from(mantissa) / 4_294_967_296.0 * 2f64.powi(i32::from(b[0]) - 128);
    if b[1] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn number_bytes(v: f64, small: bool) -> Result<[u8; 5], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if small && v.fract() == 0.0 && v.abs() <= 65_535.0 {
        let n = (v as i32 as u16).to_le_bytes();
        return Ok([0, if v < 0.0 { 0xff } else { 0 }, n[0], n[1], 0]);
    }
    if v == 0.0 {
        return Ok([0; 5]);
    }
    let mut exp = v.abs().log2().floor() as i32 + 1;
    let mut mantissa = (v.abs() / 2f64.powi(exp) * 4_294_967_296.0).round();
    // The logarithm may be off by one at powers of two; rounding may carry.
    if mantissa >= 4_294_967_296.0 {
        mantissa /= 2.0;
        exp += 1;
    } else if mantissa < 2_147_483_648.0 {
        mantissa *= 2.0;
        exp -= 1;
    }
    if exp < -127 {
        return Ok([0; 5]);
    }
    if exp > 127 {
        return Err(BasicError::OutOfRange);
    }
    let m = (mantissa as u32).to_be_bytes();
    let sign = if v < 0.0 { 0x80 } else { 0 };
    Ok([(exp + 128) as u8, m[0] & 0x7f | sign, m[1], m[2], m[3]])
}

#[derive(Clone, Copy)]
struct Line {
    addr: u16,
    number: u16,
    /// Length of the text with its end byte.
    len: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Num,
    For,
    Str,
    NumArray,
    StrArray,
}

struct Var {
    name: String,
    kind: Kind,
    addr: u16,
    /// Where the value, the text or the first element is.
    data: u16,
    dims: Vec<u32>,
    /// A string's length; an array's number of elements.
    len: u32,
}

impl Var {
    fn shown(&self) -> String {
        match self.kind {
            Kind::Str | Kind::StrArray => format!("{}$", self.name),
            _ => self.name.clone(),
        }
    }

    fn is_array(&self) -> bool {
        matches!(self.kind, Kind::NumArray | Kind::StrArray)
    }
}

struct View<'a> {
    host: &'a dyn DebugTarget,
    space: MemSpaceId,
    family: Family,
    sys: Sys,
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

    fn spectrum(&self) -> bool {
        self.family == Family::Spectrum
    }

    fn lines(&self) -> Vec<Line> {
        let (start, end) = match self.family {
            Family::Zx81 => (ZX81_PROG, self.w(ZX81_D_FILE)),
            Family::Spectrum => (self.w(SPECTRUM_PROG), self.w(self.sys.vars)),
        };
        let end = u32::from(end);
        let mut out = Vec::new();
        let mut at = u32::from(start);
        while at + 5 <= end && out.len() < MAX_LINES {
            let a = at as u16;
            let high = self.b(a);
            let len = self.w(a.wrapping_add(2));
            if high >= 0x40 || len == 0 || at + 4 + u32::from(len) > end {
                break;
            }
            out.push(Line {
                addr: a,
                number: u16::from(high) << 8 | u16::from(self.b(a.wrapping_add(1))),
                len,
            });
            at += 4 + u32::from(len);
        }
        out
    }

    fn char(&self, b: u8) -> Option<char> {
        match self.family {
            Family::Zx81 => zx81_char(b),
            Family::Spectrum => spectrum_char(b),
        }
    }

    fn code(&self, c: char) -> Option<u8> {
        match self.family {
            Family::Zx81 => zx81_code(c),
            Family::Spectrum => spectrum_code(c),
        }
    }

    /// A line's text as LIST writes it and where its statements begin.
    fn text(&self, line: &Line) -> (String, Vec<u32>) {
        let bytes: Vec<u8> = (0..line.len - 1)
            .map(|i| self.b(line.addr.wrapping_add(4 + i)))
            .collect();
        match self.family {
            Family::Zx81 => (self.text_zx81(&bytes), vec![0]),
            Family::Spectrum => self.text_spectrum(&bytes),
        }
    }

    fn word(&self, out: &mut String, word: &[u8]) {
        for &c in word {
            match self.char(c) {
                Some(c) => out.push(c),
                None => push_code(out, c),
            }
        }
    }

    /// The ZX81's LIST (`$0769`): FLAGS bit 0 holds back a keyword's
    /// leading blank; it is set at the line's start and by a keyword's
    /// trailing blank and cleared by every character but a blank.
    fn text_zx81(&self, bytes: &[u8]) -> String {
        let mut out = String::new();
        let mut held = true;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            if b == 0x7e {
                i += 5;
                continue;
            }
            if b & 0x40 == 0 {
                match self.char(b) {
                    Some(c) => out.push(c),
                    None => push_code(&mut out, b),
                }
                held &= b == 0;
                continue;
            }
            // A keyword: `$C0`–`$FF`, and RND, INKEY$ and PI at `$40`.
            let command = b & 0x80 != 0;
            let index = usize::from(if command { b & 0x3f } else { b });
            let entry = if index < 0x43 { index + 1 } else { 0 };
            let Some(word) = self.rom.words.get(entry) else {
                push_code(&mut out, b);
                held = false;
                continue;
            };
            let letter = |c: Option<&u8>| c.is_some_and(|&c| c >= 0x1c);
            if command && index >= 0x18 && letter(word.first()) && !held {
                out.push(' ');
            }
            self.word(&mut out, word);
            held = false;
            if command && (letter(word.last()) || word.last() == Some(&0x0d)) {
                out.push(' ');
                held = true;
            }
        }
        out
    }

    /// The Spectrum's LIST (PO-TOKENS `$0C10`): a keyword from OR on
    /// that begins with a letter gets a blank before it unless one was
    /// printed last; one that ends with a letter or `$` gets one behind
    /// it (not RND, INKEY$ and PI).
    fn text_spectrum(&self, bytes: &[u8]) -> (String, Vec<u32>) {
        let mut out = String::new();
        let mut statements = vec![0];
        let mut spaced = true;
        let mut quote = false;
        // Nothing of the statement seen yet; a REM takes the line's rest.
        let mut first = true;
        let mut rem = false;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            if b == 0x0e {
                i += 5;
                continue;
            }
            let word: Option<(Vec<u8>, bool)> = match b {
                0xa3 if self.rom.is128 => Some((b"SPECTRUM".to_vec(), true)),
                0xa4 if self.rom.is128 => Some((b"PLAY".to_vec(), true)),
                0xa5..=0xff => self
                    .rom
                    .words
                    .get(usize::from(b - 0xa5))
                    .map(|w| (w.clone(), b >= 0xc5)),
                _ => None,
            };
            match word {
                Some((word, late)) => {
                    let letter = |c: Option<&u8>| c.is_some_and(|&c| c >= b'A');
                    if late && letter(word.first()) && !spaced {
                        out.push(' ');
                    }
                    self.word(&mut out, &word);
                    spaced = false;
                    let plain = (0xa5..0xa8).contains(&b);
                    if !plain && (letter(word.last()) || word.last() == Some(&b'$')) {
                        out.push(' ');
                        spaced = true;
                    }
                }
                None => {
                    match self.char(b) {
                        Some(c) => out.push(c),
                        None => push_code(&mut out, b),
                    }
                    spaced = b == b' ';
                    // A colour or position code takes its operands along.
                    let operands = match b {
                        0x10..=0x15 => 1,
                        0x16 | 0x17 => 2,
                        _ => 0,
                    };
                    for _ in 0..operands {
                        if let Some(&o) = bytes.get(i) {
                            push_code(&mut out, o);
                            i += 1;
                        }
                    }
                }
            }
            if rem {
                continue;
            }
            match b {
                b'"' => quote = !quote,
                b':' | T_THEN if !quote => {
                    statements.push(out.len() as u32);
                    first = true;
                    continue;
                }
                T_REM if first => rem = true,
                _ => {}
            }
            first &= b == b' ';
        }
        (out, statements)
    }

    fn pos(&self, lines: &[Line], number: u16, statement: u16) -> Option<BasicPos> {
        lines.iter().find(|l| l.number == number).map(|l| BasicPos {
            line: u32::from(l.number),
            statement,
        })
    }

    /// Where the word is that ERR_SP pointed to when the line began to
    /// run, and how many handlers of the editor lie on top of it.
    fn base(&self) -> Option<(u16, u32)> {
        let ramtop = self.w(self.sys.ramtop);
        let mut e = self.w(self.sys.err_sp);
        if self.family == Family::Zx81 {
            return (e >= 0x4000 && e < ramtop).then_some((e, 0));
        }
        for hops in 0..4 {
            if e < 0x4000 || e >= ramtop {
                return None;
            }
            if self.rom.bases.contains(&self.w(e)) {
                return Some((e, hops));
            }
            let up = self.w(e.wrapping_add(2));
            if up <= e {
                return None;
            }
            e = up;
        }
        None
    }

    /// The GOSUB frames, innermost first: line and the statement (counted
    /// from 1) to return to.
    fn gosubs(&self) -> Vec<(u16, u8)> {
        let Some((base, _)) = self.base() else {
            return Vec::new();
        };
        let ramtop = u32::from(self.w(self.sys.ramtop));
        let size = if self.spectrum() { 3 } else { 2 };
        let mut out = Vec::new();
        let mut p = u32::from(base) + 2;
        loop {
            if p + 1 > ramtop || out.len() > MAX_FRAMES {
                return Vec::new();
            }
            let a = p as u16;
            if self.b(a.wrapping_add(1)) == END_MARKER {
                return out;
            }
            out.push(match self.family {
                Family::Spectrum => (self.w(a), self.b(a.wrapping_add(2))),
                Family::Zx81 => (self.w(a).wrapping_sub(1), 2),
            });
            p += size;
        }
    }

    fn letter(&self, first: u8) -> char {
        char::from(b'A' + ((first & 0x1f).wrapping_sub(if self.spectrum() { 1 } else { 6 })) % 26)
    }

    fn vars(&self) -> Vec<Var> {
        let end = u32::from(self.w(self.sys.e_line));
        let for_len = if self.spectrum() { 19 } else { 18 };
        let mut out = Vec::new();
        let mut at = u32::from(self.w(self.sys.vars));
        while at < end && out.len() < MAX_VARS {
            let a = at as u16;
            let first = self.b(a);
            if first == 0x80 {
                break;
            }
            let mut name = self.letter(first).to_string();
            let total = u32::from(self.w(a.wrapping_add(1)));
            let (kind, data, dims, len, size) = match first >> 5 {
                3 => (Kind::Num, a.wrapping_add(1), Vec::new(), 0, 6),
                7 => (Kind::For, a.wrapping_add(1), Vec::new(), 0, for_len),
                5 => {
                    let mut p = a.wrapping_add(1);
                    loop {
                        let c = self.b(p);
                        p = p.wrapping_add(1);
                        match self.char(c & 0x7f).filter(char::is_ascii_alphanumeric) {
                            Some(ch) => name.push(ch.to_ascii_uppercase()),
                            None => return out,
                        }
                        if c & 0x80 != 0 {
                            break;
                        }
                        if name.len() > 255 {
                            return out;
                        }
                    }
                    (
                        Kind::Num,
                        p,
                        Vec::new(),
                        0,
                        u32::from(p.wrapping_sub(a)) + 5,
                    )
                }
                2 => (Kind::Str, a.wrapping_add(3), Vec::new(), total, 3 + total),
                kind @ (4 | 6) => {
                    let count = u16::from(self.b(a.wrapping_add(3)));
                    let dims: Vec<u32> = (0..count)
                        .map(|i| u32::from(self.w(a.wrapping_add(4 + 2 * i))))
                        .collect();
                    let len = dims.iter().fold(1u32, |n, d| n.saturating_mul(*d));
                    let kind = if kind == 4 {
                        Kind::NumArray
                    } else {
                        Kind::StrArray
                    };
                    (kind, a.wrapping_add(4 + 2 * count), dims, len, 3 + total)
                }
                _ => break,
            };
            out.push(Var {
                name,
                kind,
                addr: a,
                data,
                dims,
                len,
            });
            at += size;
        }
        out
    }

    fn num(&self, at: u16) -> f64 {
        number(
            std::array::from_fn(|i| self.b(at.wrapping_add(i as u16))),
            self.spectrum(),
        )
    }

    fn string(&self, at: u16, len: u32) -> String {
        let mut s = String::new();
        for i in 0..len.min(0xffff) as u16 {
            let b = self.b(at.wrapping_add(i));
            match self.char(b) {
                // The Spectrum's empty block is told from a blank.
                Some('\u{a0}') | None => push_code(&mut s, b),
                Some(c) => s.push(c),
            }
        }
        s
    }

    fn encode(&self, s: &str) -> Result<Vec<u8>, BasicError> {
        s.chars()
            .map(|c| self.code(c))
            .collect::<Option<Vec<u8>>>()
            .ok_or(BasicError::OutOfRange)
    }

    fn state(&self) -> BasicState {
        let lines = self.lines();
        let flags = self.b(self.sys.flags);
        let ppc = self.w(self.sys.ppc);
        let ch_add = self.w(self.sys.ch_add);
        let base = self.base();
        let pos = match self.family {
            Family::Spectrum => {
                let inside = match base {
                    Some((_, 0)) => true,
                    // INPUT waits in the editor.
                    Some(_) => self.b(self.sys.flagx) & 0x20 != 0,
                    None => false,
                };
                let statement = u16::from(self.b(SPECTRUM_SUBPPC)).saturating_sub(1);
                (flags & 0x80 != 0 && inside)
                    .then(|| self.pos(&lines, ppc, statement))
                    .flatten()
            }
            Family::Zx81 => {
                let line = lines.iter().find(|l| l.number == ppc);
                // The line loop marks X_PTR after every statement; the
                // editor clears it.
                let inside = self.b(ZX81_X_PTR + 1) == 0xc0;
                line.filter(|_| flags & 0x80 != 0 && inside)
                    .map(|l| BasicPos {
                        line: u32::from(l.number),
                        statement: 0,
                    })
            }
        };
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos,
            text_addr: Some(u32::from(ch_add)),
            depth: self.gosubs().len() as u32,
        }
    }

    fn frames(&self) -> Vec<BasicFrame> {
        let lines = self.lines();
        // The statement before the one numbered `next` (from 1).
        let before = |line: u16, next: u8| BasicPos {
            line: self
                .pos(&lines, line, 0)
                .map_or(u32::from(line), |p| p.line),
            statement: u16::from(next).saturating_sub(2),
        };
        let mut out: Vec<BasicFrame> = self
            .gosubs()
            .into_iter()
            .map(|(line, next)| BasicFrame::Gosub {
                from: before(line, next),
            })
            .collect();
        let mut loops: Vec<(BasicPos, BasicFrame)> = self
            .vars()
            .iter()
            .filter(|v| v.kind == Kind::For)
            .filter_map(|v| {
                let value = self.num(v.data);
                let limit = self.num(v.data.wrapping_add(5));
                let step = self.num(v.data.wrapping_add(10));
                let open = if step >= 0.0 {
                    value <= limit
                } else {
                    value >= limit
                };
                let from = match self.family {
                    Family::Spectrum => before(
                        self.w(v.data.wrapping_add(15)),
                        self.b(v.data.wrapping_add(17)),
                    ),
                    Family::Zx81 => before(self.w(v.data.wrapping_add(15)).wrapping_sub(1), 2),
                };
                open.then(|| {
                    let frame = BasicFrame::For {
                        var: v.name.clone(),
                        limit: BasicValue::Float(limit),
                        step: BasicValue::Float(step),
                        from,
                    };
                    (from, frame)
                })
            })
            .collect();
        loops.sort_by_key(|l| std::cmp::Reverse(l.0));
        out.extend(loops.into_iter().map(|(_, f)| f));
        out
    }
}

/// A name as the debugger's user writes it: without blanks, in upper
/// case, and whether it has the string's sign.
fn wanted(name: &str) -> (String, bool) {
    let plain: String = name
        .chars()
        .filter(|c| *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match plain.strip_suffix('$') {
        Some(n) => (n.to_string(), true),
        None => (plain, false),
    }
}

impl Sinclair {
    /// The ZX81; `space` is the CPU's view with the ROM from 0.
    pub fn zx81(space: MemSpaceId) -> Self {
        Self {
            family: Family::Zx81,
            space,
            rom_space: None,
            scanned: false,
            rom: None,
        }
    }

    /// The Spectrum; `rom_space` holds its ROMs of 16 KB one behind the
    /// other, and execution breakpoints in it hit only while that ROM is
    /// the one paged in.
    pub fn spectrum(space: MemSpaceId, rom_space: MemSpaceId) -> Self {
        Self {
            family: Family::Spectrum,
            space,
            rom_space: Some(rom_space),
            scanned: false,
            rom: None,
        }
    }

    /// Look at the machine's ROMs, once: `Some` with the decoder to keep.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.scanned {
            return None;
        }
        let (space, len) = match self.rom_space {
            Some(space) => (space, 4 * ROM_BANK),
            None => (self.space, 0x2000),
        };
        let mem: Vec<u8> = (0..len as u32)
            .map_while(|a| host.peek(space, a, Size::Byte))
            .map(|v| v as u8)
            .collect();
        let rom = match self.family {
            Family::Zx81 => scan_zx81(&mem),
            Family::Spectrum => scan_spectrum(&mem),
        };
        Some(Self {
            scanned: true,
            rom,
            ..self.clone()
        })
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        self.rom.as_ref().map(|rom| View {
            host,
            space: self.space,
            family: self.family,
            sys: match self.family {
                Family::Zx81 => ZX81,
                Family::Spectrum => SPECTRUM,
            },
            rom,
        })
    }
}

impl Dialect for Sinclair {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        let dialect = match (self.family, &self.rom) {
            (Family::Zx81, _) => "Sinclair ZX81 BASIC",
            (Family::Spectrum, Some(Rom { is128: true, .. })) => "ZX Spectrum 128 BASIC",
            (Family::Spectrum, _) => "ZX Spectrum BASIC",
        };
        BasicInfo {
            dialect: dialect.into(),
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
                let (text, statements) = v.text(l);
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.addr),
                    raw: (0..4 + l.len)
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
        v.vars()
            .iter()
            .map(|var| BasicVar {
                name: var.shown(),
                ty: match var.kind {
                    Kind::Str | Kind::StrArray => BasicType::Str,
                    _ => BasicType::Float,
                },
                addr: u32::from(var.addr),
                value: match var.kind {
                    Kind::Num | Kind::For => Some(BasicValue::Float(v.num(var.data))),
                    Kind::Str => Some(BasicValue::Str(v.string(var.data, var.len))),
                    _ => None,
                },
                dims: var.dims.clone(),
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
        let (name, string) = wanted(name);
        let vars = v.vars();
        let var = vars
            .iter()
            .find(|x| x.is_array() && x.name == name && (x.kind == Kind::StrArray) == string)
            .ok_or(BasicError::NoSuchVariable)?;
        if first > var.len {
            return Err(BasicError::OutOfRange);
        }
        Ok((first..var.len.min(first.saturating_add(count)))
            .map(|i| match var.kind {
                Kind::StrArray => BasicValue::Str(v.string(var.data.wrapping_add(i as u16), 1)),
                _ => BasicValue::Float(v.num(var.data.wrapping_add((5 * i) as u16))),
            })
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
        let (name, string) = wanted(name);
        let vars = v.vars();
        let var = vars
            .iter()
            .find(|x| {
                x.name == name
                    && x.is_array() == index.is_some()
                    && matches!(x.kind, Kind::Str | Kind::StrArray) == string
            })
            .ok_or(BasicError::NoSuchVariable)?;
        let index = index.unwrap_or(0);
        if var.is_array() && index >= var.len {
            return Err(BasicError::OutOfRange);
        }
        let (at, bytes): (u32, Vec<u8>) = match (var.kind, value) {
            (Kind::Num | Kind::For | Kind::NumArray, BasicValue::Int(_) | BasicValue::Float(_)) => {
                let n = match value {
                    BasicValue::Int(n) => *n as f64,
                    BasicValue::Float(f) => *f,
                    BasicValue::Str(_) => unreachable!(),
                };
                (5 * index, number_bytes(n, v.spectrum())?.to_vec())
            }
            (Kind::Str | Kind::StrArray, BasicValue::Str(s)) => {
                let bytes = v.encode(s)?;
                // Only in place: the same length (one character in an array).
                let len = if var.is_array() { 1 } else { var.len as usize };
                if bytes.len() != len {
                    return Err(BasicError::Unsupported);
                }
                (index, bytes)
            }
            _ => return Err(BasicError::TypeMismatch),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (u32::from(var.data) + at + i as u32, b))
            .collect())
    }

    fn stop_points(&self, _host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        let space = self.rom_space.unwrap_or(self.space);
        self.rom
            .iter()
            .flat_map(|r| &r.stops)
            .map(|&addr| BasicStopPoint {
                kind: BreakpointKind::Exec { space, addr },
                granularity: BasicGranularity::Statement,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
