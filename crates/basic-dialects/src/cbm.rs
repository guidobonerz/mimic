//! Commodore BASIC (Microsoft BASIC for the 6502): PET BASIC 1.0, 2.0 and
//! 4.0, the VIC-20's and the C64's BASIC V2, the C16's and Plus/4's BASIC
//! 3.5 and the C128's BASIC 7.0.
//!
//! Nothing here is tied to a ROM revision. The interpreter is found in the
//! CPU's view of `$8000`–`$FFFF` by two patterns:
//!
//! - the statement loop `JSR CHRGET : JSR GONE3 : JMP NEWSTT`
//!   (`20 lo hi 20 lo hi 4C lo hi`, the second target right behind the
//!   sequence, the third a few bytes before it; BASIC 4.0 calls a routine
//!   of its own in place of CHRGET). NEWSTT reads the text with
//!   `LDY #0 : LDA (TXTPTR),Y`; TXTPTR is CHRGET + 7, and CHRGET's
//!   address gives every other variable, since
//!   the interpreter's zero page has the same layout in all versions and
//!   only moves as a whole: TXTTAB = CHRGET − `$48`, VARTAB, ARYTAB,
//!   STREND and FRETOP 2 bytes apart behind it, CURLIN = TXTTAB + `$0E`
//!   (C64: `$2B`, `$39`, `$7A`). Where the loop is
//!   entered through `JMP ($0308)` (IGONE; VIC-20, C64) the stop point is
//!   that jump, so programs that hook the vector are followed too.
//! - the keyword table (`EN`,`D`+`$80`,`FO`,`R`+`$80` …), read from the
//!   ROM, so the listing shows the tokens the interpreter in the machine
//!   really has.
//!
//! At the stop point CURLIN holds the line (`$FFxx`: direct mode) and
//! TXTPTR points to the byte before the statement: the `:` or, at a line's
//! first statement, the line number's high byte. A program counts as
//! running while CURLIN is a line and TXTPTR is inside a stored line
//! (after a cold start CURLIN is not `$FFxx` yet; while INPUT reads the
//! input buffer the mode shows as direct for that moment). The line the
//! text pointer is in is the place reported: halted inside RETURN, NEXT
//! or LOOP, CURLIN may still name the line left.
//!
//! Program lines: link (2), number (2), text, 0. Variables (7 bytes from
//! VARTAB): two name bytes whose bits 7 give the type (none: float, both:
//! integer, second: string, first: function), then the 5-byte float
//! (exponent + `$80`, mantissa with the sign in bit 7 of its first byte),
//! the integer (high, low) or the string (length, pointer). Arrays (from
//! ARYTAB): name, size, number of dimensions, the sizes (high, low; last
//! dimension first), the elements with the first index running fastest.
//!
//! The GOSUB and FOR frames are on the CPU stack, ending below `$01FB`:
//! GOSUB `$8D`, CURLIN, TXTPTR (5 bytes; BASIC 2 and later leave the
//! statement loop's return address above it, seen on a C64); FOR `$81`, pointer to the
//! variable, STEP (5-byte mantissa form and a sign byte), limit (5-byte
//! float), CURLIN, TXTPTR (18 bytes). While the machine is halted inside a
//! statement, return addresses lie above them; the frames are found as the
//! first chain from the stack pointer that ends at the stack's top.
//!
//! # BASIC 3.5 and 7.0
//!
//! These keep the statement loop behind `JMP ($0308)` but read the text
//! through routines in RAM (the program may lie behind the ROM), so the
//! first pattern does not fit. Their loop is found by what follows the
//! jump within `$C0` bytes, the step to the next line:
//! `STA CURLIN : INY : JSR … : STA CURLIN+1 : TYA : CLC : ADC TXTPTR :
//! STA TXTPTR` (`85 cl C8 20 lo hi 85 cl+1 98 18 65 tp 85 tp`). TXTTAB is
//! CURLIN − `$0E` as before (Plus/4: `$2B`, `$39`, `$3B`; C128: `$2D`,
//! `$3B`, `$3D`).
//!
//! GOSUB, FOR and DO frames are not on the CPU stack but on a stack of the
//! interpreter's own that grows down from a fixed top; a zero-page pointer
//! names its lowest frame. The pointer is the operand of GOSUB's
//! `LDA #$8D : STA (ptr),Y`, the top the constant CLR loads into it
//! (`LDA #lo : LDY #hi : STA ptr : STY ptr+1`; Plus/4: `$7C`, `$07B0`;
//! C128: `$7D`, `$09FF`). GOSUB `$8D` and DO `$EB`: token, CURLIN, TXTPTR
//! (5 bytes); FOR as above (18 bytes), but its TXTPTR is stored high byte
//! first.
//!
//! BASIC 7.0 has two more keyword tables right behind the first, for the
//! tokens `$FE nn` (BANK …) and `$CE nn` (POT …), both counted from 2; the
//! program is in RAM bank 0 and the variables, arrays and strings are in
//! bank 1. A machine whose ROMs hide RAM gives the decoder a RAM space
//! ([`CbmBasic::with_ram`]); the interpreter is then still followed while
//! one of its RAM routines has the ROM switched off for a moment.
//!
//! # Simons' BASIC and TSB
//!
//! Extensions that hook the interpreter's vectors run on top of BASIC V2:
//! the statement loop, the variables and the GOSUB/FOR frames stay the
//! ROM's, and since the stop point is the jump through IGONE, their
//! statements pass it too. What they add is looked for whenever IGONE
//! (`$0308`) does not point back into the ROM:
//!
//! - their keyword table, `HIRES`, `PLOT` … in `$8000`–`$BFFF`: the words
//!   end with `@` (Simons' BASIC) or with bit 7 set (TSB). The extension
//!   is looked for in the cartridge's ROM, where the machine has a space
//!   for it (the Simons' BASIC cartridge keeps its table in the half it
//!   switches off while the BASIC ROM runs), and in memory: the CPU's
//!   view below `$A000` and the RAM behind the BASIC ROM (the TSB
//!   cartridge copies itself there). Of the two the one counts whose
//!   stacks are found. Their tokens are `$64 nn`, `nn` from 1 in table order
//!   (TSB also `nn` ≥ `$80` for `nn` eor `$8F`). The second byte is never
//!   text: LOOP is `$64 $3A` and TSB's DO `$64 $22`.
//! - their statement dispatch `LDA table+1,Y : PHA : LDA table,Y : PHA :
//!   RTS` gives the handlers of REPEAT, LOOP and EXEC. Each keeps a stack
//!   of text pointers of its own: the handler begins with `LDY count` and
//!   stores with `LDA TXTPTR : STA base,Y`; an `INY` before that means the
//!   entries begin one byte later (Simons' BASIC; both: count `$C617`,
//!   `$C62C`, `$C641`). The count is in bytes, two an entry.
//!
//! EXEC (a procedure call) is reported as a GOSUB and counts for the
//! depth, REPEAT and LOOP as loops. These stacks are separate from the
//! CPU stack, so the order between them and GOSUB/FOR frames is not
//! known: the ROM's frames come first, then EXEC, REPEAT and LOOP, each
//! innermost first.
//!
//! # Applesoft
//!
//! Applesoft BASIC (Apple II+, IIe) is the same Microsoft BASIC and is
//! decoded here too ([`CbmBasic::new`] with `scan_from($D000)`); it is
//! told by its keyword `HIMEM:`. What differs:
//!
//! - the statement loop has a branch between its second `JSR` and the
//!   routine it calls (`JSR CHRGET : JSR EXECUTE : JMP NEWSTT : BEQ … :
//!   EXECUTE`), and TXTTAB is `$51` below TXTPTR, not `$4F` (`$67`, `$B8`;
//!   CURLIN `$75` is TXTTAB + `$0E` as everywhere);
//! - the tokens are others (GOSUB `$B0`, REM `$B2`): both are taken from
//!   their place in the keyword table, for every version;
//! - the text is ASCII, and the listing is LIST's: every byte from `$80`
//!   is a keyword, also inside quotes, and is shown with a blank before
//!   and behind it (the tokeniser drops the blanks typed);
//! - CLEAR sets the stack pointer to `$F8`, so the frames end below
//!   `$01F9`.
//!
//! Variables, arrays, floats and the GOSUB and FOR frames are those of
//! BASIC V2, including the statement loop's return address above a GOSUB
//! frame.
//!
//! Sources: Sheldon Leemon, "Mapping the Commodore 64" (zero page, stack
//! frames, variable formats); the PET memory maps in Raeto West,
//! "Programming the PET/CBM"; for BASIC 3.5 and 7.0 the layout was read
//! from the machines' own ROMs (statement loop, GOSUB, FOR, DO and CLR
//! routines), for Simons' BASIC and TSB from the user's disk images of them
//! (vector setup, statement handler, dispatch table, REPEAT/LOOP/EXEC),
//! for Applesoft from the user's Apple II+ ROM (NEWSTT `$D7D2`, LIST
//! `$D6DA`, GOSUB `$D921`, the stack reset `$D683`, the keyword table
//! `$D0D0`). No emulator source was used.

use emu_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use emu_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const SCAN_BASE: u16 = 0x8000;
const MAX_LINES: usize = 20_000;
const MAX_VARS: usize = 8_192;
const TOKEN_REM: u8 = 0x8f;
const TOKEN_FOR: u8 = 0x81;
const TOKEN_GOSUB: u8 = 0x8d;
const TOKEN_DO: u8 = 0xeb;
/// The prefix of Simons' BASIC's and TSB's tokens.
const TOKEN_EXT: u8 = 0x64;
/// Where an extension is looked for: `$8000`–`$BFFF`.
const EXT_BASE: u16 = 0x8000;
const EXT_LEN: usize = 0x4000;
/// Entries read from one of an extension's stacks at most.
const EXT_ENTRIES: u16 = 16;
/// The prefixes of BASIC 7.0's two-byte tokens.
const TOKEN_CE: u8 = 0xce;
const TOKEN_FE: u8 = 0xfe;
const MAX_FRAMES: usize = 128;
const FRAME_GOSUB: u16 = 5;
const FRAME_FOR: u16 = 18;

/// What the patterns found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The stop point: `JMP (IGONE)` or the `JSR CHRGET` of the loop.
    dispatch: u16,
    /// The three bytes there, to see whether the ROM is still mapped.
    signature: [u8; 3],
    /// The zero-page pointers: TXTTAB (VARTAB … behind it) and TXTPTR.
    txttab: u16,
    txtptr: u16,
    stack: Stack,
    keywords_at: u16,
    keywords: Vec<String>,
    /// The tokens of REM and GOSUB, by their place in the table.
    rem: u8,
    gosub: u8,
    /// Applesoft: ASCII text, LIST's blanks around keywords, the stack
    /// reset to `$F8`.
    apple: bool,
    /// BASIC 7.0: the words of the tokens `$FE nn` and `$CE nn`, from
    /// `nn` = 2.
    fe: Vec<String>,
    ce: Vec<String>,
}

/// An extension on top of the ROM's BASIC: Simons' BASIC or TSB.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ext {
    name: &'static str,
    /// Found in the cartridge's ROM, not in memory.
    in_cart: bool,
    /// Its keyword table, to see whether it is still there.
    table_at: u16,
    /// The words of the tokens `$64 nn`, from `nn` = 1.
    words: Vec<String>,
    stacks: Vec<ExtStack>,
}

/// One of an extension's stacks of text pointers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ExtStack {
    /// The keyword that pushes: EXEC is a call, the others open loops.
    word: &'static str,
    /// Address of the bytes in use (two an entry).
    count: u16,
    /// Address of the oldest entry.
    base: u16,
}

/// Where the GOSUB and FOR frames are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stack {
    /// On the CPU stack. `gone_return`: what `JSR GONE3` in the statement
    /// loop pushes; GOSUB leaves it above its frame (BASIC 2 and later).
    Cpu { gone_return: u16 },
    /// On the interpreter's own stack (BASIC 3.5 and 7.0): the zero-page
    /// pointer to its lowest frame and the address it ends at.
    Own { tos: u16, top: u16 },
}

/// The decoder a Commodore machine keeps; see the module text.
#[derive(Clone, Debug)]
pub struct CbmBasic {
    /// The main CPU's view: the ROM and the stop point.
    space: MemSpaceId,
    /// The RAM behind the ROMs, where the machine has such a space.
    ram: Option<MemSpaceId>,
    /// Where BASIC 7.0's variable bank begins in `ram`.
    var_bank: u32,
    scan_base: u16,
    /// The cartridge's ROM, where the machine has such a space.
    cart: Option<MemSpaceId>,
    rom: Option<Rom>,
    ext: Option<Ext>,
    /// IGONE when the extension was last looked for.
    ext_vector: u16,
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
    Float,
    Int,
    Str,
    Function,
}

impl VarKind {
    fn of(b0: u8, b1: u8) -> Self {
        match (b0 & 0x80 != 0, b1 & 0x80 != 0) {
            (false, false) => VarKind::Float,
            (true, true) => VarKind::Int,
            (false, true) => VarKind::Str,
            (true, false) => VarKind::Function,
        }
    }

    fn ty(self) -> BasicType {
        match self {
            VarKind::Int => BasicType::Int,
            VarKind::Str => BasicType::Str,
            _ => BasicType::Float,
        }
    }

    fn element_bytes(self) -> u16 {
        match self {
            VarKind::Int => 2,
            VarKind::Str => 3,
            _ => 5,
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
    /// At most 65535 for the arrays [`View::arrays`] returns.
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
        var: u16,
        step: [u8; 5],
        sign: u8,
        limit: [u8; 5],
        line: u16,
        txt: u16,
    },
    Do {
        line: u16,
        txt: u16,
    },
    /// An entry of an extension's stack: only a text pointer.
    Ext {
        word: &'static str,
        txt: u16,
    },
}

/// PETSCII (upper case / graphics set) as `LIST` shows it; what has no
/// plain counterpart becomes `{$xx}`.
fn push_petscii(out: &mut String, b: u8) {
    match b {
        0x20..=0x5a => out.push(char::from(b)),
        0x5b => out.push('['),
        0x5c => out.push('£'),
        0x5d => out.push(']'),
        0x5e => out.push('↑'),
        0x5f => out.push('←'),
        _ => out.push_str(&format!("{{${b:02x}}}")),
    }
}

/// ASCII (Applesoft); what is not printable becomes `{$xx}`.
fn push_ascii(out: &mut String, b: u8) {
    match b {
        0x20..=0x7e => out.push(char::from(b)),
        _ => out.push_str(&format!("{{${b:02x}}}")),
    }
}

fn push_char(out: &mut String, b: u8, ascii: bool) {
    if ascii {
        push_ascii(out, b);
    } else {
        push_petscii(out, b);
    }
}

fn char_from(c: char, ascii: bool) -> Option<u8> {
    if ascii {
        (' '..='~').contains(&c).then_some(c as u8)
    } else {
        petscii_from(c)
    }
}

fn petscii_from(c: char) -> Option<u8> {
    Some(match c {
        ' '..='Z' => c as u8,
        '[' => 0x5b,
        '£' => 0x5c,
        ']' => 0x5d,
        '↑' => 0x5e,
        '←' => 0x5f,
        _ => return None,
    })
}

/// A 5-byte float. `packed`: the sign is bit 7 of the first mantissa
/// byte (variables); otherwise that bit is part of the mantissa.
fn float_value(b: [u8; 5], packed: bool) -> f64 {
    if b[0] == 0 {
        return 0.0;
    }
    let mantissa = u32::from_be_bytes([b[1] | 0x80, b[2], b[3], b[4]]);
    let v = f64::from(mantissa) / 4_294_967_296.0 * 2f64.powi(i32::from(b[0]) - 128);
    if packed && b[1] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn float_bytes(v: f64) -> Result<[u8; 5], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
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

fn var_name(b0: u8, b1: u8, kind: VarKind, ascii: bool) -> String {
    let mut name = String::new();
    for c in [b0 & 0x7f, b1 & 0x7f] {
        if c != 0 {
            push_char(&mut name, c, ascii);
        }
    }
    match kind {
        VarKind::Int => name.push('%'),
        VarKind::Str => name.push('$'),
        _ => {}
    }
    name
}

/// The interpreter as it is mapped now.
struct View<'a> {
    host: &'a dyn DebugTarget,
    /// The space the interpreter's data is read from.
    data: MemSpaceId,
    /// Added to the addresses of variables, arrays and strings.
    var_off: u32,
    rom: &'a Rom,
    ext: Option<&'a Ext>,
}

fn peek(host: &dyn DebugTarget, space: MemSpaceId, addr: u32) -> u8 {
    host.peek(space, addr, Size::Byte).map_or(0xff, |v| v as u8)
}

impl View<'_> {
    /// Zero page, program text and the stack.
    fn b(&self, addr: u16) -> u8 {
        peek(self.host, self.data, u32::from(addr))
    }

    /// Variables, arrays and strings.
    fn vb(&self, addr: u16) -> u8 {
        peek(self.host, self.data, self.var_off + u32::from(addr))
    }

    fn vw(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.vb(addr), self.vb(addr.wrapping_add(1))])
    }

    fn vbytes<const N: usize>(&self, addr: u16) -> [u8; N] {
        std::array::from_fn(|i| self.vb(addr.wrapping_add(i as u16)))
    }

    fn w(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.b(addr), self.b(addr.wrapping_add(1))])
    }

    fn bytes<const N: usize>(&self, addr: u16) -> [u8; N] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    /// A zero-page pointer by its distance from TXTTAB.
    fn pointer(&self, offset: u16) -> u16 {
        self.w(self.rom.txttab.wrapping_add(offset))
    }

    fn vartab(&self) -> u16 {
        self.pointer(2)
    }

    fn arytab(&self) -> u16 {
        self.pointer(4)
    }

    fn strend(&self) -> u16 {
        self.pointer(6)
    }

    fn fretop(&self) -> u16 {
        self.pointer(8)
    }

    fn curlin(&self) -> u16 {
        self.pointer(0x0e)
    }

    fn txtptr(&self) -> u16 {
        self.w(self.rom.txtptr)
    }

    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let mut at = self.pointer(0);
        while out.len() < MAX_LINES {
            let link = self.w(at);
            if link < 0x100 {
                break;
            }
            let text = at.wrapping_add(4);
            let mut end = text;
            while self.b(end) != 0 && end.wrapping_sub(text) < 255 {
                end = end.wrapping_add(1);
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
        let mut text = String::new();
        let mut statements = vec![0];
        let mut colons = Vec::new();
        let (mut quote, mut rem) = (false, false);
        let apple = self.rom.apple;
        let mut at = line.addr.wrapping_add(4);
        while at != line.end {
            let b = self.b(at);
            match b {
                // Applesoft's LIST knows no quotes: a blank, the word, a
                // blank.
                0x80..=0xff if apple => {
                    rem |= b == self.rom.rem && !quote;
                    match self.rom.keywords.get(usize::from(b - 0x80)) {
                        Some(k) => {
                            text.push(' ');
                            text.push_str(k);
                            text.push(' ');
                        }
                        None => push_ascii(&mut text, b),
                    }
                }
                b'"' => {
                    quote = !quote;
                    text.push('"');
                }
                0xff if !quote && !apple => text.push('π'),
                TOKEN_EXT if !quote && self.ext.is_some() && at.wrapping_add(1) != line.end => {
                    at = at.wrapping_add(1);
                    let n = self.b(at);
                    let index = if n < 0x80 { n } else { n ^ 0x8f };
                    let word = index
                        .checked_sub(1)
                        .and_then(|i| self.ext?.words.get(usize::from(i)));
                    match word {
                        Some(k) => text.push_str(k),
                        None => {
                            push_petscii(&mut text, b);
                            push_petscii(&mut text, n);
                        }
                    }
                }
                TOKEN_CE | TOKEN_FE if !quote && !self.rom.fe.is_empty() => {
                    let table = if b == TOKEN_FE {
                        &self.rom.fe
                    } else {
                        &self.rom.ce
                    };
                    let next = at.wrapping_add(1);
                    if next == line.end {
                        push_petscii(&mut text, b);
                    } else {
                        let n = self.b(next);
                        match n.checked_sub(2).and_then(|i| table.get(usize::from(i))) {
                            Some(k) => text.push_str(k),
                            None => {
                                push_petscii(&mut text, b);
                                push_petscii(&mut text, n);
                            }
                        }
                        at = next;
                    }
                }
                0x80..=0xfe if !quote => {
                    rem |= b == self.rom.rem;
                    match self.rom.keywords.get(usize::from(b - 0x80)) {
                        Some(k) => text.push_str(k),
                        None => push_petscii(&mut text, b),
                    }
                }
                b':' if !quote && !rem => {
                    text.push(':');
                    statements.push(text.len() as u32);
                    colons.push(at);
                }
                _ => push_char(&mut text, b, apple),
            }
            at = at.wrapping_add(1);
        }
        (text, statements, colons)
    }

    /// The statement of line `number` the text pointer `txt` is in.
    /// `before`: a pointer on a colon still belongs to the statement
    /// before it (saved pointers of FOR and GOSUB).
    fn pos(&self, lines: &[Line], number: u16, txt: u16, before: bool) -> BasicPos {
        let statement = lines
            .iter()
            .find(|l| l.number == number && (l.addr..=l.end).contains(&txt))
            .map_or(0, |l| {
                let (_, _, colons) = self.scan(l);
                colons
                    .iter()
                    .filter(|&&c| if before { c < txt } else { c <= txt })
                    .count()
            });
        BasicPos {
            line: u32::from(number),
            statement: statement as u16,
        }
    }

    /// The GOSUB, FOR and DO frames, innermost first.
    fn frames(&self) -> Vec<RawFrame> {
        let mut frames = match self.rom.stack {
            Stack::Cpu { gone_return } => self.cpu_frames(gone_return),
            Stack::Own { tos, top } => self.own_frames(tos, top),
        };
        for stack in self.ext.iter().flat_map(|e| &e.stacks) {
            let entries = (u16::from(self.b(stack.count)) / 2).min(EXT_ENTRIES);
            frames.extend((0..entries).rev().map(|i| RawFrame::Ext {
                word: stack.word,
                txt: self.w(stack.base.wrapping_add(2 * i)),
            }));
        }
        frames
    }

    /// The statement the text pointer `txt` is in or, on a colon or a
    /// line's end, behind.
    fn pos_of(&self, lines: &[Line], txt: u16) -> BasicPos {
        let number = lines
            .iter()
            .find(|l| (l.addr..=l.end).contains(&txt))
            .map_or(0, |l| l.number);
        self.pos(lines, number, txt, true)
    }

    /// The frames on the interpreter's own stack, from the pointer `tos`
    /// names up to `top`; nothing if that is no chain of frames.
    fn own_frames(&self, tos: u16, top: u16) -> Vec<RawFrame> {
        let mut out = Vec::new();
        let mut p = self.w(tos);
        while p != top {
            let token = self.b(p);
            let size = if token == TOKEN_FOR {
                FRAME_FOR
            } else {
                FRAME_GOSUB
            };
            if p > top || top - p < size || out.len() == MAX_FRAMES {
                return Vec::new();
            }
            let (line, txt) = (self.w(p + 1), self.w(p + 3));
            out.push(match token {
                TOKEN_FOR => RawFrame::For {
                    var: self.w(p + 1),
                    step: self.bytes(p + 3),
                    sign: self.b(p + 8),
                    limit: self.bytes(p + 9),
                    line: self.w(p + 14),
                    txt: u16::from_be_bytes([self.b(p + 16), self.b(p + 17)]),
                },
                TOKEN_GOSUB => RawFrame::Gosub { line, txt },
                TOKEN_DO => RawFrame::Do { line, txt },
                _ => return Vec::new(),
            });
            p += size;
        }
        out
    }

    /// The frames on the CPU stack.
    fn cpu_frames(&self, gone_return: u16) -> Vec<RawFrame> {
        let sp = self
            .host
            .regs()
            .get("SP")
            .or_else(|| self.host.regs().get("S"))
            .unwrap_or(0xff) as u16
            & 0xff;
        // Where the frames end: the stack pointer CLR sets, plus 1.
        let end = if self.rom.apple { 0x1f9 } else { 0x1fa };
        let chain = |start: u16| -> Option<Vec<RawFrame>> {
            let mut out = Vec::new();
            let mut p = start;
            loop {
                if (end..=0x200).contains(&p) {
                    return Some(out);
                }
                match self.b(p) {
                    t if t == self.rom.gosub && p + FRAME_GOSUB <= 0x200 => {
                        out.push(RawFrame::Gosub {
                            line: self.w(p + 1),
                            txt: self.w(p + 3),
                        });
                        p += FRAME_GOSUB;
                        if p + 2 <= 0x200 && self.w(p) == gone_return {
                            p += 2;
                        }
                    }
                    TOKEN_FOR if p + FRAME_FOR <= 0x200 => {
                        out.push(RawFrame::For {
                            var: self.w(p + 1),
                            step: self.bytes(p + 3),
                            sign: self.b(p + 8),
                            limit: self.bytes(p + 9),
                            line: self.w(p + 14),
                            txt: self.w(p + 16),
                        });
                        p += FRAME_FOR;
                    }
                    _ => return None,
                }
            }
        };
        let first = 0x101 + sp;
        (first..first + 64).find_map(chain).unwrap_or_default()
    }

    /// Plain variables: (entry address, name, kind).
    fn scalars(&self) -> Vec<(u16, String, VarKind)> {
        let (start, end) = (self.vartab(), self.arytab());
        let mut out = Vec::new();
        let mut at = start;
        while at < end && end - at >= 7 && out.len() < MAX_VARS {
            let (b0, b1) = (self.vb(at), self.vb(at + 1));
            let kind = VarKind::of(b0, b1);
            if kind != VarKind::Function {
                out.push((at, var_name(b0, b1, kind, self.rom.apple), kind));
            }
            at += 7;
        }
        out
    }

    fn arrays(&self) -> Vec<Array> {
        let (start, end) = (self.arytab(), self.strend());
        let mut out = Vec::new();
        let mut at = start;
        while at < end && end - at >= 7 && out.len() < MAX_VARS {
            let (b0, b1) = (self.vb(at), self.vb(at + 1));
            let kind = VarKind::of(b0, b1);
            let size = self.vw(at + 2);
            let n = u16::from(self.vb(at + 4));
            let header = 5 + 2 * n;
            if n == 0 || size < header || size > end - at {
                break;
            }
            let dims: Vec<u32> = (0..n)
                .map(|i| {
                    let d = at + 5 + 2 * i;
                    u32::from(u16::from_be_bytes([self.vb(d), self.vb(d + 1)]))
                })
                .collect();
            let array = Array {
                name: var_name(b0, b1, kind, self.rom.apple),
                kind,
                addr: at,
                data: at + header,
                dims,
            };
            // The elements have to fit the array's size.
            let fits = u64::from(array.elements()) * u64::from(kind.element_bytes())
                <= u64::from(size - header);
            if fits && kind != VarKind::Function {
                out.push(array);
            }
            at += size;
        }
        out
    }

    fn string(&self, at: u16) -> String {
        let (len, ptr) = (self.vb(at), self.vw(at.wrapping_add(1)));
        let mut s = String::new();
        for i in 0..u16::from(len) {
            push_char(&mut s, self.vb(ptr.wrapping_add(i)), self.rom.apple);
        }
        s
    }

    /// The value stored at `at` (a variable's bytes behind its name, or an
    /// array element).
    fn value(&self, at: u16, kind: VarKind) -> BasicValue {
        match kind {
            VarKind::Int => {
                let bytes = [self.vb(at), self.vb(at.wrapping_add(1))];
                BasicValue::Int(i64::from(i16::from_be_bytes(bytes)))
            }
            VarKind::Str => BasicValue::Str(self.string(at)),
            _ => BasicValue::Float(float_value(self.vbytes(at), true)),
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
            VarKind::Int => (at, int16(value)?.to_be_bytes().to_vec()),
            VarKind::Str => {
                let BasicValue::Str(s) = value else {
                    return Err(BasicError::TypeMismatch);
                };
                let bytes = s
                    .chars()
                    .map(|c| char_from(c, self.rom.apple))
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // Only in place: the same length, and not a literal in
                // the program text.
                let ptr = self.vw(at.wrapping_add(1));
                if bytes.len() != usize::from(self.vb(at)) || ptr < self.fretop() {
                    return Err(BasicError::Unsupported);
                }
                (ptr, bytes)
            }
            _ => (at, float_bytes(number(value)?)?.to_vec()),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| {
                let at = target.wrapping_add(i as u16);
                (self.var_off + u32::from(at), b)
            })
            .collect())
    }
}

impl CbmBasic {
    /// A decoder that reads everything through the CPU's view `space`.
    pub fn new(space: MemSpaceId) -> Self {
        Self {
            space,
            ram: None,
            var_bank: 0,
            scan_base: SCAN_BASE,
            cart: None,
            rom: None,
            ext: None,
            ext_vector: 0,
        }
    }

    /// Read the interpreter's data from `ram`, the RAM as it is behind
    /// the ROMs (address 0 = CPU address 0); a BASIC 7.0 has its
    /// variables at `var_bank` in it.
    pub fn with_ram(mut self, ram: MemSpaceId, var_bank: u32) -> Self {
        self.ram = Some(ram);
        self.var_bank = var_bank;
        self
    }

    /// Look for BASIC extensions in `cart` too: the cartridge's ROM at its
    /// CPU addresses, readable whatever is mapped (no answer without a
    /// cartridge).
    pub fn with_cartridge(mut self, cart: MemSpaceId) -> Self {
        self.cart = Some(cart);
        self
    }

    /// Search the CPU's view from `base` (default `$8000`).
    pub fn scan_from(mut self, base: u16) -> Self {
        self.scan_base = base;
        self
    }

    /// Look at the machine again: `Some` with the decoder to keep from now
    /// on if the interpreter appeared, went away or changed.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        let rom = self.rescan_rom(host);
        let rom_now = rom.as_ref().unwrap_or(&self.rom);
        let ext = self.rescan_ext(host, rom_now.as_ref());
        if rom.is_none() && ext.is_none() {
            return None;
        }
        let (ext, ext_vector) = ext.unwrap_or_else(|| (self.ext.clone(), self.ext_vector));
        Some(Self {
            rom: rom.unwrap_or_else(|| self.rom.clone()),
            ext,
            ext_vector,
            ..self.clone()
        })
    }

    /// `Some` with the interpreter there is now if that changed.
    fn rescan_rom(&self, host: &dyn DebugTarget) -> Option<Option<Rom>> {
        if self.rom.as_ref().is_some_and(|r| self.mapped(host, r)) {
            return None;
        }
        let rom = find(host, self.space, self.scan_base);
        // Nothing else there and the ROM only switched off: keep it.
        if rom.is_none() && self.view(host).is_some() {
            return None;
        }
        (rom != self.rom).then_some(rom)
    }

    /// `Some` with the extension there is now (and IGONE) if it has to
    /// be looked for again and that changed anything.
    fn rescan_ext(&self, host: &dyn DebugTarget, rom: Option<&Rom>) -> Option<(Option<Ext>, u16)> {
        // IGONE as the ROM sets it points behind its own jump.
        let hooked = rom.and_then(|r| {
            let [0x6c, lo, hi] = r.signature else {
                return None;
            };
            let at = u32::from(u16::from_le_bytes([lo, hi]));
            let data = self.ram.unwrap_or(self.space);
            let vector = u16::from_le_bytes([peek(host, data, at), peek(host, data, at + 1)]);
            (vector != r.dispatch.wrapping_add(3)).then_some((r, vector))
        });
        let Some((rom, vector)) = hooked else {
            return (self.ext.is_some() || self.ext_vector != 0).then_some((None, 0));
        };
        let known = match &self.ext {
            Some(e) => {
                let at = e.table_at;
                [0, 1, 2].map(|i| self.ext_byte(host, e.in_cart, at.wrapping_add(i))) == *b"HIR"
            }
            None => vector == self.ext_vector,
        };
        if known {
            return None;
        }
        let image = |in_cart: bool| -> Vec<u8> {
            (0..EXT_LEN)
                .map(|i| self.ext_byte(host, in_cart, EXT_BASE.wrapping_add(i as u16)))
                .collect()
        };
        // In memory (the TSB cartridge runs its copy in RAM, its ROM bank
        // holds no code that runs there) or in the cartridge's ROM: the
        // one whose stacks are found, memory first.
        let txtptr = rom.txtptr as u8;
        let in_memory = find_ext(&image(false), false, txtptr);
        let has_cart = self
            .cart
            .is_some_and(|c| host.peek(c, u32::from(EXT_BASE), Size::Byte).is_some());
        let in_cart = has_cart
            .then(|| find_ext(&image(true), true, txtptr))
            .flatten();
        let stacks = |e: &Option<Ext>| e.as_ref().map_or(0, |e| e.stacks.len() + 1);
        let ext = if stacks(&in_cart) > stacks(&in_memory) {
            in_cart
        } else {
            in_memory
        };
        (ext != self.ext || vector != self.ext_vector).then_some((ext, vector))
    }

    /// A byte of an extension: of the cartridge's ROM, or of memory (the
    /// CPU's view below `$A000`, behind the BASIC ROM the RAM).
    fn ext_byte(&self, host: &dyn DebugTarget, in_cart: bool, addr: u16) -> u8 {
        let space = match (in_cart, self.cart, self.ram) {
            (true, Some(cart), _) => cart,
            (false, _, Some(ram)) if addr >= 0xa000 => ram,
            _ => self.space,
        };
        peek(host, space, u32::from(addr))
    }

    /// The interpreter is in the CPU's view where it was found.
    fn mapped(&self, host: &dyn DebugTarget, rom: &Rom) -> bool {
        three(host, self.space, u32::from(rom.dispatch)) == rom.signature
            && three(host, self.space, u32::from(rom.keywords_at)) == [b'E', b'N', b'D' | 0x80]
    }

    /// The CPU sees RAM where the interpreter was found: one of its RAM
    /// routines (or a program it called) has switched the ROM off.
    fn banked_out(&self, host: &dyn DebugTarget, rom: &Rom) -> bool {
        let Some(ram) = self.ram else {
            return false;
        };
        let at = u32::from(rom.dispatch);
        let seen = three(host, self.space, at);
        seen == three(host, ram, at)
            || (self.var_bank != 0 && seen == three(host, ram, self.var_bank + at))
    }

    /// The interpreter, if it is mapped where it was found.
    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let rom = self.rom.as_ref()?;
        (self.mapped(host, rom) || self.banked_out(host, rom)).then_some(View {
            host,
            data: self.ram.unwrap_or(self.space),
            var_off: if rom.fe.is_empty() { 0 } else { self.var_bank },
            rom,
            ext: self.ext.as_ref(),
        })
    }
}

fn three(host: &dyn DebugTarget, space: MemSpaceId, addr: u32) -> [u8; 3] {
    std::array::from_fn(|i| peek(host, space, addr + i as u32))
}

/// Look for Simons' BASIC or TSB in `mem`, what is at `$8000`–`$BFFF`
/// (see the module text). `txtptr`: the zero-page address of TXTPTR.
fn find_ext(mem: &[u8], in_cart: bool, txtptr: u8) -> Option<Ext> {
    let at = mem.windows(4).position(|w| w == b"HIRE")?;
    let (name, list) = match mem.get(at + 4..at + 6)? {
        [b'S', b'@'] => {
            let end = mem[at..].iter().position(|&b| b == 0)?;
            let list = mem[at..at + end]
                .split(|&b| b == b'@')
                .take(0x7f)
                .map(|w| {
                    w.iter().fold(String::new(), |mut s, &b| {
                        push_petscii(&mut s, b);
                        s
                    })
                })
                .collect();
            ("Simons' BASIC", list)
        }
        [0xd3, _] => ("TSB", words(&mem[at..], 0x7f, false).0),
        _ => return None,
    };
    // The statement dispatch: LDA table+1,Y : PHA : LDA table,Y : PHA : RTS.
    let table = mem.windows(9).find_map(|w| {
        let (hi, lo) = (
            u16::from_le_bytes([w[1], w[2]]),
            u16::from_le_bytes([w[5], w[6]]),
        );
        (w[0] == 0xb9 && w[3] == 0x48 && w[4] == 0xb9 && w[7..9] == [0x48, 0x60] && hi == lo + 1)
            .then_some(lo)
    });
    let code = |addr: u16| {
        let rel = usize::from(addr.wrapping_sub(EXT_BASE));
        mem.get(rel).copied().unwrap_or(0)
    };
    let stack = |word: &'static str| -> Option<ExtStack> {
        let n = list.iter().position(|w| w == word)? as u16;
        let entry = table?.wrapping_add(2 * n);
        let handler =
            u16::from_le_bytes([code(entry), code(entry.wrapping_add(1))]).wrapping_add(1);
        let body: Vec<u8> = (0..0x40).map(|i| code(handler.wrapping_add(i))).collect();
        // LDA TXTPTR : STA base,Y, and the LDY count before it.
        let store = body
            .windows(5)
            .position(|w| w[0] == 0xa5 && w[1] == txtptr && w[2] == 0x99)?;
        let count = body[..store]
            .windows(3)
            .rev()
            .find(|w| w[0] == 0xac)
            .map(|w| u16::from_le_bytes([w[1], w[2]]))?;
        let pre = u16::from(store > 0 && body[store - 1] == 0xc8);
        let base = u16::from_le_bytes([body[store + 3], body[store + 4]]).wrapping_add(pre);
        Some(ExtStack { word, count, base })
    };
    let stacks = ["EXEC", "REPEAT", "LOOP"]
        .into_iter()
        .filter_map(stack)
        .collect();
    Some(Ext {
        name,
        in_cart,
        table_at: EXT_BASE + at as u16,
        words: list,
        stacks,
    })
}

/// A keyword table: words that end with bit 7 set, up to a 0. Returns
/// them and the bytes they take.
fn words(mem: &[u8], max: usize, ascii: bool) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut used = 0;
    for &b in mem {
        if b == 0 || out.len() == max {
            break;
        }
        used += 1;
        push_char(&mut word, b & 0x7f, ascii);
        if b & 0x80 != 0 {
            out.push(std::mem::take(&mut word));
        }
    }
    (out, used)
}

/// The statement loop of BASIC 1.0 to 4.0, V2 and Applesoft: (index of
/// the loop, TXTPTR).
fn loop_v2(mem: &[u8], addr: impl Fn(usize) -> u16) -> Option<(usize, u16)> {
    let word = |i: usize| u16::from_le_bytes([mem[i], mem[i + 1]]);
    (0x80..mem.len().saturating_sub(9)).find_map(|i| {
        let w = &mem[i..i + 9];
        let here = addr(i);
        let back = usize::from(here.wrapping_sub(word(i + 7)));
        let fits = w[0] == 0x20
            && w[3] == 0x20
            // Right behind the loop; Applesoft has a branch between.
            && [9, 11].contains(&word(i + 4).wrapping_sub(here))
            && w[6] == 0x4c
            && (1..0x80).contains(&back);
        if !fits {
            return None;
        }
        // NEWSTT looks at the text: LDY #0 : LDA (TXTPTR),Y.
        let txtptr = mem[i - back..i]
            .windows(4)
            .find(|c| c[..3] == [0xa0, 0x00, 0xb1])
            .map(|c| u16::from(c[3]))?;
        Some((i, txtptr))
    })
}

/// The statement loop of BASIC 3.5 and 7.0: (index of `JMP ($0308)`,
/// TXTTAB, TXTPTR, the stack).
fn loop_v35(mem: &[u8]) -> Option<(usize, u16, u16, Stack)> {
    let (at, curlin, txtptr) = mem
        .windows(3)
        .enumerate()
        .filter(|(_, w)| *w == [0x6c, 0x08, 0x03])
        .find_map(|(i, _)| {
            let tail = &mem[i + 3..mem.len().min(i + 0xc0)];
            tail.windows(14)
                .find(|w| {
                    w[0] == 0x85
                        && w[2] == 0xc8
                        && w[3] == 0x20
                        && w[6] == 0x85
                        && w[7] == w[1].wrapping_add(1)
                        && w[8..11] == [0x98, 0x18, 0x65]
                        && w[12] == 0x85
                        && w[13] == w[11]
                })
                .map(|w| (i, u16::from(w[1]), u16::from(w[11])))
        })?;
    // GOSUB stores its token: LDA #$8D : STA (ptr),Y.
    let tos = mem
        .windows(4)
        .find(|w| w[..3] == [0xa9, TOKEN_GOSUB, 0x91])
        .map(|w| w[3])?;
    // CLR empties the stack: LDA #lo : LDY #hi : STA ptr : STY ptr+1.
    let top = mem
        .windows(8)
        .find(|w| {
            w[0] == 0xa9
                && w[2] == 0xa0
                && w[4..6] == [0x85, tos]
                && w[6..8] == [0x84, tos.wrapping_add(1)]
        })
        .map(|w| u16::from_le_bytes([w[1], w[3]]))?;
    let stack = Stack::Own {
        tos: u16::from(tos),
        top,
    };
    Some((at, curlin.checked_sub(0x0e)?, txtptr, stack))
}

/// Search the CPU's view from `base` to `$FFFF` for the interpreter.
fn find(host: &dyn DebugTarget, space: MemSpaceId, base: u16) -> Option<Rom> {
    let base = usize::from(base);
    let mem: Vec<u8> = (base..0x10000)
        .map(|a| peek(host, space, a as u32))
        .collect();
    let addr = |i: usize| (base + i) as u16;
    const HEAD: &[u8] = &[
        b'E',
        b'N',
        b'D' | 0x80,
        b'F',
        b'O',
        b'R' | 0x80,
        b'N',
        b'E',
        b'X',
        b'T' | 0x80,
    ];
    let table = mem.windows(HEAD.len()).position(|w| w == HEAD)?;
    let apple = mem[table..]
        .windows(6)
        .take(0x200)
        .any(|w| w == [b'H', b'I', b'M', b'E', b'M', b':' | 0x80]);
    let (dispatch, txttab, txtptr, stack) = match loop_v2(&mem, addr) {
        Some((loop_at, txtptr)) => {
            // CHRGET is 7 bytes before TXTPTR, TXTTAB $48 before CHRGET
            // (Applesoft: $4A).
            let txttab = txtptr.checked_sub(if apple { 0x51 } else { 0x4f })?;
            let dispatch = if mem[loop_at - 3..loop_at] == [0x6c, 0x08, 0x03] {
                loop_at - 3
            } else {
                loop_at
            };
            let stack = Stack::Cpu {
                gone_return: addr(loop_at).wrapping_add(5),
            };
            (dispatch, txttab, txtptr, stack)
        }
        None => loop_v35(&mem)?,
    };
    let (keywords, used) = words(&mem[table..], 0x7f, apple);
    let token = |word: &str, default: u8| {
        keywords
            .iter()
            .position(|k| k == word)
            .map_or(default, |i| 0x80 + i as u8)
    };
    // BASIC 7.0: the tables of the $FE and the $CE tokens follow.
    let (mut fe, mut ce) = (Vec::new(), Vec::new());
    if let Some(rest) = mem.get(table + used + 1..) {
        let (second, used) = words(rest, 0xfe, false);
        if second.first().is_some_and(|w| w == "BANK") {
            fe = second;
            ce = rest
                .get(used + 1..)
                .map_or_else(Vec::new, |r| words(r, 0xfe, false).0);
        }
    }
    Some(Rom {
        dispatch: addr(dispatch),
        signature: [mem[dispatch], mem[dispatch + 1], mem[dispatch + 2]],
        txttab,
        txtptr,
        stack,
        keywords_at: addr(table),
        rem: token("REM", TOKEN_REM),
        gosub: token("GOSUB", TOKEN_GOSUB),
        apple,
        keywords,
        fe,
        ce,
    })
}

impl Dialect for CbmBasic {
    fn space(&self) -> MemSpaceId {
        self.ram.unwrap_or(self.space)
    }

    fn info(&self) -> BasicInfo {
        let has = |word: &str| {
            self.rom
                .as_ref()
                .is_some_and(|r| r.keywords.iter().any(|k| k == word))
        };
        let dialect = if self.rom.as_ref().is_some_and(|r| r.apple) {
            "Applesoft BASIC"
        } else if self.rom.is_none() {
            "Commodore BASIC"
        } else if self.rom.as_ref().is_some_and(|r| !r.fe.is_empty()) {
            "Commodore BASIC 7.0"
        } else if has("RLUM") {
            "Commodore BASIC 3.5"
        } else if has("DOPEN") {
            "Commodore BASIC 4.0"
        } else if has("GO") {
            "Commodore BASIC V2"
        } else {
            "Commodore BASIC 1.0"
        };
        let dialect = match &self.ext {
            Some(e) => format!("{dialect} + {}", e.name),
            None => dialect.to_string(),
        };
        BasicInfo {
            dialect,
            granularity: BasicGranularity::Statement,
            space: self.ram.unwrap_or(self.space),
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
        let (curlin, txt) = (v.curlin(), v.txtptr());
        // After a cold start CURLIN is not yet $FFxx; a running program's
        // text pointer is inside a stored line. That line is the place:
        // RETURN, NEXT and LOOP restore CURLIN and TXTPTR one after the
        // other.
        let lines = v.lines();
        let here = lines
            .iter()
            .find(|l| (l.addr..=l.end).contains(&txt))
            .filter(|_| curlin >> 8 != 0xff);
        let running = here.is_some();
        let depth = v
            .frames()
            .iter()
            .filter(|f| {
                matches!(
                    f,
                    RawFrame::Gosub { .. } | RawFrame::Ext { word: "EXEC", .. }
                )
            })
            .count();
        BasicState {
            mode: if running {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos: here.map(|l| v.pos(&lines, l.number, txt, false)),
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
                addr: v.var_off + u32::from(at),
                value: Some(v.value(at + 2, kind)),
                dims: Vec::new(),
            })
            .collect();
        vars.extend(v.arrays().into_iter().map(|a| BasicVar {
            name: a.name,
            ty: a.kind.ty(),
            addr: v.var_off + u32::from(a.addr),
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
        let size = u32::from(a.kind.element_bytes());
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
                    from: v.pos(&lines, line, txt, true),
                },
                RawFrame::Do { line, txt } => BasicFrame::Loop {
                    word: "DO".into(),
                    from: v.pos(&lines, line, txt, true),
                },
                RawFrame::Ext { word: "EXEC", txt } => BasicFrame::Gosub {
                    from: v.pos_of(&lines, txt),
                },
                RawFrame::Ext { word, txt } => BasicFrame::Loop {
                    word: word.into(),
                    from: v.pos_of(&lines, txt),
                },
                RawFrame::For {
                    var,
                    step,
                    sign,
                    limit,
                    line,
                    txt,
                } => {
                    let entry = var.wrapping_sub(2);
                    let (b0, b1) = (v.vb(entry), v.vb(entry.wrapping_add(1)));
                    let magnitude = float_value(step, false);
                    BasicFrame::For {
                        var: var_name(b0, b1, VarKind::of(b0, b1), v.rom.apple),
                        limit: BasicValue::Float(float_value(limit, true)),
                        step: BasicValue::Float(if sign & 0x80 != 0 {
                            -magnitude
                        } else {
                            magnitude
                        }),
                        from: v.pos(&lines, line, txt, true),
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
                v.plan(at + 2, kind, value)
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
                let at = a
                    .data
                    .wrapping_add((i * u32::from(a.kind.element_bytes())) as u16);
                v.plan(at, a.kind, value)
            }
        }
    }

    fn stop_points(&self, host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        self.view(host)
            .map(|v| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.space,
                    addr: u32::from(v.rom.dispatch),
                },
                granularity: BasicGranularity::Statement,
            })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests;
