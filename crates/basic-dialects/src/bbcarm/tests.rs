//! The decoder against a hand-built image: the statement loop and the
//! keyword table in the ROM, the workspace below R8, a tokenised program,
//! the variable chains and frames on the stack.

use mimic_core::cpu::{Flow, Instr, Register, RegisterSet};

use super::*;

const CPU: MemSpaceId = MemSpaceId(0);

/// Sparse memory: the application space from 0 and a piece of the ROM.
struct Mem {
    ram: Vec<u8>,
    rom: Vec<u8>,
    regs: [(&'static str, u32); 6],
}

const ROM_AT: u32 = ROM_START + 0x0b_2000;

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        RegisterSet(
            self.regs
                .iter()
                .map(|&(name, value)| Register {
                    name,
                    value,
                    bits: 32,
                })
                .collect(),
        )
    }

    fn set_reg(&mut self, _name: &str, _v: u32) {}

    fn peek(&self, _space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> u32 {
            if a >= ROM_AT {
                u32::from(self.rom.get((a - ROM_AT) as usize).copied().unwrap_or(0))
            } else {
                u32::from(self.ram.get(a as usize).copied().unwrap_or(0))
            }
        };
        Some((0..size.bytes()).fold(0, |v, i| v | byte(addr + i) << (8 * i)))
    }

    fn poke(&mut self, _space: MemSpaceId, addr: u32, _size: Size, v: u32) -> bool {
        self.ram[addr as usize] = v as u8;
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
    fn put(&mut self, addr: u32, bytes: &[u8]) {
        let at = addr as usize;
        self.ram[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn word(&mut self, addr: u32, v: u32) {
        self.put(addr, &v.to_le_bytes());
    }

    fn words(&mut self, addr: u32, v: &[u32]) {
        for (i, w) in v.iter().enumerate() {
            self.word(addr + 4 * i as u32, *w);
        }
    }

    fn reg(&mut self, name: &str, v: u32) {
        self.regs.iter_mut().find(|r| r.0 == name).unwrap().1 = v;
    }

    fn apply(&mut self, writes: Vec<(u32, u8)>) {
        for (a, b) in writes {
            self.ram[a as usize] = b;
        }
    }
}

const R8: u32 = 0x8700;
const PROGRAM: u32 = 0x8f00;
const HEAP: u32 = 0x9000;
const STACK_TOP: u32 = 0xa000;
const STOP: u32 = ROM_AT + 0x54;

const T_GOSUB: u8 = 0xe4;
const T_FOR: u8 = 0xe3;
const T_TO: u8 = 0xb8;
const T_NEXT: u8 = 0xed;
const T_UNTIL: u8 = 0xfd;
const T_PRINT: u8 = 0xf1;
const T_IF: u8 = 0xe7;

fn line(number: u16, text: &[u8]) -> Vec<u8> {
    let mut out = vec![
        0x0d,
        (number >> 8) as u8,
        number as u8,
        text.len() as u8 + 4,
    ];
    out.extend(text);
    out
}

fn target(n: u16) -> [u8; 4] {
    let (low, high) = (n as u8, (n >> 8) as u8);
    [
        T_LINE,
        ((low & 0xc0) >> 2 | (high & 0xc0) >> 4) ^ 0x54,
        low & 0x3f | 0x40,
        high & 0x3f | 0x40,
    ]
}

/// ```text
/// 10 A=5:GOSUB100
/// 20 FORI=1TO3:NEXT
/// 30  REPEAT: UNTILA:REM a:b
/// 40 IFA THEN PRINT"x:y" ELSE 100
/// 100 WHILEA:WAIT:PTR#1=SUM(A)
/// ```
fn image() -> (Mem, BbcBasicV, Vec<u32>) {
    let mut m = Mem {
        ram: vec![0; 0xa100],
        rom: vec![0; 0x2000],
        regs: [
            ("R8", R8),
            ("R10", 0),
            ("R12", 0x8500),
            ("R13", STACK_TOP),
            ("PC", 0),
            ("R11", 0),
        ],
    };
    for (i, w) in LOOP.iter().enumerate() {
        let at = 0x50 + 4 * i;
        m.rom[at..at + 4].copy_from_slice(&w.to_le_bytes());
    }
    let mut table = Vec::new();
    for (word, token, flags) in [
        ("AND", 0x80u8, 2u8),
        ("ELSE", T_ELSE, 0x14),
        ("FOR", T_FOR, 2),
        ("GOSUB", T_GOSUB, 0x12),
        ("IF", T_IF, 2),
        ("NEXT", T_NEXT, 2),
        ("PRINT", T_PRINT, 2),
        ("PTR", 0x8f, 0x43),
        ("REPEAT", T_REPEAT, 0),
        ("REM", T_REM, 0x20),
        ("SUM", 0x8e, 0x0e),
        ("THEN", T_THEN, 0x14),
        ("TO", T_TO, 0),
        ("UNTIL", T_UNTIL, 2),
        ("WHILE", 0x95, 0x4a),
        ("WAIT", 0x96, 0x49),
        ("AUTO", 0x8f, 0x18),
    ] {
        table.extend(word.as_bytes());
        table.extend([token, flags]);
    }
    table.extend(b" unlistable token ");
    m.rom[0x1000..0x1000 + table.len()].copy_from_slice(&table);

    let mut addrs = Vec::new();
    let mut program = Vec::new();
    let mut add = |program: &mut Vec<u8>, number: u16, text: &[u8]| {
        addrs.push(PROGRAM + program.len() as u32);
        program.extend(line(number, text));
    };
    let mut l10 = b"A=5:".to_vec();
    l10.push(T_GOSUB);
    l10.extend(target(100));
    add(&mut program, 10, &l10);
    add(
        &mut program,
        20,
        &[T_FOR, b'I', b'=', b'1', T_TO, b'3', b':', T_NEXT],
    );
    let mut l30 = vec![b' ', T_REPEAT, b':', b' ', T_UNTIL, b'A', b':', T_REM];
    l30.extend(b" a:b");
    add(&mut program, 30, &l30);
    let mut l40 = vec![T_IF, b'A', b' ', T_THEN, b' ', T_PRINT];
    l40.extend(b"\"x:y\" ");
    l40.push(T_ELSE);
    l40.push(b' ');
    l40.extend(target(100));
    add(&mut program, 40, &l40);
    let mut l100 = vec![0xc8, 0x95, b'A', b':', 0xc8, 0x96, b':', 0xcf];
    l100.extend(b"#1=");
    l100.extend([0xc6, 0x8e]);
    l100.extend(b"(A)");
    add(&mut program, 100, &l100);
    program.extend([0x0d, 0xff]);
    m.put(PROGRAM, &program);
    m.word(R8 - PAGE, PROGRAM);
    m.word(R8 - TOP, PROGRAM + program.len() as u32);
    m.word(R8 - LOMEM, HEAP);
    m.word(R8 - VARS_END, HEAP + 0x200);
    m.word(R8 - HIMEM, STACK_TOP);

    // A (real, 5), the string A$, the array AB%(1,2); I; LONG% (an
    // aligned integer); the resident B%.
    let mut a = (HEAP + 0x10).to_le_bytes().to_vec();
    a.push(0);
    a.extend(real_bytes(5.0).unwrap());
    m.put(HEAP, &a);
    let mut s = (HEAP + 0x20).to_le_bytes().to_vec();
    s.extend([b'$', 0]);
    s.extend((HEAP + 0x100).to_le_bytes());
    s.push(2);
    m.put(HEAP + 0x10, &s);
    m.put(HEAP + 0x100, b"HI");
    m.put(HEAP + 0x20, &[0, 0, 0, 0, b'B', b'%', b'(', 0]);
    m.word(HEAP + 0x28, HEAP + 0x30);
    m.words(HEAP + 0x30, &[2, 3, 0, 6, 0, 0, 0, 0, 0, -7i32 as u32]);
    m.word(R8 - HEADS + 4, HEAP);
    let mut i = vec![0, 0, 0, 0, 0];
    i.extend(real_bytes(1.0).unwrap());
    m.put(HEAP + 0x60, &i);
    m.word(R8 - HEADS + 4 * 9, HEAP + 0x60);
    m.put(HEAP + 0x70, &[0, 0, 0, 0, b'O', b'N', b'G', b'%', 0]);
    m.word(HEAP + 0x7c, -123_456i32 as u32);
    m.word(R8 - HEADS + 4 * 12, HEAP + 0x70);
    m.word(R8 - RESIDENT + 4 * 2, 300);

    let basic = BbcBasicV::new(CPU).rescan(&m).expect("looked at");
    (m, basic, addrs)
}

fn pos(line: u32, statement: u16) -> BasicPos {
    BasicPos { line, statement }
}

#[test]
fn the_rom_is_found_once() {
    let (mut m, basic, _) = image();
    let rom = basic.rom.as_ref().unwrap();
    assert_eq!(rom.stop, STOP);
    assert_eq!(rom.keywords[usize::from(T_GOSUB)], b"GOSUB");
    // PTR as a statement has the token + &40.
    assert_eq!(rom.keywords[0xcf], b"PTR");
    assert!(rom.long.contains(&(0xc8, 0x95, b"WHILE".to_vec())));
    assert!(rom.long.contains(&(0xc6, 0x8e, b"SUM".to_vec())));
    assert!(rom.long.contains(&(0xc7, 0x8f, b"AUTO".to_vec())));
    assert_eq!(basic.stop_points(&m).len(), 1);
    assert!(basic.rescan(&m).is_none());
    // Another program has the registers: not BASIC's workspace.
    m.reg("R8", 0x1234);
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
    assert!(basic.program(&m).is_empty());
    // A ROM without the interpreter: looked at once, nothing found.
    let empty = Mem {
        ram: vec![0; 0x100],
        rom: vec![0; 0x100],
        regs: m.regs,
    };
    let none = BbcBasicV::new(CPU).rescan(&empty).unwrap();
    assert!(none.rom.is_none() && none.rescan(&empty).is_none());
    assert!(none.stop_points(&empty).is_empty());
}

#[test]
fn the_listing_and_its_statements() {
    let (m, basic, addrs) = image();
    let lines = basic.program(&m);
    let texts: Vec<(u32, &str, &[u32])> = lines
        .iter()
        .map(|l| (l.number, l.text.as_str(), l.statements.as_slice()))
        .collect();
    assert_eq!(
        texts,
        [
            (10, "A=5:GOSUB100", &[0, 4][..]),
            (20, "FORI=1TO3:NEXT", &[0, 10]),
            // The colon behind REPEAT begins the statement; REM takes
            // the rest.
            (30, " REPEAT: UNTILA:REM a:b", &[0, 8, 16]),
            (40, "IFA THEN PRINT\"x:y\" ELSE 100", &[0, 8]),
            // Tokens of two bytes; PTR as a statement.
            (100, "WHILEA:WAIT:PTR#1=SUM(A)", &[0, 7, 12]),
        ]
    );
    assert_eq!(lines[1].addr, addrs[1]);
}

#[test]
fn the_place_is_where_r12_points() {
    let (mut m, basic, addrs) = image();
    // A typed command: the pointer is not in the program.
    let state = basic.state(&m);
    assert_eq!((state.mode, state.pos), (BasicMode::Direct, None));
    // At the stop point of line 10's second statement: R12 behind its
    // first byte.
    m.reg("PC", STOP);
    m.reg("R10", u32::from(T_GOSUB));
    m.reg("R12", addrs[0] + 9);
    let state = basic.state(&m);
    assert_eq!(
        (state.mode, state.pos),
        (BasicMode::Running, Some(pos(10, 1)))
    );
    assert_eq!(state.text_addr, Some(addrs[0] + 8));
    // At the stop point on a blank, a colon or the line's end: between
    // two statements.
    for c in [b' ', b':', 0x0d] {
        m.reg("R10", u32::from(c));
        let state = basic.state(&m);
        assert_eq!((state.mode, state.pos), (BasicMode::Running, None));
    }
    // Elsewhere the same registers are a place.
    m.reg("PC", STOP + 0x100);
    assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
    // The line's end belongs to the line.
    m.reg("R12", addrs[1] + 1);
    assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
    m.reg("R12", addrs[1] + 5);
    assert_eq!(basic.state(&m).pos, Some(pos(20, 0)));
}

#[test]
fn frames_are_found_by_their_shape() {
    let (mut m, basic, addrs) = image();
    assert!(basic.stack(&m).is_empty());
    // From the top down: GOSUB in line 10; a function called in line 40
    // with a saved variable; the real FOR of line 20; an integer FOR on
    // J%; WHILE of line 100; REPEAT of line 30; and below, values of an
    // expression under way, one of them looking like a mark.
    let i_value = HEAP + 0x65;
    let j_value = R8 - RESIDENT + 4 * 10;
    let frames: Vec<u32> = [
        vec![0x1234, F_GOSUB, 0x5678],
        vec![F_REPEAT, addrs[2] + 8],
        vec![F_WHILE, addrs[4] + 8, addrs[4] + 7],
        vec![F_FOR_INT, j_value, addrs[1] + 11, 2, 9],
        vec![
            F_FOR_REAL,
            i_value,
            addrs[1] + 11,
            0x8000_0000,
            0x81,
            0xc000_0000,
            0x8000_0082,
        ],
        vec![F_FN, addrs[3] + 9, addrs[3] + 5, HEAP + 5, 5, 0],
        vec![F_GOSUB, addrs[0] + 13],
    ]
    .concat();
    let at = STACK_TOP - 4 * frames.len() as u32;
    m.words(at, &frames);
    m.reg("R13", at);
    assert_eq!(
        basic.stack(&m),
        vec![
            BasicFrame::Loop {
                word: "REPEAT".into(),
                from: pos(30, 0),
            },
            BasicFrame::Loop {
                word: "WHILE".into(),
                from: pos(100, 0),
            },
            BasicFrame::For {
                var: "J%".into(),
                limit: BasicValue::Int(9),
                step: BasicValue::Int(2),
                from: pos(20, 0),
            },
            BasicFrame::For {
                var: "I".into(),
                limit: BasicValue::Float(-3.0),
                step: BasicValue::Float(1.0),
                from: pos(20, 0),
            },
            BasicFrame::Gosub { from: pos(40, 0) },
            BasicFrame::Gosub { from: pos(10, 1) },
        ]
    );
    m.reg("R12", addrs[4] + 5);
    assert_eq!(basic.state(&m).depth, 2);
}

#[test]
fn variables_are_read_and_written() {
    let (mut m, basic, _) = image();
    let vars = basic.variables(&m);
    let seen: Vec<(&str, BasicType, Option<BasicValue>, &[u32])> = vars
        .iter()
        .map(|v| (v.name.as_str(), v.ty, v.value.clone(), v.dims.as_slice()))
        .collect();
    assert_eq!(
        seen,
        [
            ("B%", BasicType::Int, Some(BasicValue::Int(300)), &[][..]),
            ("A", BasicType::Float, Some(BasicValue::Float(5.0)), &[]),
            (
                "A$",
                BasicType::Str,
                Some(BasicValue::Str("HI".into())),
                &[]
            ),
            ("AB%", BasicType::Int, None, &[2, 3]),
            ("I", BasicType::Float, Some(BasicValue::Float(1.0)), &[]),
            (
                "LONG%",
                BasicType::Int,
                Some(BasicValue::Int(-123_456)),
                &[]
            ),
        ]
    );
    assert_eq!(
        basic.array(&m, "AB%", 4, 9),
        Ok(vec![BasicValue::Int(0), BasicValue::Int(-7)])
    );
    let set = |m: &mut Mem, name: &str, index, value: BasicValue| {
        basic.plan_set(m, name, index, &value).map(|w| m.apply(w))
    };
    for v in [-2.5, 1e10, 0.125, -0.007_812_5, 0.0, 16_777_217.0] {
        set(&mut m, "A", None, BasicValue::Float(v)).unwrap();
        assert_eq!(basic.variables(&m)[1].value, Some(BasicValue::Float(v)));
    }
    assert_eq!(real_bytes(1.0), Ok([0, 0, 0, 0, 0x81]));
    assert_eq!(real_bytes(-0.5), Ok([0, 0, 0, 0x80, 0x80]));
    set(&mut m, "LONG%", None, BasicValue::Int(77)).unwrap();
    assert_eq!(basic.variables(&m)[5].value, Some(BasicValue::Int(77)));
    set(&mut m, "A$", None, BasicValue::Str("ok".into())).unwrap();
    assert_eq!(&m.ram[0x9100..0x9102], b"ok");
    assert_eq!(
        set(&mut m, "A$", None, BasicValue::Str("long".into())),
        Err(BasicError::Unsupported)
    );
    set(&mut m, "AB%", Some(5), BasicValue::Float(12.0)).unwrap();
    assert_eq!(basic.array(&m, "AB%", 5, 1), Ok(vec![BasicValue::Int(12)]));
    assert_eq!(
        set(&mut m, "AB%", Some(6), BasicValue::Int(1)),
        Err(BasicError::OutOfRange)
    );
    set(&mut m, "Z%", None, BasicValue::Int(-1)).unwrap();
    assert_eq!(basic.variables(&m)[1].name, "Z%");
    assert_eq!(
        set(&mut m, "QQ", None, BasicValue::Int(1)),
        Err(BasicError::NoSuchVariable)
    );
}
