//! The decoder against a hand-built image: the console ROM's loop top,
//! the GROM's keyword table, the scratch pad, and in the video RAM a line
//! table, a program, the symbol table and the value stack.

use mimic_core::cpu::{Flow, Instr, RegisterSet};

use super::*;

const CPU: MemSpaceId = MemSpaceId(0);
const VRAM: MemSpaceId = MemSpaceId(1);
const GROM: MemSpaceId = MemSpaceId(2);

struct Mem {
    cpu: Vec<u8>,
    vram: Vec<u8>,
    grom: Vec<u8>,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        RegisterSet(Vec::new())
    }

    fn set_reg(&mut self, _name: &str, _v: u32) {}

    fn peek(&self, space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
        let mem = match space {
            CPU => &self.cpu,
            VRAM => &self.vram,
            _ => &self.grom,
        };
        mem.get(addr as usize).map(|b| u32::from(*b))
    }

    fn poke(&mut self, _space: MemSpaceId, addr: u32, _size: Size, v: u32) -> bool {
        self.vram[addr as usize] = v as u8;
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
    fn pad(&mut self, addr: u16, v: u16) {
        let at = usize::from(addr);
        self.cpu[at..at + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn put(&mut self, addr: u16, bytes: &[u8]) {
        let at = usize::from(addr);
        self.vram[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn apply(&mut self, writes: Vec<(u32, u8)>) {
        for (a, b) in writes {
            self.vram[a as usize] = b;
        }
    }
}

const STOP: u16 = 0x1976;
const TABLE: u16 = 0x3700;
const TEXT: u16 = 0x3740;
const SYMBOLS: u16 = 0x3600;
const STRINGS: u16 = 0x3500;
const STACK: u16 = 0x06f8;

const T_GOSUB: u8 = 0x87;
const T_FOR: u8 = 0x8c;
const T_TO: u8 = 0xb1;
const T_PRINT: u8 = 0x9c;
const T_INT: u8 = 0xcf;
const T_TAB: u8 = 0xfc;
const T_EQ: u8 = 0xbe;
const T_OPEN: u8 = 0xb7;
const T_CLOSE: u8 = 0xb6;
const T_SEMI: u8 = 0xb4;

fn groups() -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = |words: &[(&str, u8)]| {
        for (w, t) in words {
            out.extend(w.as_bytes());
            out.push(*t);
        }
        out.push(0xff);
    };
    group(&[(")", T_CLOSE), ("(", T_OPEN), ("=", T_EQ), (";", T_SEMI)]);
    group(&[("TO", T_TO)]);
    // NEW is a command: a token below >80.
    group(&[
        ("FOR", T_FOR),
        ("INT", T_INT),
        ("TAB", T_TAB),
        ("REM", T_REM),
        ("NEW", 1),
    ]);
    group(&[]);
    group(&[("GOSUB", T_GOSUB), ("PRINT", T_PRINT)]);
    for _ in 6..=10 {
        group(&[]);
    }
    out
}

fn unquoted(text: &str) -> Vec<u8> {
    let mut out = vec![T_UNQUOTED, text.len() as u8];
    out.extend(text.as_bytes());
    out
}

/// ```text
/// 10 A=5
/// 20 GOSUB 100
/// 30 FOR I=1 TO 3
/// 40 PRINT TAB(3);INT(A);"X""Y"
/// 100 REM A  B
/// ```
fn image() -> (Mem, TiBasic) {
    let mut m = Mem {
        cpu: vec![0; 0x10000],
        vram: vec![0; 0x4000],
        grom: vec![0; 0x6000],
    };
    m.cpu[usize::from(STOP)..usize::from(STOP) + LOOP.len()].copy_from_slice(&LOOP);
    let table = groups();
    m.grom[0x2870..0x2870 + table.len()].copy_from_slice(&table);

    let mut texts: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut l10 = vec![b'A', T_EQ];
    l10.extend(unquoted("5"));
    texts.push((10, l10));
    texts.push((20, vec![T_GOSUB, T_LINE, 0, 100]));
    let mut l30 = vec![T_FOR, b'I', T_EQ];
    l30.extend(unquoted("1"));
    l30.push(T_TO);
    l30.extend(unquoted("3"));
    texts.push((30, l30));
    let mut l40 = vec![T_PRINT, T_TAB, T_OPEN];
    l40.extend(unquoted("3"));
    l40.extend([T_CLOSE, T_SEMI, T_INT, T_OPEN, b'A', T_CLOSE, T_SEMI]);
    l40.extend([T_QUOTED, 3, b'X', b'"', b'Y']);
    texts.push((40, l40));
    let mut l100 = vec![T_REM];
    l100.extend(b" A  B");
    texts.push((100, l100));
    // The texts one behind the other, the table with the highest number
    // first.
    let mut at = TEXT;
    let mut entries = Vec::new();
    for (number, text) in &texts {
        m.vram[usize::from(at)] = text.len() as u8 + 1;
        m.put(at + 1, text);
        entries.push((*number, at + 1));
        at += text.len() as u16 + 2;
    }
    for (i, (number, text)) in entries.iter().rev().enumerate() {
        let e = TABLE + 4 * i as u16;
        m.put(e, &number.to_be_bytes());
        m.put(e + 2, &text.to_be_bytes());
    }
    m.pad(STLN, TABLE);
    m.pad(ENLN, TABLE + 4 * 5 - 1);
    m.pad(STACK_BASE, STACK);
    m.pad(VSPTR, STACK);

    // The symbol table, the newest first: the array D(1,2), C$, I, A.
    // Their names are in the program's text; here in a place of their own.
    m.put(0x3400, b"DC$IA");
    let mut d = vec![2, 1];
    d.extend((SYMBOLS + 0x80).to_be_bytes());
    d.extend(0x3400u16.to_be_bytes());
    d.extend([0, 1, 0, 2]);
    for i in 0..6 {
        d.extend(number_bytes(f64::from(i)).unwrap());
    }
    m.put(SYMBOLS, &d);
    let mut c = vec![0x80, 2];
    c.extend((SYMBOLS + 0x90).to_be_bytes());
    c.extend(0x3401u16.to_be_bytes());
    c.extend((STRINGS + 1).to_be_bytes());
    m.put(SYMBOLS + 0x80, &c);
    m.put(STRINGS, &[2, b'H', b'I']);
    let mut i = vec![0, 1];
    i.extend((SYMBOLS + 0xa0).to_be_bytes());
    i.extend(0x3403u16.to_be_bytes());
    i.extend(number_bytes(1.0).unwrap());
    m.put(SYMBOLS + 0x90, &i);
    let mut a = vec![0, 1, 0, 0];
    a.extend(0x3404u16.to_be_bytes());
    a.extend(number_bytes(5.0).unwrap());
    m.put(SYMBOLS + 0xa0, &a);
    m.pad(SYMTAB, SYMBOLS);

    let basic = TiBasic::new(CPU, VRAM, GROM);
    let basic = basic.rescan(&m).expect("the ROMs are found");
    (m, basic)
}

/// EXTRAM for the line with index `i` in line order.
fn extram(i: u16) -> u16 {
    TABLE + 4 * (4 - i) + 2
}

fn pos(line: u32) -> BasicPos {
    BasicPos { line, statement: 0 }
}

#[test]
fn the_roms_are_found() {
    let (mut m, basic) = image();
    let rom = basic.rom.as_ref().unwrap();
    assert_eq!(rom.stop, STOP);
    assert_eq!(rom.keywords[usize::from(T_GOSUB - 0x80)], b"GOSUB");
    assert_eq!(rom.keywords[usize::from(T_EQ - 0x80)], b"=");
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
    assert_eq!(basic.info().space, VRAM);
    // Found once; the title screen has no line table.
    assert!(basic.rescan(&m).is_none());
    m.pad(STLN, 0);
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
    assert!(basic.program(&m).is_empty());
    // No program: both ends on the same byte.
    m.pad(STLN, 0x37d7);
    m.pad(ENLN, 0x37d7);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    assert!(basic.program(&m).is_empty());
}

#[test]
fn the_listing_is_lists() {
    let (m, basic) = image();
    let lines = basic.program(&m);
    let texts: Vec<(u32, &str)> = lines.iter().map(|l| (l.number, l.text.as_str())).collect();
    assert_eq!(
        texts,
        [
            (10, "A=5"),
            (20, "GOSUB 100"),
            (30, "FOR I=1 TO 3"),
            // No blanks around functions and TAB; a quote is doubled.
            (40, "PRINT TAB(3);INT(A);\"X\"\"Y\""),
            (100, "REM A  B"),
        ]
    );
    assert!(lines.iter().all(|l| l.statements == [0]));
    assert_eq!(lines[0].addr, u32::from(TEXT) + 1);
    assert_eq!(lines[0].raw, [b'A', T_EQ, T_UNQUOTED, 1, b'5', 0]);
}

#[test]
fn the_place_is_the_line_extram_points_to() {
    let (mut m, basic) = image();
    // A typed command: PRGFLG is 0, whatever EXTRAM holds.
    m.pad(EXTRAM, extram(2));
    let state = basic.state(&m);
    assert_eq!((state.mode, state.pos), (BasicMode::Direct, None));
    m.cpu[usize::from(PRGFLG)] = 0xff;
    let state = basic.state(&m);
    assert_eq!((state.mode, state.pos), (BasicMode::Running, Some(pos(30))));
    assert_eq!(
        state.text_addr,
        Some(
            u32::from(m.vram[usize::from(extram(2))]) << 8
                | u32::from(m.vram[usize::from(extram(2)) + 1])
        )
    );
    m.pad(EXTRAM, extram(0));
    assert_eq!(basic.state(&m).pos, Some(pos(10)));
    m.pad(EXTRAM, extram(4));
    assert_eq!(basic.state(&m).pos, Some(pos(100)));
    // Not on an entry's address field: no place.
    m.pad(EXTRAM, extram(2) + 1);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    m.pad(EXTRAM, TABLE + 4 * 5 + 2);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
}

#[test]
fn frames_are_the_entries_with_an_id() {
    let (mut m, basic) = image();
    assert!(basic.stack(&m).is_empty());
    // GOSUB from line 20; inside it the FOR of line 30 with its limit
    // and step below it; on top a number of an expression under way.
    let mut stack = extram(1).to_be_bytes().to_vec();
    stack.extend([ID_GOSUB, 0, 0, 0, 0, 0]);
    stack.extend(number_bytes(3.0).unwrap());
    stack.extend(number_bytes(-0.5).unwrap());
    stack.extend((SYMBOLS + 0x90).to_be_bytes());
    stack.extend([ID_FOR, 0]);
    stack.extend((SYMBOLS + 0x96).to_be_bytes());
    stack.extend(extram(2).to_be_bytes());
    stack.extend(number_bytes(66.0).unwrap());
    m.put(STACK + 8, &stack);
    m.pad(VSPTR, STACK + stack.len() as u16);
    assert_eq!(
        basic.stack(&m),
        vec![
            BasicFrame::For {
                var: "I".into(),
                limit: BasicValue::Float(3.0),
                step: BasicValue::Float(-0.5),
                from: pos(30),
            },
            BasicFrame::Gosub { from: pos(20) },
        ]
    );
    m.cpu[usize::from(PRGFLG)] = 0xff;
    m.pad(EXTRAM, extram(3));
    assert_eq!(basic.state(&m).depth, 1);
}

#[test]
fn variables_are_read_and_written() {
    let (mut m, basic) = image();
    let vars = basic.variables(&m);
    let seen: Vec<(&str, BasicType, Option<BasicValue>, &[u32])> = vars
        .iter()
        .map(|v| (v.name.as_str(), v.ty, v.value.clone(), v.dims.as_slice()))
        .collect();
    assert_eq!(
        seen,
        [
            ("D", BasicType::Float, None, &[2, 3][..]),
            (
                "C$",
                BasicType::Str,
                Some(BasicValue::Str("HI".into())),
                &[]
            ),
            ("I", BasicType::Float, Some(BasicValue::Float(1.0)), &[]),
            ("A", BasicType::Float, Some(BasicValue::Float(5.0)), &[]),
        ]
    );
    assert_eq!(
        basic.array(&m, "d", 4, 9),
        Ok(vec![BasicValue::Float(4.0), BasicValue::Float(5.0)])
    );
    let set = |m: &mut Mem, name: &str, index, value: BasicValue| {
        basic.plan_set(m, name, index, &value).map(|w| m.apply(w))
    };
    for v in [-2.5, 1e10, 0.1, -1e-10, 0.0, 123_456.789, 99.0, 100.0, -7.0] {
        set(&mut m, "a", None, BasicValue::Float(v)).unwrap();
        assert_eq!(basic.variables(&m)[3].value, Some(BasicValue::Float(v)));
    }
    // 7 is 7 × 100^0; −7.5 has its first word negated.
    assert_eq!(number_bytes(7.0), Ok([0x40, 7, 0, 0, 0, 0, 0, 0]));
    assert_eq!(number_bytes(-7.5), Ok([0xbf, 0xf9, 50, 0, 0, 0, 0, 0]));
    assert_eq!(number_bytes(240.0), Ok([0x41, 2, 40, 0, 0, 0, 0, 0]));
    set(&mut m, "C$", None, BasicValue::Str("ok".into())).unwrap();
    assert_eq!(
        &m.vram[usize::from(STRINGS) + 1..usize::from(STRINGS) + 3],
        b"ok"
    );
    assert_eq!(
        set(&mut m, "C$", None, BasicValue::Str("long".into())),
        Err(BasicError::Unsupported)
    );
    assert_eq!(
        set(&mut m, "A", None, BasicValue::Str("x".into())),
        Err(BasicError::TypeMismatch)
    );
    set(&mut m, "D", Some(5), BasicValue::Int(12)).unwrap();
    assert_eq!(
        basic.array(&m, "D", 5, 1),
        Ok(vec![BasicValue::Float(12.0)])
    );
    assert_eq!(
        set(&mut m, "D", Some(6), BasicValue::Int(1)),
        Err(BasicError::OutOfRange)
    );
    assert_eq!(
        set(&mut m, "Q", None, BasicValue::Int(1)),
        Err(BasicError::NoSuchVariable)
    );
}
