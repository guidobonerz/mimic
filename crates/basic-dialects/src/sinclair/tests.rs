//! The decoder against hand-built memory images: the patterns of the
//! ROMs with their keyword tables, programs, variables of every kind and
//! GOSUB frames below RAMTOP.

use mimic_core::cpu::{Flow, Instr, RegisterSet};

use super::*;

const CPU: MemSpaceId = MemSpaceId(0);
const ROMS: MemSpaceId = MemSpaceId(2);

struct Mem {
    ram: Vec<u8>,
    /// The Spectrum's ROMs one behind the other.
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

/// A stored line: number, length, text and its end byte.
fn line(number: u16, text: &[u8], end: u8) -> Vec<u8> {
    let mut out = number.to_be_bytes().to_vec();
    out.extend((text.len() as u16 + 1).to_le_bytes());
    out.extend(text);
    out.push(end);
    out
}

/// A keyword table: the last character of each word with bit 7.
fn table(words: &[&str], code: impl Fn(char) -> u8) -> Vec<u8> {
    let mut out = Vec::new();
    for w in words {
        let mut codes: Vec<u8> = w.chars().map(&code).collect();
        *codes.last_mut().unwrap() |= 0x80;
        out.extend(codes);
    }
    out
}

const SPECTRUM_WORDS: [&str; 91] = [
    "RND",
    "INKEY$",
    "PI",
    "FN",
    "POINT",
    "SCREEN$",
    "ATTR",
    "AT",
    "TAB",
    "VAL$",
    "CODE",
    "VAL",
    "LEN",
    "SIN",
    "COS",
    "TAN",
    "ASN",
    "ACS",
    "ATN",
    "LN",
    "EXP",
    "INT",
    "SQR",
    "SGN",
    "ABS",
    "PEEK",
    "IN",
    "USR",
    "STR$",
    "CHR$",
    "NOT",
    "BIN",
    "OR",
    "AND",
    "<=",
    ">=",
    "<>",
    "LINE",
    "THEN",
    "TO",
    "STEP",
    "DEF FN",
    "CAT",
    "FORMAT",
    "MOVE",
    "ERASE",
    "OPEN #",
    "CLOSE #",
    "MERGE",
    "VERIFY",
    "BEEP",
    "CIRCLE",
    "INK",
    "PAPER",
    "FLASH",
    "BRIGHT",
    "INVERSE",
    "OVER",
    "OUT",
    "LPRINT",
    "LLIST",
    "STOP",
    "READ",
    "DATA",
    "RESTORE",
    "NEW",
    "BORDER",
    "CONTINUE",
    "DIM",
    "REM",
    "FOR",
    "GO TO",
    "GO SUB",
    "INPUT",
    "LOAD",
    "LIST",
    "LET",
    "PAUSE",
    "NEXT",
    "POKE",
    "PRINT",
    "PLOT",
    "RUN",
    "SAVE",
    "RANDOMIZE",
    "IF",
    "CLS",
    "DRAW",
    "CLEAR",
    "RETURN",
    "COPY",
];

const LOOP_48: [u8; 20] = [
    0xfd, 0x34, 0x0d, 0xfa, 0x8a, 0x1c, 0xdf, 0x06, 0x00, 0xfe, 0x0d, 0x28, 0x7a, 0xfe, 0x3a, 0x28,
    0xeb, 0x21, 0x76, 0x1b,
];
const RUN_48: [u8; 7] = [0xfd, 0x36, 0x0a, 0x01, 0xcd, 0x8a, 0x1b];

/// The 48K ROM as far as the decoder looks at it.
fn rom_48() -> Vec<u8> {
    let mut rom = vec![0; ROM_BANK];
    rom[0x1b2c..0x1b2c + 20].copy_from_slice(&LOOP_48);
    rom[0x12fc..0x12fc + 7].copy_from_slice(&RUN_48);
    let words = table(&SPECTRUM_WORDS, |c| c as u8);
    rom[0x0095] = 0xbf;
    rom[0x0096..0x0096 + words.len()].copy_from_slice(&words);
    rom
}

const PROG: u16 = 0x5ccb;
const RAMTOP: u16 = 0xff57;

/// A number in the text: its digits, the marker and five bytes.
fn lit(digits: &str) -> Vec<u8> {
    let mut out = digits.as_bytes().to_vec();
    out.push(0x0e);
    out.extend(number_bytes(digits.parse().unwrap(), true).unwrap());
    out
}

fn text(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// A 48K Spectrum with a program of three lines, variables and one
/// GOSUB, running line 10's second statement.
fn spectrum_machine(rom: Vec<u8>) -> (Mem, Sinclair) {
    let mut m = Mem {
        ram: vec![0; 0x10000],
        rom,
    };
    let mut prog = Vec::new();
    // 10 LET a=5: GO SUB 100: IF a>1 THEN PRINT "x:y": REM a:b
    prog.extend(line(
        10,
        &text(&[
            &[0xf1],
            b"a=",
            &lit("5"),
            b":",
            &[0xed],
            &lit("100"),
            b":",
            &[0xfa],
            b"a>",
            &lit("1"),
            &[0xcb, 0xf5],
            b"\"x:y\":",
            &[0xea],
            b"a:b",
        ]),
        0x0d,
    ));
    // 20 LET b=RND*PI+VAL "2": LET c=a<=b OR NOT c
    prog.extend(line(
        20,
        &text(&[
            &[0xf1],
            b"b=",
            &[0xa5],
            b"*",
            &[0xa7],
            b"+",
            &[0xb0],
            b"\"2\":",
            &[0xf1],
            b"c=a",
            &[0xc7],
            b"b",
            &[0xc5, 0xc3],
            b"c",
        ]),
        0x0d,
    ));
    // 100 PRINT, the colour code for ink 2, a pound sign and a full
    // block in quotes, CHR$ with a user-defined graphic: RETURN
    prog.extend(line(
        100,
        &text(&[
            &[0xf5, 0x10, 0x02],
            b";\"\x60\x8f\";",
            &[0xc2, 0x90],
            b":",
            &[0xfe],
        ]),
        0x0d,
    ));
    m.put(PROG, &prog);
    let vars = PROG + prog.len() as u16;
    let mut v = Vec::new();
    // a = 6
    v.push(b'a');
    v.extend(number_bytes(6.0, true).unwrap());
    // count = -2.5
    v.extend([0xa0 | (b'c' & 0x1f), b'o', b'u', b'n', b't' | 0x80]);
    v.extend(number_bytes(-2.5, true).unwrap());
    // s$ = "A£"
    v.extend([0x40 | (b's' & 0x1f), 2, 0, b'A', 0x60]);
    // i: a loop from line 30's first statement, open
    v.push(0xe0 | (b'i' & 0x1f));
    for n in [2.0, 3.0, 1.0] {
        v.extend(number_bytes(n, true).unwrap());
    }
    v.extend([30, 0, 2]);
    // j: a loop from line 10's third statement that counts down, open
    v.push(0xe0 | (b'j' & 0x1f));
    for n in [0.5, 0.0, -0.5] {
        v.extend(number_bytes(n, true).unwrap());
    }
    v.extend([10, 0, 4]);
    // k: a loop that has ended
    v.push(0xe0 | (b'k' & 0x1f));
    for n in [4.0, 3.0, 1.0] {
        v.extend(number_bytes(n, true).unwrap());
    }
    v.extend([10, 0, 2]);
    // DIM d(2,3)
    v.extend([0x80 | (b'd' & 0x1f), 35, 0, 2, 2, 0, 3, 0]);
    for n in 0..6 {
        v.extend(number_bytes(f64::from(n) * 1.5, true).unwrap());
    }
    // DIM t$(2,2)
    v.extend([0xc0 | (b't' & 0x1f), 9, 0, 2, 2, 0, 2, 0]);
    v.extend(b"HI  ");
    v.push(0x80);
    m.put(vars, &v);
    m.word(SPECTRUM_PROG, PROG);
    m.word(SPECTRUM.vars, vars);
    m.word(SPECTRUM.e_line, vars + v.len() as u16);
    m.word(SPECTRUM.ramtop, RAMTOP);
    // The stack: the end marker, a GOSUB from line 10's second statement,
    // the return into the main loop.
    m.put(RAMTOP - 1, &[0x00, 0x3e]);
    m.put(RAMTOP - 4, &[10, 0, 3]);
    m.word(RAMTOP - 6, 0x1303);
    m.word(SPECTRUM.err_sp, RAMTOP - 6);
    m.ram[usize::from(SPECTRUM.flags)] = 0x80;
    m.word(SPECTRUM.ppc, 100);
    m.ram[usize::from(SPECTRUM_SUBPPC)] = 2;
    m.word(SPECTRUM.ch_add, PROG + 80);
    let basic = Sinclair::spectrum(CPU, ROMS);
    let basic = basic.rescan(&m).expect("first look");
    assert!(basic.rescan(&m).is_none(), "the ROMs are read once");
    (m, basic)
}

#[test]
fn spectrum_lists_as_the_rom_does() {
    let (m, basic) = spectrum_machine(rom_48());
    assert_eq!(basic.info().dialect, "ZX Spectrum BASIC");
    let lines = basic.program(&m);
    let shown: Vec<(u32, &str, &[u32])> = lines
        .iter()
        .map(|l| (l.number, l.text.as_str(), l.statements.as_slice()))
        .collect();
    assert_eq!(
        shown,
        [
            (
                10,
                "LET a=5: GO SUB 100: IF a>1 THEN PRINT \"x:y\": REM a:b",
                // THEN begins a statement; nothing behind REM does.
                &[0, 8, 20, 33, 45][..]
            ),
            (
                20,
                "LET b=RND*PI+VAL \"2\": LET c=a<=b OR NOT c",
                &[0, 21][..]
            ),
            (
                100,
                "PRINT {$10}{$02};\"£█\";CHR$ {$90}: RETURN ",
                &[0, 36][..]
            ),
        ]
    );
    assert_eq!(lines[0].addr, u32::from(PROG));
    assert_eq!(lines[0].raw[..4], [0, 10, lines[0].raw.len() as u8 - 4, 0]);
}

#[test]
fn spectrum_place_mode_and_depth() {
    let (mut m, basic) = spectrum_machine(rom_48());
    let state = basic.state(&m);
    assert_eq!(state.mode, BasicMode::Running);
    assert_eq!(
        state.pos,
        Some(BasicPos {
            line: 100,
            statement: 1
        })
    );
    assert_eq!(state.depth, 1);
    // A command typed in runs as line -2.
    m.word(SPECTRUM.ppc, 0xfffe);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    assert_eq!(basic.state(&m).pos, None);
    // The syntax check passes the same loop with FLAGS bit 7 clear.
    m.word(SPECTRUM.ppc, 100);
    m.ram[usize::from(SPECTRUM.flags)] = 0;
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    m.ram[usize::from(SPECTRUM.flags)] = 0x80;
    // The editor's handler on top: not running, unless INPUT waits.
    m.word(RAMTOP - 20, 0x107f);
    m.word(RAMTOP - 18, RAMTOP - 6);
    m.word(SPECTRUM.err_sp, RAMTOP - 20);
    let state = basic.state(&m);
    assert_eq!((state.mode, state.depth), (BasicMode::Direct, 1));
    m.ram[usize::from(SPECTRUM.flagx)] = 0x20;
    assert_eq!(basic.state(&m).mode, BasicMode::Running);
    // A stack that is not the interpreter's.
    m.word(SPECTRUM.err_sp, 0x8000);
    let state = basic.state(&m);
    assert_eq!((state.mode, state.depth), (BasicMode::Direct, 0));
}

#[test]
fn spectrum_frames_are_gosubs_and_open_loops() {
    let (m, basic) = spectrum_machine(rom_48());
    let at = |line, statement| BasicPos { line, statement };
    assert_eq!(
        basic.stack(&m),
        [
            BasicFrame::Gosub { from: at(10, 1) },
            BasicFrame::For {
                var: "I".into(),
                limit: BasicValue::Float(3.0),
                step: BasicValue::Float(1.0),
                from: at(30, 0),
            },
            BasicFrame::For {
                var: "J".into(),
                limit: BasicValue::Float(0.0),
                step: BasicValue::Float(-0.5),
                from: at(10, 2),
            },
        ]
    );
}

#[test]
fn spectrum_variables_read_and_written() {
    let (mut m, basic) = spectrum_machine(rom_48());
    let vars = basic.variables(&m);
    let shown: Vec<(&str, Option<BasicValue>, &[u32])> = vars
        .iter()
        .map(|v| (v.name.as_str(), v.value.clone(), v.dims.as_slice()))
        .collect();
    let f = |n| Some(BasicValue::Float(n));
    assert_eq!(
        shown,
        [
            ("A", f(6.0), &[][..]),
            ("COUNT", f(-2.5), &[][..]),
            ("S$", Some(BasicValue::Str("A£".into())), &[][..]),
            ("I", f(2.0), &[][..]),
            ("J", f(0.5), &[][..]),
            ("K", f(4.0), &[][..]),
            ("D", None, &[2, 3][..]),
            ("T$", None, &[2, 2][..]),
        ]
    );
    assert_eq!(
        basic.array(&m, "d", 4, 9),
        Ok(vec![BasicValue::Float(6.0), BasicValue::Float(7.5)])
    );
    assert_eq!(
        basic.array(&m, "T$", 0, 2),
        Ok(vec![
            BasicValue::Str("H".into()),
            BasicValue::Str("I".into())
        ])
    );
    assert_eq!(basic.array(&m, "A", 0, 1), Err(BasicError::NoSuchVariable));

    let set = |m: &mut Mem, name: &str, index, value: BasicValue| {
        let writes = basic.plan_set(m, name, index, &value)?;
        m.apply(writes);
        Ok::<(), BasicError>(())
    };
    // A whole number becomes a small integer, a fraction a float.
    set(&mut m, "a", None, BasicValue::Int(-3)).unwrap();
    let a = usize::from(m.ram[usize::from(SPECTRUM.vars)])
        | usize::from(m.ram[usize::from(SPECTRUM.vars) + 1]) << 8;
    assert_eq!(m.ram[a + 1..a + 6], [0, 0xff, 0xfd, 0xff, 0]);
    set(&mut m, "Count", None, BasicValue::Float(0.1)).unwrap();
    set(&mut m, "S$", None, BasicValue::Str("↑©".into())).unwrap();
    set(&mut m, "D", Some(5), BasicValue::Float(1e10)).unwrap();
    set(&mut m, "t$", Some(3), BasicValue::Str("!".into())).unwrap();
    let vars = basic.variables(&m);
    assert_eq!(vars[0].value, Some(BasicValue::Float(-3.0)));
    let Some(BasicValue::Float(count)) = vars[1].value else {
        panic!("{:?}", vars[1]);
    };
    assert!((count - 0.1).abs() < 1e-10, "{count}");
    assert_eq!(vars[2].value, Some(BasicValue::Str("↑©".into())));
    assert_eq!(
        basic.array(&m, "D", 5, 1),
        Ok(vec![BasicValue::Float(1e10)])
    );
    assert_eq!(
        basic.array(&m, "T$", 3, 1),
        Ok(vec![BasicValue::Str("!".into())])
    );
    for (name, index, value, error) in [
        (
            "S$",
            None,
            BasicValue::Str("ABC".into()),
            BasicError::Unsupported,
        ),
        ("S$", None, BasicValue::Int(1), BasicError::TypeMismatch),
        (
            "A",
            None,
            BasicValue::Str("A".into()),
            BasicError::TypeMismatch,
        ),
        ("A", None, BasicValue::Float(1e40), BasicError::OutOfRange),
        (
            "S$",
            None,
            BasicValue::Str("ä`".into()),
            BasicError::OutOfRange,
        ),
        ("D", Some(6), BasicValue::Int(1), BasicError::OutOfRange),
        ("D", None, BasicValue::Int(1), BasicError::NoSuchVariable),
        ("ZZ", None, BasicValue::Int(1), BasicError::NoSuchVariable),
    ] {
        assert_eq!(set(&mut m, name, index, value), Err(error), "{name}");
    }
}

#[test]
fn numbers_as_the_roms_store_them() {
    // From the manual: 0.5 is 80 00 00 00 00, a tenth 7D 4C CC CC CD.
    assert_eq!(number_bytes(0.5, true), Ok([0x80, 0, 0, 0, 0]));
    assert_eq!(number_bytes(0.1, true), Ok([0x7d, 0x4c, 0xcc, 0xcc, 0xcd]));
    assert_eq!(number_bytes(-1.0, false), Ok([0x81, 0x80, 0, 0, 0]));
    assert_eq!(number_bytes(65_535.0, true), Ok([0, 0, 0xff, 0xff, 0]));
    assert_eq!(number_bytes(65_536.0, true), Ok([0x91, 0, 0, 0, 0]));
    // The ZX81 has no small integers; exponent 0 is zero there.
    assert_eq!(number_bytes(3.0, false), Ok([0x82, 0x40, 0, 0, 0]));
    assert_eq!(number([0, 0, 3, 0, 0], false), 0.0);
    assert_eq!(number([0, 0, 3, 0, 0], true), 3.0);
    for v in [
        1.0, -1.0, 0.0, 12_345.678, -1e-20, 1e38, 70_000.0, -65_535.0,
    ] {
        for small in [false, true] {
            let back = number(number_bytes(v, small).unwrap(), small);
            assert!((back - v).abs() <= v.abs() * 1e-9, "{v} {back}");
        }
    }
    assert_eq!(number_bytes(f64::NAN, true), Err(BasicError::OutOfRange));
}

#[test]
fn the_128k_has_a_loop_in_each_rom() {
    // ROM 0: the editor with its own loop (JP Z to the line's end) and
    // the start of a run; ROM 1 is the 48K's.
    let mut rom = vec![0; ROM_BANK];
    rom[0x17c4..0x17c4 + 21].copy_from_slice(&[
        0xfd, 0x34, 0x0d, 0xfa, 0x12, 0x19, 0xdf, 0x06, 0x00, 0xfe, 0x0d, 0xca, 0x63, 0x18, 0xfe,
        0x3a, 0x28, 0xea, 0x21, 0x21, 0x18,
    ]);
    rom[0x0308..0x0308 + 12].copy_from_slice(&[
        0xfd, 0x36, 0x0a, 0x01, 0x21, 0x00, 0x3e, 0xe5, 0x21, 0x1d, 0x5b, 0xe5,
    ]);
    rom.extend(rom_48());
    let (mut m, basic) = spectrum_machine(rom);
    assert_eq!(basic.info().dialect, "ZX Spectrum 128 BASIC");
    let stop = |addr| BasicStopPoint {
        kind: BreakpointKind::Exec { space: ROMS, addr },
        granularity: BasicGranularity::Statement,
    };
    assert_eq!(basic.stop_points(&m), [stop(0x17d6), stop(0x4000 + 0x1b3d)]);
    // Running in 48 BASIC, and in 128 BASIC with its handler in RAM.
    assert_eq!(basic.state(&m).mode, BasicMode::Running);
    m.word(RAMTOP - 6, 0x5b1d);
    assert_eq!(basic.state(&m).mode, BasicMode::Running);
    // SPECTRUM and PLAY take the places of two graphics characters.
    let prog = text(&[&line(10, &[0xa4], 0x0d), &line(20, b"\xa3:\xf5a", 0x0d)]);
    m.put(PROG, &prog);
    m.word(SPECTRUM.vars, PROG + prog.len() as u16);
    let listed: Vec<String> = basic.program(&m).into_iter().map(|l| l.text).collect();
    assert_eq!(listed, ["PLAY ", "SPECTRUM : PRINT a"]);
    // The 48K alone: one stop point, and the codes are graphics.
    let (m, basic) = spectrum_machine(rom_48());
    assert_eq!(basic.stop_points(&m), [stop(0x1b3d)]);
}

#[test]
fn without_the_interpreter_nothing_is_reported() {
    let mut rom = rom_48();
    rom[0x1b2c] = 0;
    let (mut m, basic) = spectrum_machine(rom);
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
    assert!(basic.program(&m).is_empty());
    assert!(basic.variables(&m).is_empty());
    assert!(basic.stack(&m).is_empty());
    assert!(basic.stop_points(&m).is_empty());
    assert_eq!(basic.array(&m, "D", 0, 1), Err(BasicError::Inactive));
    let value = BasicValue::Int(1);
    assert_eq!(
        basic.plan_set(&m, "A", None, &value),
        Err(BasicError::Inactive)
    );
    // Memory that is no program and no variables ends the lists.
    let (_, basic) = spectrum_machine(rom_48());
    m.put(PROG, &[0xff; 64]);
    m.word(SPECTRUM.vars, 0xfff0);
    m.word(SPECTRUM.e_line, 0xffff);
    m.put(0xfff0, &[0xff; 16]);
    assert!(basic.program(&m).is_empty());
    assert!(basic.variables(&m).len() <= 1);
}

const ZX81_WORDS: [&str; 68] = [
    "?", "\"\"", "AT", "TAB", "?", "CODE", "VAL", "LEN", "SIN", "COS", "TAN", "ASN", "ACS", "ATN",
    "LN", "EXP", "INT", "SQR", "SGN", "ABS", "PEEK", "USR", "STR$", "CHR$", "NOT", "**", "OR",
    "AND", "<=", ">=", "<>", "THEN", "TO", "STEP", "LPRINT", "LLIST", "STOP", "SLOW", "FAST",
    "NEW", "SCROLL", "CONT", "DIM", "REM", "FOR", "GOTO", "GOSUB", "INPUT", "LOAD", "LIST", "LET",
    "PAUSE", "NEXT", "POKE", "PRINT", "PLOT", "RUN", "SAVE", "RAND", "IF", "CLS", "UNPLOT",
    "CLEAR", "RETURN", "COPY", "RND", "INKEY$", "PI",
];

fn zx(s: &str) -> Vec<u8> {
    s.chars().map(|c| zx81_code(c).unwrap()).collect()
}

/// A ZX81 with 16 KB, a program of four lines, running line 30 inside a
/// GOSUB from line 20.
fn zx81_machine() -> (Mem, Sinclair) {
    let mut m = Mem {
        ram: vec![0; 0x10000],
        rom: Vec::new(),
    };
    m.put(0x0695, &[0xed, 0x53, 0x07, 0x40]);
    m.put(
        0x0975,
        &[
            0xe5, 0x21, 0x11, 0x01, 0xcb, 0x7f, 0x28, 0x02, 0xe6, 0x3f, 0xfe, 0x43,
        ],
    );
    m.put(0x0111, &table(&ZX81_WORDS, |c| zx81_code(c).unwrap()));
    let mut number = zx("5");
    number.push(0x7e);
    number.extend(number_bytes(5.0, false).unwrap());
    let mut prog = Vec::new();
    // 10 LET A=5
    prog.extend(line(10, &text(&[&[0xf1], &zx("A="), &number]), 0x76));
    // 20 IF A<=RND OR NOT A THEN GOSUB VAL "30"
    prog.extend(line(
        20,
        &text(&[
            &[0xfa],
            &zx("A"),
            &[0xdb, 0x40, 0xd9, 0xd7],
            &zx("A"),
            &[0xde, 0xed, 0xc5],
            &zx("\"30\""),
        ]),
        0x76,
    ));
    // 30 PRINT "A B";A**2, with an inverse B
    prog.extend(line(
        30,
        &text(&[
            &[0xf5],
            &zx("\"A "),
            &[0xa7],
            &zx("\";A"),
            &[0xd8],
            &zx("2"),
        ]),
        0x76,
    ));
    // 40 RETURN
    prog.extend(line(40, &[0xfe], 0x76));
    m.put(ZX81_PROG, &prog);
    let d_file = ZX81_PROG + prog.len() as u16;
    m.word(ZX81_D_FILE, d_file);
    let vars = d_file + 25;
    let mut v = Vec::new();
    // A = 5, NO2 = -0.25, A$ = "£1", a loop I from line 50, DIM B(2)
    v.push(0x66);
    v.extend(number_bytes(5.0, false).unwrap());
    v.extend([0xa0 | 0x33 & 0x1f, 0x34, 0x1e | 0x80]);
    v.extend(number_bytes(-0.25, false).unwrap());
    v.extend([0x46, 2, 0, 0x0c, 0x1d]);
    v.push(0xe0 | 0x2e & 0x1f);
    for n in [1.0, 3.0, 1.0] {
        v.extend(number_bytes(n, false).unwrap());
    }
    v.extend(51u16.to_le_bytes());
    v.extend([0x80 | 0x27 & 0x1f, 13, 0, 1, 2, 0]);
    for n in [1.5, 0.0] {
        v.extend(number_bytes(n, false).unwrap());
    }
    v.push(0x80);
    m.put(vars, &v);
    m.word(ZX81.vars, vars);
    m.word(ZX81.e_line, vars + v.len() as u16);
    m.word(ZX81.ramtop, 0x8000);
    // The stack: the end marker, the line behind the GOSUB, the return
    // address into the line loop.
    m.put(0x7ffe, &[0x00, 0x3e]);
    m.word(0x7ffc, 21);
    m.word(0x7ffa, 0x0676);
    m.word(ZX81.err_sp, 0x7ffa);
    m.ram[usize::from(ZX81.flags)] = 0x80;
    m.word(ZX81.ppc, 30);
    m.ram[usize::from(ZX81_X_PTR) + 1] = 0xc0;
    let basic = Sinclair::zx81(CPU).rescan(&m).expect("first look");
    (m, basic)
}

#[test]
fn zx81_lists_as_the_rom_does() {
    let (m, basic) = zx81_machine();
    assert_eq!(basic.info().dialect, "Sinclair ZX81 BASIC");
    let lines = basic.program(&m);
    let listed: Vec<(u32, &str)> = lines.iter().map(|l| (l.number, l.text.as_str())).collect();
    assert_eq!(
        listed,
        [
            (10, "LET A=5"),
            // No blank before <= and RND, one before OR; NOT has none.
            (20, "IF A<=RND OR NOT A THEN GOSUB VAL \"30\""),
            (30, "PRINT \"A {$a7}\";A**2"),
            (40, "RETURN "),
        ]
    );
    assert!(lines.iter().all(|l| l.statements == [0]));
    assert_eq!(lines[0].addr, u32::from(ZX81_PROG));
    assert_eq!(
        basic.stop_points(&m),
        [BasicStopPoint {
            kind: BreakpointKind::Exec {
                space: CPU,
                addr: 0x0699
            },
            granularity: BasicGranularity::Statement,
        }]
    );
}

#[test]
fn zx81_place_stack_and_variables() {
    let (mut m, basic) = zx81_machine();
    let at = |line| BasicPos { line, statement: 0 };
    let state = basic.state(&m);
    assert_eq!(
        (state.mode, state.pos, state.depth),
        (BasicMode::Running, Some(at(30)), 1)
    );
    assert_eq!(
        basic.stack(&m),
        [
            BasicFrame::Gosub { from: at(20) },
            BasicFrame::For {
                var: "I".into(),
                limit: BasicValue::Float(3.0),
                step: BasicValue::Float(1.0),
                from: at(50),
            },
        ]
    );
    let vars = basic.variables(&m);
    let shown: Vec<(&str, Option<BasicValue>)> = vars
        .iter()
        .map(|v| (v.name.as_str(), v.value.clone()))
        .collect();
    let f = |n| Some(BasicValue::Float(n));
    assert_eq!(
        shown,
        [
            ("A", f(5.0)),
            ("NO2", f(-0.25)),
            ("A$", Some(BasicValue::Str("£1".into()))),
            ("I", f(1.0)),
            ("B", None),
        ]
    );
    assert_eq!(vars[4].dims, [2]);
    assert_eq!(
        basic.array(&m, "B", 0, 5),
        Ok(vec![f(1.5).unwrap(), f(0.0).unwrap()])
    );
    for (name, index, value) in [
        ("no 2", None, BasicValue::Int(7)),
        ("A$", None, BasicValue::Str("Z9".into())),
        ("B", Some(1), BasicValue::Float(-0.5)),
    ] {
        let writes = basic.plan_set(&m, name, index, &value).unwrap();
        m.apply(writes);
    }
    let vars = basic.variables(&m);
    assert_eq!(vars[1].value, f(7.0));
    assert_eq!(vars[2].value, Some(BasicValue::Str("Z9".into())));
    assert_eq!(basic.array(&m, "B", 1, 1), Ok(vec![f(-0.5).unwrap()]));
    // Lower case is not in the character set.
    let lower = BasicValue::Str("ab".into());
    assert_eq!(
        basic.plan_set(&m, "A$", None, &lower),
        Err(BasicError::OutOfRange)
    );
    // The editor clears X_PTR; a command typed in is line -2.
    m.ram[usize::from(ZX81_X_PTR) + 1] = 0;
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    m.ram[usize::from(ZX81_X_PTR) + 1] = 0xc0;
    m.word(ZX81.ppc, 0xfffe);
    let state = basic.state(&m);
    assert_eq!(
        (state.mode, state.pos, state.depth),
        (BasicMode::Direct, None, 1)
    );
    // Without the line loop: no interpreter.
    m.put(0x0695, &[0; 4]);
    let basic = Sinclair::zx81(CPU).rescan(&m).unwrap();
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
}
