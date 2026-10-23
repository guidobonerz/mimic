//! 68000/68020 disassembler (Motorola syntax), for the debugger: reads
//! through `peek` only. Opcode layout from the M68000 Family Programmer's
//! Reference Manual, chapter 8 ("Instruction format summary"); the 68020
//! forms (full-format extension words, the added instructions) are decoded
//! only for a 68020.

use emu_core::{Bus, Flow, Instr, Size};

const CC: [&str; 16] = [
    "T", "F", "HI", "LS", "CC", "CS", "NE", "EQ", "VC", "VS", "PL", "MI", "GE", "LT", "GT", "LE",
];

/// Reads instruction words after the opcode.
struct Words<'a> {
    bus: &'a dyn Bus,
    start: u32,
    pc: u32,
    /// Decode the 68020's forms.
    m020: bool,
}

impl Words<'_> {
    fn word(&mut self) -> u16 {
        let w = self.bus.peek(self.pc & 0xff_ffff, Size::Word) as u16;
        self.pc = self.pc.wrapping_add(2);
        w
    }

    fn long(&mut self) -> u32 {
        u32::from(self.word()) << 16 | u32::from(self.word())
    }

    fn bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        let mut a = self.start;
        while a != self.pc {
            let w = self.bus.peek(a & 0xff_ffff, Size::Word) as u16;
            v.extend([(w >> 8) as u8, w as u8]);
            a = a.wrapping_add(2);
        }
        v
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sz {
    B,
    W,
    L,
}

impl Sz {
    fn suffix(self) -> &'static str {
        match self {
            Sz::B => ".B",
            Sz::W => ".W",
            Sz::L => ".L",
        }
    }

    /// The common size field (0 byte, 1 word, 2 long).
    fn from_bits(bits: u16) -> Option<Sz> {
        match bits & 3 {
            0 => Some(Sz::B),
            1 => Some(Sz::W),
            2 => Some(Sz::L),
            _ => None,
        }
    }
}

fn hex(v: u32) -> String {
    format!("${v:X}")
}

fn signed_hex(v: i32) -> String {
    if v < 0 {
        format!("-${:X}", v.unsigned_abs())
    } else {
        format!("${v:X}")
    }
}

/// Index register of an extension word: "D3.W", "A1.L", with the 68020's
/// scale ("D1.L*4").
fn index(ext: u16, m020: bool) -> String {
    let kind = if ext & 0x8000 != 0 { 'A' } else { 'D' };
    let size = if ext & 0x0800 != 0 { 'L' } else { 'W' };
    let scale = match (m020, (ext >> 9) & 3) {
        (true, 1) => "*2",
        (true, 2) => "*4",
        (true, 3) => "*8",
        _ => "",
    };
    format!("{kind}{}.{size}{scale}", (ext >> 12) & 7)
}

/// A 68020 full-format indexed mode after its first extension word `ext`;
/// `base` is "A0" or "PC". Returns `None` for a reserved encoding.
fn full_format(w: &mut Words, base: &str, ext: u16) -> Option<String> {
    let mut inner = Vec::new();
    let bd = match (ext >> 4) & 3 {
        0 => return None,
        2 => Some(signed_hex(i32::from(w.word() as i16))),
        3 => Some(signed_hex(w.long() as i32)),
        _ => None,
    };
    if let Some(bd) = bd {
        inner.push(bd);
    }
    if ext & 0x80 == 0 {
        inner.push(base.to_string());
    } else if bd_is_none(ext) {
        inner.push(format!("Z{base}"));
    }
    let idx = (ext & 0x40 == 0).then(|| index(ext, true));
    let iis = ext & 7;
    if iis == 0 {
        if let Some(i) = idx {
            inner.push(i);
        }
        return Some(format!("({})", inner.join(",")));
    }
    if ext & 0x40 != 0 && iis & 4 != 0 {
        return None;
    }
    let od = match iis & 3 {
        0 => return None,
        2 => Some(signed_hex(i32::from(w.word() as i16))),
        3 => Some(signed_hex(w.long() as i32)),
        _ => None,
    };
    let mut outer = Vec::new();
    if iis & 4 != 0 {
        outer.push(format!("[{}]", inner.join(",")));
        if let Some(i) = idx {
            outer.push(i);
        }
    } else {
        if let Some(i) = idx {
            inner.push(i);
        }
        outer.push(format!("[{}]", inner.join(",")));
    }
    if let Some(od) = od {
        outer.push(od);
    }
    Some(format!("({})", outer.join(",")))
}

fn bd_is_none(ext: u16) -> bool {
    (ext >> 4) & 3 == 1
}

/// Effective address text; `None` for an invalid mode. `target` receives
/// the absolute address of PC-relative and absolute modes (for JMP/JSR).
fn ea(w: &mut Words, m: u16, r: u16, sz: Sz, target: &mut Option<u32>) -> Option<String> {
    Some(match m {
        0 => format!("D{r}"),
        1 => format!("A{r}"),
        2 => format!("(A{r})"),
        3 => format!("(A{r})+"),
        4 => format!("-(A{r})"),
        5 => format!("{}(A{r})", signed_hex(i32::from(w.word() as i16))),
        6 => {
            let ext = w.word();
            if w.m020 && ext & 0x100 != 0 {
                return full_format(w, &format!("A{r}"), ext);
            }
            format!(
                "{}(A{r},{})",
                signed_hex(i32::from(ext as u8 as i8)),
                index(ext, w.m020)
            )
        }
        7 => match r {
            0 => {
                let a = w.word() as i16 as i32 as u32 & 0xff_ffff;
                *target = Some(a);
                format!("{}.W", hex(a))
            }
            1 => {
                let a = w.long() & 0xff_ffff;
                *target = Some(a);
                format!("{}.L", hex(a))
            }
            2 => {
                let base = w.pc;
                let a = base.wrapping_add(w.word() as i16 as i32 as u32) & 0xff_ffff;
                *target = Some(a);
                format!("{}(PC)", hex(a))
            }
            3 => {
                let base = w.pc;
                let ext = w.word();
                if w.m020 && ext & 0x100 != 0 {
                    return full_format(w, "PC", ext);
                }
                let a = base.wrapping_add(ext as u8 as i8 as i32 as u32) & 0xff_ffff;
                format!("{}(PC,{})", hex(a), index(ext, w.m020))
            }
            4 => match sz {
                Sz::B => format!("#{}", hex(u32::from(w.word() & 0xff))),
                Sz::W => format!("#{}", hex(u32::from(w.word()))),
                Sz::L => format!("#{}", hex(w.long())),
            },
            _ => return None,
        },
        _ => return None,
    })
}

/// Register list of MOVEM; `reversed` for -(An) (bit 0 = A7).
fn reglist(mask: u16, reversed: bool) -> String {
    let bit = |i: u16| {
        if reversed {
            mask & (0x8000 >> i) != 0
        } else {
            mask & (1 << i) != 0
        }
    };
    let mut parts = Vec::new();
    for (base, kind) in [(0u16, 'D'), (8, 'A')] {
        let mut i = 0;
        while i < 8 {
            if !bit(base + i) {
                i += 1;
                continue;
            }
            let first = i;
            while i + 1 < 8 && bit(base + i + 1) {
                i += 1;
            }
            parts.push(if first == i {
                format!("{kind}{first}")
            } else {
                format!("{kind}{first}-{kind}{i}")
            });
            i += 1;
        }
    }
    parts.join("/")
}

struct Out {
    mnemonic: String,
    operand: String,
    target: Option<u32>,
    flow: Flow,
}

fn out(mnemonic: impl Into<String>, operand: impl Into<String>) -> Out {
    Out {
        mnemonic: mnemonic.into(),
        operand: operand.into(),
        target: None,
        flow: Flow::Normal,
    }
}

/// Disassemble the instruction at `addr` using peeks only; `m020` adds
/// the 68020's forms.
pub(crate) fn disassemble(bus: &dyn Bus, addr: u32, m020: bool) -> Instr {
    let addr = addr & 0xff_ffff;
    let mut w = Words {
        bus,
        start: addr,
        pc: addr,
        m020,
    };
    let op = w.word();
    let o = if m020 { decode020(&mut w, op) } else { None }
        .or_else(|| decode(&mut w, op))
        .unwrap_or_else(|| {
            w.pc = addr.wrapping_add(2);
            out("DC.W", hex(u32::from(op)))
        });
    Instr {
        addr,
        bytes: w.bytes(),
        mnemonic: o.mnemonic,
        operand: o.operand,
        target: o.target,
        flow: o.flow,
    }
}

/// The 68020's instructions and forms; `None` for everything else (the
/// stream position is restored by the caller's retry).
fn decode020(w: &mut Words, op: u16) -> Option<Out> {
    let start = w.pc;
    let r = decode020_inner(w, op);
    if r.is_none() {
        w.pc = start;
    }
    r
}

fn decode020_inner(w: &mut Words, op: u16) -> Option<Out> {
    let m = (op >> 3) & 7;
    let r = op & 7;
    let rx = (op >> 9) & 7;
    let mut t = None;
    let reg = |ext: u16| {
        let kind = if ext & 0x8000 != 0 { 'A' } else { 'D' };
        format!("{kind}{}", (ext >> 12) & 7)
    };
    Some(match op >> 12 {
        0x0 if op & 0x0100 == 0 && op & 0xc0 == 0xc0 && rx <= 2 => {
            let ext = w.word();
            let mn = if ext & 0x0800 != 0 { "CHK2" } else { "CMP2" };
            let sz = Sz::from_bits(rx)?;
            out(
                format!("{mn}{}", sz.suffix()),
                format!("{},{}", ea(w, m, r, sz, &mut t)?, reg(ext)),
            )
        }
        0x0 if op & 0x0100 == 0 && op & 0xc0 == 0xc0 && rx >= 5 => {
            let sz = match rx {
                5 => Sz::B,
                6 => Sz::W,
                _ => Sz::L,
            };
            if op & 0x3f == 0x3c && rx != 5 {
                let e1 = w.word();
                let e2 = w.word();
                out(
                    format!("CAS2{}", sz.suffix()),
                    format!(
                        "D{}:D{},D{}:D{},({}):({})",
                        e1 & 7,
                        e2 & 7,
                        (e1 >> 6) & 7,
                        (e2 >> 6) & 7,
                        reg(e1),
                        reg(e2)
                    ),
                )
            } else {
                let ext = w.word();
                out(
                    format!("CAS{}", sz.suffix()),
                    format!(
                        "D{},D{},{}",
                        ext & 7,
                        (ext >> 6) & 7,
                        ea(w, m, r, sz, &mut t)?
                    ),
                )
            }
        }
        0x0 if op & 0x0f00 == 0x0e00 => {
            let sz = Sz::from_bits(op >> 6)?;
            let ext = w.word();
            let e = ea(w, m, r, sz, &mut t)?;
            let operand = if ext & 0x0800 != 0 {
                format!("{},{e}", reg(ext))
            } else {
                format!("{e},{}", reg(ext))
            };
            out(format!("MOVES{}", sz.suffix()), operand)
        }
        0x4 => match op {
            0x4e74 => Out {
                flow: Flow::Return,
                ..out(
                    "RTD",
                    format!("#{}", signed_hex(i32::from(w.word() as i16))),
                )
            },
            0x4e7a | 0x4e7b => {
                let ext = w.word();
                let control = match ext & 0xfff {
                    0x000 => "SFC",
                    0x001 => "DFC",
                    0x002 => "CACR",
                    0x800 => "USP",
                    0x801 => "VBR",
                    0x802 => "CAAR",
                    0x803 => "MSP",
                    0x804 => "ISP",
                    _ => return None,
                };
                if op & 1 != 0 {
                    out("MOVEC", format!("{},{control}", reg(ext)))
                } else {
                    out("MOVEC", format!("{control},{}", reg(ext)))
                }
            }
            _ => match op & 0xfff8 {
                0x4808 => out("LINK.L", format!("A{r},#{}", signed_hex(w.long() as i32))),
                0x4848 => out("BKPT", format!("#{r}")),
                0x49c0 => out("EXTB.L", format!("D{r}")),
                _ => match op & 0xffc0 {
                    0x42c0 => out("MOVE", format!("CCR,{}", ea(w, m, r, Sz::W, &mut t)?)),
                    0x4c00 | 0x4c40 => {
                        let ext = w.word();
                        let signed = if ext & 0x0800 != 0 { "S" } else { "U" };
                        let wide = ext & 0x0400 != 0;
                        let (dl, dh) = ((ext >> 12) & 7, ext & 7);
                        let e = ea(w, m, r, Sz::L, &mut t)?;
                        if op & 0x40 == 0 {
                            if wide {
                                out(format!("MUL{signed}.L"), format!("{e},D{dh}:D{dl}"))
                            } else {
                                out(format!("MUL{signed}.L"), format!("{e},D{dl}"))
                            }
                        } else if wide {
                            out(format!("DIV{signed}.L"), format!("{e},D{dh}:D{dl}"))
                        } else if dh != dl {
                            out(format!("DIV{signed}L.L"), format!("{e},D{dh}:D{dl}"))
                        } else {
                            out(format!("DIV{signed}.L"), format!("{e},D{dl}"))
                        }
                    }
                    _ if op & 0x01c0 == 0x0100 => {
                        out("CHK.L", format!("{},D{rx}", ea(w, m, r, Sz::L, &mut t)?))
                    }
                    _ => return None,
                },
            },
        },
        0x5 if op & 0xf0f8 == 0x50f8 && (2..=4).contains(&r) => {
            let cc = CC[usize::from((op >> 8) & 15)];
            match r {
                2 => out(
                    format!("TRAP{cc}.W"),
                    format!("#{}", hex(u32::from(w.word()))),
                ),
                3 => out(format!("TRAP{cc}.L"), format!("#{}", hex(w.long()))),
                _ => out(format!("TRAP{cc}"), ""),
            }
        }
        0x6 if op as u8 == 0xff => {
            let base = w.pc;
            let d = w.long();
            let target = base.wrapping_add(d) & 0xff_ffff;
            let (mn, flow) = match (op >> 8) & 15 {
                0 => ("BRA".to_string(), Flow::Jump),
                1 => ("BSR".to_string(), Flow::Call),
                c => (format!("B{}", CC[usize::from(c)]), Flow::Branch),
            };
            Out {
                mnemonic: format!("{mn}.L"),
                operand: hex(target),
                target: Some(target),
                flow,
            }
        }
        0x8 if op & 0x01f0 == 0x0140 || op & 0x01f0 == 0x0180 => {
            let mn = if op & 0x40 != 0 { "PACK" } else { "UNPK" };
            let adj = hex(u32::from(w.word()));
            if op & 8 == 0 {
                out(mn, format!("D{r},D{rx},#{adj}"))
            } else {
                out(mn, format!("-(A{r}),-(A{rx}),#{adj}"))
            }
        }
        0xe if op & 0x08c0 == 0x08c0 => {
            const BF: [&str; 8] = [
                "BFTST", "BFEXTU", "BFCHG", "BFEXTS", "BFCLR", "BFFFO", "BFSET", "BFINS",
            ];
            let kind = usize::from((op >> 8) & 7);
            let ext = w.word();
            let offset = if ext & 0x0800 != 0 {
                format!("D{}", (ext >> 6) & 7)
            } else {
                format!("{}", (ext >> 6) & 31)
            };
            let width = if ext & 0x0020 != 0 {
                format!("D{}", ext & 7)
            } else {
                format!("{}", if ext & 31 == 0 { 32 } else { ext & 31 })
            };
            let field = format!("{}{{{offset}:{width}}}", ea(w, m, r, Sz::L, &mut t)?);
            let dn = (ext >> 12) & 7;
            match kind {
                1 | 3 | 5 => out(BF[kind], format!("{field},D{dn}")),
                7 => out(BF[kind], format!("D{dn},{field}")),
                _ => out(BF[kind], field),
            }
        }
        _ => return None,
    })
}

fn decode(w: &mut Words, op: u16) -> Option<Out> {
    let m = (op >> 3) & 7;
    let r = op & 7;
    let rx = (op >> 9) & 7;
    let opmode = (op >> 6) & 7;
    let mut t = None;
    Some(match op >> 12 {
        0x0 => group0(w, op)?,
        0x1..=0x3 => {
            let sz = match op >> 12 {
                1 => Sz::B,
                3 => Sz::W,
                _ => Sz::L,
            };
            let src = ea(w, m, r, sz, &mut t)?;
            let dm = (op >> 6) & 7;
            if dm == 1 {
                if sz == Sz::B {
                    return None;
                }
                out(format!("MOVEA{}", sz.suffix()), format!("{src},A{rx}"))
            } else {
                if dm == 7 && rx > 1 {
                    return None;
                }
                let dst = ea(w, dm, rx, sz, &mut t)?;
                out(format!("MOVE{}", sz.suffix()), format!("{src},{dst}"))
            }
        }
        0x4 => group4(w, op)?,
        0x5 => {
            if op & 0xc0 == 0xc0 {
                let cc = CC[usize::from((op >> 8) & 15)];
                if m == 1 {
                    let base = w.pc;
                    let d = w.word() as i16 as i32 as u32;
                    let target = base.wrapping_add(d) & 0xff_ffff;
                    let mn = if cc == "F" {
                        "DBRA".into()
                    } else {
                        format!("DB{cc}")
                    };
                    Out {
                        mnemonic: mn,
                        operand: format!("D{r},{}", hex(target)),
                        target: Some(target),
                        flow: Flow::Branch,
                    }
                } else {
                    out(format!("S{cc}"), ea(w, m, r, Sz::B, &mut t)?)
                }
            } else {
                let sz = Sz::from_bits(op >> 6)?;
                let q = if rx == 0 { 8 } else { rx };
                let mn = if op & 0x100 != 0 { "SUBQ" } else { "ADDQ" };
                out(
                    format!("{mn}{}", sz.suffix()),
                    format!("#{q},{}", ea(w, m, r, sz, &mut t)?),
                )
            }
        }
        0x6 => {
            let base = w.pc;
            let d = match op as u8 {
                0 => w.word() as i16 as i32 as u32,
                b => b as i8 as i32 as u32,
            };
            let target = base.wrapping_add(d) & 0xff_ffff;
            let short = if op as u8 == 0 { "" } else { ".S" };
            let (mn, flow) = match (op >> 8) & 15 {
                0 => ("BRA".to_string(), Flow::Jump),
                1 => ("BSR".to_string(), Flow::Call),
                c => (format!("B{}", CC[usize::from(c)]), Flow::Branch),
            };
            Out {
                mnemonic: format!("{mn}{short}"),
                operand: hex(target),
                target: Some(target),
                flow,
            }
        }
        0x7 => {
            if op & 0x100 != 0 {
                return None;
            }
            out(
                "MOVEQ",
                format!("#{},D{rx}", signed_hex(i32::from(op as u8 as i8))),
            )
        }
        0x8 | 0xc => {
            let logic = if op >> 12 == 0x8 { "OR" } else { "AND" };
            match opmode {
                3 | 7 => {
                    let mn = match (op >> 12, opmode) {
                        (0x8, 3) => "DIVU.W",
                        (0x8, _) => "DIVS.W",
                        (_, 3) => "MULU.W",
                        _ => "MULS.W",
                    };
                    out(mn, format!("{},D{rx}", ea(w, m, r, Sz::W, &mut t)?))
                }
                4 if m < 2 => {
                    let mn = if op >> 12 == 0x8 { "SBCD" } else { "ABCD" };
                    out(mn, xform(m, r, rx))
                }
                5 | 6 if op >> 12 == 0xc && m < 2 && (opmode, m) != (6, 0) => match (opmode, m) {
                    (5, 0) => out("EXG", format!("D{rx},D{r}")),
                    (5, _) => out("EXG", format!("A{rx},A{r}")),
                    _ => out("EXG", format!("D{rx},A{r}")),
                },
                _ => arith(w, op, logic)?,
            }
        }
        0x9 | 0xd => {
            let mn = if op >> 12 == 0x9 { "SUB" } else { "ADD" };
            match opmode {
                3 | 7 => {
                    let sz = if opmode == 3 { Sz::W } else { Sz::L };
                    out(
                        format!("{mn}A{}", sz.suffix()),
                        format!("{},A{rx}", ea(w, m, r, sz, &mut t)?),
                    )
                }
                4..=6 if m < 2 => out(
                    format!("{mn}X{}", Sz::from_bits(opmode)?.suffix()),
                    xform(m, r, rx),
                ),
                _ => arith(w, op, mn)?,
            }
        }
        0xb => match opmode {
            3 | 7 => {
                let sz = if opmode == 3 { Sz::W } else { Sz::L };
                out(
                    format!("CMPA{}", sz.suffix()),
                    format!("{},A{rx}", ea(w, m, r, sz, &mut t)?),
                )
            }
            0..=2 => {
                let sz = Sz::from_bits(opmode)?;
                out(
                    format!("CMP{}", sz.suffix()),
                    format!("{},D{rx}", ea(w, m, r, sz, &mut t)?),
                )
            }
            _ => {
                let sz = Sz::from_bits(opmode)?;
                if m == 1 {
                    out(format!("CMPM{}", sz.suffix()), format!("(A{r})+,(A{rx})+"))
                } else {
                    out(
                        format!("EOR{}", sz.suffix()),
                        format!("D{rx},{}", ea(w, m, r, sz, &mut t)?),
                    )
                }
            }
        },
        0xa => out("DC.W", format!("{} ; line A", hex(u32::from(op)))),
        0xe => shift(w, op)?,
        _ => out("DC.W", format!("{} ; line F", hex(u32::from(op)))),
    })
}

/// "Dy,Dx" or "-(Ay),-(Ax)" of ABCD/SBCD/ADDX/SUBX.
fn xform(m: u16, r: u16, rx: u16) -> String {
    if m == 0 {
        format!("D{r},D{rx}")
    } else {
        format!("-(A{r}),-(A{rx})")
    }
}

/// OR/AND/ADD/SUB in their "<ea>,Dn" and "Dn,<ea>" forms.
fn arith(w: &mut Words, op: u16, mn: &str) -> Option<Out> {
    let opmode = (op >> 6) & 7;
    let sz = Sz::from_bits(opmode)?;
    let rx = (op >> 9) & 7;
    let mut t = None;
    let e = ea(w, (op >> 3) & 7, op & 7, sz, &mut t)?;
    let operand = if opmode & 4 == 0 {
        format!("{e},D{rx}")
    } else {
        format!("D{rx},{e}")
    };
    Some(out(format!("{mn}{}", sz.suffix()), operand))
}

/// Bit operations, MOVEP and the immediate instructions.
fn group0(w: &mut Words, op: u16) -> Option<Out> {
    let m = (op >> 3) & 7;
    let r = op & 7;
    let mut t = None;
    const BIT: [&str; 4] = ["BTST", "BCHG", "BCLR", "BSET"];
    if op & 0x100 != 0 {
        let rx = (op >> 9) & 7;
        if m == 1 {
            let d = signed_hex(i32::from(w.word() as i16));
            let sz = if op & 0x40 != 0 { ".L" } else { ".W" };
            return Some(if op & 0x80 != 0 {
                out(format!("MOVEP{sz}"), format!("D{rx},{d}(A{r})"))
            } else {
                out(format!("MOVEP{sz}"), format!("{d}(A{r}),D{rx}"))
            });
        }
        let bit = BIT[usize::from((op >> 6) & 3)];
        return Some(out(bit, format!("D{rx},{}", ea(w, m, r, Sz::B, &mut t)?)));
    }
    if op & 0x0f00 == 0x0800 {
        let n = w.word() & 0xff;
        let bit = BIT[usize::from((op >> 6) & 3)];
        return Some(out(bit, format!("#{n},{}", ea(w, m, r, Sz::B, &mut t)?)));
    }
    let mn = match (op >> 9) & 7 {
        0 => "ORI",
        1 => "ANDI",
        2 => "SUBI",
        3 => "ADDI",
        5 => "EORI",
        6 => "CMPI",
        _ => return None,
    };
    if op & 0x3f == 0x3c && matches!(mn, "ORI" | "ANDI" | "EORI") {
        return match (op >> 6) & 3 {
            0 => Some(out(mn, format!("#{},CCR", hex(u32::from(w.word() & 0xff))))),
            1 => Some(out(mn, format!("#{},SR", hex(u32::from(w.word()))))),
            _ => None,
        };
    }
    let sz = Sz::from_bits(op >> 6)?;
    let imm = ea(w, 7, 4, sz, &mut t)?;
    Some(out(
        format!("{mn}{}", sz.suffix()),
        format!("{imm},{}", ea(w, m, r, sz, &mut t)?),
    ))
}

/// The miscellaneous group ($4xxx).
fn group4(w: &mut Words, op: u16) -> Option<Out> {
    let m = (op >> 3) & 7;
    let r = op & 7;
    let rx = (op >> 9) & 7;
    let mut t = None;
    match op {
        0x4afc => return Some(out("ILLEGAL", "")),
        0x4e70 => return Some(out("RESET", "")),
        0x4e71 => return Some(out("NOP", "")),
        0x4e72 => return Some(out("STOP", format!("#{}", hex(u32::from(w.word()))))),
        0x4e73 | 0x4e75 | 0x4e77 => {
            let mn = match op {
                0x4e73 => "RTE",
                0x4e75 => "RTS",
                _ => "RTR",
            };
            return Some(Out {
                flow: Flow::Return,
                ..out(mn, "")
            });
        }
        0x4e76 => return Some(out("TRAPV", "")),
        _ => {}
    }
    match op & 0xfff0 {
        0x4e40 => return Some(out("TRAP", format!("#{}", op & 15))),
        0x4e50 => {
            return Some(match op & 8 {
                0 => out(
                    "LINK",
                    format!("A{r},#{}", signed_hex(i32::from(w.word() as i16))),
                ),
                _ => out("UNLK", format!("A{r}")),
            })
        }
        0x4e60 => {
            return Some(match op & 8 {
                0 => out("MOVE", format!("A{r},USP")),
                _ => out("MOVE", format!("USP,A{r}")),
            })
        }
        _ => {}
    }
    match op & 0xffc0 {
        0x4e80 | 0x4ec0 => {
            if !matches!(m, 2 | 5 | 6 | 7) || (m == 7 && r > 3) {
                return None;
            }
            let call = op & 0xffc0 == 0x4e80;
            let e = ea(w, m, r, Sz::L, &mut t)?;
            return Some(Out {
                mnemonic: if call { "JSR" } else { "JMP" }.into(),
                operand: e,
                target: t,
                flow: if call { Flow::Call } else { Flow::Jump },
            });
        }
        0x40c0 => return Some(out("MOVE", format!("SR,{}", ea(w, m, r, Sz::W, &mut t)?))),
        0x44c0 => return Some(out("MOVE", format!("{},CCR", ea(w, m, r, Sz::W, &mut t)?))),
        0x46c0 => return Some(out("MOVE", format!("{},SR", ea(w, m, r, Sz::W, &mut t)?))),
        0x4800 => return Some(out("NBCD", ea(w, m, r, Sz::B, &mut t)?)),
        0x4840 => {
            return match m {
                0 => Some(out("SWAP", format!("D{r}"))),
                2 | 5 | 6 => Some(out("PEA", ea(w, m, r, Sz::L, &mut t)?)),
                7 if r < 4 => Some(out("PEA", ea(w, m, r, Sz::L, &mut t)?)),
                _ => None,
            };
        }
        0x4ac0 => return Some(out("TAS", ea(w, m, r, Sz::B, &mut t)?)),
        0x4880 | 0x48c0 | 0x4c80 | 0x4cc0 => {
            let sz = if op & 0x40 != 0 { ".L" } else { ".W" };
            if op & 0x0400 == 0 && m == 0 {
                return Some(out(format!("EXT{sz}"), format!("D{r}")));
            }
            let mask = w.word();
            let e = ea(w, m, r, Sz::W, &mut t)?;
            return Some(if op & 0x0400 == 0 {
                out(
                    format!("MOVEM{sz}"),
                    format!("{},{e}", reglist(mask, m == 4)),
                )
            } else {
                out(
                    format!("MOVEM{sz}"),
                    format!("{e},{}", reglist(mask, false)),
                )
            });
        }
        _ => {}
    }
    if op & 0x1c0 == 0x1c0 {
        if !matches!(m, 2 | 5 | 6 | 7) || (m == 7 && r > 3) {
            return None;
        }
        return Some(out("LEA", format!("{},A{rx}", ea(w, m, r, Sz::L, &mut t)?)));
    }
    if op & 0x1c0 == 0x180 {
        return Some(out(
            "CHK.W",
            format!("{},D{rx}", ea(w, m, r, Sz::W, &mut t)?),
        ));
    }
    let sz = Sz::from_bits(op >> 6)?;
    let mn = match op & 0x0f00 {
        0x0000 => "NEGX",
        0x0200 => "CLR",
        0x0400 => "NEG",
        0x0600 => "NOT",
        0x0a00 => "TST",
        _ => return None,
    };
    Some(out(
        format!("{mn}{}", sz.suffix()),
        ea(w, m, r, sz, &mut t)?,
    ))
}

/// Shifts and rotates, register and memory forms.
fn shift(w: &mut Words, op: u16) -> Option<Out> {
    const KIND: [&str; 4] = ["AS", "LS", "ROX", "RO"];
    let dir = if op & 0x100 != 0 { "L" } else { "R" };
    let mut t = None;
    if op & 0xc0 == 0xc0 {
        if op & 0x0800 != 0 {
            return None;
        }
        let kind = KIND[usize::from((op >> 9) & 3)];
        let e = ea(w, (op >> 3) & 7, op & 7, Sz::W, &mut t)?;
        return Some(out(format!("{kind}{dir}.W"), e));
    }
    let sz = Sz::from_bits(op >> 6)?;
    let kind = KIND[usize::from((op >> 3) & 3)];
    let rx = (op >> 9) & 7;
    let count = if op & 0x20 != 0 {
        format!("D{rx}")
    } else {
        format!("#{}", if rx == 0 { 8 } else { rx })
    };
    Some(out(
        format!("{kind}{dir}{}", sz.suffix()),
        format!("{count},D{}", op & 7),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(Vec<u8>);

    impl Bus for Mem {
        fn read(&mut self, addr: u32, size: Size) -> u32 {
            self.peek(addr, size)
        }
        fn peek(&self, addr: u32, size: Size) -> u32 {
            let b = |a: u32| u32::from(self.0.get(a as usize).copied().unwrap_or(0));
            match size {
                Size::Byte => b(addr),
                _ => b(addr) << 8 | b(addr + 1),
            }
        }
        fn write(&mut self, _: u32, _: Size, _: u32) {}
        fn addr_bits(&self) -> u8 {
            24
        }
    }

    fn dis(words: &[u16]) -> (String, usize, Flow, Option<u32>) {
        let bytes = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        let i = disassemble(&Mem(bytes), 0, false);
        (i.to_string(), i.bytes.len(), i.flow, i.target)
    }

    fn dis020(words: &[u16]) -> (String, usize) {
        let bytes = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        let i = disassemble(&Mem(bytes), 0, true);
        (i.to_string(), i.bytes.len())
    }

    #[test]
    fn decodes_68020_forms() {
        let cases: &[(&[u16], &str, usize)] = &[
            (&[0x45f0, 0x1d20, 0x1234], "LEA ($1234,A0,D1.L*4),A2", 6),
            (
                &[0x2030, 0x1326, 0x0100, 0x0020],
                "MOVE.L ([$100,A0],D1.W*2,$20),D0",
                8,
            ),
            (&[0x41f0, 0x01e1, 0x2000], "LEA ([$2000]),A0", 6),
            (&[0x49c0], "EXTB.L D0", 2),
            (&[0x4c3c, 0x1000, 0x0000, 0x0003], "MULU.L #$3,D1", 8),
            (&[0x4c00, 0x3c02], "MULS.L D0,D2:D3", 4),
            (
                &[0x4c7c, 0x3c02, 0xffff, 0xfffd],
                "DIVS.L #$FFFFFFFD,D2:D3",
                8,
            ),
            (&[0x4c41, 0x1002], "DIVUL.L D1,D2:D1", 4),
            (&[0xe9c0, 0x1108], "BFEXTU D0{4:8},D1", 4),
            (&[0xefc0, 0x1708], "BFINS D1,D0{28:8}", 4),
            (&[0x0cd0, 0x0040], "CAS.W D0,D1,(A0)", 4),
            (&[0x00d0, 0x0800], "CHK2.B (A0),D0", 4),
            (&[0x4e7b, 0x0801], "MOVEC D0,VBR", 4),
            (&[0x4e74, 0x0008], "RTD #$8", 4),
            (&[0x57fc], "TRAPEQ", 2),
            (&[0x60ff, 0x0001, 0x0000], "BRA.L $10002", 6),
            (&[0x8340, 0x0000], "PACK D0,D1,#$0", 4),
            (&[0x3070, 0x1004], "MOVEA.W $4(A0,D1.W),A0", 4),
        ];
        for (words, text, len) in cases {
            assert_eq!(dis020(words), ((*text).to_string(), *len), "{words:04x?}");
        }
        // The same words on a 68000 stay what they were.
        assert_eq!(dis(&[0x49c0]).0, "DC.W $49C0");
    }

    #[test]
    fn decodes_common_instructions() {
        let cases: &[(&[u16], &str, usize)] = &[
            (&[0x4e71], "NOP", 2),
            (&[0x4e75], "RTS", 2),
            (&[0x7005], "MOVEQ #$5,D0", 2),
            (&[0x70ff], "MOVEQ #-$1,D0", 2),
            (&[0x2010], "MOVE.L (A0),D0", 2),
            (
                &[0x33fc, 0x1234, 0x00df, 0xf180],
                "MOVE.W #$1234,$DFF180.L",
                8,
            ),
            (&[0x41f9, 0x00fc, 0x0000], "LEA $FC0000.L,A0", 6),
            (&[0x3070, 0x1004], "MOVEA.W $4(A0,D1.W),A0", 4),
            (&[0x48e7, 0xc0c0], "MOVEM.L D0-D1/A0-A1,-(A7)", 4),
            (&[0x4cdf, 0x0303], "MOVEM.L (A7)+,D0-D1/A0-A1", 4),
            (&[0x0c40, 0x0010], "CMPI.W #$10,D0", 4),
            (&[0x08f9, 0x0001, 0x00bf, 0xe001], "BSET #1,$BFE001.L", 8),
            (&[0x5341], "SUBQ.W #1,D1", 2),
            (&[0xe348], "LSL.W #1,D0", 2),
            (&[0xe2a9], "LSR.L D1,D1", 2),
            (&[0xc342], "EXG D1,D2", 2),
            (&[0xd1c1], "ADDA.L D1,A0", 2),
            (&[0x80c1], "DIVU.W D1,D0", 2),
            (&[0x46fc, 0x2700], "MOVE #$2700,SR", 4),
            (&[0x4e4f], "TRAP #15", 2),
            (&[0xa000], "DC.W $A000 ; line A", 2),
            (&[0x4848], "DC.W $4848", 2),
        ];
        for (words, text, len) in cases {
            let (got, n, _, _) = dis(words);
            assert_eq!((got.as_str(), n), (*text, *len), "{words:04x?}");
        }
    }

    #[test]
    fn branches_carry_targets_and_flow() {
        assert_eq!(
            dis(&[0x6604]),
            ("BNE.S $6".into(), 2, Flow::Branch, Some(6))
        );
        assert_eq!(
            dis(&[0x6000, 0x0010]),
            ("BRA $12".into(), 4, Flow::Jump, Some(0x12))
        );
        assert_eq!(dis(&[0x61fe]), ("BSR.S $0".into(), 2, Flow::Call, Some(0)));
        assert_eq!(
            dis(&[0x51c8, 0xfffe]),
            ("DBRA D0,$0".into(), 4, Flow::Branch, Some(0))
        );
        assert_eq!(
            dis(&[0x4eb9, 0x00fc, 0x00d2]),
            ("JSR $FC00D2.L".into(), 6, Flow::Call, Some(0xfc00d2))
        );
        assert_eq!(dis(&[0x4ed0]), ("JMP (A0)".into(), 2, Flow::Jump, None));
    }
}
