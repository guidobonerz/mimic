//! The decoder against hand-built memory images: a TRS-80 with its loop
//! at `$1D25` and an MSX with its loop at `$4601`, each with a keyword
//! table of a few words (the tokens follow from the table).

use emu_core::cpu::{Flow, Instr, RegisterSet};

use super::*;

const SPACE: MemSpaceId = MemSpaceId(0);

struct Mem {
    ram: Vec<u8>,
}

impl DebugTarget for Mem {
    fn regs(&self) -> RegisterSet {
        RegisterSet(Vec::new())
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
    fn put(&mut self, addr: usize, bytes: &[u8]) {
        self.ram[addr..addr + bytes.len()].copy_from_slice(bytes);
    }

    fn word(&mut self, addr: usize, v: u16) {
        self.put(addr, &v.to_le_bytes());
    }

    /// Store `lines` from `at`; returns the address behind the program.
    fn program(&mut self, mut at: usize, lines: &[(u16, Vec<u8>)]) -> usize {
        for (number, body) in lines {
            let next = at + 4 + body.len() + 1;
            self.word(at, next as u16);
            self.word(at + 2, *number);
            self.put(at + 4, body);
            self.ram[next - 1] = 0;
            at = next;
        }
        self.word(at, 0);
        at + 2
    }

    fn apply(&mut self, writes: Vec<(u32, u8)>) {
        for (addr, byte) in writes {
            self.ram[addr as usize] = byte;
        }
    }
}

fn pos(line: u32, statement: u16) -> BasicPos {
    BasicPos { line, statement }
}

mod trs80 {
    use super::*;

    const FOR: u8 = 0x81;
    const GOSUB: u8 = 0x83;
    const REM: u8 = 0x85;
    const ELSE: u8 = 0x86;
    const TICK: u8 = 0x87;
    const PRINT: u8 = 0x88;
    const START: usize = 0x42e9;

    /// ```text
    /// 10 PRINT"A:B":GOSUB100'N:M
    /// 20 X ELSE Y
    /// 100 RETURN
    /// ```
    fn machine() -> Mem {
        let mut m = Mem {
            ram: vec![0; 0x10000],
        };
        let mut table = Vec::new();
        for word in [
            "END", "FOR", "NEXT", "GOSUB", "RETURN", "REM", "ELSE", "'", "PRINT",
        ] {
            let at = table.len();
            table.extend(word.bytes());
            table[at] |= 0x80;
        }
        table.push(0x80);
        m.put(0x1650, &table);
        m.put(0x1d25, &[0x22, 0xe6, 0x40, 0xed, 0x73, 0xe8, 0x40]);
        m.put(0x1d5a, &[0xd7, 0x11, 0x1e, 0x1d, 0xd5]);
        let lines = [
            (
                10,
                [
                    &[PRINT][..],
                    b"\"A:B\":",
                    &[GOSUB],
                    b"100:",
                    &[REM, TICK],
                    b"N:M",
                ]
                .concat(),
            ),
            (20, [b"X:", &[ELSE][..], b"Y"].concat()),
            (100, vec![0x84]),
        ];
        let vartab = m.program(START, &lines);
        // N% = -2, S$ = "hey" in the heap, F = -2.5, D# = 1/3, QZ = 4.
        m.put(vartab, &[2, 0, b'N', 0xfe, 0xff]);
        m.put(vartab + 5, &[3, 0, b'S', 3, 0x00, 0x70]);
        m.put(0x7000, b"hey");
        m.put(vartab + 11, &[4, 0, b'F', 0, 0, 0xa0, 0x82]);
        m.put(vartab + 18, &[8, 0, b'D']);
        m.put(
            vartab + 21,
            &[0xab, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0x2a, 0x7f],
        );
        m.put(vartab + 29, &[4, b'Z', b'Q', 0, 0, 0, 0x83]);
        let arytab = vartab + 36;
        // DIM A%(2,1): 3 × 2 integers, sizes stored last dimension first.
        m.put(arytab, &[2, 0, b'A', 17, 0, 2, 2, 0, 3, 0]);
        m.put(arytab + 10 + 2 * 4, &[7, 0]);
        let strend = arytab + 22;
        m.word(0x40a4, START as u16);
        m.word(0x40f9, vartab as u16);
        m.word(0x40fb, arytab as u16);
        m.word(0x40fd, strend as u16);
        m.word(0x40a2, 0xffff);
        m.word(0x40e6, 0x41e8);
        m.word(0x40e8, 0x6ff0);
        m
    }

    fn decoder(m: &Mem) -> Z80Basic {
        Z80Basic::trs80(SPACE, "TRS-80 Level II BASIC")
            .rescan(m)
            .expect("BASIC found")
    }

    #[test]
    fn found_by_the_stores_at_the_loops_top() {
        let m = machine();
        let basic = decoder(&m);
        let rom = basic.rom.as_ref().unwrap();
        assert_eq!(
            (rom.stop, rom.t_for, rom.t_gosub, rom.t_rem, rom.t_tick),
            (0x1d5a, FOR, GOSUB, REM, TICK)
        );
        assert_eq!(basic.info().dialect, "TRS-80 Level II BASIC");
        assert!(basic.rescan(&m).is_none());
        assert_eq!(
            basic.stop_points(&m),
            [BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: SPACE,
                    addr: 0x1d5a
                },
                granularity: BasicGranularity::Statement,
            }]
        );
        // Another ROM: nothing to look at.
        let mut other = machine();
        other.put(0x1d5a, &[0, 0, 0]);
        let gone = basic.rescan(&other).expect("changed");
        assert_eq!(gone.state(&other).mode, BasicMode::Inactive);
        assert!(gone.program(&other).is_empty());
    }

    #[test]
    fn the_listing_hides_the_colons_before_the_tick_and_else() {
        let m = machine();
        let p = decoder(&m).program(&m);
        assert_eq!(p[0].text, "PRINT\"A:B\":GOSUB100'N:M");
        // The tick begins a statement; the colons in the string and
        // behind it do not.
        assert_eq!(p[0].statements, [0, 11, 19]);
        assert_eq!(p[1].text, "XELSEY");
        assert_eq!(p[1].statements, [0, 1]);
        assert_eq!((p[2].number, p[2].text.as_str()), (100, "RETURN"));
        assert_eq!(p[0].raw.last(), Some(&0));
    }

    #[test]
    fn the_place_comes_from_savtxt() {
        let mut m = machine();
        let basic = decoder(&m);
        let l10 = basic.program(&m)[0].addr as usize;
        let l20 = basic.program(&m)[1].addr as usize;
        // Direct mode, whatever SAVTXT says.
        m.word(0x40e6, l10 as u16 - 1);
        assert_eq!(basic.state(&m).mode, BasicMode::Direct);
        // On the 0 before a line: its first statement.
        m.word(0x40a2, 10);
        let s = basic.state(&m);
        assert_eq!((s.mode, s.pos), (BasicMode::Running, Some(pos(10, 0))));
        m.word(0x40e6, l20 as u16 - 1);
        assert_eq!(basic.state(&m).pos, Some(pos(20, 0)));
        // On a colon: the statement behind it.
        m.word(0x40e6, (l10 + 4 + 6) as u16);
        assert_eq!(basic.state(&m).pos, Some(pos(10, 1)));
        m.word(0x40e6, (l10 + 4 + 11) as u16);
        assert_eq!(basic.state(&m).pos, Some(pos(10, 2)));
        // The colon inside the string is no place.
        m.word(0x40e6, (l10 + 4 + 3) as u16);
        assert_eq!(basic.state(&m).mode, BasicMode::Direct);
    }

    #[test]
    fn frames_from_savstk() {
        let mut m = machine();
        let basic = decoder(&m);
        let l10 = basic.program(&m)[0].addr as u16;
        let vartab = u16::from_le_bytes([m.ram[0x40f9], m.ram[0x40fa]]);
        // An integer loop over N% (step 2, limit 9) inside a GOSUB from
        // line 10's second statement.
        let sp = 0x6ff0;
        m.put(sp, &[FOR]);
        m.word(sp + 1, vartab + 3);
        m.put(sp + 3, &[1, 0xff, 0, 0, 0, 0, 2, 0, 9, 0]);
        m.word(sp + 13, 10);
        m.word(sp + 15, l10 + 4 + 8);
        m.put(sp + 17, &[GOSUB, 10, 0]);
        m.word(sp + 20, l10 + 4 + 9);
        let frames = basic.stack(&m);
        assert_eq!(
            frames,
            [
                BasicFrame::For {
                    var: "N%".into(),
                    limit: BasicValue::Int(9),
                    step: BasicValue::Int(2),
                    from: pos(10, 1),
                },
                BasicFrame::Gosub { from: pos(10, 1) },
            ]
        );
        assert_eq!(basic.state(&m).depth, 1);
        // A single precision loop: step and limit as floats.
        m.word(sp + 1, vartab + 11 + 3);
        m.put(sp + 5, &[0, 0, 0, 0x80, 0, 0, 0x40, 0x82]);
        let BasicFrame::For {
            var, limit, step, ..
        } = &basic.stack(&m)[0]
        else {
            panic!("no FOR frame");
        };
        assert_eq!(
            (var.as_str(), limit, step),
            ("F", &BasicValue::Float(3.0), &BasicValue::Float(0.5))
        );
    }

    #[test]
    fn variables_arrays_and_writing_them() {
        let mut m = machine();
        let basic = decoder(&m);
        let vars = basic.variables(&m);
        let named: Vec<(&str, Option<BasicValue>)> = vars
            .iter()
            .map(|v| (v.name.as_str(), v.value.clone()))
            .collect();
        assert_eq!(
            named[..3],
            [
                ("N%", Some(BasicValue::Int(-2))),
                ("S$", Some(BasicValue::Str("hey".into()))),
                ("F", Some(BasicValue::Float(-2.5))),
            ]
        );
        let BasicValue::Float(third) = named[3].1.clone().unwrap() else {
            panic!("D# is no float");
        };
        assert_eq!(named[3].0, "D#");
        assert!((third - 1.0 / 3.0).abs() < 1e-15, "{third}");
        // The second character is stored first.
        assert_eq!(named[4], ("QZ", Some(BasicValue::Float(4.0))));
        assert_eq!((named[5].0, &vars[5].dims[..]), ("A%", &[3, 2][..]));
        assert_eq!(
            basic.array(&m, "A%", 3, 9),
            Ok(vec![
                BasicValue::Int(0),
                BasicValue::Int(7),
                BasicValue::Int(0)
            ])
        );
        for (name, value) in [
            ("N%", BasicValue::Int(300)),
            ("F", BasicValue::Float(0.1)),
            ("D#", BasicValue::Float(-123456.789)),
            ("S$", BasicValue::Str("YOU".into())),
        ] {
            let writes = basic.plan_set(&m, name, None, &value).unwrap();
            m.apply(writes);
            let now = basic.variables(&m);
            let got = now.iter().find(|v| v.name == name).unwrap().value.clone();
            match (got.unwrap(), &value) {
                (BasicValue::Float(a), BasicValue::Float(b)) => {
                    assert!((a - b).abs() < b.abs() * 1e-7, "{name}: {a}")
                }
                (a, b) => assert_eq!(&a, b, "{name}"),
            }
        }
        let writes = basic
            .plan_set(&m, "A%", Some(5), &BasicValue::Int(-1))
            .unwrap();
        m.apply(writes);
        assert_eq!(basic.array(&m, "A%", 5, 1), Ok(vec![BasicValue::Int(-1)]));
        let set = |name: &str, index, value| basic.plan_set(&m, name, index, &value);
        assert_eq!(
            set("S$", None, BasicValue::Str("LONGER".into())),
            Err(BasicError::Unsupported)
        );
        assert_eq!(
            set("N%", None, BasicValue::Int(40000)),
            Err(BasicError::OutOfRange)
        );
        assert_eq!(
            set("N%", None, BasicValue::Str("X".into())),
            Err(BasicError::TypeMismatch)
        );
        assert_eq!(
            set("A%", Some(6), BasicValue::Int(1)),
            Err(BasicError::OutOfRange)
        );
        assert_eq!(
            set("ZZ", None, BasicValue::Int(1)),
            Err(BasicError::NoSuchVariable)
        );
    }

    #[test]
    fn garbage_does_not_panic() {
        let mut m = machine();
        let basic = decoder(&m);
        let mut seed = 0x1234_5678u32;
        for round in 0..200 {
            for a in (0x4000..0x8000).step_by(1 + round % 7) {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                m.ram[a] = (seed >> 24) as u8;
            }
            let _ = basic.state(&m);
            let _ = basic.program(&m);
            let _ = basic.variables(&m);
            let _ = basic.stack(&m);
            let _ = basic.array(&m, "A%", 0, 100);
            let _ = basic.plan_set(&m, "N%", None, &BasicValue::Int(1));
        }
    }
}

mod msx {
    use super::*;

    const FOR: u8 = 0x82;
    const DATA: u8 = 0x84;
    const GOTO: u8 = 0x89;
    const GOSUB: u8 = 0x8d;
    const REM: u8 = 0x8f;
    const PRINT: u8 = 0x91;
    const ELSE: u8 = 0xa1;
    const TICK: u8 = 0xe6;
    const START: usize = 0x8001;

    fn machine() -> Mem {
        let mut m = Mem {
            ram: vec![0; 0x10000],
        };
        // The table: per letter the words without it, then the signs.
        let groups: [(u8, &[(&str, u8)]); 9] = [
            (b'A', &[("UTO", 0xa9), ("ND", 0xf6)]),
            (b'D', &[("ATA", DATA)]),
            (b'E', &[("LSE", ELSE)]),
            (b'F', &[("OR", FOR)]),
            (b'G', &[("OTO", GOTO), ("O TO", GOTO), ("OSUB", GOSUB)]),
            (b'L', &[("EFT$", 0x01)]),
            (b'P', &[("RINT", PRINT)]),
            (b'R', &[("EM", REM)]),
            (b'[', &[("'", TICK), ("+", 0xf1)]),
        ];
        let mut table = Vec::new();
        let mut letter = b'A';
        for (group, words) in groups {
            while letter < group {
                table.push(0);
                letter += 1;
            }
            for (word, token) in words {
                table.extend(word.bytes());
                *table.last_mut().unwrap() |= 0x80;
                table.push(*token);
            }
        }
        table.push(0);
        m.put(0x3a72, &table);
        m.put(0x4604, &[0xed, 0x73, 0xb1, 0xf6]);
        m.put(0x4613, &[0x22, 0xaf, 0xf6]);
        m.put(0x4640, &[0xd7, 0x11, 0x01, 0x46, 0xd5]);
        m.word(0xf676, START as u16);
        m.ram[START - 1] = 0;
        m.word(0xf41c, 0xffff);
        m.word(0xf6b1, 0xe000);
        m
    }

    fn decoder(m: &Mem) -> Z80Basic {
        Z80Basic::msx(SPACE, "MSX BASIC 1.0")
            .rescan(m)
            .expect("BASIC found")
    }

    fn end_of_variables(m: &mut Mem, at: usize) {
        for zp in [0xf6c2, 0xf6c4, 0xf6c6] {
            m.word(zp, at as u16);
        }
    }

    #[test]
    fn the_listing_writes_out_the_packed_constants() {
        let mut m = machine();
        let single = [0x41, 0x15, 0x00, 0x00];
        let double = [0x41, 0x15, 0, 0, 0, 0, 0, 0];
        let lines = [
            // PRINT 200+&H1F+&O17+5+12345+1.5+1.5#, a colon in a constant.
            (
                10,
                [
                    &[PRINT, 0x0f, 200, 0xf1, 0x0c, 0x1f, 0, 0xf1, 0x0b, 15, 0][..],
                    &[0xf1, 0x16, 0xf1, 0x1c, 0x39, 0x30, 0xf1, 0x1d],
                    &single,
                    &[0xf1, 0x1f],
                    &double,
                    &[b':', PRINT, 0x1c, b':', 0],
                ]
                .concat(),
            ),
            // GOTO with a line number and with the pointer it becomes (to
            // the byte before the line).
            (20, vec![GOTO, 0x0e, 10, 0, b':', GOTO, 0x0d, 0x00, 0x80]),
            // Text behind DATA, in quotes and behind REM stays as it is.
            (
                30,
                [
                    &[DATA, 0x91, b':', PRINT][..],
                    b"\"",
                    &[0x0f],
                    b"\":",
                    &[REM, 0x91, b':'],
                ]
                .concat(),
            ),
            (
                40,
                vec![b'X', b':', ELSE, 0xff, 0x81, b':', REM, TICK, b'T'],
            ),
        ];
        let end = m.program(START, &lines);
        end_of_variables(&mut m, end);
        let p = decoder(&m).program(&m);
        assert_eq!(p[0].text, "PRINT200+&H1F+&O17+5+12345+1.5+1.5#:PRINT58");
        assert_eq!(p[0].statements, [0, 36]);
        assert_eq!(p[1].text, "GOTO10:GOTO10");
        assert_eq!(p[2].text, "DATA{$91}:PRINT\"{$0f}\":REM{$91}:");
        assert_eq!(p[2].statements, [0, 10, 23]);
        // GOTO is the table's first word for its token; LEFT$ a function.
        assert_eq!(p[3].text, "XELSELEFT$'T");
        assert_eq!(p[3].statements, [0, 1, 10]);
        // The lines end where the links say, not at a constant's zero.
        assert_eq!(p.len(), 4);
    }

    #[test]
    fn floats_are_decimal() {
        let text = |bytes: &[u8]| bcd_text(bytes);
        assert_eq!(text(&[0x41, 0x25, 0, 0]), "2.5");
        assert_eq!(text(&[0x40, 0x50, 0, 0]), ".5");
        assert_eq!(text(&[0x3f, 0x10, 0, 0]), ".01");
        assert_eq!(text(&[0x3e, 0x10, 0, 0]), "1E-03");
        assert_eq!(text(&[0xba, 0x35, 0, 0]), "-3.5E-07");
        assert_eq!(text(&[0x46, 0x12, 0x34, 0x56]), "123456");
        assert_eq!(text(&[0x4b, 0x10, 0, 0]), "10000000000");
        assert_eq!(text(&[0x4f, 0x10, 0, 0]), "1E+14");
        assert_eq!(text(&[0x4f, 0x12, 0, 0, 0, 0, 0, 0]), "1.2D+14");
        assert_eq!(text(&[0, 0x99, 0, 0]), "0");
        assert_eq!(bcd_value(&[0xc1, 0x25, 0, 0]), -2.5);
        for v in [1.0, -0.0125, 123456.0, 1e-20, 9.99999e30, 0.0] {
            assert_eq!(bcd_value(&bcd_bytes(v, 4).unwrap()), v, "{v}");
        }
        assert_eq!(
            bcd_bytes(1.0 / 3.0, 8).unwrap(),
            [0x40, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33]
        );
        assert_eq!(bcd_bytes(1e70, 4), Err(BasicError::OutOfRange));
        assert_eq!(bcd_bytes(1e-70, 4), Ok(vec![0; 4]));
    }

    #[test]
    fn frames_and_variables() {
        let mut m = machine();
        let lines = [(10, vec![b'X', b':', GOSUB, 0x0e, 20, 0]), (20, vec![b'Y'])];
        let vartab = m.program(START, &lines);
        // AB! = 2.5 (the first character first), N% = 7, D = -0.5.
        m.put(vartab, &[4, b'A', b'B', 0x41, 0x25, 0, 0]);
        m.put(vartab + 7, &[2, b'N', 0, 7, 0]);
        m.put(vartab + 12, &[8, b'D', 0, 0xc0, 0x50, 0, 0, 0, 0, 0, 0]);
        end_of_variables(&mut m, vartab + 23);
        m.word(0xf6c2, vartab as u16);
        let basic = decoder(&m);
        let named: Vec<(String, Option<BasicValue>)> = basic
            .variables(&m)
            .into_iter()
            .map(|v| (v.name, v.value))
            .collect();
        assert_eq!(
            named,
            [
                ("AB!".to_string(), Some(BasicValue::Float(2.5))),
                ("N%".to_string(), Some(BasicValue::Int(7))),
                ("D".to_string(), Some(BasicValue::Float(-0.5))),
            ]
        );
        let writes = basic
            .plan_set(&m, "D", None, &BasicValue::Float(12.25))
            .unwrap();
        m.apply(writes);
        assert_eq!(basic.variables(&m)[2].value, Some(BasicValue::Float(12.25)));
        // A single precision loop over AB! (step 0.5, limit 3: the limit's
        // halves swapped) inside a GOSUB with its trap word.
        let l10 = START as u16;
        let sp = 0xe000;
        m.put(sp, &[FOR]);
        m.word(sp + 1, vartab as u16 + 3);
        m.put(sp + 13, &[0x40, 0x50, 0, 0]);
        m.put(sp + 17, &[0, 0, 0x41, 0x30]);
        m.word(sp + 21, 10);
        m.word(sp + 23, l10 + 4);
        m.put(sp + 25, &[GOSUB, 0, 0, 10, 0]);
        m.word(sp + 30, l10 + 4 + 3);
        assert_eq!(
            basic.stack(&m),
            [
                BasicFrame::For {
                    var: "AB!".into(),
                    limit: BasicValue::Float(3.0),
                    step: BasicValue::Float(0.5),
                    from: pos(10, 0),
                },
                BasicFrame::Gosub { from: pos(10, 1) },
            ]
        );
        m.word(0xf41c, 20);
        m.word(0xf6af, l10 + 4 + 6);
        let s = basic.state(&m);
        assert_eq!(
            (s.mode, s.pos, s.depth),
            (BasicMode::Running, Some(pos(20, 0)), 1)
        );
    }
}
