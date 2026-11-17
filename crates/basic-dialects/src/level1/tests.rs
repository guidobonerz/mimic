//! The decoder against a hand-built memory image: the two patterns of
//! the ROM, a program of four lines, loops and a GOSUB frame on the stack.

use mimic_core::cpu::{Flow, Instr, Register, RegisterSet};

use super::*;

const SPACE: MemSpaceId = MemSpaceId(0);

struct Mem {
    ram: Vec<u8>,
    de: u16,
    sp: u16,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        let reg = |name, value: u16, bits| Register {
            name,
            value: u32::from(value),
            bits,
        };
        RegisterSet(vec![
            reg("D", self.de >> 8, 8),
            reg("E", self.de & 0xff, 8),
            reg("SP", self.sp, 16),
        ])
    }

    fn set_reg(&mut self, _name: &str, _v: u32) {}

    fn peek(&self, _space: MemSpaceId, addr: u32, _size: Size) -> Option<u32> {
        Some(u32::from(self.ram[addr as usize & 0xffff]))
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
}

const LINES: [(u16, &str); 4] = [
    (10, " A=1:GOSUB 50:P.\"X:Y\""),
    (20, " REM A:B"),
    (30, " D.1:2"),
    (50, " RETURN"),
];

/// The machine and where its four lines are.
fn machine() -> (Mem, [u16; 4]) {
    let mut m = Mem {
        ram: vec![0; 0x10000],
        de: 0,
        sp: STACK_TOP,
    };
    m.put(0x039c, &[0xed, 0x53, 0x9f, 0x40, 0x13, 0x13]);
    m.put(0x08b3, &[0xcf, b':', 0x04, 0xf1, 0xc3]);
    let mut at = TXTBGN;
    let mut addrs = [0; 4];
    for (i, (number, text)) in LINES.into_iter().enumerate() {
        addrs[i] = at;
        m.word(at, number);
        m.put(at + 2, text.as_bytes());
        at += 2 + text.len() as u16;
        m.ram[usize::from(at)] = 0x0d;
        at += 1;
    }
    m.word(TXTUNF, at);
    (m, addrs)
}

fn decoder(m: &Mem) -> Level1 {
    Level1::new(SPACE).rescan(m).expect("BASIC found")
}

fn pos(line: u32, statement: u16) -> BasicPos {
    BasicPos { line, statement }
}

#[test]
fn found_by_its_two_statement_entries() {
    let (m, _) = machine();
    let basic = decoder(&m);
    assert_eq!(basic.stops, Some([0x03a0, 0x08b6]));
    assert_eq!(basic.stop_points(&m).len(), 2);
    assert!(basic.rescan(&m).is_none());
    // Another ROM: nothing.
    let other = Mem {
        ram: vec![0; 0x10000],
        de: 0,
        sp: 0,
    };
    let none = Level1::new(SPACE);
    assert!(none.rescan(&other).is_none());
    assert_eq!(none.state(&other).mode, BasicMode::Inactive);
    assert!(none.stop_points(&other).is_empty());
    assert!(none.program(&other).is_empty());
}

#[test]
fn numbers_as_the_rom_stores_them() {
    // Seen in the machine: 1, -2.5, 100, 0.5 and 0.
    for (bytes, value) in [
        ([0x00, 0x00, 0x80, 0x00], 1.0),
        ([0x00, 0x00, 0x20, 0x81], -2.5),
        ([0x00, 0x00, 0xc8, 0x03], 100.0),
        ([0x00, 0x00, 0x00, 0x00], 0.5),
        ([0x18, 0x00, 0x00, 0x40], 0.0),
    ] {
        assert_eq!(float_value(bytes), value, "{bytes:02x?}");
        if value != 0.0 {
            assert_eq!(float_bytes(value), Ok(bytes), "{value}");
        }
    }
    assert!((float_value([0xcc, 0xcc, 0xcc, 0x7e]) - 0.1).abs() < 1e-8);
    assert_eq!(float_value(float_bytes(0.0).unwrap()), 0.0);
    assert_eq!(float_bytes(1e40), Err(BasicError::OutOfRange));
    assert_eq!(float_value(float_bytes(1e-45).unwrap()), 0.0);
}

#[test]
fn statements_end_at_colons_but_not_behind_rem_and_data() {
    let (m, _) = machine();
    let p = decoder(&m).program(&m);
    assert_eq!(p[0].text, LINES[0].1);
    // The colon in the string does not split.
    assert_eq!(p[0].statements, [0, 5, 14]);
    assert_eq!(p[1].statements, [0]);
    assert_eq!(p[2].statements, [0]);
    assert_eq!(p[0].raw.last(), Some(&0x0d));
    assert_eq!(p.len(), 4);
}

#[test]
fn place_from_the_line_pointer_and_de() {
    let (mut m, lines) = machine();
    let basic = decoder(&m);
    assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    // At the next line's stop DE is on the line.
    m.word(CURRNT, lines[0]);
    m.de = lines[0];
    let s = basic.state(&m);
    assert_eq!((s.mode, s.pos), (BasicMode::Running, Some(pos(10, 0))));
    // Behind the first colon, and behind the second.
    m.de = lines[0] + 2 + 5;
    assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
    m.de = lines[0] + 2 + 14;
    assert_eq!(basic.state(&m).pos, Some(pos(10, 2)));
    // DE somewhere else: the line still holds.
    m.de = 0x1234;
    assert_eq!(basic.state(&m).pos, Some(pos(10, 0)));
}

#[test]
fn the_running_loop_the_saved_ones_and_the_gosub_chain() {
    let (mut m, lines) = machine();
    let basic = decoder(&m);
    // Running: K. On the stack: no loop saved by FOR K, the GOSUB from
    // line 10's second statement, loop J saved by it, no loop saved by
    // FOR J.
    m.word(LOPVAR, VARS + 4 * 10);
    m.word(LOPINC, 1);
    m.word(LOPLMT, 6);
    m.word(LOPLN, lines[3]);
    m.word(LOPPT, lines[3] + 9);
    let sp = STACK_TOP - 20;
    m.word(sp, 0);
    m.word(sp + 2, 0);
    m.word(sp + 4, lines[0]);
    m.word(sp + 6, lines[0] + 2 + 13);
    m.word(sp + 8, VARS + 4 * 9);
    m.word(sp + 10, 0xfffe);
    m.word(sp + 12, 9);
    m.word(sp + 14, lines[0]);
    m.word(sp + 16, lines[0] + 2 + 4);
    m.word(sp + 18, 0);
    m.word(STKGOS, sp + 2);
    m.sp = sp;
    let frames = [
        BasicFrame::For {
            var: "K".into(),
            limit: BasicValue::Int(6),
            step: BasicValue::Int(1),
            from: pos(50, 0),
        },
        BasicFrame::Gosub { from: pos(10, 1) },
        BasicFrame::For {
            var: "J".into(),
            limit: BasicValue::Int(9),
            step: BasicValue::Int(-2),
            from: pos(10, 0),
        },
    ];
    assert_eq!(basic.stack(&m), frames);
    // Inside a statement, with other words below: the same.
    m.sp = sp - 5;
    m.put(sp - 5, &[0x34, 0x12, 0x99, 0x07, 0x03]);
    assert_eq!(basic.stack(&m), frames);
    m.word(CURRNT, lines[3]);
    assert_eq!(basic.state(&m).depth, 1);
    // No chain at all: the GOSUB frames by their links.
    m.word(sp + 18, 0x1111);
    assert_eq!(basic.stack(&m), frames[..2]);
}

#[test]
fn variables_and_writing_them() {
    let (mut m, _) = machine();
    let basic = decoder(&m);
    m.put(VARS + 4, &[0x00, 0x00, 0x20, 0x81]);
    m.put(0x4080, b"HEY\0");
    let vars = basic.variables(&m);
    assert_eq!(vars.len(), 28);
    assert_eq!(
        (vars[1].name.as_str(), vars[1].value.clone()),
        ("B", Some(BasicValue::Float(-2.5)))
    );
    assert_eq!(vars[27].value, Some(BasicValue::Str("HEY".into())));
    for (name, value) in [
        ("Z", BasicValue::Float(0.75)),
        ("B", BasicValue::Int(12)),
        ("B$", BasicValue::Str("YOU".into())),
    ] {
        for (addr, byte) in basic.plan_set(&m, name, None, &value).unwrap() {
            m.ram[addr as usize] = byte;
        }
    }
    let vars = basic.variables(&m);
    assert_eq!(vars[25].value, Some(BasicValue::Float(0.75)));
    assert_eq!(vars[1].value, Some(BasicValue::Float(12.0)));
    assert_eq!(vars[27].value, Some(BasicValue::Str("YOU".into())));
    let set = |name: &str, index, value| basic.plan_set(&m, name, index, &value);
    assert_eq!(
        set("B$", None, BasicValue::Str("LONG".into())),
        Err(BasicError::Unsupported)
    );
    assert_eq!(
        set("B", None, BasicValue::Str("X".into())),
        Err(BasicError::TypeMismatch)
    );
    assert_eq!(
        set("AB", None, BasicValue::Int(1)),
        Err(BasicError::NoSuchVariable)
    );
    assert_eq!(
        set("A", Some(1), BasicValue::Int(1)),
        Err(BasicError::NoSuchVariable)
    );
    assert_eq!(basic.array(&m, "A", 0, 1), Err(BasicError::NoSuchVariable));
}

#[test]
fn garbage_does_not_panic() {
    let (mut m, _) = machine();
    let basic = decoder(&m);
    let mut seed = 0x9e37_79b9u32;
    for round in 0..200 {
        for a in (0x4000..0x4400).step_by(1 + round % 5) {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            m.ram[a] = (seed >> 24) as u8;
        }
        m.sp = (seed >> 8) as u16;
        m.de = seed as u16;
        let _ = basic.state(&m);
        let _ = basic.program(&m);
        let _ = basic.variables(&m);
        let _ = basic.stack(&m);
    }
}
