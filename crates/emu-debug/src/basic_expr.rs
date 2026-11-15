//! Conditions of BASIC breakpoints: an expression over the program's
//! variables, written the way BASIC writes it, evaluated when the program
//! reaches the breakpoint's place.
//!
//! ```text
//! I = 5 AND A$ <> "END"
//! SC% >= 1000 OR L(3) < 0
//! ```
//!
//! - Numbers: decimal with fraction and exponent (`1.5E3`), `$hex`.
//! - Strings: `"…"`.
//! - Variables by name with their type sign (`A`, `N%`, `S$`), in upper or
//!   lower case; a name is cut to the characters the dialect tells apart
//!   (`COUNT` is `CO` in Commodore BASIC). A variable the program has not
//!   set yet is `0` or `""`, as in BASIC.
//! - Array elements with one index: `A(3)`, `N%(I+1)`.
//! - Operators, loosest first: `OR`, `AND`, `NOT`, comparisons
//!   (`= <> < <= > >=`), `+ -`, `* /`, unary `-`. `||`, `&&`, `!`, `==` and
//!   `!=` are accepted as well. Words stand apart: `A AND B`, not `AANDB`.
//!   `AND`, `OR` and `NOT` take truth values (not bit patterns); comparisons
//!   yield 1 or 0. Strings compare by characters and join with `+`.
//!
//! Numbers and strings do not mix; that is found when the breakpoint is
//! set. What only shows when evaluating (an array that does not exist, an
//! index outside it, a division by zero) makes the condition true, so a
//! broken condition stops rather than silently runs (as in [`crate::expr`]).

use std::cmp::Ordering;

use emu_core::basic::BasicValue;

use crate::expr::{ParseError, MAX_LEN};

/// Deepest accepted nesting.
const MAX_DEPTH: u32 = 32;

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    /// Upper case, with the type sign.
    Var(String),
    Element(String, Box<Expr>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}

/// The program's variables, as a condition sees them.
pub trait Vars {
    /// A plain variable; `None`: not set yet.
    fn var(&self, name: &str) -> Option<BasicValue>;
    /// An element of a one-dimensional array; `None`: no such array or
    /// element.
    fn element(&self, name: &str, index: u32) -> Option<BasicValue>;
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Num(f64),
    Str(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Name(String),
    Op(&'static str),
}

const OPS: [&str; 16] = [
    "||", "&&", "==", "!=", "<>", "<=", ">=", "<", ">", "=", "+", "-", "*", "/", "(", ")",
];

fn lex(s: &str) -> Result<Vec<Tok>, ParseError> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let run = |from: usize, ok: &dyn Fn(u8) -> bool| {
        (from..b.len()).find(|&j| !ok(b[j])).unwrap_or(b.len())
    };
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'"' {
            let end = (i + 1..b.len())
                .find(|&j| b[j] == b'"')
                .ok_or_else(|| ParseError(format!("string not closed at {i}")))?;
            out.push(Tok::Str(s[i + 1..end].to_owned()));
            i = end + 1;
        } else if c == b'$' {
            let end = run(i + 1, &|d| d.is_ascii_hexdigit());
            let v = u32::from_str_radix(&s[i + 1..end], 16)
                .map_err(|_| ParseError(format!("hex number expected at {i}")))?;
            out.push(Tok::Num(f64::from(v)));
            i = end;
        } else if c.is_ascii_digit() || c == b'.' {
            let mut end = run(i, &|d| d.is_ascii_digit());
            if b.get(end) == Some(&b'.') {
                end = run(end + 1, &|d| d.is_ascii_digit());
            }
            // An exponent only with digits behind it (`1E3`, not `1 EQ`).
            if matches!(b.get(end), Some(b'e' | b'E')) {
                let sign = usize::from(matches!(b.get(end + 1), Some(b'+' | b'-')));
                let digits = run(end + 1 + sign, &|d| d.is_ascii_digit());
                if digits > end + 1 + sign {
                    end = digits;
                }
            }
            let v = s[i..end]
                .parse()
                .map_err(|_| ParseError(format!("number expected at {i}")))?;
            out.push(Tok::Num(v));
            i = end;
        } else if c.is_ascii_alphabetic() {
            let mut end = run(i, &|d| d.is_ascii_alphanumeric());
            if matches!(b.get(end), Some(b'%' | b'$')) {
                end += 1;
            }
            out.push(Tok::Name(s[i..end].to_ascii_uppercase()));
            i = end;
        } else if c == b'!' && b.get(i + 1) != Some(&b'=') {
            out.push(Tok::Name("NOT".into()));
            i += 1;
        } else if let Some(op) = OPS.iter().find(|op| s[i..].starts_with(*op)) {
            out.push(match *op {
                "||" => Tok::Name("OR".into()),
                "&&" => Tok::Name("AND".into()),
                op => Tok::Op(op),
            });
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    Num,
    Str,
}

fn ty_of(name: &str) -> Ty {
    if name.ends_with('$') {
        Ty::Str
    } else {
        Ty::Num
    }
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: u32,
    /// Significant characters of a name; 0: all.
    name_len: usize,
}

impl Parser {
    fn word(&mut self, word: &str) -> bool {
        let found = matches!(self.toks.get(self.pos), Some(Tok::Name(n)) if n == word);
        self.pos += usize::from(found);
        found
    }

    fn op(&mut self, ops: &[(&str, Op)]) -> Option<Op> {
        let Some(Tok::Op(t)) = self.toks.get(self.pos) else {
            return None;
        };
        let op = ops.iter().find(|(s, _)| s == t)?.1;
        self.pos += 1;
        Some(op)
    }

    fn deeper(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError("expression nested too deeply".into()));
        }
        Ok(())
    }

    fn number(&mut self, e: (Expr, Ty)) -> Result<Expr, ParseError> {
        match e {
            (e, Ty::Num) => Ok(e),
            _ => Err(ParseError("a number is expected, not a string".into())),
        }
    }

    fn or(&mut self) -> Result<(Expr, Ty), ParseError> {
        let mut lhs = self.and()?;
        while self.word("OR") {
            let (a, b) = (self.number(lhs)?, self.and()?);
            let b = self.number(b)?;
            lhs = (Expr::Binary(Op::Or, Box::new(a), Box::new(b)), Ty::Num);
        }
        Ok(lhs)
    }

    fn and(&mut self) -> Result<(Expr, Ty), ParseError> {
        let mut lhs = self.not()?;
        while self.word("AND") {
            let (a, b) = (self.number(lhs)?, self.not()?);
            let b = self.number(b)?;
            lhs = (Expr::Binary(Op::And, Box::new(a), Box::new(b)), Ty::Num);
        }
        Ok(lhs)
    }

    fn not(&mut self) -> Result<(Expr, Ty), ParseError> {
        if !self.word("NOT") {
            return self.comparison();
        }
        self.deeper()?;
        let e = self.not()?;
        self.depth -= 1;
        Ok((Expr::Not(Box::new(self.number(e)?)), Ty::Num))
    }

    fn comparison(&mut self) -> Result<(Expr, Ty), ParseError> {
        const OPS: [(&str, Op); 8] = [
            ("=", Op::Eq),
            ("==", Op::Eq),
            ("<>", Op::Ne),
            ("!=", Op::Ne),
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("<", Op::Lt),
            (">", Op::Gt),
        ];
        let mut lhs = self.sum()?;
        while let Some(op) = self.op(&OPS) {
            let rhs = self.sum()?;
            if lhs.1 != rhs.1 {
                return Err(ParseError("a number is compared with a string".into()));
            }
            lhs = (Expr::Binary(op, Box::new(lhs.0), Box::new(rhs.0)), Ty::Num);
        }
        Ok(lhs)
    }

    fn sum(&mut self) -> Result<(Expr, Ty), ParseError> {
        let mut lhs = self.product()?;
        while let Some(op) = self.op(&[("+", Op::Add), ("-", Op::Sub)]) {
            let rhs = self.product()?;
            if lhs.1 != rhs.1 || (op == Op::Sub && lhs.1 == Ty::Str) {
                return Err(ParseError(
                    "numbers and strings do not mix in + and -".into(),
                ));
            }
            lhs = (Expr::Binary(op, Box::new(lhs.0), Box::new(rhs.0)), lhs.1);
        }
        Ok(lhs)
    }

    fn product(&mut self) -> Result<(Expr, Ty), ParseError> {
        let mut lhs = self.unary()?;
        while let Some(op) = self.op(&[("*", Op::Mul), ("/", Op::Div)]) {
            let (a, b) = (self.number(lhs)?, self.unary()?);
            let b = self.number(b)?;
            lhs = (Expr::Binary(op, Box::new(a), Box::new(b)), Ty::Num);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<(Expr, Ty), ParseError> {
        self.deeper()?;
        let e = if self.op(&[("-", Op::Sub)]).is_some() {
            let e = self.unary()?;
            (Expr::Neg(Box::new(self.number(e)?)), Ty::Num)
        } else {
            self.primary()?
        };
        self.depth -= 1;
        Ok(e)
    }

    /// `COUNT%` as the dialect keeps it (`CO%`).
    fn name(&self, name: &str) -> String {
        let sign = name.len() - usize::from(name.ends_with(['%', '$']));
        let keep = match self.name_len {
            0 => sign,
            n => n.min(sign),
        };
        format!("{}{}", &name[..keep], &name[sign..])
    }

    fn primary(&mut self) -> Result<(Expr, Ty), ParseError> {
        let tok = self.toks.get(self.pos).cloned();
        self.pos += 1;
        match tok {
            Some(Tok::Num(v)) => Ok((Expr::Num(v), Ty::Num)),
            Some(Tok::Str(s)) => Ok((Expr::Str(s), Ty::Str)),
            Some(Tok::Name(n)) if ["AND", "OR", "NOT"].contains(&n.as_str()) => {
                Err(ParseError(format!("unexpected {n}")))
            }
            Some(Tok::Name(n)) => {
                let (name, ty) = (self.name(&n), ty_of(&n));
                if self.op(&[("(", Op::Add)]).is_none() {
                    return Ok((Expr::Var(name), ty));
                }
                let index = self.or()?;
                let index = self.number(index)?;
                match self.toks.get(self.pos) {
                    Some(Tok::Op(")")) => self.pos += 1,
                    _ => {
                        return Err(ParseError(
                            "')' expected (arrays take one index here)".into(),
                        ))
                    }
                }
                Ok((Expr::Element(name, Box::new(index)), ty))
            }
            Some(Tok::Op("(")) => {
                let e = self.or()?;
                match self.toks.get(self.pos) {
                    Some(Tok::Op(")")) => self.pos += 1,
                    _ => return Err(ParseError("')' expected".into())),
                }
                Ok(e)
            }
            Some(t) => Err(ParseError(format!("unexpected {t:?}"))),
            None => Err(ParseError("unexpected end of condition".into())),
        }
    }
}

/// Parse a condition. `name_len`: the characters of a variable name the
/// dialect tells apart (0: all).
pub fn parse(s: &str, name_len: u8) -> Result<Expr, ParseError> {
    if s.len() > MAX_LEN {
        return Err(ParseError(format!("condition longer than {MAX_LEN}")));
    }
    let mut p = Parser {
        toks: lex(s)?,
        pos: 0,
        depth: 0,
        name_len: usize::from(name_len),
    };
    let e = p.or()?;
    if p.pos != p.toks.len() {
        return Err(ParseError(format!("unexpected {:?}", p.toks[p.pos])));
    }
    p.number(e)
        .map_err(|_| ParseError("a condition is a number, not a string".into()))
}

fn val(v: BasicValue) -> Val {
    match v {
        BasicValue::Int(n) => Val::Num(n as f64),
        BasicValue::Float(f) => Val::Num(f),
        BasicValue::Str(s) => Val::Str(s),
    }
}

impl Expr {
    fn eval(&self, vars: &dyn Vars) -> Option<Val> {
        let num = |e: &Expr| match e.eval(vars)? {
            Val::Num(v) => Some(v),
            Val::Str(_) => None,
        };
        let truth = |b: bool| Val::Num(f64::from(u8::from(b)));
        Some(match self {
            Expr::Num(v) => Val::Num(*v),
            Expr::Str(s) => Val::Str(s.clone()),
            Expr::Var(name) => match vars.var(name) {
                Some(v) => val(v),
                None if name.ends_with('$') => Val::Str(String::new()),
                None => Val::Num(0.0),
            },
            Expr::Element(name, index) => {
                let i = num(index)?;
                if !(0.0..=f64::from(u32::MAX)).contains(&i) {
                    return None;
                }
                val(vars.element(name, i as u32)?)
            }
            Expr::Not(e) => truth(num(e)? == 0.0),
            Expr::Neg(e) => Val::Num(-num(e)?),
            Expr::Binary(op, a, b) => {
                match op {
                    // The right side is not looked at where the left one
                    // decides, so `N > 0 AND A(N) = 1` is safe.
                    Op::And if num(a)? == 0.0 => return Some(truth(false)),
                    Op::Or if num(a)? != 0.0 => return Some(truth(true)),
                    Op::And | Op::Or => return Some(truth(num(b)? != 0.0)),
                    _ => {}
                }
                let (a, b) = (a.eval(vars)?, b.eval(vars)?);
                let order = match (&a, &b) {
                    (Val::Num(a), Val::Num(b)) => a.partial_cmp(b),
                    (Val::Str(a), Val::Str(b)) => Some(a.cmp(b)),
                    _ => return None,
                };
                let is = |want: &[Ordering]| truth(order.is_some_and(|o| want.contains(&o)));
                match (op, a, b) {
                    (Op::Eq, ..) => is(&[Ordering::Equal]),
                    (Op::Ne, ..) => truth(order != Some(Ordering::Equal)),
                    (Op::Lt, ..) => is(&[Ordering::Less]),
                    (Op::Le, ..) => is(&[Ordering::Less, Ordering::Equal]),
                    (Op::Gt, ..) => is(&[Ordering::Greater]),
                    (Op::Ge, ..) => is(&[Ordering::Greater, Ordering::Equal]),
                    (Op::Add, Val::Str(a), Val::Str(b)) => Val::Str(a + &b),
                    (Op::Add, Val::Num(a), Val::Num(b)) => Val::Num(a + b),
                    (Op::Sub, Val::Num(a), Val::Num(b)) => Val::Num(a - b),
                    (Op::Mul, Val::Num(a), Val::Num(b)) => Val::Num(a * b),
                    (Op::Div, Val::Num(a), Val::Num(b)) if b != 0.0 => Val::Num(a / b),
                    _ => return None,
                }
            }
        })
    }

    /// Condition semantics: not zero is true; evaluation errors are true.
    pub fn holds(&self, vars: &dyn Vars) -> bool {
        !matches!(self.eval(vars), Some(Val::Num(v)) if v == 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Program;

    impl Vars for Program {
        fn var(&self, name: &str) -> Option<BasicValue> {
            match name {
                "I" => Some(BasicValue::Float(5.0)),
                "CO%" => Some(BasicValue::Int(-3)),
                "A$" => Some(BasicValue::Str("END".into())),
                _ => None,
            }
        }

        fn element(&self, name: &str, index: u32) -> Option<BasicValue> {
            match (name, index) {
                ("L", 0..=3) => Some(BasicValue::Float(f64::from(index) * 10.0)),
                ("N$", 0) => Some(BasicValue::Str("X".into())),
                _ => None,
            }
        }
    }

    fn holds(s: &str) -> bool {
        parse(s, 2)
            .unwrap_or_else(|e| panic!("{s}: {e}"))
            .holds(&Program)
    }

    #[test]
    fn variables_numbers_and_strings() {
        assert!(holds("I = 5"));
        assert!(holds("i=5 and a$=\"END\""));
        assert!(!holds("I <> 5 OR A$ < \"E\""));
        assert!(holds("COUNT% = -3"), "cut to the dialect's two characters");
        assert!(holds("I * 2 + 1 = 11 AND I / 2 = 2.5"));
        assert!(holds("I >= $5 AND I <= 0.5E1 AND 1E3 = 1000"));
        assert!(holds("A$ + \"E\" = \"ENDE\""));
        assert!(holds("NOT I = 4"));
        assert!(holds("!(I == 4) && I != 4 || 0"));
        assert!(holds("-I < -(2 + 2)"));
        assert!(!holds("0"));
        assert!(holds("I"), "not zero is true");
    }

    #[test]
    fn a_variable_not_set_is_zero_or_empty() {
        assert!(holds("Q = 0 AND Q$ = \"\" AND Q% = 0"));
        assert!(!holds("Q"));
    }

    #[test]
    fn array_elements_with_one_index() {
        assert!(holds("L(3) = 30 AND L(I - 4) = 10"));
        assert!(holds("N$(0) = \"X\""));
        // No such array, or outside it: true, but only where it is reached.
        assert!(holds("L(4) = 99"));
        assert!(holds("Z(0) = 1"));
        assert!(holds("L(-1) = 1"));
        assert!(!holds("I > 9 AND L(4) = 99"));
        assert!(holds("I = 5 OR L(4) = 99"));
        assert!(holds("1 / (I - 5) = 7"), "division by zero");
    }

    #[test]
    fn with_all_characters_of_a_name() {
        let e = parse("COUNT = 0", 0).unwrap();
        assert_eq!(
            e,
            Expr::Binary(
                Op::Eq,
                Box::new(Expr::Var("COUNT".into())),
                Box::new(Expr::Num(0.0))
            )
        );
        assert_eq!(
            parse("a1$ = \"\"", 1).unwrap(),
            Expr::Binary(
                Op::Eq,
                Box::new(Expr::Var("A$".into())),
                Box::new(Expr::Str(String::new()))
            )
        );
    }

    #[test]
    fn what_is_refused_when_set() {
        for bad in [
            "",
            "I =",
            "(I",
            "I 5",
            "A$",
            "A$ = 1",
            "A$ - \"X\" = \"\"",
            "I + A$ = 1",
            "NOT A$",
            "A$ AND 1",
            "L(1, 2) = 0",
            "L(A$) = 0",
            "\"open",
            "$",
            "I = AND",
            "I # 1",
        ] {
            assert!(parse(bad, 2).is_err(), "{bad:?}");
        }
        assert!(parse(&"(".repeat(100), 2).is_err());
        assert!(parse(&"NOT ".repeat(100), 2).is_err());
        assert!(parse(&"1+".repeat(200), 2).is_err());
    }
}
