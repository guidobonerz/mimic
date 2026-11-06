//! Tokens of Brass (BRASS.md 3): Go's lexical rules, automatic
//! semicolons at the end of a line after an identifier, a literal, one
//! of `break continue return ++ -- ) ] }`.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Pos {
    pub file: u16,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    /// A character literal: its byte.
    Char(u8),
    /// A string literal: its ISO 8859-1 bytes.
    Str(Vec<u8>),
    Kw(&'static str),
    Op(&'static str),
    Semi,
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "{s}"),
            Tok::Int(v) => write!(f, "{v}"),
            Tok::Char(c) => write!(f, "'{}'", *c as char),
            Tok::Str(_) => write!(f, "string literal"),
            Tok::Kw(k) | Tok::Op(k) => write!(f, "{k}"),
            Tok::Semi => write!(f, "newline or ;"),
            Tok::Eof => write!(f, "end of file"),
        }
    }
}

pub const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "else",
    "for",
    "func",
    "go",
    "if",
    "import",
    "interface",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

/// Longest first.
const OPS: &[&str] = &[
    "&^=", "<<=", ">>=", "...", "&&", "||", "<-", "++", "--", "==", "!=", "<=", ">=", ":=", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "&^", "+", "-", "*", "/", "%", "&", "|",
    "^", "<", ">", "=", "!", "(", ")", "[", "]", "{", "}", ",", ";", ".", ":", "~",
];

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
}

pub fn lex(file: u16, src: &str) -> Result<Vec<Token>, (Pos, String)> {
    let b = src.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let semi_after = |out: &[Token]| {
        matches!(
            out.last().map(|t| &t.tok),
            Some(Tok::Ident(_) | Tok::Int(_) | Tok::Char(_) | Tok::Str(_))
                | Some(Tok::Kw("break" | "continue" | "return"))
                | Some(Tok::Op("++" | "--" | ")" | "]" | "}"))
        )
    };
    while i < b.len() {
        let pos = Pos { file, line, col };
        let c = b[i];
        if c == b'\n' {
            if semi_after(&out) {
                out.push(Token {
                    tok: Tok::Semi,
                    pos,
                });
            }
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            col += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            let mut newline = false;
            i += 2;
            col += 2;
            loop {
                if i + 1 >= b.len() {
                    return Err((pos, "comment not closed".into()));
                }
                if b[i] == b'*' && b[i + 1] == b'/' {
                    i += 2;
                    col += 2;
                    break;
                }
                if b[i] == b'\n' {
                    newline = true;
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
                i += 1;
            }
            if newline && semi_after(&out) {
                out.push(Token {
                    tok: Tok::Semi,
                    pos,
                });
            }
            continue;
        }
        let start = i;
        let tok = if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let w = &src[start..i];
            match KEYWORDS.iter().find(|k| **k == w) {
                Some(k) => Tok::Kw(k),
                None => Tok::Ident(w.to_string()),
            }
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let w: String = src[start..i].chars().filter(|c| *c != '_').collect();
            let lw = w.to_ascii_lowercase();
            let v = if let Some(h) = lw.strip_prefix("0x") {
                i64::from_str_radix(h, 16)
            } else if let Some(o) = lw.strip_prefix("0o") {
                i64::from_str_radix(o, 8)
            } else if let Some(bn) = lw.strip_prefix("0b") {
                i64::from_str_radix(bn, 2)
            } else {
                lw.parse::<i64>()
            };
            Tok::Int(v.map_err(|_| (pos, format!("bad number {}", &src[start..i])))?)
        } else if c == b'\'' {
            i += 1;
            let (v, n) = escape(src, i).map_err(|e| (pos, e))?;
            i += n;
            if b.get(i) != Some(&b'\'') {
                return Err((pos, "character literal not closed".into()));
            }
            i += 1;
            Tok::Char(v)
        } else if c == b'"' {
            i += 1;
            let mut s = Vec::new();
            loop {
                match b.get(i) {
                    None | Some(b'\n') => return Err((pos, "string not closed".into())),
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    _ => {
                        let (v, n) = escape(src, i).map_err(|e| (pos, e))?;
                        s.push(v);
                        i += n;
                    }
                }
            }
            Tok::Str(s)
        } else if c == b'`' {
            i += 1;
            let mut s = Vec::new();
            loop {
                match src[i..].chars().next() {
                    None => return Err((pos, "raw string not closed".into())),
                    Some('`') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        if ch == '\n' {
                            line += 1;
                        }
                        if ch != '\r' {
                            s.push(latin1(ch).map_err(|e| (pos, e))?);
                        }
                        i += ch.len_utf8();
                    }
                }
            }
            Tok::Str(s)
        } else if let Some(op) = OPS.iter().find(|o| b[i..].starts_with(o.as_bytes())) {
            i += op.len();
            if *op == ";" {
                Tok::Semi
            } else {
                Tok::Op(op)
            }
        } else {
            let ch = src[i..].chars().next().unwrap_or('?');
            return Err((pos, format!("unexpected character {ch:?}")));
        };
        col += src[start..i].chars().count() as u32;
        out.push(Token { tok, pos });
    }
    let pos = Pos { file, line, col };
    if semi_after(&out) {
        out.push(Token {
            tok: Tok::Semi,
            pos,
        });
    }
    out.push(Token { tok: Tok::Eof, pos });
    Ok(out)
}

fn latin1(ch: char) -> Result<u8, String> {
    u8::try_from(u32::from(ch)).map_err(|_| format!("{ch:?} is not in ISO 8859-1"))
}

/// One (escaped) character at byte `i`: its byte and the bytes it took.
fn escape(src: &str, i: usize) -> Result<(u8, usize), String> {
    let mut it = src[i..].chars();
    let c = it.next().ok_or("unexpected end")?;
    if c != '\\' {
        return Ok((latin1(c)?, c.len_utf8()));
    }
    let e = it.next().ok_or("unexpected end")?;
    Ok(match e {
        'n' => (b'\n', 2),
        't' => (b'\t', 2),
        'r' => (b'\r', 2),
        '0' => (0, 2),
        'a' => (7, 2),
        'b' => (8, 2),
        'f' => (12, 2),
        'v' => (11, 2),
        '\\' => (b'\\', 2),
        '\'' => (b'\'', 2),
        '"' => (b'"', 2),
        'x' => {
            let h = src.get(i + 2..i + 4).ok_or("\\x needs two hex digits")?;
            (u8::from_str_radix(h, 16).map_err(|_| "bad \\x escape")?, 4)
        }
        _ => return Err(format!("unknown escape \\{e}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(0, s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn semicolons_numbers_strings() {
        assert_eq!(
            toks("x := 0x1_0 + 'a'\nreturn\n}"),
            vec![
                Tok::Ident("x".into()),
                Tok::Op(":="),
                Tok::Int(16),
                Tok::Op("+"),
                Tok::Char(b'a'),
                Tok::Semi,
                Tok::Kw("return"),
                Tok::Semi,
                Tok::Op("}"),
                Tok::Semi,
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("\"a\\n\\x41é\""),
            vec![Tok::Str(vec![b'a', 10, 0x41, 0xe9]), Tok::Semi, Tok::Eof]
        );
        assert!(lex(0, "\"€\"").is_err(), "not ISO 8859-1");
        assert_eq!(
            toks("a /* x\n */ b"),
            vec![
                Tok::Ident("a".into()),
                Tok::Semi,
                Tok::Ident("b".into()),
                Tok::Semi,
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("a&^=b"),
            vec![
                Tok::Ident("a".into()),
                Tok::Op("&^="),
                Tok::Ident("b".into()),
                Tok::Semi,
                Tok::Eof
            ]
        );
    }
}
