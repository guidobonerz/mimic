//! The decoder against a hand-built memory image: the pieces of the ROM
//! the addresses are read from, both keyword tables, a tokenised program,
//! the variables and frames on the Z80's stack.

use mimic_core::cpu::{Flow, Instr, Register, RegisterSet};

use super::*;

const CPU: MemSpaceId = MemSpaceId(0);

struct Mem {
    ram: Vec<u8>,
    sp: u16,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        RegisterSet(vec![Register {
            name: "SP",
            value: u32::from(self.sp),
            bits: 16,
        }])
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

const SAVTXT: u16 = 0x03cf;
const CURLIN: u16 = 0x0358;
const TXTTAB: u16 = 0x035f;
const VARTAB: u16 = 0x03d7;
const STACK_TOP: u16 = 0x0356;
const PROGRAM: u16 = 0x0401;
const VARS: u16 = 0x0500;
const TOP: u16 = 0x2000;
const STOP: u16 = 0xc88a;

const T_FOR: u8 = 0x81;
const T_NEXT: u8 = 0x82;
const T_DATA: u8 = 0x83;
const T_GOSUB: u8 = 0x84;
const T_REM: u8 = 0x85;
const T_TICK: u8 = 0x86;
const T_TO: u8 = 0x87;
/// The extension's second word.
const T_PAPER: u8 = 0xd6;

fn table(words: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for w in words {
        let mut b = w.as_bytes().to_vec();
        b[0] |= 0x80;
        out.extend(b);
    }
    out.push(0x80);
    out
}

fn line(addr: u16, number: u16, text: &[u8]) -> Vec<u8> {
    let mut out = (addr + 5 + text.len() as u16).to_le_bytes().to_vec();
    out.extend(number.to_le_bytes());
    out.extend(text);
    out.push(0);
    out
}

/// ```text
/// 10 A=5:GOSUB100
/// 20 FORI=1TO3:NEXT
/// 30 DATA a:b,"c:d":REM x:y
/// 40 !x:PAPER1
/// 100 PAPER1:A$="x:y"
/// ```
fn image() -> (Mem, HcBasic, Vec<u16>) {
    let mut m = Mem {
        ram: vec![0; 0x10000],
        sp: TOP - 2,
    };
    // The statement loop, NEW, CLEAR's end and LIST's lookup.
    let (s, c) = (SAVTXT.to_le_bytes(), CURLIN.to_le_bytes());
    m.put(0xc854, &[0xcd, 0x16, 0xde, 0xcd, 0xf9, 0xc8]);
    m.put(
        0xc85a,
        &[
            0x22, s[0], s[1], 0x7e, 0xfe, 0x3a, 0x28, 0x28, 0xb7, 0xc2, 0x48, 0xc3,
        ],
    );
    m.put(0xc86e, &[0x5e, 0x23, 0x56, 0xed, 0x53, c[0], c[1]]);
    m.put(STOP, &[0xcd, 0xbd, 0xc8, 0x11, 0x54, 0xc8, 0xd5]);
    let (t, v) = (TXTTAB.to_le_bytes(), VARTAB.to_le_bytes());
    m.put(
        0xc641,
        &[
            0x2a, t[0], t[1], 0xaf, 0x32, 0x5e, 0x03, 0x77, 0x23, 0x77, 0x23, 0x22, v[0], v[1],
        ],
    );
    let top = STACK_TOP.to_le_bytes();
    m.put(0xc669, &[0xc1, 0x2a, top[0], top[1], 0xf9]);
    m.put(
        0xc7a2,
        &[
            0xed, 0x5b, 0x0c, 0xe0, 0x18, 0x03, 0x11, 0x21, 0xc1, 0x47, 0x1a, 0x13, 0xb7,
        ],
    );
    m.put(
        0xc121,
        &table(&["END", "FOR", "NEXT", "DATA", "GOSUB", "REM", "!", "TO"]),
    );
    m.word(0xe00c, 0xe100);
    m.put(0xe100, &table(&["INK", "PAPER"]));

    let mut addrs = Vec::new();
    let mut at = PROGRAM;
    let mut add = |m: &mut Mem, number: u16, text: &[u8]| {
        let l = line(at, number, text);
        m.put(at, &l);
        addrs.push(at);
        at += l.len() as u16;
        at
    };
    let mut l10 = b"A=5:".to_vec();
    l10.push(T_GOSUB);
    l10.extend(b"100");
    add(&mut m, 10, &l10);
    add(
        &mut m,
        20,
        &[T_FOR, b'I', b'=', b'1', T_TO, b'3', b':', T_NEXT],
    );
    let mut l30 = vec![T_DATA];
    l30.extend(b" a:b,\"c:d\":");
    l30.push(T_REM);
    l30.extend(b" x:y");
    add(&mut m, 30, &l30);
    add(&mut m, 40, &[T_TICK, b'x', b':', T_PAPER, b'1']);
    let mut l100 = vec![T_PAPER, b'1', b':'];
    l100.extend(b"A$=\"x:y\"");
    let end = add(&mut m, 100, &l100);
    m.word(TXTTAB, PROGRAM);
    m.word(STACK_TOP, TOP);

    // A=5, I=1, A$ (a constant in line 100), AB$ on the heap; then the
    // array D(1,2): 2 × 3 numbers.
    let mut vars = vec![0, b'A'];
    vars.extend(number_bytes(5.0).unwrap());
    vars.extend([0, b'I']);
    vars.extend(number_bytes(1.0).unwrap());
    let constant = (end - 5).to_le_bytes();
    vars.extend([0x80, b'A', 3, 0, constant[0], constant[1]]);
    vars.extend([b'B' | 0x80, b'A', 2, 0, 0x00, 0x1f]);
    m.put(VARS, &vars);
    m.put(0x1f00, b"HI");
    let arrays = VARS + vars.len() as u16;
    let mut d = vec![0, b'D', 5 + 24, 0, 2, 3, 0, 2, 0];
    for i in 0..6 {
        d.extend(number_bytes(f64::from(i)).unwrap());
    }
    m.put(arrays, &d);
    m.word(VARTAB, VARS);
    m.word(VARTAB + 2, arrays);
    m.word(VARTAB + 4, arrays + d.len() as u16);
    m.word(CURLIN, 0xffff);

    let basic = HcBasic::new(CPU).rescan(&m).expect("the ROM is found");
    (m, basic, addrs)
}

fn pos(line: u32, statement: u16) -> BasicPos {
    BasicPos { line, statement }
}

#[test]
fn the_rom_is_found() {
    let (mut m, basic, _) = image();
    let rom = basic.rom.as_ref().unwrap();
    assert_eq!(
        (rom.stop, rom.savtxt, rom.curlin, rom.txttab, rom.vartab),
        (STOP, SAVTXT, CURLIN, TXTTAB, VARTAB)
    );
    assert_eq!((rom.stack_top, rom.extension), (STACK_TOP, Some(0xe00c)));
    assert_eq!(
        (rom.t_gosub, rom.t_data, rom.t_rem),
        (T_GOSUB, T_DATA, [T_REM, T_TICK])
    );
    assert_eq!(basic.stop_points(&m).len(), 1);
    assert_eq!(basic.info().name_len, 2);
    // BASIC not started yet: no program start.
    m.word(TXTTAB, 0);
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
    assert!(basic.stop_points(&m).is_empty());
    // The ROM switched off.
    let (mut m, basic, _) = image();
    m.put(0xc000, &[0xff; 0x2000]);
    assert!(basic.rescan(&m).is_none());
    assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
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
            // DATA ends at a colon outside quotes, REM takes the rest.
            (30, "DATA a:b,\"c:d\":REM x:y", &[0, 7, 15]),
            (40, "!x:PAPER1", &[0]),
            // A word of the extension's table; no colon inside quotes.
            (100, "PAPER1:A$=\"x:y\"", &[0, 7]),
        ]
    );
    assert_eq!(lines[1].addr, u32::from(addrs[1]));
    assert_eq!(lines[0].raw.len(), 4 + 8 + 1);
}

#[test]
fn the_place_follows_the_saved_text_pointer() {
    let (mut m, basic, addrs) = image();
    // A typed command.
    let state = basic.state(&m);
    assert_eq!((state.mode, state.pos), (BasicMode::Direct, None));
    // RUN: on the byte before the program, the first line's number set.
    m.word(CURLIN, 10);
    m.word(SAVTXT, PROGRAM - 1);
    assert_eq!(basic.state(&m).pos, Some(pos(10, 0)));
    assert_eq!(basic.state(&m).mode, BasicMode::Running);
    // On the colon before the second statement.
    m.word(SAVTXT, addrs[0] + 7);
    assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
    // On the end of line 10, line 20's number set.
    m.word(CURLIN, 20);
    m.word(SAVTXT, addrs[1] - 1);
    assert_eq!(basic.state(&m).pos, Some(pos(20, 0)));
    // FOR keeps its variable's address there for a while.
    m.word(SAVTXT, VARS + 8);
    assert_eq!(basic.state(&m).pos, Some(pos(20, 0)));
    // A line that is not there.
    m.word(CURLIN, 15);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
}

#[test]
fn frames_are_found_from_the_stack_pointer() {
    let (mut m, basic, addrs) = image();
    assert!(basic.stack(&m).is_empty());
    // GOSUB in line 10 (the text pointer behind its token), inside it the
    // FOR of line 20 (the text pointer on the colon behind it).
    let mut frames = vec![T_FOR];
    frames.extend((VARS + 8).to_le_bytes());
    frames.push(1);
    frames.extend(number_bytes(1.0).unwrap());
    frames.extend(number_bytes(3.0).unwrap());
    frames.extend(20u16.to_le_bytes());
    frames.extend((addrs[1] + 10).to_le_bytes());
    frames.push(T_GOSUB);
    frames.extend(10u16.to_le_bytes());
    frames.extend((addrs[0] + 9).to_le_bytes());
    let at = TOP - 2 - frames.len() as u16;
    m.put(at, &frames);
    let want = vec![
        BasicFrame::For {
            var: "I".into(),
            limit: BasicValue::Float(3.0),
            step: BasicValue::Float(1.0),
            from: pos(20, 0),
        },
        BasicFrame::Gosub { from: pos(10, 1) },
    ];
    // At the stop point the stack pointer is on the first frame.
    m.sp = at;
    assert_eq!(basic.stack(&m), want);
    // Inside a statement return addresses and values lie below, among
    // them bytes that look like a frame's first.
    m.put(
        at - 9,
        &[T_GOSUB, 0x12, 0x34, T_FOR, 0, 0, 0x54, 0xc8, T_GOSUB],
    );
    m.sp = at - 9;
    assert_eq!(basic.stack(&m), want);
    m.word(CURLIN, 20);
    m.word(SAVTXT, addrs[1] + 10);
    assert_eq!(basic.state(&m).depth, 1);
    // A stack pointer elsewhere (machine code): nothing.
    m.sp = 0x8000;
    assert!(basic.stack(&m).is_empty());
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
            ("A", BasicType::Float, Some(BasicValue::Float(5.0)), &[][..]),
            ("I", BasicType::Float, Some(BasicValue::Float(1.0)), &[]),
            (
                "A$",
                BasicType::Str,
                Some(BasicValue::Str("x:y".into())),
                &[]
            ),
            (
                "AB$",
                BasicType::Str,
                Some(BasicValue::Str("HI".into())),
                &[]
            ),
            ("D", BasicType::Float, None, &[2, 3]),
        ]
    );
    assert_eq!(
        basic.array(&m, "d", 4, 9),
        Ok(vec![BasicValue::Float(4.0), BasicValue::Float(5.0)])
    );
    let set = |m: &mut Mem, name: &str, index, value: BasicValue| {
        basic.plan_set(m, name, index, &value).map(|w| m.apply(w))
    };
    for v in [-2.5, 1e10, 0.125, -0.007_812_5, 0.0, 65_537.0] {
        set(&mut m, "a", None, BasicValue::Float(v)).unwrap();
        assert_eq!(basic.variables(&m)[0].value, Some(BasicValue::Float(v)));
    }
    assert_eq!(number_bytes(1.0), Ok([0, 0, 0, 0x81]));
    assert_eq!(number_bytes(-0.5), Ok([0, 0, 0x80, 0x80]));
    // Two characters of a name count.
    set(&mut m, "ABEND$", None, BasicValue::Str("ok".into())).unwrap();
    assert_eq!(&m.ram[0x1f00..0x1f02], b"ok");
    assert_eq!(
        set(&mut m, "AB$", None, BasicValue::Str("long".into())),
        Err(BasicError::Unsupported)
    );
    // A constant in the program's text is not written.
    assert_eq!(
        set(&mut m, "A$", None, BasicValue::Str("abc".into())),
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
