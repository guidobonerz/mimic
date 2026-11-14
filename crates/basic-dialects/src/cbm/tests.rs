//! The decoder against a hand-built memory image laid out like a C64's:
//! the two ROM patterns at `$A000`, a tokenised program at `$0801`,
//! variables, an array, a string in the heap and two frames on the stack.

use emu_core::cpu::{Flow, Instr, Register, RegisterSet};

use super::*;

const SPACE: MemSpaceId = MemSpaceId(0);

/// BASIC V2's keywords in token order (`$80` END … `$CB` GO).
const KEYWORDS: &[&str] = &[
    "END", "FOR", "NEXT", "DATA", "INPUT#", "INPUT", "DIM", "READ", "LET", "GOTO", "RUN", "IF",
    "RESTORE", "GOSUB", "RETURN", "REM", "STOP", "ON", "WAIT", "LOAD", "SAVE", "VERIFY", "DEF",
    "POKE", "PRINT#", "PRINT", "CONT", "LIST", "CLR", "CMD", "SYS", "OPEN", "CLOSE", "GET", "NEW",
    "TAB(", "TO", "FN", "SPC(", "THEN", "NOT", "STEP", "+", "-", "*", "/", "^", "AND", "OR", ">",
    "=", "<", "SGN", "INT", "ABS", "USR", "FRE", "POS", "SQR", "RND", "LOG", "EXP", "COS", "SIN",
    "TAN", "ATN", "PEEK", "LEN", "STR$", "VAL", "ASC", "CHR$", "LEFT$", "RIGHT$", "MID$", "GO",
];

struct Mem {
    ram: Vec<u8>,
    sp: u8,
    rom_on: bool,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        RegisterSet(vec![Register {
            name: "S",
            value: u32::from(self.sp),
            bits: 8,
        }])
    }

    fn set_reg(&mut self, _name: &str, _v: u32) {}

    fn peek(&self, _space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
        let rom = (0xa000..0xc000).contains(&addr);
        Some(if rom && !self.rom_on {
            0
        } else {
            u32::from(self.ram[addr as usize & 0xffff])
        })
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
    fn put(&mut self, addr: usize, bytes: &[u8]) {
        self.ram[addr..addr + bytes.len()].copy_from_slice(bytes);
    }

    fn word(&mut self, addr: usize, v: u16) {
        self.put(addr, &v.to_le_bytes());
    }
}

fn token(word: &str) -> u8 {
    0x80 + KEYWORDS.iter().position(|k| *k == word).unwrap() as u8
}

/// Tokenise a line written with `{WORD}` for tokens.
fn tokens(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '{' {
            let end = rest.find('}').unwrap();
            out.push(token(&rest[1..end]));
            rest = &rest[end + 1..];
        } else {
            out.push(c as u8);
            rest = &rest[1..];
        }
    }
    out
}

/// ```text
/// 10 FOR I=1 TO 3:PRINT "A:B":NEXT
/// 20 GOSUB 100:REM X:Y
/// 100 RETURN
/// ```
fn machine() -> Mem {
    let mut m = Mem {
        ram: vec![0; 0x10000],
        sp: 0xff,
        rom_on: true,
    };
    // ROM: the keyword table and the statement loop behind JMP (IGONE).
    let mut table = Vec::new();
    for k in KEYWORDS {
        table.extend(k.bytes());
        *table.last_mut().unwrap() |= 0x80;
    }
    table.push(0);
    m.put(0xa09e, &table);
    m.put(0xa7ce, &[0xa0, 0x00, 0xb1, 0x7a]);
    m.put(
        0xa7e1,
        &[
            0x6c, 0x08, 0x03, 0x20, 0x73, 0x00, 0x20, 0xed, 0xa7, 0x4c, 0xae, 0xa7,
        ],
    );
    // The program.
    let mut at = 0x0801;
    for (number, text) in [
        (10u16, "{FOR}I{=}1{TO}3:{PRINT}\"A:B\":{NEXT}"),
        (20, "{GOSUB}100:{REM}X:Y"),
        (100, "{RETURN}"),
    ] {
        let body = tokens(text);
        let next = at + 4 + body.len() + 1;
        m.word(at, next as u16);
        m.word(at + 2, number);
        m.put(at + 4, &body);
        at = next;
    }
    let vartab = at + 2;
    // Variables: I = 2, N% = -5, S$ = "HEY" in the heap, T$ a literal.
    m.put(vartab, &[b'I', 0, 0x82, 0, 0, 0, 0]);
    m.put(vartab + 7, &[b'N' | 0x80, 0x80, 0xff, 0xfb, 0, 0, 0]);
    m.put(vartab + 14, &[b'S', 0x80, 3, 0xfd, 0x9f, 0, 0]);
    m.put(vartab + 21, &[b'T', 0x80, 1, 0x0e, 0x08, 0, 0]);
    m.put(0x9ffd, b"HEY");
    let arytab = vartab + 28;
    // DIM AB(1,2): sizes stored last dimension first; 6 floats.
    m.put(arytab, &[b'A', b'B', 39, 0, 2, 0, 3, 0, 2]);
    m.put(arytab + 9 + 5 * 4, &[0x84, 0x20, 0, 0, 0]);
    let strend = arytab + 39;
    for (zp, v) in [
        (0x2b, 0x0801),
        (0x2d, vartab),
        (0x2f, arytab),
        (0x31, strend),
        (0x33, 0x9ffd),
        (0x37, 0xa000),
    ] {
        m.word(zp, v as u16);
    }
    m
}

/// Where line `n` is stored.
fn line_addr(m: &Mem, n: u32) -> u16 {
    let basic = decoder(m);
    let lines = basic.program(m);
    lines.iter().find(|l| l.number == n).unwrap().addr as u16
}

fn decoder(m: &Mem) -> CbmBasic {
    CbmBasic::new(SPACE).rescan(m).expect("BASIC found")
}

#[test]
fn finds_the_interpreter_and_reads_its_keywords() {
    let m = machine();
    let basic = decoder(&m);
    let rom = basic.rom.as_ref().unwrap();
    assert_eq!(
        (rom.dispatch, rom.chrget, rom.keywords_at),
        (0xa7e1, 0x73, 0xa09e)
    );
    assert_eq!(rom.keywords.len(), KEYWORDS.len());
    assert_eq!(basic.info().dialect, "Commodore BASIC V2");
    assert_eq!(
        basic.stop_points(&m),
        [BasicStopPoint {
            kind: BreakpointKind::Exec {
                space: SPACE,
                addr: 0xa7e1
            },
            granularity: BasicGranularity::Statement,
        }]
    );
    // Nothing changed: no new decoder.
    assert!(basic.rescan(&m).is_none());
}

#[test]
fn a_loop_without_the_vector_stops_at_its_jsr() {
    let mut m = machine();
    m.put(0xa7e1, &[0xea, 0xea, 0xea]);
    m.put(0xa7e4, &[0x20, 0x21, 0xbf]);
    m.put(0xa7d1, &[0x77]);
    let basic = decoder(&m);
    let rom = basic.rom.as_ref().unwrap();
    assert_eq!((rom.dispatch, rom.chrget), (0xa7e4, 0x70));
}

#[test]
fn banked_out_means_inactive() {
    let mut m = machine();
    let basic = decoder(&m);
    m.rom_on = false;
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
    assert!(basic.program(&m).is_empty());
    assert!(basic.variables(&m).is_empty());
    assert!(basic.stop_points(&m).is_empty());
    assert_eq!(basic.array(&m, "AB", 0, 1), Err(BasicError::Inactive));
    // The decoder to keep is one without an interpreter; then nothing new.
    let gone = basic.rescan(&m).expect("changed");
    assert_eq!(gone.info().dialect, "Commodore BASIC");
    assert!(gone.rescan(&m).is_none());
    m.rom_on = true;
    assert!(gone.rescan(&m).is_some());
}

#[test]
fn lists_lines_with_their_statements() {
    let m = machine();
    let p = decoder(&m).program(&m);
    let numbers: Vec<u32> = p.iter().map(|l| l.number).collect();
    assert_eq!(numbers, [10, 20, 100]);
    assert_eq!(p[0].text, "FORI=1TO3:PRINT\"A:B\":NEXT");
    // The colon inside the string does not split.
    assert_eq!(p[0].statements, [0, 10, 21]);
    // Nor do colons behind REM.
    assert_eq!(p[1].text, "GOSUB100:REMX:Y");
    assert_eq!(p[1].statements, [0, 9]);
    assert_eq!(p[0].addr, 0x0801);
    assert_eq!(p[0].raw.len(), (p[1].addr - p[0].addr) as usize);
    assert_eq!(p[0].raw.last(), Some(&0));
}

#[test]
fn position_from_curlin_and_the_text_pointer() {
    let mut m = machine();
    let basic = decoder(&m);
    let l10 = line_addr(&m, 10);
    let at = |m: &mut Mem, curlin: u16, txt: u16| {
        m.word(0x39, curlin);
        m.word(0x7a, txt);
    };
    // Direct mode.
    at(&mut m, 0xff00, 0x0200);
    let s = basic.state(&m);
    assert_eq!((s.mode, s.pos, s.depth), (BasicMode::Direct, None, 0));
    // The stop point of a line's first statement: on the number's high byte.
    at(&mut m, 10, l10 + 3);
    let pos = |m: &Mem| basic.state(m).pos.unwrap();
    assert_eq!((pos(&m).line, pos(&m).statement), (10, 0));
    // On the first colon (the next statement's stop point), then inside
    // the string, on its colon, and on the colon behind it.
    let colon = l10 + 4 + 6;
    at(&mut m, 10, colon);
    assert_eq!(pos(&m).statement, 1);
    at(&mut m, 10, colon + 4);
    assert_eq!(pos(&m).statement, 1);
    at(&mut m, 10, colon + 7);
    assert_eq!(pos(&m).statement, 2);
    assert_eq!(basic.state(&m).text_addr, Some(u32::from(colon + 7)));
    // A text pointer outside the line CURLIN names (after a cold start,
    // or while INPUT reads its buffer): not running.
    at(&mut m, 20, 0x0200);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    at(&mut m, 0, 0x0800);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
}

#[test]
fn variables_arrays_and_writing_them() {
    let mut m = machine();
    let basic = decoder(&m);
    let vars = basic.variables(&m);
    let seen: Vec<(&str, BasicType, Option<BasicValue>)> = vars
        .iter()
        .map(|v| (v.name.as_str(), v.ty, v.value.clone()))
        .collect();
    assert_eq!(
        seen,
        [
            ("I", BasicType::Float, Some(BasicValue::Float(2.0))),
            ("N%", BasicType::Int, Some(BasicValue::Int(-5))),
            ("S$", BasicType::Str, Some(BasicValue::Str("HEY".into()))),
            ("T$", BasicType::Str, Some(BasicValue::Str("A".into()))),
            ("AB", BasicType::Float, None),
        ]
    );
    // DIM AB(1,2) in source order.
    assert_eq!(vars[4].dims, [2, 3]);
    assert_eq!(
        basic.array(&m, "AB", 3, 9).unwrap(),
        [
            BasicValue::Float(0.0),
            BasicValue::Float(10.0),
            BasicValue::Float(0.0)
        ]
    );
    assert_eq!(basic.array(&m, "AB", 6, 1), Err(BasicError::OutOfRange));
    assert_eq!(basic.array(&m, "I", 0, 1), Err(BasicError::NoSuchVariable));

    let set = |m: &mut Mem, name: &str, index, value: BasicValue| {
        let writes = basic.plan_set(m, name, index, &value)?;
        for (addr, b) in writes {
            m.poke(SPACE, addr, Size::Byte, u32::from(b));
        }
        Ok::<(), BasicError>(())
    };
    set(&mut m, "I", None, BasicValue::Float(-0.75)).unwrap();
    set(&mut m, "N%", None, BasicValue::Int(300)).unwrap();
    set(&mut m, "S$", None, BasicValue::Str("YOU".into())).unwrap();
    set(&mut m, "AB", Some(5), BasicValue::Int(7)).unwrap();
    let value = |m: &Mem, name: &str| {
        basic
            .variables(m)
            .into_iter()
            .find(|v| v.name == name)
            .unwrap()
            .value
            .unwrap()
    };
    assert_eq!(value(&m, "I"), BasicValue::Float(-0.75));
    assert_eq!(value(&m, "N%"), BasicValue::Int(300));
    assert_eq!(value(&m, "S$"), BasicValue::Str("YOU".into()));
    assert_eq!(
        basic.array(&m, "AB", 5, 1).unwrap(),
        [BasicValue::Float(7.0)]
    );
    for (name, index, value, error) in [
        ("N%", None, BasicValue::Int(40_000), BasicError::OutOfRange),
        ("N%", None, BasicValue::Float(1.5), BasicError::OutOfRange),
        (
            "I",
            None,
            BasicValue::Str("1".into()),
            BasicError::TypeMismatch,
        ),
        ("S$", None, BasicValue::Int(1), BasicError::TypeMismatch),
        // Another length, a literal in the program, a character PETSCII
        // lacks.
        (
            "S$",
            None,
            BasicValue::Str("LONGER".into()),
            BasicError::Unsupported,
        ),
        (
            "T$",
            None,
            BasicValue::Str("X".into()),
            BasicError::Unsupported,
        ),
        (
            "S$",
            None,
            BasicValue::Str("a~b".into()),
            BasicError::OutOfRange,
        ),
        ("Q", None, BasicValue::Int(1), BasicError::NoSuchVariable),
        ("AB", Some(6), BasicValue::Int(1), BasicError::OutOfRange),
        ("I", None, BasicValue::Float(1e39), BasicError::OutOfRange),
    ] {
        assert_eq!(set(&mut m, name, index, value), Err(error), "{name}");
    }
}

#[test]
fn floats_convert_both_ways() {
    for (v, bytes) in [
        (1.0, [0x81, 0, 0, 0, 0]),
        (-1.0, [0x81, 0x80, 0, 0, 0]),
        (10.0, [0x84, 0x20, 0, 0, 0]),
        (0.5, [0x80, 0, 0, 0, 0]),
        (0.0, [0, 0, 0, 0, 0]),
        (-32768.0, [0x90, 0x80, 0, 0, 0]),
    ] {
        assert_eq!(float_bytes(v).unwrap(), bytes, "{v}");
        assert_eq!(float_value(bytes, true), v);
    }
    // π as the ROM stores it.
    let pi = [0x82, 0x49, 0x0f, 0xda, 0xa2];
    assert!((float_value(pi, true) - std::f64::consts::PI).abs() < 1e-9);
    assert_eq!(float_bytes(std::f64::consts::PI).unwrap(), pi);
    for v in [0.1, 123456.789, -1e-20, 1.7e38, 255.999_999_99] {
        let back = float_value(float_bytes(v).unwrap(), true);
        assert!((back - v).abs() <= v.abs() * 1e-9, "{v} -> {back}");
    }
}

#[test]
fn frames_on_the_cpu_stack() {
    let mut m = machine();
    let basic = decoder(&m);
    let (l10, l20) = (line_addr(&m, 10), line_addr(&m, 20));
    let vartab = u16::from_le_bytes([m.ram[0x2d], m.ram[0x2e]]);
    // Outer: FOR I … TO 3 STEP 1 in line 10, saved pointer on the colon
    // that ends the FOR. Inner: GOSUB in line 20, pointer behind its token.
    let mut frame = vec![0x81];
    frame.extend((vartab + 2).to_le_bytes());
    frame.extend([0x81, 0x80, 0, 0, 0, 0x01]);
    frame.extend([0x82, 0x40, 0, 0, 0]);
    frame.extend(10u16.to_le_bytes());
    frame.extend((l10 + 4 + 6).to_le_bytes());
    m.put(0x1fb - 18, &frame);
    let mut gosub = vec![0x8d];
    gosub.extend(20u16.to_le_bytes());
    gosub.extend((l20 + 5).to_le_bytes());
    m.put(0x1fb - 23, &gosub);
    let expected = [
        BasicFrame::Gosub {
            from: BasicPos {
                line: 20,
                statement: 0,
            },
        },
        BasicFrame::For {
            var: "I".into(),
            limit: BasicValue::Float(3.0),
            step: BasicValue::Float(1.0),
            from: BasicPos {
                line: 10,
                statement: 0,
            },
        },
    ];
    // At the stop point the frames begin right above the stack pointer.
    m.sp = (0x1fb - 23 - 0x101) as u8;
    assert_eq!(basic.stack(&m), expected);
    assert_eq!(basic.state(&m).depth, 1);
    // Inside a statement: return addresses above them.
    m.sp -= 6;
    m.put(
        0x101 + usize::from(m.sp),
        &[0x12, 0xa8, 0xe9, 0xa7, 0x33, 0xb1],
    );
    assert_eq!(basic.stack(&m), expected);
    // An empty stack, and one that is no chain of frames at all.
    m.sp = 0xf9;
    assert!(basic.stack(&m).is_empty());
    m.sp = 0x40;
    assert!(basic.stack(&m).is_empty());
}

#[test]
fn garbage_does_not_panic() {
    let mut m = machine();
    let basic = decoder(&m);
    for seed in 0..8u32 {
        for a in 0..0xa000usize {
            m.ram[a] = (a as u32)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(seed * 977) as u8;
        }
        m.sp = (seed * 37) as u8;
        basic.state(&m);
        basic.program(&m);
        basic.variables(&m);
        basic.stack(&m);
        let _ = basic.array(&m, "A", 0, 100);
        let _ = basic.plan_set(&m, "A", Some(3), &BasicValue::Int(1));
    }
}
