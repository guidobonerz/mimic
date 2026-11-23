//! The decoder against a hand-built memory image: the ROM's header, its
//! statement loop, keyword table and the code the table addresses are
//! read from, a tokenised program, the variable chains and the stacks.

use mimic_core::cpu::{Flow, Instr, Register, RegisterSet};

use super::*;

const CPU: MemSpaceId = MemSpaceId(0);

struct Mem {
    ram: Vec<u8>,
    s: u8,
    pc: u16,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        let reg = |name, value, bits| Register { name, value, bits };
        RegisterSet(vec![
            reg("PC", u32::from(self.pc), 16),
            reg("S", u32::from(self.s), 8),
        ])
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

    fn apply(&mut self, writes: Vec<(u32, u8)>) {
        for (a, b) in writes {
            self.ram[a as usize] = b;
        }
    }
}

const PAGE: u16 = 0x1900;
const STOP: u16 = 0x8bad;
const MOVE_AT: u16 = 0x986d;
const HEAP: u16 = 0x1a00;

const T_GOSUB: u8 = 0xe4;
const T_FOR: u8 = 0xe3;
const T_TO: u8 = 0xb8;
const T_NEXT: u8 = 0xed;
const T_IF: u8 = 0xe7;
const T_PRINT: u8 = 0xf1;
const T_UNTIL: u8 = 0xfd;

/// A stored line; returns its bytes.
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

/// A line number as the tokeniser packs it.
fn target(n: u16) -> [u8; 4] {
    let (low, high) = (n as u8, (n >> 8) as u8);
    [
        T_LINE,
        ((low & 0xc0) >> 2 | (high & 0xc0) >> 4) ^ 0x54,
        low & 0x3f | 0x40,
        high & 0x3f | 0x40,
    ]
}

/// BASIC II (`fors` `&0500`) or BASIC IV (`&0528`) with the program
/// ```text
/// 10 A=5:GOSUB100
/// 20 FORI=1TO3:NEXT
/// 30 REPEAT:UNTILA:REM a:b
/// 40 IFA THEN PRINT"x:y" ELSE 100
/// 100 PROCp
/// ```
fn image(four: bool) -> (Mem, BbcBasic) {
    let mut m = Mem {
        ram: vec![0; 0x10000],
        s: 0xff,
        pc: 0,
    };
    m.put(0x8009, b"BASIC\0(C)");
    m.put(0x8012, if four { b"1984" } else { b"1982" });
    let mut table = Vec::new();
    for (word, token) in [
        ("AND", 0x80u8),
        ("ABS", 0x94),
        ("ELSE", T_ELSE),
        ("FOR", T_FOR),
        ("GOSUB", T_GOSUB),
        ("IF", T_IF),
        ("NEXT", T_NEXT),
        ("PRINT", T_PRINT),
        ("PROC", T_PROC),
        ("PTR", 0x8f),
        ("REPEAT", T_REPEAT),
        ("REM", T_REM),
        ("THEN", T_THEN),
        ("TO", T_TO),
        ("UNTIL", T_UNTIL),
        ("PTR", 0xcf),
    ] {
        table.extend(word.as_bytes());
        table.extend([token, 0]);
    }
    m.put(0x8071, &table);
    m.put(STOP - LOOP_STOP, &LOOP);
    m.put(MOVE_AT, &MOVE);
    let (fors, repeats, gosubs) = if four {
        (0x0528u16, (0x0500u16, 0x0514u16), (0x05ccu16, 0x05e6u16))
    } else {
        (0x0500, (0x05a4, 0x05b8), (0x05cc, 0x05e6))
    };
    let code = |store: u8, (low, high): (u16, u16), count: u8| {
        let (l, h) = (low.to_le_bytes(), high.to_le_bytes());
        vec![
            0xa5, 0x0b, store, l[0], l[1], 0xa5, 0x0c, store, h[0], h[1], 0xe6, count,
        ]
    };
    m.put(0xb894, &code(0x99, gosubs, 0x25));
    m.put(0xbbed, &code(0x9d, repeats, 0x24));
    let f = fors.to_le_bytes();
    m.put(0xb7da, &[0xa5, 0x37, 0x99, f[0], f[1], 0xa5, 0x38, 0x99]);

    let mut program = Vec::new();
    let mut l10 = b"A=5:".to_vec();
    l10.push(T_GOSUB);
    l10.extend(target(100));
    program.extend(line(10, &l10));
    program.extend(line(
        20,
        &[T_FOR, b'I', b'=', b'1', T_TO, b'3', b':', T_NEXT],
    ));
    let mut l30 = vec![T_REPEAT, b':', T_UNTIL, b'A', b':', T_REM];
    l30.extend(b" a:b");
    program.extend(line(30, &l30));
    let mut l40 = vec![T_IF, b'A', b' ', T_THEN, b' ', T_PRINT];
    l40.extend(b"\"x:y\" ");
    l40.push(T_ELSE);
    l40.push(b' ');
    l40.extend(target(100));
    program.extend(line(40, &l40));
    program.extend(line(100, &[T_PROC, b'p']));
    program.extend([0x0d, 0xff]);
    m.put(PAGE, &program);
    m.ram[usize::from(PAGE_HIGH)] = (PAGE >> 8) as u8;
    m.put(HIMEM, &0x7c00u16.to_le_bytes());
    m.put(STACK, &0x7c00u16.to_le_bytes());

    // Variables: A (real) and A$( in the chain of "A", I in that of "I",
    // n%( in that of "n"; the resident B%.
    let mut a = vec![0x10, 0x1a, 0]; // next: &1A10, no more name
    a.extend(real_bytes(5.0).unwrap());
    m.put(HEAP, &a);
    // "A$" with room for 4, length 2, text at &1A80.
    m.put(0x1a10, &[0x20, 0x1a, b'$', 0, 0x80, 0x1a, 4, 2]);
    m.put(0x1a80, b"HI");
    // "AB%(": two dimensions, 2 × 3 integers.
    m.put(0x1a20, &[0, 0, b'B', b'%', b'(', 0, 5, 2, 0, 3, 0]);
    m.put(0x1a2b + 4 * 5, &(-7i32).to_le_bytes());
    m.put(RESIDENT + 2 * u16::from(b'A'), &HEAP.to_le_bytes());
    let mut i = vec![0, 0, 0];
    i.extend(real_bytes(1.0).unwrap());
    m.put(0x1a60, &i);
    m.put(RESIDENT + 2 * u16::from(b'I'), &0x1a60u16.to_le_bytes());
    m.put(RESIDENT + 4 * 2, &300i32.to_le_bytes());

    let basic = BbcBasic::new(CPU);
    let basic = basic.rescan(&m).expect("the ROM is found");
    (m, basic)
}

/// The address of line `number`.
fn at(m: &Mem, basic: &BbcBasic, number: u32) -> u16 {
    let line = basic.program(m).into_iter().find(|l| l.number == number);
    line.unwrap().addr as u16
}

fn point(m: &mut Mem, base: u16, offset: u8) {
    m.put(BASE, &base.to_le_bytes());
    m.ram[usize::from(OFFSET)] = offset;
}

fn pos(line: u32, statement: u16) -> BasicPos {
    BasicPos { line, statement }
}

#[test]
fn the_rom_is_found_and_named() {
    for (four, name, fors) in [
        (false, "BBC BASIC II", 0x0500),
        (true, "BBC BASIC IV", 0x0528),
    ] {
        let (m, basic) = image(four);
        assert_eq!(basic.info().dialect, name);
        let rom = basic.rom.as_ref().unwrap();
        assert_eq!((rom.stop, rom.fors), (STOP, fors));
        assert_eq!(rom.moving, (MOVE_AT + 6, MOVE_AT + 12));
        assert_eq!(rom.gosubs, (0x05cc, 0x05e6));
        assert_eq!(rom.repeats.0, if four { 0x0500 } else { 0x05a4 });
        assert_eq!(
            basic.stop_points(&m),
            vec![BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: CPU,
                    addr: u32::from(STOP)
                },
                granularity: BasicGranularity::Statement,
            }]
        );
        // The statement form of PTR has a token of its own.
        assert_eq!(rom.keywords[0x4f], b"PTR");
    }
    // Another ROM switched in: inactive, the decoder is kept.
    let (mut m, basic) = image(false);
    m.put(0x8000, &[0xff; 0x4000]);
    assert!(basic.rescan(&m).is_none());
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
    assert!(basic.program(&m).is_empty());
    assert_eq!(basic.stop_points(&m).len(), 1);
}

#[test]
fn the_listing_and_its_statements() {
    let (m, basic) = image(false);
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
            // REPEAT begins a statement behind it; REM takes the rest.
            (30, "REPEAT:UNTILA:REM a:b", &[0, 6, 7, 14]),
            // THEN begins one, ELSE with a line number does not; the
            // colon in the string is text.
            (40, "IFA THEN PRINT\"x:y\" ELSE 100", &[0, 8]),
            (100, "PROCp", &[0]),
        ]
    );
    assert_eq!(lines[0].addr, u32::from(PAGE));
    assert_eq!(lines[0].raw.len(), 4 + 9);
}

#[test]
fn the_place_follows_the_text_pointer() {
    let (mut m, basic) = image(false);
    let l10 = at(&m, &basic, 10);
    let l20 = at(&m, &basic, 20);
    // A typed command runs from page 7.
    point(&mut m, 0x0700, 3);
    let state = basic.state(&m);
    assert_eq!((state.mode, state.pos), (BasicMode::Direct, None));
    // At the stop point of line 10's first statement: the base on the
    // length byte, the offset behind the first character.
    point(&mut m, l10 + 3, 2);
    let state = basic.state(&m);
    assert_eq!(state.mode, BasicMode::Running);
    assert_eq!(state.pos, Some(pos(10, 0)));
    assert_eq!(state.text_addr, Some(u32::from(l10) + 4));
    // The statement ended: the base on its colon, which is still its own.
    point(&mut m, l10 + 7, 1);
    assert_eq!(basic.state(&m).pos, Some(pos(10, 0)));
    // At the stop point of the second.
    point(&mut m, l10 + 7, 2);
    assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
    // The line's end belongs to the line; then the next one begins.
    point(&mut m, l20, 1);
    assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
    point(&mut m, l20 + 3, 1);
    assert_eq!(basic.state(&m).pos, Some(pos(20, 0)));
    // While the base moves to the delimiter the offset is the old one.
    point(&mut m, l20, 9);
    assert_eq!(basic.state(&m).pos, Some(pos(20, 0)));
    m.pc = MOVE_AT + 8;
    assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
    // Outside the program: not running.
    m.pc = 0;
    point(&mut m, 0x3000, 1);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
}

#[test]
fn gosub_for_and_repeat_frames() {
    for four in [false, true] {
        let (mut m, basic) = image(four);
        let rom = basic.rom.clone().unwrap();
        let (l10, l20, l30) = (at(&m, &basic, 10), at(&m, &basic, 20), at(&m, &basic, 30));
        // GOSUB in line 10: the base on the line's end.
        m.ram[usize::from(GOSUBS)] = 1;
        m.ram[usize::from(rom.gosubs.0)] = l20 as u8;
        m.ram[usize::from(rom.gosubs.1)] = (l20 >> 8) as u8;
        // FOR I: the base on the colon behind the FOR statement.
        m.ram[usize::from(FORS)] = 15;
        let mut entry = 0x1a63u16.to_le_bytes().to_vec();
        entry.push(5);
        entry.extend(real_bytes(1.0).unwrap());
        entry.extend(real_bytes(3.0).unwrap());
        entry.extend((l20 + 10).to_le_bytes());
        m.put(rom.fors, &entry);
        // REPEAT: the base on its token.
        m.ram[usize::from(REPEATS)] = 1;
        m.ram[usize::from(rom.repeats.0)] = (l30 + 4) as u8;
        m.ram[usize::from(rom.repeats.1)] = ((l30 + 4) >> 8) as u8;
        assert_eq!(
            basic.stack(&m),
            vec![
                BasicFrame::For {
                    var: "I".into(),
                    limit: BasicValue::Float(3.0),
                    step: BasicValue::Float(1.0),
                    from: pos(20, 0),
                },
                BasicFrame::Loop {
                    word: "REPEAT".into(),
                    from: pos(30, 0),
                },
                BasicFrame::Gosub { from: pos(10, 1) },
            ]
        );
        point(&mut m, l10 + 3, 2);
        assert_eq!(basic.state(&m).depth, 1);
        // An integer loop on a resident variable, begun at a line's end:
        // the base on the next line's length byte.
        let mut entry = (RESIDENT + 4 * 10).to_le_bytes().to_vec();
        entry.push(4);
        entry.extend(2i32.to_le_bytes());
        entry.push(0);
        entry.extend(9i32.to_le_bytes());
        entry.push(0);
        entry.extend((l20 + 3).to_le_bytes());
        m.put(rom.fors, &entry);
        assert_eq!(
            basic.stack(&m)[0],
            BasicFrame::For {
                var: "J%".into(),
                limit: BasicValue::Int(9),
                step: BasicValue::Int(2),
                from: pos(10, 1),
            }
        );
    }
}

#[test]
fn procedure_calls_are_found_on_basics_stack() {
    let (mut m, basic) = image(false);
    let (l10, l100) = (at(&m, &basic, 10), at(&m, &basic, 100));
    // Not in a procedure: the 6502's stack is nearly empty.
    assert!(basic.stack(&m).is_empty());
    // The main program's 6502 stack as PROC saved it: the stack pointer
    // and the return address into the ROM.
    m.put(0x7bfd, &[0xfd, 0x14, 0x93]);
    // Below it the outer procedure's saved variable (an integer) and the
    // 6502 stack it had when it called the inner function: one variable
    // saved, the caller's base and offset (line 100), PROC's token.
    let mut outer = vec![0x00, 0x04, 4, 0, 1, 0, 0, 0];
    let b = (l100 + 3).to_le_bytes();
    let mut saved = vec![0xf8, 0x11, 0x22, 1, b[1], b[0], 2, T_PROC];
    outer.append(&mut saved);
    let below = 0x7bfd - outer.len() as u16;
    m.put(below, &outer);
    // The values of an expression under way lie between that and the
    // inner call's own saved variables (a real and a string).
    let mut inner = vec![0x00, 0x04, 5, 0, 0x81, 0, 0, 0, 0];
    inner.extend([0x00, 0x06, 0x81, 0, 2, b'H', b'I']);
    inner.extend([0xaa, 0xbb, 0xcc, 0xdd]);
    let top = below - inner.len() as u16;
    m.put(top, &inner);
    m.put(STACK, &top.to_le_bytes());
    // The 6502's stack now: two variables saved, the caller in line 10,
    // FN's token.
    let b = (l10 + 7).to_le_bytes();
    m.put(0x01fb, &[2, b[1], b[0], 3, T_FN]);
    m.s = 0xf5;
    assert_eq!(
        basic.stack(&m),
        vec![
            BasicFrame::Gosub { from: pos(10, 1) },
            BasicFrame::Gosub { from: pos(100, 0) },
        ]
    );
    point(&mut m, l10 + 3, 2);
    assert_eq!(basic.state(&m).depth, 2);
}

#[test]
fn variables_are_read_and_written() {
    let (mut m, basic) = image(false);
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
        ]
    );
    assert_eq!(
        basic.array(&m, "AB%", 4, 9),
        Ok(vec![BasicValue::Int(0), BasicValue::Int(-7)])
    );
    assert_eq!(
        basic.array(&m, "ab%", 0, 1),
        Err(BasicError::NoSuchVariable)
    );

    let set = |m: &mut Mem, name: &str, index, value: BasicValue| {
        basic.plan_set(m, name, index, &value).map(|w| m.apply(w))
    };
    for v in [-2.5, 1e10, 0.125, -0.007_812_5, 0.0, 16_777_217.0] {
        set(&mut m, "A", None, BasicValue::Float(v)).unwrap();
        let read = basic.variables(&m)[1].value.clone();
        assert_eq!(read, Some(BasicValue::Float(v)));
    }
    assert_eq!(real_bytes(1.0), Ok([0x81, 0, 0, 0, 0]));
    assert_eq!(real_bytes(-0.5), Ok([0x80, 0x80, 0, 0, 0]));
    // A string grows up to its room.
    set(&mut m, "A$", None, BasicValue::Str("£ 4!".into())).unwrap();
    assert_eq!(&m.ram[0x1a80..0x1a84], b"` 4!");
    assert_eq!(m.ram[0x1a17], 4);
    assert_eq!(
        set(&mut m, "A$", None, BasicValue::Str("12345".into())),
        Err(BasicError::Unsupported)
    );
    assert_eq!(
        set(&mut m, "A$", None, BasicValue::Int(1)),
        Err(BasicError::TypeMismatch)
    );
    set(&mut m, "AB%", Some(5), BasicValue::Float(12.0)).unwrap();
    assert_eq!(basic.array(&m, "AB%", 5, 1), Ok(vec![BasicValue::Int(12)]));
    assert_eq!(
        set(&mut m, "AB%", Some(6), BasicValue::Int(1)),
        Err(BasicError::OutOfRange)
    );
    assert_eq!(
        set(&mut m, "B%", None, BasicValue::Float(1.5)),
        Err(BasicError::OutOfRange)
    );
    // A resident integer that is 0 is not listed but can be written.
    set(&mut m, "Z%", None, BasicValue::Int(-1)).unwrap();
    assert_eq!(basic.variables(&m)[1].name, "Z%");
    assert_eq!(
        set(&mut m, "QQ", None, BasicValue::Int(1)),
        Err(BasicError::NoSuchVariable)
    );
}
