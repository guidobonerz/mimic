//! The decoder against a hand-built memory image: the ROM's statement
//! loop and its two name tables, a tokenised program, the variable tables
//! and the run time stack.

use mimic_core::cpu::{Flow, Instr, RegisterSet};

use super::*;

const CPU: MemSpaceId = MemSpaceId(0);

struct Mem {
    ram: Vec<u8>,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        RegisterSet(Vec::new())
    }

    fn set_reg(&mut self, _name: &str, _v: u32) {}

    fn peek(&self, _space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
        self.ram.get(addr as usize).map(|b| u32::from(*b))
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

const STATEMENTS: [&str; 56] = [
    "REM", "DATA", "INPUT", "COLOR", "LIST", "ENTER", "LET", "IF", "FOR", "NEXT", "GOTO", "GO TO",
    "GOSUB", "TRAP", "BYE", "CONT", "COM", "CLOSE", "CLR", "DEG", "DIM", "END", "NEW", "OPEN",
    "LOAD", "SAVE", "STATUS", "NOTE", "POINT", "XIO", "ON", "POKE", "PRINT", "RAD", "READ",
    "RESTORE", "RETURN", "RUN", "STOP", "POP", "?", "GET", "PUT", "GRAPHICS", "PLOT", "POSITION",
    "DOS", "DRAWTO", "SETCOLOR", "LOCATE", "SOUND", "LPRINT", "CSAVE", "CLOAD", "\0", "ERROR-",
];

/// From token `$10`; `\0` where the ROM has an empty name.
const OPERATORS: [&str; 48] = [
    "\u{2}", "\0", ",", "$", ":", ";", "\u{1b}", "GOTO", "GOSUB", "TO", "STEP", "THEN", "#", "<=",
    "<>", ">=", "<", ">", "=", "^", "*", "+", "-", "/", "NOT", "OR", "AND", "(", ")", "=", "=",
    "<=", "<>", ">=", "<", ">", "=", "+", "-", "(", "\0", "\0", "(", "(", ",", "STR$", "CHR$",
    "USR",
];

const STOP: u16 = 0xa977;
const STATEMENT_TABLE: u16 = 0xa49f;
const OPERATOR_TABLE: u16 = 0xa7de;

const VNT: u16 = 0x0800;
const VVT: u16 = 0x0820;
const PROGRAM: u16 = 0x0900;
const ARRAYS: u16 = 0x0a00;
const STACK: u16 = 0x0b00;

fn names_table(words: &[&str], prefix: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for w in words {
        out.extend(std::iter::repeat_n(0x55, prefix));
        let mut bytes = w.as_bytes().to_vec();
        *bytes.last_mut().unwrap() |= 0x80;
        out.extend(bytes);
    }
    out.push(0);
    out
}

fn num(v: f64) -> Vec<u8> {
    let mut out = vec![T_NUMBER];
    out.extend(number_bytes(v).unwrap());
    out
}

/// A stored line and the offsets of its statements.
fn line(number: u16, statements: &[(u8, Vec<u8>)]) -> (Vec<u8>, Vec<u8>) {
    let mut out = number.to_le_bytes().to_vec();
    out.push(0);
    let mut starts = Vec::new();
    for (token, body) in statements {
        starts.push(out.len() as u8);
        out.push((out.len() + 2 + body.len()) as u8);
        out.push(*token);
        out.extend(body);
    }
    out[2] = out.len() as u8;
    (out, starts)
}

struct Image {
    mem: Mem,
    basic: AtariBasic,
    /// Address and statement offsets of the lines 10 to 50.
    lines: Vec<(u16, Vec<u8>)>,
}

/// ```text
/// 10 A=5:GOSUB 100
/// 20 FOR I=1 TO 3:NEXT I
/// 30 IF A>1 THEN C$="HI"
/// 40 REM X:Y
/// 50 D(1)=COUNT
/// ```
fn image() -> Image {
    let mut mem = Mem {
        ram: vec![0; 0x10000],
    };
    // The ROM: the statement loop and LIST's references to the tables.
    let mut head = vec![
        0x20, 0xf2, 0xa9, 0xf0, 0x36, 0xa4, 0xa7, 0xc4, 0x9f, 0xb0, 0x1d,
    ];
    head.extend(LOOP);
    mem.put(STOP + 1 - head.len() as u16, &head);
    let [hi, lo] = STATEMENT_TABLE.to_be_bytes();
    mem.put(0xb66f, &[0x85, 0xaf, 0xa2, 0x02, 0xa9, hi, 0xa0, lo, 0x20]);
    let [hi, lo] = OPERATOR_TABLE.to_be_bytes();
    mem.put(
        0xb63b,
        &[0x38, 0xe9, 0x10, 0x85, 0xaf, 0xa2, 0x00, 0xa9, hi, 0xa0, lo],
    );
    mem.put(STATEMENT_TABLE, &names_table(&STATEMENTS, 2));
    mem.put(OPERATOR_TABLE, &names_table(&OPERATORS, 0));

    // The variables: A, I, C$, D( and COUNT.
    mem.put(VNT, b"\xc1\xc9C\xa4D\xa8COUN\xd4\0");
    let entry = |kind: u8, n: u8, rest: [u8; 6]| {
        let mut e = vec![kind, n];
        e.extend(rest);
        e
    };
    let pair = |a: u16, b: u16, c: u16| {
        let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
        [a[0], a[1], b[0], b[1], c[0], c[1]]
    };
    let mut values = entry(0x00, 0, number_bytes(5.0).unwrap());
    values.extend(entry(0x00, 1, number_bytes(2.0).unwrap()));
    values.extend(entry(0x81, 2, pair(0, 2, 5)));
    values.extend(entry(0x41, 3, pair(5, 3, 1)));
    values.extend(entry(0x00, 4, number_bytes(-0.25).unwrap()));
    mem.put(VVT, &values);
    mem.put(ARRAYS, b"HI???");
    mem.put(ARRAYS + 5 + 12, &number_bytes(7.5).unwrap());

    let program = [
        line(
            10,
            &[
                (
                    T_IMPLIED_LET,
                    [vec![0x80, 0x2d], num(5.0), vec![0x14]].concat(),
                ),
                (0x0c, [num(100.0), vec![T_EOL]].concat()),
            ],
        ),
        line(
            20,
            &[
                (
                    0x08,
                    [vec![0x81, 0x2d], num(1.0), vec![0x19], num(3.0), vec![0x14]].concat(),
                ),
                (0x09, vec![0x81, T_EOL]),
            ],
        ),
        line(
            30,
            &[
                (0x07, [vec![0x80, 0x21], num(1.0), vec![0x1b]].concat()),
                (
                    T_IMPLIED_LET,
                    vec![0x82, 0x2e, T_STRING, 2, b'H', b'I', T_EOL],
                ),
            ],
        ),
        line(40, &[(T_REM, b"X:Y\x9b".to_vec())]),
        line(
            50,
            &[(
                T_IMPLIED_LET,
                [vec![0x83, 0x38], num(1.0), vec![0x2c, 0x2d, 0x84, T_EOL]].concat(),
            )],
        ),
        // The line typed in last: RUN.
        line(0x8000, &[(0x25, vec![T_EOL])]),
    ];
    let mut at = PROGRAM;
    let mut lines = Vec::new();
    for (bytes, starts) in program {
        mem.put(at, &bytes);
        lines.push((at, starts));
        at += bytes.len() as u16;
    }

    mem.word(VNTP, VNT);
    mem.word(VNTD, VNT + 13);
    mem.word(VVTP, VVT);
    mem.word(STMTAB, PROGRAM);
    mem.word(STMCUR, lines[5].0);
    mem.word(STARP, ARRAYS);
    mem.word(RUNSTK, STACK);
    mem.word(STACK_TOP, STACK);
    let basic = AtariBasic::new(CPU);
    let basic = basic.rescan(&mem).expect("the ROM is found");
    assert!(basic.rescan(&mem).is_none(), "nothing changed");
    Image { mem, basic, lines }
}

impl Image {
    /// Inside statement `statement` of the line at index `line`.
    fn at(&mut self, line: usize, statement: usize) {
        let (addr, starts) = self.lines[line].clone();
        let next = self.mem.ram[usize::from(addr) + usize::from(starts[statement])];
        self.mem.word(STMCUR, addr);
        self.mem.ram[usize::from(NXTSTD)] = next;
        self.mem.ram[usize::from(STINDEX)] = starts[statement] + 2;
        self.mem.word(CPU_STACK, STOP + 2);
    }

    fn pos(&self) -> Option<(u32, u16)> {
        let state = self.basic.state(&self.mem);
        state.pos.map(|p| (p.line, p.statement))
    }
}

#[test]
fn listing_as_list_writes_it() {
    let i = image();
    let listed: Vec<(u32, String, Vec<u32>)> = i
        .basic
        .program(&i.mem)
        .into_iter()
        .map(|l| (l.number, l.text, l.statements))
        .collect();
    let want = [
        (10, "A=5:GOSUB 100", vec![0, 4]),
        (20, "FOR I=1 TO 3:NEXT I", vec![0, 13]),
        (30, "IF A>1 THEN C$=\"HI\"", vec![0, 12]),
        (40, "REM X:Y", vec![0]),
        (50, "D(1)=COUNT", vec![0]),
    ]
    .map(|(n, text, statements)| (n, text.to_string(), statements));
    assert_eq!(listed, want);
    let first = &i.basic.program(&i.mem)[0];
    assert_eq!(first.addr, u32::from(PROGRAM));
    assert_eq!(first.raw.len(), usize::from(first.raw[2]));
}

#[test]
fn place_and_mode() {
    let mut i = image();
    assert_eq!(i.basic.info().dialect, "Atari BASIC");
    // The editor: the line typed in last, another return address.
    let state = i.basic.state(&i.mem);
    assert_eq!((state.mode, state.pos), (BasicMode::Direct, None));
    // The command typed in runs through the loop too.
    i.mem.word(CPU_STACK, STOP + 2);
    assert_eq!(i.basic.state(&i.mem).mode, BasicMode::Direct);
    i.at(1, 1);
    assert_eq!(i.basic.state(&i.mem).mode, BasicMode::Running);
    assert_eq!(i.pos(), Some((20, 1)));
    // Between two statements the loop's test for BREAK is on the stack,
    // or the place where an interrupt met the loop.
    for inside in [STOP - 20, STOP - 5, STOP - LOOP_BEFORE, STOP + LOOP_AFTER] {
        i.mem.word(CPU_STACK, inside);
        assert_eq!(i.pos(), Some((20, 1)), "{inside:04x}");
    }
    for outside in [STOP - LOOP_BEFORE - 1, STOP + LOOP_AFTER + 1] {
        i.mem.word(CPU_STACK, outside);
        assert_eq!(i.pos(), None, "{outside:04x}");
    }
    i.mem.word(CPU_STACK, STOP + 2);
    i.at(2, 1);
    assert_eq!(i.pos(), Some((30, 1)));
    // A jump under way: the line is the new one, NXTSTD its first offset.
    i.mem.ram[usize::from(NXTSTD)] = 3;
    assert_eq!(i.pos(), Some((30, 0)));
    // A stored line the editor has just looked at is not a running one.
    i.mem.word(CPU_STACK, 0xa070);
    assert_eq!(i.basic.state(&i.mem).mode, BasicMode::Direct);
    assert_eq!(
        i.basic.stop_points(&i.mem),
        [BasicStopPoint {
            kind: BreakpointKind::Exec {
                space: CPU,
                addr: u32::from(STOP)
            },
            granularity: BasicGranularity::Statement,
        }]
    );
}

#[test]
fn the_rom_switched_off() {
    let mut i = image();
    i.at(1, 0);
    // The XL's RAM under the ROM.
    i.mem.ram[0xa000..0xc000].fill(0);
    assert_eq!(i.basic.state(&i.mem).mode, BasicMode::Inactive);
    assert!(i.basic.program(&i.mem).is_empty());
    assert!(i.basic.stop_points(&i.mem).is_empty());
    let gone = i.basic.rescan(&i.mem).expect("the ROM went away");
    assert_eq!(gone.state(&i.mem).mode, BasicMode::Inactive);
}

#[test]
fn frames_of_for_and_gosub() {
    let mut i = image();
    i.at(0, 1);
    assert!(i.basic.stack(&i.mem).is_empty());
    // FOR in line 20, then a GOSUB from line 10's second statement.
    let mut stack = number_bytes(3.0).unwrap().to_vec();
    stack.extend(number_bytes(1.0).unwrap());
    stack.extend([0x81, 20, 0, i.lines[1].1[0] + 1]);
    stack.extend([0x00, 10, 0, i.lines[0].1[1] + 1]);
    i.mem.put(STACK, &stack);
    i.mem.word(STACK_TOP, STACK + stack.len() as u16);
    assert_eq!(
        i.basic.stack(&i.mem),
        [
            BasicFrame::Gosub {
                from: BasicPos {
                    line: 10,
                    statement: 1
                }
            },
            BasicFrame::For {
                var: "I".into(),
                limit: BasicValue::Float(3.0),
                step: BasicValue::Float(1.0),
                from: BasicPos {
                    line: 20,
                    statement: 0
                },
            },
        ]
    );
    assert_eq!(i.basic.state(&i.mem).depth, 1);
    // Half an entry is not one.
    i.mem.word(STACK_TOP, STACK + 10);
    assert!(i.basic.stack(&i.mem).is_empty());
}

#[test]
fn variables_read_and_written() {
    let mut i = image();
    let vars = i.basic.variables(&i.mem);
    let shown: Vec<(&str, Option<BasicValue>, &[u32])> = vars
        .iter()
        .map(|v| (v.name.as_str(), v.value.clone(), v.dims.as_slice()))
        .collect();
    assert_eq!(
        shown,
        [
            ("A", Some(BasicValue::Float(5.0)), &[][..]),
            ("I", Some(BasicValue::Float(2.0)), &[]),
            ("C$", Some(BasicValue::Str("HI".into())), &[]),
            ("D", None, &[3]),
            ("COUNT", Some(BasicValue::Float(-0.25)), &[]),
        ]
    );
    assert_eq!(vars[2].addr, u32::from(VVT) + 16);
    assert_eq!(
        i.basic.array(&i.mem, "d", 1, 9),
        Ok(vec![BasicValue::Float(0.0), BasicValue::Float(7.5)])
    );
    assert_eq!(
        i.basic.array(&i.mem, "A", 0, 1),
        Err(BasicError::NoSuchVariable)
    );

    let mut set = |name: &str, index, value: BasicValue| {
        let writes = i.basic.plan_set(&i.mem, name, index, &value)?;
        i.mem.apply(writes);
        Ok(())
    };
    assert_eq!(set("a", None, BasicValue::Float(-1e10)), Ok(()));
    assert_eq!(set("COUNT", None, BasicValue::Int(42)), Ok(()));
    assert_eq!(set("D", Some(0), BasicValue::Float(0.001)), Ok(()));
    assert_eq!(
        set("D", Some(3), BasicValue::Int(1)),
        Err(BasicError::OutOfRange)
    );
    // A string takes any length up to its dimension.
    assert_eq!(set("C$", None, BasicValue::Str("HELLO".into())), Ok(()));
    assert_eq!(
        set("C$", None, BasicValue::Str("TOO LONG".into())),
        Err(BasicError::Unsupported)
    );
    assert_eq!(
        set("C$", None, BasicValue::Int(1)),
        Err(BasicError::TypeMismatch)
    );
    assert_eq!(
        set("A", None, BasicValue::Float(1e99)),
        Err(BasicError::OutOfRange)
    );
    assert_eq!(
        set("Q", None, BasicValue::Int(1)),
        Err(BasicError::NoSuchVariable)
    );
    let vars = i.basic.variables(&i.mem);
    assert_eq!(vars[0].value, Some(BasicValue::Float(-1e10)));
    assert_eq!(vars[2].value, Some(BasicValue::Str("HELLO".into())));
    assert_eq!(vars[4].value, Some(BasicValue::Float(42.0)));
    assert_eq!(
        i.basic.array(&i.mem, "D", 0, 1),
        Ok(vec![BasicValue::Float(0.001)])
    );
}

#[test]
fn numbers_as_stored_and_as_listed() {
    // 100 is 01 with the exponent of 100, 0.5 is 50 a hundredth.
    assert_eq!(number_bytes(100.0), Ok([0x41, 0x01, 0, 0, 0, 0]));
    assert_eq!(number_bytes(-0.5), Ok([0xbf, 0x50, 0, 0, 0, 0]));
    assert_eq!(
        number_bytes(1_234_567_890.0),
        Ok([0x44, 0x12, 0x34, 0x56, 0x78, 0x90])
    );
    // Nine digits behind a first pair of one: the tenth is rounded away.
    assert_eq!(number_bytes(9.999_999_999), Ok([0x40, 0x10, 0, 0, 0, 0]));
    for (v, text) in [
        (0.0, "0"),
        (5.0, "5"),
        (100.0, "100"),
        (-2.25, "-2.25"),
        (0.5, "0.5"),
        (0.05, "0.05"),
        (0.01, "0.01"),
        (9_999_999_999.0, "9999999999"),
        (1e10, "1E+10"),
        (1e11, "1.0E+11"),
        (1.5e10, "1.5E+10"),
        (123_456_789_000.0, "1.23456789E+11"),
        (0.001, "1.0E-03"),
        (0.0015, "1.5E-03"),
        (-1e-20, "-1E-20"),
    ] {
        let b = number_bytes(v).unwrap();
        assert_eq!(fasc(b), text, "{b:02x?}");
        assert_eq!(number(b), v, "{b:02x?}");
    }
    assert_eq!(number_bytes(1e-200), Ok([0; 6]));
    assert_eq!(number_bytes(f64::NAN), Err(BasicError::OutOfRange));
}
