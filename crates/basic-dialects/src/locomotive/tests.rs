//! The decoder against a hand-built memory image: the ROM's patterns and
//! its keyword table, a tokenised program, both variable areas and
//! BASIC's stack.

use emu_core::cpu::{Flow, Instr, RegisterSet};

use super::*;

const RAM: MemSpaceId = MemSpaceId(1);
const ROMS: MemSpaceId = MemSpaceId(2);

struct Mem {
    ram: Vec<u8>,
    /// The OS and BASIC, 16 KB each.
    rom: Vec<u8>,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        RegisterSet(Vec::new())
    }

    fn set_reg(&mut self, _name: &str, _v: u32) {}

    fn peek(&self, space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
        let mem = if space == ROMS { &self.rom } else { &self.ram };
        mem.get(addr as usize).map(|b| u32::from(*b))
    }

    fn poke(&mut self, _space: MemSpaceId, addr: u32, _size: Size, v: u32) -> bool {
        self.ram[addr as usize & 0xffff] = v as u8;
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        Instr {
            addr,
            bytes: vec![0],
            mnemonic: String::new(),
            operand: String::new(),
            target: None,
            flow: Flow::Normal,
        }
    }
}

impl Mem {
    fn put(&mut self, addr: u16, bytes: &[u8]) {
        let at = usize::from(addr);
        self.ram[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn word(&mut self, addr: u16, v: u16) {
        self.put(addr, &v.to_le_bytes());
    }

    fn apply(&mut self, writes: Vec<(u32, u8)>) {
        for (a, b) in writes {
            self.ram[a as usize] = b;
        }
    }
}

/// The system variables of BASIC 1.1.
const STATEMENT: u16 = 0xae1b;
const LINE: u16 = 0xae1d;
const AREAS: u16 = 0xae66;
const STACK: u16 = 0xb06f;
const STACK_BASE: u16 = 0xae70;
/// The statement loop in the ROM.
const LOOP_AT: u16 = 0xde60;
const PROGRAM: u16 = 0x0170;
const HEAP: u16 = 0xa000;

const WORDS: [(&str, u8); 21] = [
    ("ZONE", 0xda),
    ("WHILE", 0xd6),
    ("WEND", 0xd5),
    ("TO", 0xec),
    ("THEN", 0xeb),
    ("STOP", 0xce),
    ("STEP", 0xe6),
    ("RETURN", 0xc9),
    ("REM", 0xc5),
    ("PRINT", 0xbf),
    ("NEXT", 0xb0),
    ("IF", 0xa1),
    ("GOTO", 0xa0),
    ("GOSUB", 0x9f),
    ("FOR", 0x9e),
    ("FN", 0xe4),
    ("ELSE", 0x97),
    ("DIM", 0x93),
    ("DEF", 0x8d),
    ("ABS", 0x00),
    ("AND", 0xfa),
];

/// The tab stands for blanks the tokeniser allows; `= >` is another way
/// to write `>=` and comes second.
const SYMBOLS: [(&str, u8); 11] = [
    (">\t=", 0xf0),
    ("= >", 0xf0),
    (">", 0xee),
    ("=", 0xef),
    ("<", 0xf1),
    ("+", 0xf4),
    ("-", 0xf5),
    ("*", 0xf6),
    (":", 0x01),
    ("'", 0xc0),
    ("\\", 0xf9),
];

fn keyword_table() -> Vec<u8> {
    let mut out = Vec::new();
    let entry = |out: &mut Vec<u8>, text: &str, token: u8| {
        let mut bytes = text.as_bytes().to_vec();
        *bytes.last_mut().unwrap() |= 0x80;
        out.extend(bytes);
        out.push(token);
    };
    for letter in (b'A'..=b'Z').rev() {
        for (word, token) in WORDS.iter().filter(|w| w.0.as_bytes()[0] == letter) {
            entry(&mut out, &word[1..], *token);
        }
        out.push(0);
    }
    for (text, token) in SYMBOLS {
        entry(&mut out, text, token);
    }
    out.push(0);
    out
}

fn rom(version: u8) -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    let mut put = |addr: u16, bytes: &[u8]| {
        let at = 0x4000 + usize::from(addr - 0xc000);
        rom[at..at + bytes.len()].copy_from_slice(bytes);
    };
    put(0xc000, &[0x80, 0x01, version]);
    let [s0, s1] = STATEMENT.to_le_bytes();
    put(
        LOOP_AT,
        &[
            0x22, s0, s1, 0xcd, 0x21, 0xb9, 0xdc, 0xb2, 0xc8, 0xcd, 0x2c, 0xde, 0xc4, 0x8f, 0xde,
            0x7e, 0xfe, 0x01, 0x28, 0xec,
        ],
    );
    let [l0, l1] = LINE.to_le_bytes();
    put(
        0xde7b,
        &[
            0x28, 0x0f, 0x22, l0, l1, 0x23, 0x3a, 0x1f, 0xae, 0xb7, 0x28, 0xd9,
        ],
    );
    let [k0, k1] = STACK.to_le_bytes();
    put(0xc7cf, &[0x2a, k0, k1, 0x2b, 0x7e, 0xf5, 0x7d, 0x96, 0x6f]);
    let area = |n: u16| (AREAS + 2 * n).to_le_bytes();
    let (a, b, c, d) = (area(0), area(1), area(2), area(3));
    put(
        0xd5ed,
        &[
            0x2a, a[0], a[1], 0x22, b[0], b[1], 0x22, c[0], c[1], 0x22, d[0], d[1],
        ],
    );
    put(0xe44c, &keyword_table());
    rom
}

/// A line's tokens and the offsets of the bytes before its statements.
struct Body {
    bytes: Vec<u8>,
    marks: Vec<u16>,
}

impl Body {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            marks: vec![3],
        }
    }

    fn t(mut self, bytes: &[u8]) -> Self {
        self.bytes.extend(bytes);
        self
    }

    /// A variable: its token, the two bytes the interpreter fills in,
    /// the name.
    fn var(self, token: u8, name: &str) -> Self {
        let mut bytes = vec![token, 0, 0];
        bytes.extend(name.as_bytes());
        *bytes.last_mut().unwrap() |= 0x80;
        self.t(&bytes)
    }

    /// The statement separator.
    fn colon(mut self) -> Self {
        self.marks.push(4 + self.bytes.len() as u16);
        self.t(&[T_COLON])
    }
}

struct Image {
    mem: Mem,
    basic: Locomotive,
    /// Address and marks of every line.
    lines: Vec<(u16, Vec<u16>)>,
    /// Where the values of the plain variables are, by name.
    data: Vec<(&'static str, u16)>,
}

/// ```text
/// 10 a=5:GOSUB 100
/// 20 FOR i%=1 TO 300:WHILE x!<1.5:WEND:NEXT
/// 30 IF a>1 THEN c$="A:B" ELSE PRINT ABS(-1)' x:y
/// 40 GOTO 10:REM :
/// 50 z=&1F+&X101+255+FNq(2)>=1
/// 60 IFaTHEN10
/// ```
fn image(version: u8) -> Image {
    let mut mem = Mem {
        ram: vec![0; 0x10000],
        rom: rom(version),
    };
    let one_and_a_half = real_bytes(1.5).unwrap();
    let program = [
        (
            10,
            Body::new()
                .var(0x0d, "a")
                .t(&[0xef, 0x13])
                .colon()
                .t(&[0x9f, b' ', 0x1e, 100, 0]),
        ),
        (
            20,
            Body::new()
                .t(&[0x9e, b' '])
                .var(0x02, "i")
                .t(&[0xef, 0x0f, b' ', 0xec, b' ', 0x1a, 0x2c, 0x01])
                .colon()
                .t(&[0xd6, b' '])
                .var(0x04, "x")
                .t(&[0xf1, 0x1f])
                .t(&one_and_a_half)
                .colon()
                .t(&[0xd5])
                .colon()
                .t(&[0xb0]),
        ),
        (
            30,
            Body::new()
                .t(&[0xa1, b' '])
                .var(0x0d, "a")
                .t(&[0xee, 0x0f, b' ', 0xeb, b' '])
                .var(0x03, "c")
                .t(b"\xef\"A:B\" ")
                .colon()
                .t(&[T_ELSE, b' ', 0xbf, b' ', 0xff, 0x00, b'(', 0xf5, 0x0f, b')'])
                .colon()
                .t(&[T_TICK, b' ', b'x', b':', b'y']),
        ),
        (
            40,
            // The line number is already the address of the byte
            // before line 10.
            Body::new()
                .t(&[0xa0, b' ', 0x1d])
                .t(&(PROGRAM - 1).to_le_bytes())
                .colon()
                .t(&[T_REM, b' ', b':']),
        ),
        (
            50,
            Body::new()
                .var(0x0d, "z")
                .t(&[0xef, 0x1c, 0x1f, 0, 0xf4, 0x1b, 5, 0, 0xf4, 0x19, 255, 0xf4])
                .t(&[T_FN])
                .var(0x0d, "q")
                .t(&[b'(', 0x10, b')', 0xf0, 0x0f]),
        ),
        (
            60,
            Body::new()
                .t(&[0xa1])
                .var(0x0d, "a")
                .t(&[0xeb, 0x1e, 10, 0]),
        ),
    ];
    let mut at = PROGRAM;
    let mut lines = Vec::new();
    for (number, body) in program {
        let len = body.bytes.len() as u16 + 5;
        mem.word(at, len);
        mem.word(at + 2, number);
        mem.put(at + 4, &body.bytes);
        lines.push((at, body.marks));
        at += len;
    }
    // The program's end and, behind it, the variables.
    mem.word(AREAS - 2, PROGRAM - 1);
    mem.word(AREAS, at + 2);
    mem.word(AREAS + 2, at + 2);
    at += 2;
    let constant = lines[2].0
        + 4
        + mem.ram[usize::from(lines[2].0) + 4..]
            .iter()
            .position(|b| *b == b'"')
            .unwrap() as u16
        + 1;
    mem.put(HEAP, b"A:B");
    let mut data = Vec::new();
    let variables: [(&'static str, &[u8], u8, Vec<u8>); 6] = [
        ("A", b"\xc1", 4, real_bytes(5.0).unwrap().to_vec()),
        // DEF FNq: its type with bit 6, the address of the definition.
        ("FNQ", b"\xd1", 0x44, vec![0x88, 0x01]),
        ("I%", b"\xc9", 1, vec![2, 0]),
        ("C$", b"\xc3", 2, [&[3][..], &HEAP.to_le_bytes()].concat()),
        (
            "K$",
            b"\xcb",
            2,
            [&[3][..], &constant.to_le_bytes()].concat(),
        ),
        ("COUNT", b"COUN\xd4", 4, real_bytes(-0.25).unwrap().to_vec()),
    ];
    for (shown, name, ty, value) in variables {
        mem.put(at + 2, name);
        at += 2 + name.len() as u16;
        mem.ram[usize::from(at)] = ty;
        at += 1;
        data.push((shown, at));
        mem.put(at, &value);
        at += value.len() as u16;
    }
    mem.word(AREAS + 4, at);
    // DIM d(2,3): the last size first; d(1,1) is the fifth element.
    mem.put(at + 2, &[0xc4, 4, 65, 0, 2, 4, 0, 3, 0]);
    mem.put(at + 11 + 20, &real_bytes(7.0).unwrap());
    at += 11 + 60;
    // DIM e%(4).
    mem.put(at + 2, &[0xc5, 1, 13, 0, 1, 5, 0]);
    mem.put(at + 9 + 6, &(-2i16).to_le_bytes());
    at += 9 + 10;
    mem.word(AREAS + 6, at);
    mem.word(STACK, STACK_BASE);
    let basic = Locomotive::new(RAM, ROMS);
    let basic = basic.rescan(&mem).expect("the ROM is looked at once");
    assert!(basic.rescan(&mem).is_none());
    Image {
        mem,
        basic,
        lines,
        data,
    }
}

impl Image {
    /// The address of mark `statement` in the line at index `line`.
    fn mark(&self, line: usize, statement: usize) -> u16 {
        self.lines[line].0 + self.lines[line].1[statement]
    }

    /// Where the number of the line at index `line` is stored.
    fn number_at(&self, line: usize) -> u16 {
        self.lines[line].0 + 2
    }

    /// At the beginning of statement `statement` of the line at `line`.
    fn at(&mut self, line: usize, statement: usize) {
        let (number, mark) = (self.number_at(line), self.mark(line, statement));
        self.mem.word(LINE, number);
        self.mem.word(STATEMENT, mark);
    }

    fn data(&self, name: &str) -> u16 {
        self.data.iter().find(|d| d.0 == name).unwrap().1
    }
}

#[test]
fn listing_as_list_writes_it() {
    let i = image(2);
    let listed: Vec<(u32, String, Vec<u32>)> = i
        .basic
        .program(&i.mem)
        .into_iter()
        .map(|l| (l.number, l.text, l.statements))
        .collect();
    let want = [
        (10, "a=5:GOSUB 100", vec![0, 4]),
        (
            20,
            "FOR i%=1 TO 300:WHILE x!<1.5:WEND:NEXT",
            vec![0, 16, 29, 34],
        ),
        (
            30,
            "IF a>1 THEN c$=\"A:B\" ELSE PRINT ABS(-1)' x:y",
            vec![0, 21, 39],
        ),
        (40, "GOTO 10:REM :", vec![0, 8]),
        (50, "z=&1F+&X101+255+FNq(2)>=1", vec![0]),
        // Stored without blanks: LIST keeps the words apart.
        (60, "IF a THEN 10", vec![0]),
    ]
    .map(|(n, text, statements)| (n, text.to_string(), statements));
    assert_eq!(listed, want);
    let first = &i.basic.program(&i.mem)[0];
    assert_eq!(first.addr, u32::from(PROGRAM));
    assert_eq!(first.raw.len(), 17);
    assert_eq!(first.raw.last(), Some(&0));
}

#[test]
fn place_and_mode() {
    let mut i = image(2);
    assert_eq!(i.basic.info().dialect, "Locomotive BASIC 1.1");
    assert_eq!(i.basic.info().space, RAM);
    // At the prompt no line is the current one.
    let state = i.basic.state(&i.mem);
    assert_eq!((state.mode, state.pos), (BasicMode::Direct, None));
    let pos = |i: &Image| i.basic.state(&i.mem).pos.map(|p| (p.line, p.statement));
    i.at(1, 0);
    assert_eq!(i.basic.state(&i.mem).mode, BasicMode::Running);
    assert_eq!(pos(&i), Some((20, 0)));
    // The 1 in the number 300 is no separator.
    i.at(1, 1);
    assert_eq!(pos(&i), Some((20, 1)));
    i.at(1, 3);
    assert_eq!(pos(&i), Some((20, 3)));
    // ELSE and the tick behind their hidden colons.
    i.at(2, 2);
    assert_eq!(pos(&i), Some((30, 2)));
    // Anywhere inside a statement.
    let inside = i.mark(2, 1) + 3;
    i.mem.word(STATEMENT, inside);
    assert_eq!(pos(&i), Some((30, 1)));
    // The stop point: in the ROM space, behind the store.
    assert_eq!(
        i.basic.stop_points(&i.mem),
        [BasicStopPoint {
            kind: BreakpointKind::Exec {
                space: ROMS,
                addr: 0x4000 + u32::from(LOOP_AT - 0xc000) + 3,
            },
            granularity: BasicGranularity::Statement,
        }]
    );
}

#[test]
fn other_roms() {
    assert_eq!(image(0).basic.info().dialect, "Locomotive BASIC 1.0");
    assert_eq!(image(1).basic.info().dialect, "Locomotive BASIC 1.1");
    // No BASIC in the ROM space: nothing to follow.
    let mut i = image(2);
    i.mem.rom.fill(0);
    let none = Locomotive::new(RAM, ROMS).rescan(&i.mem).unwrap();
    assert_eq!(none.state(&i.mem).mode, BasicMode::Inactive);
    assert!(none.program(&i.mem).is_empty());
    assert!(none.stop_points(&i.mem).is_empty());
    assert_eq!(
        none.plan_set(&i.mem, "A", None, &BasicValue::Int(1)),
        Err(BasicError::Inactive)
    );
}

#[test]
fn frames_of_every_kind() {
    let mut i = image(2);
    i.at(0, 1);
    assert!(i.basic.stack(&i.mem).is_empty());
    let word = |v: u16| v.to_le_bytes();
    // FOR i% in line 20.
    let mut stack: Vec<u8> = Vec::new();
    stack.extend(word(i.data("I%")));
    stack.extend(word(300));
    stack.extend(word(1));
    stack.push(1);
    stack.extend(word(i.mark(1, 1)));
    stack.extend(word(i.number_at(1)));
    stack.extend([0, 0, 0, 0, 16]);
    // WHILE behind it: the line, behind WEND, the condition.
    stack.extend(word(i.number_at(1)));
    stack.extend(word(i.mark(1, 3)));
    stack.extend(word(i.mark(1, 1) + 3));
    stack.push(7);
    // FOR a in line 10, a real variable counting down.
    stack.extend(word(i.data("A")));
    stack.extend(real_bytes(2.5).unwrap());
    stack.extend(real_bytes(-0.5).unwrap());
    stack.push(0xff);
    stack.extend(word(i.mark(0, 1)));
    stack.extend(word(i.number_at(0)));
    stack.extend([0, 0, 0, 0, 22]);
    // GOSUB from line 10's second statement, which ends the line.
    stack.push(0);
    stack.extend(word(i.lines[1].0 - 1));
    stack.extend(word(i.number_at(0)));
    stack.push(6);
    i.mem.put(STACK_BASE, &stack);
    i.mem.word(STACK, STACK_BASE + stack.len() as u16);
    let pos = |line, statement| BasicPos { line, statement };
    assert_eq!(
        i.basic.stack(&i.mem),
        [
            BasicFrame::Gosub { from: pos(10, 1) },
            BasicFrame::For {
                var: "A".into(),
                limit: BasicValue::Float(2.5),
                step: BasicValue::Float(-0.5),
                from: pos(10, 0),
            },
            BasicFrame::Loop {
                word: "WHILE".into(),
                from: pos(20, 1),
            },
            BasicFrame::For {
                var: "I%".into(),
                limit: BasicValue::Int(300),
                step: BasicValue::Int(1),
                from: pos(20, 0),
            },
        ]
    );
    assert_eq!(i.basic.state(&i.mem).depth, 1);
    // A subroutine called from a command typed in has no line.
    i.mem.word(STACK_BASE + stack.len() as u16 - 3, 0);
    assert_eq!(
        i.basic.stack(&i.mem)[0],
        BasicFrame::Gosub { from: pos(0, 0) }
    );
}

#[test]
fn variables_read_and_written() {
    let mut i = image(2);
    let vars = i.basic.variables(&i.mem);
    let shown: Vec<(&str, BasicType, Option<BasicValue>, &[u32])> = vars
        .iter()
        .map(|v| (v.name.as_str(), v.ty, v.value.clone(), v.dims.as_slice()))
        .collect();
    let text = |s: &str| Some(BasicValue::Str(s.into()));
    assert_eq!(
        shown,
        [
            ("A", BasicType::Float, Some(BasicValue::Float(5.0)), &[][..]),
            ("I%", BasicType::Int, Some(BasicValue::Int(2)), &[]),
            ("C$", BasicType::Str, text("A:B"), &[]),
            ("K$", BasicType::Str, text("A:B"), &[]),
            (
                "COUNT",
                BasicType::Float,
                Some(BasicValue::Float(-0.25)),
                &[]
            ),
            ("D", BasicType::Float, None, &[3, 4]),
            ("E%", BasicType::Int, None, &[5]),
        ]
    );
    assert_eq!(
        i.basic.array(&i.mem, "d", 3, 3),
        Ok(vec![
            BasicValue::Float(0.0),
            BasicValue::Float(7.0),
            BasicValue::Float(0.0)
        ])
    );
    assert_eq!(
        i.basic.array(&i.mem, "E%", 3, 9),
        Ok(vec![BasicValue::Int(-2), BasicValue::Int(0)])
    );
    assert_eq!(
        i.basic.array(&i.mem, "E", 0, 1),
        Err(BasicError::NoSuchVariable)
    );

    let mut set = |name: &str, index, value: BasicValue| {
        let writes = i.basic.plan_set(&i.mem, name, index, &value)?;
        i.mem.apply(writes);
        Ok(())
    };
    assert_eq!(set("a!", None, BasicValue::Float(-1e10)), Ok(()));
    assert_eq!(set("count", None, BasicValue::Int(42)), Ok(()));
    assert_eq!(set("I%", None, BasicValue::Float(-7.0)), Ok(()));
    assert_eq!(
        set("I%", None, BasicValue::Float(0.5)),
        Err(BasicError::OutOfRange)
    );
    assert_eq!(
        set("I%", None, BasicValue::Int(40_000)),
        Err(BasicError::OutOfRange)
    );
    assert_eq!(set("D", Some(11), BasicValue::Float(0.001)), Ok(()));
    assert_eq!(
        set("D", Some(12), BasicValue::Int(1)),
        Err(BasicError::OutOfRange)
    );
    assert_eq!(set("e%", Some(0), BasicValue::Int(9)), Ok(()));
    assert_eq!(set("C$", None, BasicValue::Str("xyz".into())), Ok(()));
    assert_eq!(
        set("C$", None, BasicValue::Str("TOO LONG".into())),
        Err(BasicError::Unsupported)
    );
    // The constant in the program's text is left alone.
    assert_eq!(
        set("K$", None, BasicValue::Str("xyz".into())),
        Err(BasicError::Unsupported)
    );
    assert_eq!(
        set("C$", None, BasicValue::Int(1)),
        Err(BasicError::TypeMismatch)
    );
    assert_eq!(
        set("FNQ", None, BasicValue::Int(1)),
        Err(BasicError::NoSuchVariable)
    );
    let vars = i.basic.variables(&i.mem);
    assert_eq!(vars[0].value, Some(BasicValue::Float(-1e10)));
    assert_eq!(vars[1].value, Some(BasicValue::Int(-7)));
    assert_eq!(vars[2].value, text("xyz"));
    assert_eq!(vars[3].value, text("A:B"));
    assert_eq!(vars[4].value, Some(BasicValue::Float(42.0)));
    assert_eq!(
        i.basic.array(&i.mem, "D", 11, 1),
        Ok(vec![BasicValue::Float(real(real_bytes(0.001).unwrap()))])
    );
    assert_eq!(
        i.basic.array(&i.mem, "E%", 0, 1),
        Ok(vec![BasicValue::Int(9)])
    );
    assert!(i.basic.program(&i.mem)[2].text.contains("\"A:B\""));
}

#[test]
fn real_numbers_as_stored_and_as_listed() {
    assert_eq!(real_bytes(1.5), Ok([0, 0, 0, 0x40, 0x81]));
    assert_eq!(real_bytes(-1.0), Ok([0, 0, 0, 0x80, 0x81]));
    assert_eq!(real_bytes(0.0), Ok([0; 5]));
    assert_eq!(real([0xff, 0xff, 0xff, 0x7f, 0]), 0.0);
    for (v, text) in [
        (0.0, "0"),
        (1.5, "1.5"),
        (-7.0, "-7"),
        (0.5, "0.5"),
        (100_000.0, "100000"),
        (123_456_789.0, "123456789"),
        (1_234_567_890.0, "1.23456789E+09"),
        (1e20, "1E+20"),
        (0.001, "0.001"),
        (0.000_012_345, "0.000012345"),
        (1.5e-10, "1.5E-10"),
        (-2.5e-12, "-2.5E-12"),
    ] {
        let b = real_bytes(v).unwrap();
        assert_eq!(real_text(real(b)), text, "{b:02x?}");
        assert!((real(b) - v).abs() <= v.abs() * 1e-9, "{b:02x?}");
    }
    assert_eq!(real_bytes(1e-60), Ok([0; 5]));
    assert_eq!(real_bytes(1e60), Err(BasicError::OutOfRange));
    assert_eq!(real_bytes(f64::INFINITY), Err(BasicError::OutOfRange));
}
