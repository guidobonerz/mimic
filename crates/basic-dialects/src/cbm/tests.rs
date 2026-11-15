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
        (rom.dispatch, rom.txttab, rom.txtptr, rom.keywords_at),
        (0xa7e1, 0x2b, 0x7a, 0xa09e)
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
    assert_eq!((rom.dispatch, rom.txttab, rom.txtptr), (0xa7e4, 0x28, 0x77));
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
    // Halted inside RETURN: CURLIN still names the line left.
    at(&mut m, 100, colon);
    assert_eq!((pos(&m).line, pos(&m).statement), (10, 1));
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

/// BASIC 3.5 and 7.0 against an image laid out like a C128's: the ROM at
/// `$4000` in the CPU's view, 128 KB of RAM as a space of its own, the
/// program in bank 0 and the variables in bank 1.
mod banked {
    use super::*;

    const CPU: MemSpaceId = MemSpaceId(0);
    const RAM: MemSpaceId = MemSpaceId(1);
    const BANK1: usize = 0x1_0000;

    #[derive(Clone, Copy, PartialEq)]
    enum Map {
        Rom,
        /// One of the interpreter's RAM routines has switched to RAM.
        Ram,
        /// Something else is mapped where the ROM was.
        Other,
    }

    struct Mem {
        rom: Vec<u8>,
        ram: Vec<u8>,
        map: Map,
    }

    impl DebugTarget for Mem {
        fn regs(&self) -> RegisterSet {
            RegisterSet(Vec::new())
        }

        fn set_reg(&mut self, _name: &str, _v: u32) {}

        fn peek(&self, space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
            let a = addr as usize;
            Some(u32::from(match space {
                RAM => self.ram[a & 0x1ffff],
                _ if (0x4000..0xc000).contains(&a) => match self.map {
                    Map::Rom => self.rom[a],
                    Map::Ram => self.ram[a],
                    Map::Other => 0x55,
                },
                _ => self.ram[a & 0xffff],
            }))
        }

        fn poke(&mut self, _space: MemSpaceId, addr: u32, _size: Size, v: u32) -> bool {
            self.ram[addr as usize & 0x1ffff] = v as u8;
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
        fn rom(&mut self, at: usize, bytes: &[u8]) {
            self.rom[at..at + bytes.len()].copy_from_slice(bytes);
        }

        fn ram(&mut self, at: usize, bytes: &[u8]) {
            self.ram[at..at + bytes.len()].copy_from_slice(bytes);
        }

        fn zp(&mut self, at: usize, v: u16) {
            self.ram(at, &v.to_le_bytes());
        }
    }

    fn table(words: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for w in words {
            out.extend(w.bytes());
            *out.last_mut().unwrap() |= 0x80;
        }
        out.push(0);
        out
    }

    const LINE10: u16 = 0x1c01;
    /// The 0 that ends line 10.
    const LINE10_END: u16 = LINE10 + 4 + 19;
    const VARS: u16 = 0x0400;
    const CURLIN: usize = 0x3b;
    const TXTPTR: usize = 0x3d;
    const TOS: usize = 0x7d;
    const TOP: usize = 0x09ff;

    /// ```text
    /// 10 BANK 15:A=POT(1):FOR I=1 TO 3
    /// ```
    /// `two_byte`: with the tables of BASIC 7.0.
    fn machine(two_byte: bool) -> Mem {
        let mut m = Mem {
            rom: vec![0; 0x10000],
            ram: vec![0; 0x20000],
            map: Map::Rom,
        };
        let mut words = table(KEYWORDS);
        if two_byte {
            words.extend(table(&["BANK", "FILTER"]));
            words.extend(table(&["POT", "BUMP"]));
        }
        m.rom(0x4417, &words);
        // The statement loop, CLR and GOSUB.
        m.rom(0x4a9f, &[0x6c, 0x08, 0x03]);
        m.rom(
            0x4b1c,
            &[
                0x85, 0x3b, 0xc8, 0x20, 0xc9, 0x03, 0x85, 0x3c, 0x98, 0x18, 0x65, 0x3d, 0x85, 0x3d,
            ],
        );
        m.rom(0x5216, &[0xa9, 0xff, 0xa0, 0x09, 0x85, 0x7d, 0x84, 0x7e]);
        m.rom(0x5a38, &[0xa9, 0x8d, 0x91, 0x7d]);
        // The program in bank 0.
        let mut body = vec![0xfe, 0x02, b'1', b'5', b':', b'A', token("=")];
        body.extend([0xce, 0x02, b'(', b'1', b')', b':']);
        body.extend(tokens("{FOR}I{=}1{TO}3"));
        assert_eq!(body.len(), 19);
        let line = usize::from(LINE10);
        m.zp(line, LINE10_END + 1);
        m.zp(line + 2, 10);
        m.ram(line + 4, &body);
        // The variables in bank 1 (bank 0 without the tables): A = 2,
        // S$ = "HEY", I = 1.
        let bank = if two_byte { BANK1 } else { 0 };
        let vars = bank + usize::from(VARS);
        m.ram(vars, &[b'A', 0, 0x82, 0, 0, 0, 0]);
        m.ram(vars + 7, &[b'S', 0x80, 3, 0x00, 0xf0, 0, 0]);
        m.ram(vars + 14, &[b'I', 0, 0x81, 0, 0, 0, 0]);
        m.ram(bank + 0xf000, b"HEY");
        for (zp, v) in [
            (0x2d, LINE10),
            (0x2f, VARS),
            (0x31, VARS + 21),
            (0x33, VARS + 21),
            (0x35, 0xf000),
            (CURLIN, 0xff00),
            (TOS, TOP as u16),
        ] {
            m.zp(zp, v);
        }
        m
    }

    fn decoder(m: &Mem) -> CbmBasic {
        CbmBasic::new(CPU)
            .with_ram(RAM, BANK1 as u32)
            .scan_from(0x4000)
            .rescan(m)
            .expect("BASIC found")
    }

    #[test]
    fn finds_the_loop_the_stack_and_the_tables() {
        let m = machine(true);
        let basic = decoder(&m);
        let rom = basic.rom.as_ref().unwrap();
        assert_eq!(
            (rom.dispatch, rom.txttab, rom.txtptr, rom.keywords_at),
            (0x4a9f, 0x2d, 0x3d, 0x4417)
        );
        assert_eq!(
            rom.stack,
            Stack::Own {
                tos: 0x7d,
                top: 0x09ff
            }
        );
        assert_eq!((rom.fe.len(), rom.ce.len()), (2, 2));
        let info = basic.info();
        assert_eq!(info.dialect, "Commodore BASIC 7.0");
        // Addresses are RAM addresses; the stop point is the CPU's.
        assert_eq!(info.space, RAM);
        assert_eq!(
            basic.stop_points(&m),
            [BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: CPU,
                    addr: 0x4a9f
                },
                granularity: BasicGranularity::Statement,
            }]
        );
        // Without the second table: one bank, no tokens of two bytes.
        let m = machine(false);
        let basic = decoder(&m);
        assert!(basic.rom.as_ref().unwrap().fe.is_empty());
        assert_eq!(basic.variables(&m)[0].addr, u32::from(VARS));
    }

    #[test]
    fn lists_tokens_of_two_bytes() {
        let mut m = machine(true);
        let p = decoder(&m).program(&m);
        assert_eq!(p[0].text, "BANK15:A=POT(1):FORI=1TO3");
        assert_eq!(p[0].statements, [0, 7, 16]);
        // An unknown second byte, and a prefix as the line's last byte.
        m.ram[usize::from(LINE10) + 5] = 0x30;
        m.ram[usize::from(LINE10_END) - 1] = 0xfe;
        let p = decoder(&m).program(&m);
        assert_eq!(p[0].text, "{$fe}015:A=POT(1):FORI=1TO{$fe}");
    }

    #[test]
    fn variables_are_in_bank_1() {
        let mut m = machine(true);
        let basic = decoder(&m);
        let vars = basic.variables(&m);
        let seen: Vec<(&str, u32, Option<BasicValue>)> = vars
            .iter()
            .map(|v| (v.name.as_str(), v.addr, v.value.clone()))
            .collect();
        assert_eq!(
            seen,
            [
                ("A", 0x1_0400, Some(BasicValue::Float(2.0))),
                ("S$", 0x1_0407, Some(BasicValue::Str("HEY".into()))),
                ("I", 0x1_040e, Some(BasicValue::Float(1.0))),
            ]
        );
        let writes = basic
            .plan_set(&m, "S$", None, &BasicValue::Str("YOU".into()))
            .unwrap();
        assert_eq!(writes[0], (0x1_f000, b'Y'));
        for (addr, b) in writes {
            m.poke(RAM, addr, Size::Byte, u32::from(b));
        }
        assert_eq!(
            basic.variables(&m)[1].value,
            Some(BasicValue::Str("YOU".into()))
        );
    }

    #[test]
    fn frames_on_the_interpreters_own_stack() {
        let mut m = machine(true);
        let basic = decoder(&m);
        // Outer: DO in line 10's second statement. Inner: FOR I, its text
        // pointer (the line's end) stored high byte first.
        let mut frames = vec![0x81];
        frames.extend((VARS + 16).to_le_bytes());
        frames.extend([0x81, 0x80, 0, 0, 0, 0x01]);
        frames.extend([0x82, 0x40, 0, 0, 0]);
        frames.extend(10u16.to_le_bytes());
        frames.extend(LINE10_END.to_be_bytes());
        frames.push(0xeb);
        frames.extend(10u16.to_le_bytes());
        frames.extend((LINE10 + 4 + 6).to_le_bytes());
        let tos = TOP - frames.len();
        m.ram(tos, &frames);
        m.zp(TOS, tos as u16);
        let pos = |statement| BasicPos {
            line: 10,
            statement,
        };
        assert_eq!(
            basic.stack(&m),
            [
                BasicFrame::For {
                    var: "I".into(),
                    limit: BasicValue::Float(3.0),
                    step: BasicValue::Float(1.0),
                    from: pos(2),
                },
                BasicFrame::Loop {
                    word: "DO".into(),
                    from: pos(1),
                },
            ]
        );
        // A GOSUB frame counts for the depth.
        m.ram[TOP - 5] = 0x8d;
        assert_eq!(basic.stack(&m)[1], BasicFrame::Gosub { from: pos(1) });
        m.zp(CURLIN, 10);
        m.zp(TXTPTR, LINE10_END);
        assert_eq!(basic.state(&m).depth, 1);
        // No chain of frames: an unknown token, a pointer past the top, a
        // frame that would reach past it.
        m.ram[TOP - 5] = 0x99;
        assert!(basic.stack(&m).is_empty());
        m.zp(TOS, 0x0a10);
        assert!(basic.stack(&m).is_empty());
        m.zp(TOS, 0x09fc);
        m.ram[0x09fc] = 0x8d;
        assert!(basic.stack(&m).is_empty());
    }

    #[test]
    fn a_rom_switched_off_for_a_moment_is_still_followed() {
        let mut m = machine(true);
        let basic = decoder(&m);
        m.zp(CURLIN, 10);
        m.zp(TXTPTR, LINE10 + 3);
        assert_eq!(basic.state(&m).mode, BasicMode::Running);
        m.map = Map::Ram;
        assert_eq!(basic.state(&m).mode, BasicMode::Running);
        assert_eq!(basic.program(&m).len(), 1);
        assert!(basic.rescan(&m).is_none());
        // Something else there: the interpreter is gone.
        m.map = Map::Other;
        assert_eq!(basic.state(&m).mode, BasicMode::Inactive);
        let gone = basic.rescan(&m).expect("changed");
        assert_eq!(gone.info().dialect, "Commodore BASIC");
        m.map = Map::Rom;
        assert!(gone.rescan(&m).is_some());
    }

    #[test]
    fn garbage_does_not_panic() {
        let mut m = machine(true);
        let basic = decoder(&m);
        for seed in 0..8u32 {
            for a in 0..0x20000usize {
                m.ram[a] = (a as u32)
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(seed * 977) as u8;
            }
            basic.state(&m);
            basic.program(&m);
            basic.variables(&m);
            basic.stack(&m);
            let _ = basic.array(&m, "A", 0, 100);
            let _ = basic.plan_set(&m, "A", Some(3), &BasicValue::Int(1));
        }
    }
}

/// Simons' BASIC and TSB on top of the image of [`machine`]: a keyword
/// table at `$8100`, the statement dispatch and the handlers of REPEAT,
/// LOOP and EXEC behind it, IGONE pointing into the extension.
mod extension {
    use super::*;

    const TABLE: usize = 0x8100;
    const DISPATCH: usize = 0x8600;
    const HANDLERS: usize = 0x8700;
    /// Token numbers as both extensions have them.
    const REPEAT: u8 = 0x20;
    const EXEC: u8 = 0x33;
    const LOOP: u8 = 0x3a;

    fn words() -> Vec<String> {
        (1..=0x7fu8)
            .map(|n| match n {
                1 => "HIRES".into(),
                REPEAT => "REPEAT".into(),
                EXEC => "EXEC".into(),
                LOOP => "LOOP".into(),
                0x36 => "END LOOP".into(),
                _ => format!("W{n}"),
            })
            .collect()
    }

    /// `tsb`: the table with bit 7 ends and the stacks without the byte
    /// Simons' BASIC skips.
    fn with_extension(tsb: bool) -> Mem {
        let mut m = machine();
        let mut table = Vec::new();
        for w in words() {
            table.extend(w.bytes());
            if tsb {
                *table.last_mut().unwrap() |= 0x80;
            } else {
                table.push(b'@');
            }
        }
        table.push(0);
        m.put(TABLE, &table);
        // LDA table+1,Y : PHA : LDA table,Y : PHA : RTS
        m.put(
            0x85f0,
            &[0xb9, 0x01, 0x86, 0x48, 0xb9, 0x00, 0x86, 0x48, 0x60],
        );
        for (i, (token, count, base)) in [
            (REPEAT, 0xc617u16, 0xc603u16),
            (EXEC, 0xc62c, 0xc618),
            (LOOP, 0xc641, 0xc62d),
        ]
        .into_iter()
        .enumerate()
        {
            let handler = HANDLERS + 0x40 * i;
            m.word(
                DISPATCH + 2 * (usize::from(token) - 1),
                (handler - 1) as u16,
            );
            let [cl, ch] = count.to_le_bytes();
            let [bl, bh] = base.to_le_bytes();
            let mut code = vec![0xac, cl, ch, 0xc0, 0x09, 0x90, 0x05];
            if !tsb {
                code.push(0xc8);
            }
            code.extend([0xa5, 0x7a, 0x99, bl, bh]);
            m.put(handler, &code);
        }
        m.word(0x0308, 0x8800);
        m
    }

    /// Line 10 of the program, rewritten: `LOOP:REPEAT:EXEC`.
    fn program(m: &mut Mem) -> u16 {
        let at = usize::from(line_addr(m, 10));
        let body = [0x64, LOOP, b':', 0x64, REPEAT, b':', 0x64, EXEC, 0];
        m.put(at + 4, &body);
        m.word(at, (at + 4 + body.len()) as u16);
        at as u16 + 4
    }

    #[test]
    fn found_by_its_table_while_the_vector_is_hooked() {
        for (tsb, name) in [(false, "Simons' BASIC"), (true, "TSB")] {
            let mut m = with_extension(tsb);
            let basic = decoder(&m);
            let ext = basic.ext.as_ref().expect("extension");
            assert_eq!(
                (ext.name, ext.table_at, ext.words.len()),
                (name, 0x8100, 0x7f)
            );
            let first = u16::from(!tsb);
            assert_eq!(
                ext.stacks,
                [
                    ExtStack {
                        word: "EXEC",
                        count: 0xc62c,
                        base: 0xc618 + first
                    },
                    ExtStack {
                        word: "REPEAT",
                        count: 0xc617,
                        base: 0xc603 + first
                    },
                    ExtStack {
                        word: "LOOP",
                        count: 0xc641,
                        base: 0xc62d + first
                    },
                ]
            );
            assert_eq!(basic.info().dialect, format!("Commodore BASIC V2 + {name}"));
            assert!(basic.rescan(&m).is_none());
            // The vector back in the ROM: plain BASIC V2 again.
            m.word(0x0308, 0xa7e4);
            let plain = basic.rescan(&m).expect("changed");
            assert!(plain.ext.is_none());
            assert_eq!(plain.info().dialect, "Commodore BASIC V2");
            assert!(plain.rescan(&m).is_none());
            m.word(0x0308, 0x8800);
            assert!(plain.rescan(&m).expect("changed").ext.is_some());
        }
    }

    #[test]
    fn another_wedge_is_looked_at_once() {
        let mut m = machine();
        m.word(0x0308, 0xc000);
        let basic = decoder(&m);
        assert!(basic.ext.is_none());
        assert_eq!(basic.ext_vector, 0xc000);
        assert!(basic.rescan(&m).is_none());
        assert_eq!(basic.info().dialect, "Commodore BASIC V2");
    }

    #[test]
    fn its_tokens_are_two_bytes_whatever_the_second_is() {
        let mut m = with_extension(false);
        program(&mut m);
        let basic = decoder(&m);
        let p = basic.program(&m);
        // LOOP is $64 $3A: no colon.
        assert_eq!(p[0].text, "LOOP:REPEAT:EXEC");
        assert_eq!(p[0].statements, [0, 5, 12]);
        // Plain BASIC V2 would take the $3A for a colon.
        m.word(0x0308, 0xa7e4);
        let plain = basic.rescan(&m).unwrap();
        assert_eq!(plain.program(&m)[0].statements.len(), 4);
        m.word(0x0308, 0x8800);
        // TSB's other spelling of a token, and an unknown one.
        let at = usize::from(line_addr(&m, 10)) + 4;
        m.put(at, &[0x64, LOOP ^ 0x8f]);
        m.put(at + 3, &[0x64, 0x8f]);
        assert_eq!(basic.program(&m)[0].text, "LOOP:{$64}{$8f}:EXEC");
    }

    #[test]
    fn its_stacks_are_frames_behind_the_roms() {
        for tsb in [false, true] {
            let mut m = with_extension(tsb);
            let text = program(&mut m);
            let basic = decoder(&m);
            assert!(basic.stack(&m).is_empty());
            let first = usize::from(!tsb);
            // Two EXECs (their pointers at the line's end and behind the
            // EXEC token), a REPEAT, a LOOP.
            m.word(0xc618 + first, text + 8);
            m.word(0xc618 + first + 2, text + 8);
            m.ram[0xc62c] = 4;
            m.word(0xc603 + first, text + 5);
            m.ram[0xc617] = 2;
            m.word(0xc62d + first, text + 2);
            m.ram[0xc641] = 2;
            let pos = |statement| BasicPos {
                line: 10,
                statement,
            };
            let looped = |word: &str, statement| BasicFrame::Loop {
                word: word.into(),
                from: pos(statement),
            };
            assert_eq!(
                basic.stack(&m),
                [
                    BasicFrame::Gosub { from: pos(2) },
                    BasicFrame::Gosub { from: pos(2) },
                    looped("REPEAT", 1),
                    looped("LOOP", 0),
                ]
            );
            m.word(0x39, 10);
            m.word(0x7a, text);
            assert_eq!(basic.state(&m).depth, 2);
            // A count of garbage reads a bounded number of entries.
            m.ram[0xc617] = 0xff;
            assert_eq!(basic.stack(&m).len(), 2 + 16 + 1);
        }
    }

    const CART: MemSpaceId = MemSpaceId(5);

    /// [`Mem`] with a cartridge whose ROM (`$8000`–`$BFFF`) has a space
    /// of its own.
    struct Plugged {
        m: Mem,
        rom: Vec<u8>,
        plugged: bool,
    }

    impl DebugTarget for Plugged {
        fn regs(&self) -> RegisterSet {
            self.m.regs()
        }

        fn set_reg(&mut self, _name: &str, _v: u32) {}

        fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
            if space != CART {
                return self.m.peek(space, addr, size);
            }
            self.plugged
                .then(|| u32::from(self.rom[(addr as usize).wrapping_sub(0x8000) & 0x3fff]))
        }

        fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
            self.m.poke(space, addr, size, v)
        }

        fn disassemble(&self, addr: u32) -> Instr {
            self.m.disassemble(addr)
        }
    }

    #[test]
    fn found_in_a_cartridge_whose_rom_is_switched_off() {
        // The extension only in the cartridge's ROM: memory shows none of
        // it (the Simons' BASIC cartridge at a stop point, its upper half
        // off; here all of it).
        let m = with_extension(false);
        let rom = m.ram[0x8000..0xc000].to_vec();
        let mut p = Plugged {
            m,
            rom,
            plugged: true,
        };
        p.m.ram[0x8000..0xa000].fill(0);
        let new = || CbmBasic::new(SPACE).with_cartridge(CART);
        let basic = new().rescan(&p).expect("BASIC found");
        let ext = basic.ext.as_ref().expect("extension");
        assert!(ext.in_cart);
        assert_eq!((ext.table_at, ext.stacks.len()), (0x8100, 3));
        assert!(basic.rescan(&p).is_none());
        // Unplugged: gone.
        p.plugged = false;
        let plain = basic.rescan(&p).expect("changed");
        assert!(plain.ext.is_none());
        assert!(plain.rescan(&p).is_none());
        // In memory as well (a cartridge that copies itself to RAM):
        // memory counts. A cartridge bank with the table but without the
        // handlers does not replace it either.
        p.plugged = true;
        p.m = with_extension(false);
        assert!(!new().rescan(&p).unwrap().ext.unwrap().in_cart);
        p.rom[0x0700..0x0800].fill(0);
        assert!(!new().rescan(&p).unwrap().ext.unwrap().in_cart);
    }
}
