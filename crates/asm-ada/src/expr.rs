//! Expressions: numbers (`123`, `$1f`, `0x1f`, `%101`, `'a'`), symbols,
//! `+ - * / % << >> & ^ | ~`, parentheses, and the functions `seg()`,
//! `off()`, `lo()` and `hi()` that pick parts of an address.
//!
//! A value is absolute, or relative to a section or an external symbol
//! (then only `± constant` and the part functions apply; the linker
//! resolves it through a relocation).

use std::fmt;

/// Which part of a linear address (segment × 65 536 + offset) a
/// relocatable value stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sel {
    /// The linear address (its low 16 bits where 16 are wanted: the offset).
    Lin,
    Seg,
    Off,
    OffLo,
    OffHi,
    SegLo,
    SegHi,
}

impl Sel {
    pub fn apply(self, v: u32) -> u32 {
        match self {
            Sel::Lin => v,
            Sel::Seg => v >> 16,
            Sel::Off => v & 0xffff,
            Sel::OffLo => v & 0xff,
            Sel::OffHi => (v >> 8) & 0xff,
            Sel::SegLo => (v >> 16) & 0xff,
            Sel::SegHi => v >> 24,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        [
            Sel::Lin,
            Sel::Seg,
            Sel::Off,
            Sel::OffLo,
            Sel::OffHi,
            Sel::SegLo,
            Sel::SegHi,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// What a relocatable value is relative to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Base {
    /// A section of this object (by index).
    Section(usize),
    /// A symbol defined elsewhere.
    Extern(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Val {
    Abs(i64),
    Rel { base: Base, add: i64, sel: Sel },
}

impl Val {
    pub fn abs(&self) -> Option<i64> {
        match self {
            Val::Abs(v) => Some(*v),
            Val::Rel { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Num(i64),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Num(n) => write!(f, "{n}"),
            Tok::Ident(s) => f.write_str(s),
            Tok::Op(o) => f.write_str(o),
            Tok::LParen => f.write_str("("),
            Tok::RParen => f.write_str(")"),
        }
    }
}

const OPS: [&str; 13] = [
    "<<", ">>", "+", "-", "*", "/", "%", "&", "^", "|", "~", "<", ">",
];

/// Tokenise an expression.
pub fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else if c == '\'' {
            let ch = s[i + 1..].chars().next().ok_or("empty character literal")?;
            let end = i + 1 + ch.len_utf8();
            if b.get(end) != Some(&b'\'') {
                return Err(format!("bad character literal in `{s}`"));
            }
            out.push(Tok::Num(ch as i64));
            i = end + 1;
        } else if c == '$'
            || c == '%'
                && b.get(i + 1).is_some_and(|d| *d == b'0' || *d == b'1')
                && !prev_is_value(&out)
        {
            let radix = if c == '$' { 16 } else { 2 };
            let start = i + 1;
            let mut j = start;
            while j < b.len() && (b[j] as char).is_digit(radix) || j < b.len() && b[j] == b'_' {
                j += 1;
            }
            if j == start {
                return Err(format!("number expected after `{c}` in `{s}`"));
            }
            let digits: String = s[start..j].chars().filter(|c| *c != '_').collect();
            out.push(Tok::Num(
                i64::from_str_radix(&digits, radix).map_err(|e| e.to_string())?,
            ));
            i = j;
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let t: String = s[i..j].chars().filter(|c| *c != '_').collect();
            let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                i64::from_str_radix(h, 16)
            } else if let Some(h) = t.strip_prefix("0b") {
                i64::from_str_radix(h, 2)
            } else {
                t.parse()
            }
            .map_err(|_| format!("bad number `{t}`"))?;
            out.push(Tok::Num(v));
            i = j;
        } else if c.is_ascii_alphabetic() || c == '_' || c == '.' || c == '@' {
            let mut j = i;
            while j < b.len()
                && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'.' || b[j] == b'@')
            {
                j += 1;
            }
            out.push(Tok::Ident(s[i..j].to_string()));
            i = j;
        } else if let Some(op) = OPS.iter().find(|o| s[i..].starts_with(**o)) {
            out.push(Tok::Op(op));
            i += op.len();
        } else {
            return Err(format!("unexpected `{c}` in `{s}`"));
        }
    }
    Ok(out)
}

fn prev_is_value(out: &[Tok]) -> bool {
    matches!(out.last(), Some(Tok::Num(_) | Tok::Ident(_) | Tok::RParen))
}

/// Resolves symbol names to values.
pub trait Symbols {
    fn lookup(&self, name: &str) -> Result<Val, String>;
}

/// Evaluate tokens.
pub fn eval(toks: &[Tok], syms: &dyn Symbols) -> Result<Val, String> {
    let mut p = Parser {
        t: toks,
        i: 0,
        syms,
    };
    let v = p.or()?;
    if p.i != toks.len() {
        return Err(format!("unexpected `{}`", toks[p.i]));
    }
    Ok(v)
}

/// Evaluate a string.
pub fn eval_str(s: &str, syms: &dyn Symbols) -> Result<Val, String> {
    let t = lex(s)?;
    if t.is_empty() {
        return Err("expression expected".into());
    }
    eval(&t, syms)
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
    syms: &'a dyn Symbols,
}

fn abs2(a: Val, b: Val, op: &str) -> Result<(i64, i64), String> {
    match (a.abs(), b.abs()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("`{op}` needs absolute values")),
    }
}

impl Parser<'_> {
    fn peek_op(&self, ops: &[&str]) -> Option<&'static str> {
        match self.t.get(self.i) {
            Some(Tok::Op(o)) if ops.contains(o) => Some(o),
            _ => None,
        }
    }

    fn binary(
        &mut self,
        ops: &[&str],
        next: fn(&mut Self) -> Result<Val, String>,
        f: fn(&str, i64, i64) -> Result<i64, String>,
    ) -> Result<Val, String> {
        let mut a = next(self)?;
        while let Some(op) = self.peek_op(ops) {
            self.i += 1;
            let b = next(self)?;
            let (x, y) = abs2(a, b, op)?;
            a = Val::Abs(f(op, x, y)?);
        }
        Ok(a)
    }

    fn or(&mut self) -> Result<Val, String> {
        self.binary(&["|"], Self::xor, |_, a, b| Ok(a | b))
    }
    fn xor(&mut self) -> Result<Val, String> {
        self.binary(&["^"], Self::and, |_, a, b| Ok(a ^ b))
    }
    fn and(&mut self) -> Result<Val, String> {
        self.binary(&["&"], Self::shift, |_, a, b| Ok(a & b))
    }
    fn shift(&mut self) -> Result<Val, String> {
        self.binary(&["<<", ">>"], Self::sum, |op, a, b| {
            let b = u32::try_from(b)
                .map_err(|_| "negative shift".to_string())?
                .min(63);
            Ok(if op == "<<" { a << b } else { a >> b })
        })
    }

    fn sum(&mut self) -> Result<Val, String> {
        let mut a = self.product()?;
        while let Some(op) = self.peek_op(&["+", "-"]) {
            self.i += 1;
            let b = self.product()?;
            a = match (a, b, op) {
                (Val::Abs(x), Val::Abs(y), "+") => Val::Abs(x + y),
                (Val::Abs(x), Val::Abs(y), _) => Val::Abs(x - y),
                (
                    Val::Rel {
                        base,
                        add,
                        sel: Sel::Lin,
                    },
                    Val::Abs(y),
                    op,
                ) => Val::Rel {
                    base,
                    add: if op == "+" { add + y } else { add - y },
                    sel: Sel::Lin,
                },
                (
                    Val::Abs(x),
                    Val::Rel {
                        base,
                        add,
                        sel: Sel::Lin,
                    },
                    "+",
                ) => Val::Rel {
                    base,
                    add: add + x,
                    sel: Sel::Lin,
                },
                // The distance between two places in the same section.
                (
                    Val::Rel {
                        base: b1,
                        add: a1,
                        sel: Sel::Lin,
                    },
                    Val::Rel {
                        base: b2,
                        add: a2,
                        sel: Sel::Lin,
                    },
                    "-",
                ) if b1 == b2 && matches!(b1, Base::Section(_)) => Val::Abs(a1 - a2),
                _ => return Err(format!("`{op}` on a relocatable value")),
            };
        }
        Ok(a)
    }

    fn product(&mut self) -> Result<Val, String> {
        self.binary(&["*", "/", "%"], Self::unary, |op, a, b| match op {
            "*" => Ok(a * b),
            _ if b == 0 => Err("division by zero".into()),
            "/" => Ok(a / b),
            _ => Ok(a % b),
        })
    }

    fn unary(&mut self) -> Result<Val, String> {
        if let Some(op) = self.peek_op(&["-", "~", "+", "<", ">"]) {
            self.i += 1;
            let v = self.unary()?;
            return match (op, v) {
                ("+", v) => Ok(v),
                ("-", Val::Abs(x)) => Ok(Val::Abs(-x)),
                ("~", Val::Abs(x)) => Ok(Val::Abs(!x)),
                // `<x` low byte, `>x` high byte of the offset.
                ("<", v) => part(v, "lo"),
                (">", v) => part(v, "hi"),
                (op, _) => Err(format!("`{op}` on a relocatable value")),
            };
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Val, String> {
        match self.t.get(self.i).cloned() {
            Some(Tok::Num(n)) => {
                self.i += 1;
                Ok(Val::Abs(n))
            }
            Some(Tok::LParen) => {
                self.i += 1;
                let v = self.or()?;
                if self.t.get(self.i) != Some(&Tok::RParen) {
                    return Err("`)` expected".into());
                }
                self.i += 1;
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                self.i += 1;
                let f = name.to_ascii_lowercase();
                if matches!(f.as_str(), "seg" | "off" | "lo" | "hi")
                    && self.t.get(self.i) == Some(&Tok::LParen)
                {
                    self.i += 1;
                    let v = self.or()?;
                    if self.t.get(self.i) != Some(&Tok::RParen) {
                        return Err(format!("`)` expected after {f}("));
                    }
                    self.i += 1;
                    return part(v, &f);
                }
                self.syms.lookup(&name)
            }
            Some(t) => Err(format!("unexpected `{t}`")),
            None => Err("expression ends early".into()),
        }
    }
}

/// `seg()`, `off()`, `lo()`, `hi()` of a value.
fn part(v: Val, f: &str) -> Result<Val, String> {
    match v {
        Val::Abs(x) => Ok(Val::Abs(match f {
            "seg" => (x >> 16) & 0xffff,
            "off" => x & 0xffff,
            "lo" => x & 0xff,
            _ => (x >> 8) & 0xff,
        })),
        Val::Rel { base, add, sel } => {
            let sel = match (f, sel) {
                ("seg", Sel::Lin) => Sel::Seg,
                ("off", Sel::Lin | Sel::Off) => Sel::Off,
                ("lo", Sel::Lin | Sel::Off) => Sel::OffLo,
                ("hi", Sel::Lin | Sel::Off) => Sel::OffHi,
                ("lo", Sel::Seg) => Sel::SegLo,
                ("hi", Sel::Seg) => Sel::SegHi,
                _ => return Err(format!("{f}() of this value")),
            };
            Ok(Val::Rel { base, add, sel })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoSyms;
    impl Symbols for NoSyms {
        fn lookup(&self, name: &str) -> Result<Val, String> {
            match name {
                "label" => Ok(Val::Rel {
                    base: Base::Section(0),
                    add: 0x10,
                    sel: Sel::Lin,
                }),
                "ten" => Ok(Val::Abs(10)),
                _ => Err(format!("unknown `{name}`")),
            }
        }
    }

    fn v(s: &str) -> Val {
        eval_str(s, &NoSyms).unwrap()
    }

    #[test]
    fn numbers_and_operators() {
        assert_eq!(v("$1f + 0x10 + %101 + 'A'"), Val::Abs(31 + 16 + 5 + 65));
        assert_eq!(v("1 + 2 * 3 << 1 | 1"), Val::Abs(15));
        assert_eq!(v("-ten / 3"), Val::Abs(-3));
        assert_eq!(v("~0 & $ff"), Val::Abs(255));
        assert_eq!(v("10 % 4"), Val::Abs(2));
        assert_eq!(
            v("hi($1234) + lo($1234) + seg($fffb0000)"),
            Val::Abs(0x12 + 0x34 + 0xfffb)
        );
        assert_eq!(v("<$1234 + >$1234"), Val::Abs(0x34 + 0x12));
    }

    #[test]
    fn relocatable_values() {
        assert_eq!(
            v("label + 4"),
            Val::Rel {
                base: Base::Section(0),
                add: 0x14,
                sel: Sel::Lin
            }
        );
        assert_eq!(v("label + 6 - label"), Val::Abs(6));
        assert_eq!(
            v("hi(seg(label))"),
            Val::Rel {
                base: Base::Section(0),
                add: 0x10,
                sel: Sel::SegHi
            }
        );
        assert!(eval_str("label * 2", &NoSyms).is_err());
        assert!(eval_str("seg(label) + 1", &NoSyms).is_err());
        assert!(eval_str("nothing", &NoSyms).is_err());
        assert!(eval_str("(1", &NoSyms).is_err());
    }
}
