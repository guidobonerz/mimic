//! Breakpoint conditions: a small expression language evaluated when a
//! breakpoint hits.
//!
//! ```text
//! A == $FF && [$D012] > 100
//! (P & %00000010) != 0 || line == 51
//! ```
//!
//! - Numbers: decimal, `$hex`, `0xhex`, `%binary`.
//! - Identifiers: register names of the breakpoint's device
//!   (case-insensitive), and the beam values `line`, `cycle`, `frame`.
//! - `[expr]`: the byte at that address in the breakpoint's memory space
//!   (peek, side-effect free).
//! - Operators, loosest first: `||`, `&&`, comparisons (`== != < <= > >=`),
//!   `|`, `^`, `&`, `+ -`, `* / %`, unary `! - ~`. Arithmetic wraps at 32
//!   bits; comparisons and logic yield 0 or 1.
//!
//! Conditions are parsed once when the breakpoint is set; evaluation errors
//! (unknown register, unreadable address, division by zero) make the
//! condition true, so a broken condition stops rather than silently runs.

use std::fmt;

/// Longest accepted condition text; bounds parsing work.
pub const MAX_LEN: usize = 256;
/// Deepest accepted nesting.
const MAX_DEPTH: u32 = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Num(u32),
    Ident(String),
    Mem(Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Inv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitOr,
    BitXor,
    BitAnd,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a condition can see.
pub trait Context {
    fn reg(&self, name: &str) -> Option<u32>;
    fn peek(&self, addr: u32) -> Option<u32>;
    /// `line`, `cycle`, `frame`.
    fn beam(&self, name: &str) -> Option<u32>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Num(u32),
    Ident(String),
    Op(&'static str),
}

const OPS: [&str; 22] = [
    "||", "&&", "==", "!=", "<=", ">=", "<", ">", "|", "^", "&", "+", "-", "*", "/", "%", "!", "~",
    "(", ")", "[", "]",
];

fn lex(s: &str) -> Result<Vec<Tok>, ParseError> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let number = |start: usize, radix: u32| -> Result<(u32, usize), ParseError> {
            let end = (start..b.len())
                .find(|&j| !(b[j] as char).is_digit(radix))
                .unwrap_or(b.len());
            if end == start {
                return Err(ParseError(format!("number expected at {start}")));
            }
            u32::from_str_radix(&s[start..end], radix)
                .map(|v| (v, end))
                .map_err(|_| ParseError(format!("number too large at {start}")))
        };
        if c == b'$' {
            let (v, end) = number(i + 1, 16)?;
            out.push(Tok::Num(v));
            i = end;
        } else if c == b'%' && b.get(i + 1).is_some_and(|d| *d == b'0' || *d == b'1') {
            let (v, end) = number(i + 1, 2)?;
            out.push(Tok::Num(v));
            i = end;
        } else if c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
            let (v, end) = number(i + 2, 16)?;
            out.push(Tok::Num(v));
            i = end;
        } else if c.is_ascii_digit() {
            let (v, end) = number(i, 10)?;
            out.push(Tok::Num(v));
            i = end;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let end = (i..b.len())
                .find(|&j| !(b[j].is_ascii_alphanumeric() || b[j] == b'_'))
                .unwrap_or(b.len());
            out.push(Tok::Ident(s[i..end].to_owned()));
            i = end;
        } else if let Some(op) = OPS.iter().find(|op| s[i..].starts_with(*op)) {
            out.push(Tok::Op(op));
            i += op.len();
        } else {
            return Err(ParseError(format!(
                "unexpected '{}' at {i}",
                s[i..].chars().next().unwrap_or('?')
            )));
        }
    }
    Ok(out)
}

/// Binary operator levels, loosest first.
const LEVELS: [&[(&str, BinOp)]; 8] = [
    &[("||", BinOp::Or)],
    &[("&&", BinOp::And)],
    &[
        ("==", BinOp::Eq),
        ("!=", BinOp::Ne),
        ("<=", BinOp::Le),
        (">=", BinOp::Ge),
        ("<", BinOp::Lt),
        (">", BinOp::Gt),
    ],
    &[("|", BinOp::BitOr)],
    &[("^", BinOp::BitXor)],
    &[("&", BinOp::BitAnd)],
    &[("+", BinOp::Add), ("-", BinOp::Sub)],
    &[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Rem)],
];

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: u32,
}

impl Parser {
    fn peek_op(&self) -> Option<&'static str> {
        match self.toks.get(self.pos) {
            Some(Tok::Op(op)) => Some(op),
            _ => None,
        }
    }

    fn expect(&mut self, op: &str) -> Result<(), ParseError> {
        if self.peek_op() == Some(op) {
            self.pos += 1;
            Ok(())
        } else {
            Err(ParseError(format!("'{op}' expected")))
        }
    }

    fn binary(&mut self, level: usize) -> Result<Expr, ParseError> {
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        while let Some(&(_, op)) = self
            .peek_op()
            .and_then(|t| LEVELS[level].iter().find(|(s, _)| *s == t))
        {
            self.pos += 1;
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError("expression nested too deeply".into()));
        }
        let op = match self.peek_op() {
            Some("!") => Some(UnOp::Not),
            Some("-") => Some(UnOp::Neg),
            Some("~") => Some(UnOp::Inv),
            _ => None,
        };
        let e = match op {
            Some(op) => {
                self.pos += 1;
                Expr::Unary(op, Box::new(self.unary()?))
            }
            None => self.primary()?,
        };
        self.depth -= 1;
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.toks.get(self.pos).cloned();
        self.pos += 1;
        match tok {
            Some(Tok::Num(v)) => Ok(Expr::Num(v)),
            Some(Tok::Ident(name)) => Ok(Expr::Ident(name)),
            Some(Tok::Op("(")) => {
                let e = self.binary(0)?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::Op("[")) => {
                let e = self.binary(0)?;
                self.expect("]")?;
                Ok(Expr::Mem(Box::new(e)))
            }
            Some(t) => Err(ParseError(format!("unexpected {t:?}"))),
            None => Err(ParseError("unexpected end of condition".into())),
        }
    }
}

pub fn parse(s: &str) -> Result<Expr, ParseError> {
    if s.len() > MAX_LEN {
        return Err(ParseError(format!("condition longer than {MAX_LEN}")));
    }
    let mut p = Parser {
        toks: lex(s)?,
        pos: 0,
        depth: 0,
    };
    let e = p.binary(0)?;
    if p.pos != p.toks.len() {
        return Err(ParseError(format!("unexpected {:?}", p.toks[p.pos])));
    }
    Ok(e)
}

impl Expr {
    /// Evaluate; `None` on an unknown name, unreadable address or division
    /// by zero.
    pub fn eval(&self, ctx: &dyn Context) -> Option<u32> {
        Some(match self {
            Expr::Num(v) => *v,
            Expr::Ident(name) => match name.to_ascii_lowercase().as_str() {
                n @ ("line" | "cycle" | "frame") => ctx.beam(n)?,
                _ => ctx.reg(name)?,
            },
            Expr::Mem(addr) => ctx.peek(addr.eval(ctx)?)?,
            Expr::Unary(op, e) => {
                let v = e.eval(ctx)?;
                match op {
                    UnOp::Not => u32::from(v == 0),
                    UnOp::Neg => v.wrapping_neg(),
                    UnOp::Inv => !v,
                }
            }
            Expr::Binary(op, a, b) => {
                let a = a.eval(ctx)?;
                // Short-circuit like C, so `[ptr] ...` guarded by `&&` is safe.
                match op {
                    BinOp::And if a == 0 => return Some(0),
                    BinOp::Or if a != 0 => return Some(1),
                    _ => {}
                }
                let b = b.eval(ctx)?;
                match op {
                    BinOp::Or | BinOp::And => u32::from(b != 0),
                    BinOp::Eq => u32::from(a == b),
                    BinOp::Ne => u32::from(a != b),
                    BinOp::Lt => u32::from(a < b),
                    BinOp::Le => u32::from(a <= b),
                    BinOp::Gt => u32::from(a > b),
                    BinOp::Ge => u32::from(a >= b),
                    BinOp::BitOr => a | b,
                    BinOp::BitXor => a ^ b,
                    BinOp::BitAnd => a & b,
                    BinOp::Add => a.wrapping_add(b),
                    BinOp::Sub => a.wrapping_sub(b),
                    BinOp::Mul => a.wrapping_mul(b),
                    BinOp::Div => a.checked_div(b)?,
                    BinOp::Rem => a.checked_rem(b)?,
                }
            }
        })
    }

    /// Condition semantics: non-zero is true; evaluation errors are true.
    pub fn holds(&self, ctx: &dyn Context) -> bool {
        self.eval(ctx).is_none_or(|v| v != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ctx;

    impl Context for Ctx {
        fn reg(&self, name: &str) -> Option<u32> {
            match name.to_ascii_uppercase().as_str() {
                "A" => Some(0xff),
                "X" => Some(3),
                "P" => Some(0b0010_0010),
                _ => None,
            }
        }

        fn peek(&self, addr: u32) -> Option<u32> {
            (addr < 0x10000).then_some(addr & 0xff)
        }

        fn beam(&self, name: &str) -> Option<u32> {
            (name == "line").then_some(51)
        }
    }

    fn eval(s: &str) -> Option<u32> {
        parse(s).unwrap().eval(&Ctx)
    }

    #[test]
    fn numbers_and_precedence() {
        assert_eq!(eval("$10 + 0x10 + %11 + 3"), Some(0x26));
        assert_eq!(eval("2 + 3 * 4"), Some(14));
        assert_eq!(eval("(2 + 3) * 4"), Some(20));
        assert_eq!(eval("1 | 2 & 3"), Some(3));
        assert_eq!(eval("10 % 4 - -1"), Some(3));
        assert_eq!(eval("~0"), Some(u32::MAX));
        assert_eq!(eval("1 < 2 == 1"), Some(1));
    }

    #[test]
    fn registers_memory_and_beam() {
        assert_eq!(eval("A == $FF && [$D012] > 1"), Some(1));
        assert_eq!(eval("a == 255"), Some(1), "case-insensitive");
        assert_eq!(eval("(P & %00000010) != 0 || line == 52"), Some(1));
        assert_eq!(eval("[$1000 + X]"), Some(3));
        assert_eq!(eval("LINE"), Some(51));
    }

    #[test]
    fn errors_make_conditions_true_but_short_circuit_first() {
        assert_eq!(eval("Q"), None);
        assert_eq!(eval("1 / 0"), None);
        assert_eq!(eval("[$10000]"), None);
        assert!(parse("Q == 1").unwrap().holds(&Ctx));
        assert_eq!(eval("0 && Q"), Some(0));
        assert_eq!(eval("1 || Q"), Some(1));
        assert!(!parse("A != $FF").unwrap().holds(&Ctx));
    }

    #[test]
    fn syntax_errors() {
        for bad in [
            "",
            "1 +",
            "(1",
            "[1",
            "1 2",
            "$",
            "$G",
            "@",
            "%2",
            "99999999999",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse(&"(".repeat(100)).is_err());
        assert!(parse(&"1+".repeat(200)).is_err());
    }
}
