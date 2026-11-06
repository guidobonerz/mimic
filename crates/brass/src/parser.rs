//! Parser: tokens to [`crate::ast`] (BRASS.md 2–7), recursive descent
//! with Go's precedence. As in Go, composite literals `T{…}` are not taken
//! in the headers of `if`, `for` and `switch` unless parenthesised.

use crate::ast::*;
use crate::lexer::{Pos, Tok, Token};

pub struct Parser<'a> {
    toks: &'a [Token],
    i: usize,
    no_lit: bool,
    next_id: &'a mut ExprId,
}

pub type PResult<T> = Result<T, (Pos, String)>;

fn prec(op: &str) -> u8 {
    match op {
        "||" => 1,
        "&&" => 2,
        "==" | "!=" | "<" | "<=" | ">" | ">=" => 3,
        "+" | "-" | "|" | "^" => 4,
        "*" | "/" | "%" | "<<" | ">>" | "&" | "&^" => 5,
        _ => 0,
    }
}

impl<'a> Parser<'a> {
    pub fn new(toks: &'a [Token], next_id: &'a mut ExprId) -> Self {
        Self {
            toks,
            i: 0,
            no_lit: false,
            next_id,
        }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.i + k).min(self.toks.len() - 1)].tok
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].pos
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn is_op(&self, o: &str) -> bool {
        matches!(self.peek(), Tok::Op(x) if *x == o)
    }

    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Tok::Kw(x) if *x == k)
    }

    fn eat_op(&mut self, o: &str) -> bool {
        if self.is_op(o) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        if self.is_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, what: &str) -> PResult<T> {
        Err((
            self.pos(),
            format!("expected {what}, found {}", self.peek()),
        ))
    }

    fn expect_op(&mut self, o: &str) -> PResult<()> {
        if self.eat_op(o) {
            Ok(())
        } else {
            self.err(o)
        }
    }

    fn ident(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            _ => self.err("a name"),
        }
    }

    /// `;` or the end of a block.
    fn semi(&mut self) -> PResult<()> {
        if matches!(self.peek(), Tok::Semi) {
            self.bump();
            Ok(())
        } else if self.is_op(")") || self.is_op("}") || matches!(self.peek(), Tok::Eof) {
            Ok(())
        } else {
            self.err("newline or ;")
        }
    }

    fn mk(&mut self, pos: Pos, kind: ExprKind) -> Expr {
        let id = *self.next_id;
        *self.next_id += 1;
        Expr { id, pos, kind }
    }

    // ---- file ----

    pub fn file(&mut self) -> PResult<File> {
        if !self.eat_kw("package") {
            return self.err("package");
        }
        let package = self.ident()?;
        self.semi()?;
        let mut imports = Vec::new();
        while self.eat_kw("import") {
            if self.eat_op("(") {
                while !self.eat_op(")") {
                    imports.push(self.import_spec()?);
                    self.semi()?;
                }
            } else {
                imports.push(self.import_spec()?);
            }
            self.semi()?;
        }
        let mut decls = Vec::new();
        while !matches!(self.peek(), Tok::Eof) {
            decls.push(self.top_decl()?);
            self.semi()?;
        }
        Ok(File {
            package,
            imports,
            decls,
        })
    }

    fn import_spec(&mut self) -> PResult<Import> {
        let pos = self.pos();
        let name = match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Some(s)
            }
            Tok::Op(".") => return Err((pos, "dot imports are not in Brass".into())),
            _ => None,
        };
        match self.bump() {
            Tok::Str(s) => Ok(Import {
                name,
                path: String::from_utf8_lossy(&s).into_owned(),
                pos,
            }),
            _ => Err((pos, "expected an import path".into())),
        }
    }

    fn top_decl(&mut self) -> PResult<Decl> {
        match self.peek() {
            Tok::Kw("func") => self.func_decl(),
            Tok::Kw("var") => {
                self.bump();
                Ok(Decl::Var(self.group(Self::var_spec)?))
            }
            Tok::Kw("const") => {
                self.bump();
                Ok(Decl::Const(self.const_group()?))
            }
            Tok::Kw("type") => {
                self.bump();
                Ok(Decl::Type(self.group(Self::type_spec)?))
            }
            _ => self.err("a declaration"),
        }
    }

    fn group<T>(&mut self, f: fn(&mut Self) -> PResult<T>) -> PResult<Vec<T>> {
        if self.eat_op("(") {
            let mut v = Vec::new();
            while !self.eat_op(")") {
                v.push(f(self)?);
                self.semi()?;
            }
            Ok(v)
        } else {
            Ok(vec![f(self)?])
        }
    }

    fn const_group(&mut self) -> PResult<Vec<ConstSpec>> {
        let one = |p: &mut Self,
                   iota: i64,
                   prev: &Option<(Option<TypeExpr>, Vec<Expr>)>|
         -> PResult<ConstSpec> {
            let names = p.names()?;
            let (ty, values) = if p.is_op("=") || !(matches!(p.peek(), Tok::Semi) || p.is_op(")")) {
                let ty = if p.is_op("=") {
                    None
                } else {
                    Some(p.type_expr()?)
                };
                p.expect_op("=")?;
                (ty, p.expr_list()?)
            } else {
                match prev {
                    Some((t, v)) => (t.clone(), v.clone()),
                    None => return Err((p.pos(), "a constant needs a value".into())),
                }
            };
            Ok(ConstSpec {
                names,
                ty,
                values,
                iota,
            })
        };
        if self.eat_op("(") {
            let mut v: Vec<ConstSpec> = Vec::new();
            let mut prev = None;
            let mut iota = 0;
            while !self.eat_op(")") {
                let c = one(self, iota, &prev)?;
                prev = Some((c.ty.clone(), c.values.clone()));
                v.push(c);
                iota += 1;
                self.semi()?;
            }
            Ok(v)
        } else {
            Ok(vec![one(self, 0, &None)?])
        }
    }

    fn names(&mut self) -> PResult<Vec<(String, Pos)>> {
        let mut v = vec![(self.ident()?, self.toks[self.i - 1].pos)];
        while self.eat_op(",") {
            let p = self.pos();
            v.push((self.ident()?, p));
        }
        Ok(v)
    }

    fn var_spec(&mut self) -> PResult<VarSpec> {
        let names = self.names()?;
        let ty = if self.is_op("=") {
            None
        } else {
            Some(self.type_expr()?)
        };
        let values = if self.eat_op("=") {
            self.expr_list()?
        } else {
            Vec::new()
        };
        Ok(VarSpec { names, ty, values })
    }

    fn type_spec(&mut self) -> PResult<TypeSpec> {
        let pos = self.pos();
        let name = self.ident()?;
        let alias = self.eat_op("=");
        let ty = self.type_expr()?;
        Ok(TypeSpec {
            name,
            pos,
            alias,
            ty,
        })
    }

    fn func_decl(&mut self) -> PResult<Decl> {
        self.bump();
        let mut recv = None;
        if self.eat_op("(") {
            let mut rname = String::new();
            if matches!(self.peek_at(1), Tok::Ident(_) | Tok::Op("*")) {
                rname = self.ident()?;
            }
            let ptr = self.eat_op("*");
            let tname = self.ident()?;
            self.expect_op(")")?;
            recv = Some((rname, tname, ptr));
        }
        let pos = self.pos();
        let name = self.ident()?;
        let sig = self.signature()?;
        let body = if self.is_op("{") {
            Some(self.block()?)
        } else {
            None
        };
        Ok(Decl::Func(Box::new(FuncDecl {
            name,
            pos,
            recv,
            sig,
            body,
        })))
    }

    fn signature(&mut self) -> PResult<FuncType> {
        let params = self.params()?;
        let results = if self.is_op("(") {
            self.params()?
        } else if self.starts_type() {
            let pos = self.pos();
            vec![Field {
                name: None,
                ty: self.type_expr()?,
                pos,
            }]
        } else {
            Vec::new()
        };
        Ok(FuncType { params, results })
    }

    fn starts_type(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Ident(_)
                | Tok::Op("*")
                | Tok::Op("[")
                | Tok::Kw("struct")
                | Tok::Kw("func")
                | Tok::Kw("interface")
                | Tok::Kw("chan")
        )
    }

    /// `(a, b int, c string)` or `(int, string)`.
    fn params(&mut self) -> PResult<Vec<Field>> {
        self.expect_op("(")?;
        let mut items: Vec<(Pos, TypeExpr, Option<TypeExpr>)> = Vec::new();
        while !self.is_op(")") {
            let pos = self.pos();
            let first = self.type_expr()?;
            let second = if !self.is_op(",") && !self.is_op(")") {
                Some(self.type_expr()?)
            } else {
                None
            };
            items.push((pos, first, second));
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(")")?;
        let named = items.iter().any(|(_, _, s)| s.is_some());
        let mut out = Vec::new();
        if named {
            // Names without a type take the next type: `a, b int`.
            let mut pending: Vec<(String, Pos)> = Vec::new();
            for (pos, first, second) in items {
                let name = match first {
                    TypeExpr::Name(None, n, _) => n,
                    _ => return Err((pos, "a parameter name".into())),
                };
                pending.push((name, pos));
                if let Some(ty) = second {
                    for (n, p) in pending.drain(..) {
                        out.push(Field {
                            name: Some(n),
                            ty: ty.clone(),
                            pos: p,
                        });
                    }
                }
            }
            if !pending.is_empty() {
                return Err((pending[0].1, "a parameter needs a type".into()));
            }
        } else {
            for (pos, ty, _) in items {
                out.push(Field {
                    name: None,
                    ty,
                    pos,
                });
            }
        }
        Ok(out)
    }

    // ---- types ----

    pub fn type_expr(&mut self) -> PResult<TypeExpr> {
        let pos = self.pos();
        match self.peek().clone() {
            Tok::Ident(n) => {
                self.bump();
                if self.is_op(".") && matches!(self.peek_at(1), Tok::Ident(_)) {
                    self.bump();
                    let m = self.ident()?;
                    Ok(TypeExpr::Name(Some(n), m, pos))
                } else {
                    Ok(TypeExpr::Name(None, n, pos))
                }
            }
            Tok::Op("*") => {
                self.bump();
                Ok(TypeExpr::Pointer(Box::new(self.type_expr()?)))
            }
            Tok::Op("(") => {
                self.bump();
                let t = self.type_expr()?;
                self.expect_op(")")?;
                Ok(t)
            }
            Tok::Op("[") => {
                self.bump();
                if self.eat_op("]") {
                    return Ok(TypeExpr::Slice(Box::new(self.type_expr()?)));
                }
                let save = self.no_lit;
                self.no_lit = false;
                let n = self.expr()?;
                self.no_lit = save;
                self.expect_op("]")?;
                Ok(TypeExpr::Array(Box::new(n), Box::new(self.type_expr()?)))
            }
            Tok::Kw("chan") => {
                self.bump();
                Ok(TypeExpr::Chan(Box::new(self.type_expr()?)))
            }
            Tok::Kw("func") => {
                self.bump();
                Ok(TypeExpr::Func(Box::new(self.signature()?)))
            }
            Tok::Kw("struct") => {
                self.bump();
                self.expect_op("{")?;
                let mut fields = Vec::new();
                while !self.eat_op("}") {
                    let fpos = self.pos();
                    let names = self.names()?;
                    let ty = self.type_expr()?;
                    for (n, p) in names {
                        fields.push(Field {
                            name: Some(n),
                            ty: ty.clone(),
                            pos: if p == Pos::default() { fpos } else { p },
                        });
                    }
                    self.semi()?;
                }
                Ok(TypeExpr::Struct(fields))
            }
            Tok::Kw("interface") => {
                self.bump();
                self.expect_op("{")?;
                let mut ms = Vec::new();
                let mut embedded = Vec::new();
                while !self.eat_op("}") {
                    let mpos = self.pos();
                    if matches!(self.peek_at(1), Tok::Op(o) if *o == "(") {
                        let n = self.ident()?;
                        let sig = self.signature()?;
                        ms.push((n, sig, mpos));
                    } else {
                        embedded.push(self.type_expr()?);
                    }
                    self.semi()?;
                }
                Ok(TypeExpr::Interface(ms, embedded))
            }
            _ => self.err("a type"),
        }
    }

    // ---- statements ----

    pub fn block(&mut self) -> PResult<Block> {
        self.expect_op("{")?;
        let save = self.no_lit;
        self.no_lit = false;
        let mut stmts = Vec::new();
        while !self.is_op("}") {
            if matches!(self.peek(), Tok::Eof) {
                return self.err("}");
            }
            stmts.push(self.stmt()?);
            self.semi()?;
        }
        let end = self.pos();
        self.bump();
        self.no_lit = save;
        Ok(Block { stmts, end })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        // A label?
        if let (Tok::Ident(l), Tok::Op(":")) = (self.peek().clone(), self.peek_at(1).clone()) {
            self.bump();
            self.bump();
            while matches!(self.peek(), Tok::Semi) {
                self.bump();
            }
            let mut s = self.stmt()?;
            match &mut s {
                Stmt::For { label, .. }
                | Stmt::Range { label, .. }
                | Stmt::Switch { label, .. }
                | Stmt::TypeSwitch { label, .. } => *label = Some(l),
                Stmt::Select(_, label, _) => *label = Some(l),
                _ => return Err((pos, "a label stands before for or switch".into())),
            }
            return Ok(s);
        }
        match self.peek() {
            Tok::Kw("var") => {
                self.bump();
                Ok(Stmt::VarDecl(self.group(Self::var_spec)?, pos))
            }
            Tok::Kw("const") => {
                self.bump();
                Ok(Stmt::ConstDecl(self.const_group()?, pos))
            }
            Tok::Kw("type") => {
                self.bump();
                Ok(Stmt::TypeDecl(self.group(Self::type_spec)?, pos))
            }
            Tok::Kw("return") => {
                self.bump();
                let v = if matches!(self.peek(), Tok::Semi) || self.is_op("}") {
                    Vec::new()
                } else {
                    self.expr_list()?
                };
                Ok(Stmt::Return(v, pos))
            }
            Tok::Kw("break") | Tok::Kw("continue") => {
                let brk = self.is_kw("break");
                self.bump();
                let l = match self.peek().clone() {
                    Tok::Ident(s) => {
                        self.bump();
                        Some(s)
                    }
                    _ => None,
                };
                Ok(if brk {
                    Stmt::Break(l, pos)
                } else {
                    Stmt::Continue(l, pos)
                })
            }
            Tok::Kw("go") => {
                self.bump();
                Ok(Stmt::Go(self.expr()?, pos))
            }
            Tok::Op("{") => Ok(Stmt::Block(self.block()?)),
            Tok::Kw("if") => self.if_stmt(),
            Tok::Kw("for") => self.for_stmt(),
            Tok::Kw("switch") => self.switch_stmt(),
            Tok::Kw("select") => self.select_stmt(),
            Tok::Semi => Ok(Stmt::Empty),
            _ => self.simple_stmt(),
        }
    }

    fn simple_stmt(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        let lhs = self.expr_list()?;
        match self.peek().clone() {
            Tok::Op(":=") | Tok::Op("=") => {
                let define = self.is_op(":=");
                self.bump();
                if self.is_kw("range") {
                    return Err((self.pos(), "range only in a for".into()));
                }
                let rhs = self.expr_list()?;
                Ok(Stmt::Assign {
                    lhs,
                    rhs,
                    define,
                    pos,
                })
            }
            Tok::Op(
                op
                @ ("+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" | "&^="),
            ) => {
                self.bump();
                let rhs = self.expr()?;
                let one = single(lhs, pos)?;
                Ok(Stmt::OpAssign {
                    lhs: one,
                    op: &op[..op.len() - 1],
                    rhs,
                    pos,
                })
            }
            Tok::Op("++") | Tok::Op("--") => {
                let inc = self.is_op("++");
                self.bump();
                Ok(Stmt::IncDec(single(lhs, pos)?, inc, pos))
            }
            Tok::Op("<-") => {
                self.bump();
                let v = self.expr()?;
                Ok(Stmt::Send(single(lhs, pos)?, v, pos))
            }
            _ => Ok(Stmt::Expr(single(lhs, pos)?)),
        }
    }

    fn if_stmt(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        self.bump();
        let save = self.no_lit;
        self.no_lit = true;
        let mut init = None;
        let first = self.simple_stmt()?;
        let cond = if matches!(self.peek(), Tok::Semi) {
            self.bump();
            init = Some(Box::new(first));
            self.expr()?
        } else {
            match first {
                Stmt::Expr(e) => e,
                _ => return Err((pos, "if needs a condition".into())),
            }
        };
        self.no_lit = save;
        let then = self.block()?;
        let els = if self.eat_kw("else") {
            if self.is_kw("if") {
                Some(Box::new(self.if_stmt()?))
            } else {
                Some(Box::new(Stmt::Block(self.block()?)))
            }
        } else {
            None
        };
        Ok(Stmt::If {
            init,
            cond,
            then,
            els,
            pos,
        })
    }

    fn for_stmt(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        self.bump();
        let save = self.no_lit;
        self.no_lit = true;
        if self.is_op("{") {
            self.no_lit = save;
            return Ok(Stmt::For {
                init: None,
                cond: None,
                post: None,
                body: self.block()?,
                label: None,
                pos,
            });
        }
        if self.eat_kw("range") {
            let over = self.expr()?;
            self.no_lit = save;
            return Ok(Stmt::Range {
                key: None,
                value: None,
                define: false,
                over,
                body: self.block()?,
                label: None,
                pos,
            });
        }
        let mut init = None;
        if !matches!(self.peek(), Tok::Semi) {
            // `k, v := range x` ?
            let lhs = self.expr_list()?;
            if (self.is_op(":=") || self.is_op("=")) && matches!(self.peek_at(1), Tok::Kw("range"))
            {
                let define = self.is_op(":=");
                self.bump();
                self.bump();
                let over = self.expr()?;
                self.no_lit = save;
                let mut it = lhs.into_iter();
                return Ok(Stmt::Range {
                    key: it.next(),
                    value: it.next(),
                    define,
                    over,
                    body: self.block()?,
                    label: None,
                    pos,
                });
            }
            // Re-parse as a simple statement from the start.
            let s = self.finish_simple(lhs, pos)?;
            if self.is_op("{") {
                self.no_lit = save;
                let cond = match s {
                    Stmt::Expr(e) => e,
                    _ => return Err((pos, "for needs a condition".into())),
                };
                return Ok(Stmt::For {
                    init: None,
                    cond: Some(cond),
                    post: None,
                    body: self.block()?,
                    label: None,
                    pos,
                });
            }
            init = Some(Box::new(s));
        }
        if !matches!(self.peek(), Tok::Semi) {
            return self.err(";");
        }
        self.bump();
        let cond = if matches!(self.peek(), Tok::Semi) {
            None
        } else {
            Some(self.expr()?)
        };
        if !matches!(self.peek(), Tok::Semi) {
            return self.err(";");
        }
        self.bump();
        let post = if self.is_op("{") {
            None
        } else {
            Some(Box::new(self.simple_stmt()?))
        };
        self.no_lit = save;
        Ok(Stmt::For {
            init,
            cond,
            post,
            body: self.block()?,
            label: None,
            pos,
        })
    }

    /// A simple statement whose expression list is already parsed.
    fn finish_simple(&mut self, lhs: Vec<Expr>, pos: Pos) -> PResult<Stmt> {
        match self.peek().clone() {
            Tok::Op(":=") | Tok::Op("=") => {
                let define = self.is_op(":=");
                self.bump();
                let rhs = self.expr_list()?;
                Ok(Stmt::Assign {
                    lhs,
                    rhs,
                    define,
                    pos,
                })
            }
            Tok::Op(
                op
                @ ("+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" | "&^="),
            ) => {
                self.bump();
                let rhs = self.expr()?;
                Ok(Stmt::OpAssign {
                    lhs: single(lhs, pos)?,
                    op: &op[..op.len() - 1],
                    rhs,
                    pos,
                })
            }
            Tok::Op("++") | Tok::Op("--") => {
                let inc = self.is_op("++");
                self.bump();
                Ok(Stmt::IncDec(single(lhs, pos)?, inc, pos))
            }
            _ => Ok(Stmt::Expr(single(lhs, pos)?)),
        }
    }

    fn switch_stmt(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        self.bump();
        let save = self.no_lit;
        self.no_lit = true;
        let mut init = None;
        let mut tag = None;
        if !self.is_op("{") {
            let s = if matches!(self.peek(), Tok::Semi) {
                None
            } else {
                Some(self.simple_stmt()?)
            };
            if matches!(self.peek(), Tok::Semi) {
                self.bump();
                init = s.map(Box::new);
                if !self.is_op("{") {
                    tag = Some(self.simple_stmt()?);
                }
            } else {
                tag = s;
            }
        }
        self.no_lit = save;
        // A type switch: `x.(type)` or `v := x.(type)`.
        let type_switch = |e: &Expr| matches!(&e.kind, ExprKind::Assert(_, TypeExpr::Name(None, n, _)) if n == "type");
        match tag {
            Some(Stmt::Expr(e)) if type_switch(&e) => {
                let ExprKind::Assert(x, _) = e.kind else {
                    unreachable!()
                };
                self.type_switch_body(init, None, *x, pos)
            }
            Some(Stmt::Assign {
                lhs,
                rhs,
                define: true,
                ..
            }) if lhs.len() == 1 && rhs.len() == 1 && type_switch(&rhs[0]) => {
                let bind = match &lhs[0].kind {
                    ExprKind::Ident(n) => n.clone(),
                    _ => return Err((pos, "a name".into())),
                };
                let ExprKind::Assert(x, _) = rhs.into_iter().next().unwrap().kind else {
                    unreachable!()
                };
                self.type_switch_body(init, Some(bind), *x, pos)
            }
            tag => {
                let tag = match tag {
                    None => None,
                    Some(Stmt::Expr(e)) => Some(e),
                    Some(_) => return Err((pos, "switch needs an expression".into())),
                };
                self.expect_op("{")?;
                let mut cases = Vec::new();
                while !self.eat_op("}") {
                    let cpos = self.pos();
                    let (exprs, default) = if self.eat_kw("default") {
                        (Vec::new(), true)
                    } else if self.eat_kw("case") {
                        (self.expr_list()?, false)
                    } else {
                        return self.err("case or default");
                    };
                    self.expect_op(":")?;
                    let body = self.case_body()?;
                    cases.push(Case {
                        exprs,
                        default,
                        body,
                        pos: cpos,
                    });
                }
                Ok(Stmt::Switch {
                    init,
                    tag,
                    cases,
                    label: None,
                    pos,
                })
            }
        }
    }

    fn select_stmt(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        self.bump();
        self.expect_op("{")?;
        let mut cases = Vec::new();
        while !self.eat_op("}") {
            let cpos = self.pos();
            let comm = if self.eat_kw("default") {
                Comm::Default
            } else if self.eat_kw("case") {
                if self.is_op("<-") {
                    self.bump();
                    let chan = self.expr()?;
                    Comm::Recv {
                        lhs: Vec::new(),
                        define: false,
                        chan,
                    }
                } else {
                    let lhs = self.expr_list()?;
                    if self.is_op(":=") || self.is_op("=") {
                        let define = self.is_op(":=");
                        self.bump();
                        if !self.eat_op("<-") {
                            return self.err("<- after the names of a select case");
                        }
                        let chan = self.expr()?;
                        Comm::Recv { lhs, define, chan }
                    } else if self.eat_op("<-") {
                        let v = self.expr()?;
                        Comm::Send(single(lhs, cpos)?, v)
                    } else {
                        // `case <-c:` parsed as a unary expression
                        match single(lhs, cpos)? {
                            Expr {
                                kind: ExprKind::Unary("<-", c),
                                ..
                            } => Comm::Recv {
                                lhs: Vec::new(),
                                define: false,
                                chan: *c,
                            },
                            _ => return Err((cpos, "a send or receive in a select case".into())),
                        }
                    }
                }
            } else {
                return self.err("case or default");
            };
            self.expect_op(":")?;
            let body = self.case_body()?;
            cases.push(SelectCase {
                comm,
                body,
                pos: cpos,
            });
        }
        Ok(Stmt::Select(cases, None, pos))
    }

    fn type_switch_body(
        &mut self,
        init: Option<Box<Stmt>>,
        bind: Option<String>,
        x: Expr,
        pos: Pos,
    ) -> PResult<Stmt> {
        self.expect_op("{")?;
        let mut cases = Vec::new();
        while !self.eat_op("}") {
            let cpos = self.pos();
            let (types, default) = if self.eat_kw("default") {
                (Vec::new(), true)
            } else if self.eat_kw("case") {
                let mut v = Vec::new();
                loop {
                    if matches!(self.peek(), Tok::Ident(n) if n == "nil") {
                        self.bump();
                        v.push(None);
                    } else {
                        v.push(Some(self.type_expr()?));
                    }
                    if !self.eat_op(",") {
                        break;
                    }
                }
                (v, false)
            } else {
                return self.err("case or default");
            };
            self.expect_op(":")?;
            let body = self.case_body()?;
            cases.push(TypeCase {
                types,
                default,
                body,
                pos: cpos,
            });
        }
        Ok(Stmt::TypeSwitch {
            init,
            bind,
            x,
            cases,
            label: None,
            pos,
        })
    }

    fn case_body(&mut self) -> PResult<Vec<Stmt>> {
        let mut body = Vec::new();
        while !self.is_kw("case") && !self.is_kw("default") && !self.is_op("}") {
            body.push(self.stmt()?);
            self.semi()?;
        }
        Ok(body)
    }

    // ---- expressions ----

    fn expr_list(&mut self) -> PResult<Vec<Expr>> {
        let mut v = vec![self.expr()?];
        while self.eat_op(",") {
            v.push(self.expr()?);
        }
        Ok(v)
    }

    pub fn expr(&mut self) -> PResult<Expr> {
        self.binary(1)
    }

    fn binary(&mut self, min: u8) -> PResult<Expr> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Op(o) if prec(o) >= min && prec(o) > 0 => *o,
                _ => return Ok(left),
            };
            let pos = self.pos();
            self.bump();
            let right = self.binary(prec(op) + 1)?;
            left = self.mk(pos, ExprKind::Binary(op, Box::new(left), Box::new(right)));
        }
    }

    fn unary(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        if let Tok::Op(o @ ("-" | "+" | "!" | "^" | "*" | "&" | "<-")) = self.peek().clone() {
            self.bump();
            let x = self.unary()?;
            return Ok(self.mk(pos, ExprKind::Unary(o, Box::new(x))));
        }
        self.primary()
    }

    fn primary(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        let mut x = match self.peek().clone() {
            Tok::Ident(n) => {
                self.bump();
                self.mk(pos, ExprKind::Ident(n))
            }
            Tok::Int(v) => {
                self.bump();
                self.mk(pos, ExprKind::Int(v))
            }
            Tok::Char(c) => {
                self.bump();
                self.mk(pos, ExprKind::Char(c))
            }
            Tok::Str(s) => {
                self.bump();
                self.mk(pos, ExprKind::Str(s))
            }
            Tok::Op("(") => {
                self.bump();
                let save = self.no_lit;
                self.no_lit = false;
                let e = self.expr()?;
                self.no_lit = save;
                self.expect_op(")")?;
                self.mk(pos, ExprKind::Paren(Box::new(e)))
            }
            Tok::Kw("func") => {
                self.bump();
                let sig = self.signature()?;
                if self.is_op("{") {
                    let body = self.block()?;
                    self.mk(pos, ExprKind::FuncLit(Box::new(sig), body))
                } else {
                    self.mk(pos, ExprKind::Type(TypeExpr::Func(Box::new(sig))))
                }
            }
            Tok::Op("[") | Tok::Kw("struct") | Tok::Kw("interface") | Tok::Kw("chan") => {
                let t = self.type_expr()?;
                self.mk(pos, ExprKind::Type(t))
            }
            _ => return self.err("an expression"),
        };
        loop {
            let pos = self.pos();
            if self.eat_op(".") {
                if self.eat_op("(") {
                    let t = if self.is_kw("type") {
                        let p = self.pos();
                        self.bump();
                        TypeExpr::Name(None, "type".into(), p)
                    } else {
                        self.type_expr()?
                    };
                    self.expect_op(")")?;
                    x = self.mk(pos, ExprKind::Assert(Box::new(x), t));
                } else {
                    let n = self.ident()?;
                    x = self.mk(pos, ExprKind::Selector(Box::new(x), n));
                }
            } else if self.is_op("[") {
                self.bump();
                let save = self.no_lit;
                self.no_lit = false;
                let lo = if self.is_op(":") {
                    None
                } else {
                    Some(Box::new(self.expr()?))
                };
                if self.eat_op(":") {
                    let hi = if self.is_op("]") {
                        None
                    } else {
                        Some(Box::new(self.expr()?))
                    };
                    self.no_lit = save;
                    self.expect_op("]")?;
                    x = self.mk(pos, ExprKind::Slice(Box::new(x), lo, hi));
                } else {
                    self.no_lit = save;
                    self.expect_op("]")?;
                    let i = lo.ok_or((pos, "an index".to_string()))?;
                    x = self.mk(pos, ExprKind::Index(Box::new(x), i));
                }
            } else if self.is_op("(") {
                self.bump();
                let save = self.no_lit;
                self.no_lit = false;
                let mut args = Vec::new();
                let mut spread = false;
                while !self.is_op(")") {
                    args.push(self.expr()?);
                    if self.eat_op("...") {
                        spread = true;
                    }
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.no_lit = save;
                self.expect_op(")")?;
                x = self.mk(pos, ExprKind::Call(Box::new(x), args, spread));
            } else if self.is_op("{") && !self.no_lit && could_be_type(&x) {
                let t = to_type(&x).ok_or((pos, "a type before {".to_string()))?;
                let elems = self.composite_body()?;
                x = self.mk(pos, ExprKind::Composite(Some(t), elems));
            } else {
                return Ok(x);
            }
        }
    }

    fn composite_body(&mut self) -> PResult<Vec<(Option<Expr>, Expr)>> {
        self.expect_op("{")?;
        let save = self.no_lit;
        self.no_lit = false;
        let mut elems = Vec::new();
        while !self.is_op("}") {
            let v = self.element()?;
            if self.eat_op(":") {
                let val = self.element()?;
                elems.push((Some(v), val));
            } else {
                elems.push((None, v));
            }
            if !self.eat_op(",") {
                while matches!(self.peek(), Tok::Semi) {
                    self.bump();
                }
                break;
            }
            while matches!(self.peek(), Tok::Semi) {
                self.bump();
            }
        }
        self.no_lit = save;
        self.expect_op("}")?;
        Ok(elems)
    }

    /// An element: an expression or `{…}` (the element type elided).
    fn element(&mut self) -> PResult<Expr> {
        if self.is_op("{") {
            let pos = self.pos();
            let elems = self.composite_body()?;
            Ok(self.mk(pos, ExprKind::Composite(None, elems)))
        } else {
            self.expr()
        }
    }
}

fn single(mut v: Vec<Expr>, pos: Pos) -> PResult<Expr> {
    if v.len() != 1 {
        return Err((pos, "one expression expected".into()));
    }
    Ok(v.pop().unwrap())
}

fn could_be_type(x: &Expr) -> bool {
    match &x.kind {
        ExprKind::Ident(_) | ExprKind::Type(_) => true,
        ExprKind::Selector(b, _) => matches!(b.kind, ExprKind::Ident(_)),
        _ => false,
    }
}

fn to_type(x: &Expr) -> Option<TypeExpr> {
    match &x.kind {
        ExprKind::Ident(n) => Some(TypeExpr::Name(None, n.clone(), x.pos)),
        ExprKind::Selector(b, n) => match &b.kind {
            ExprKind::Ident(p) => Some(TypeExpr::Name(Some(p.clone()), n.clone(), x.pos)),
            _ => None,
        },
        ExprKind::Type(t) => Some(t.clone()),
        _ => None,
    }
}

pub fn parse(toks: &[Token], next_id: &mut ExprId) -> PResult<File> {
    Parser::new(toks, next_id).file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn p(src: &str) -> File {
        let toks = lex(0, src).unwrap();
        let mut id = 0;
        parse(&toks, &mut id).unwrap_or_else(|(pos, e)| panic!("{}:{}: {e}", pos.line, pos.col))
    }

    #[test]
    fn parses_a_little_program() {
        let f = p(r#"
package main

import (
    "console"
    s "strs"
)

const (
    A = iota
    B
)

type Point struct { X, Y int }

func (p *Point) Move(dx, dy int) { p.X += dx; p.Y += dy }

func main() {
    var a [3]int
    b := []int{1, 2, 3}
    pt := Point{X: 1, Y: 2}
    for i := 0; i < len(b); i++ {
        a[i] = b[i] * 2
    }
    for _, v := range b {
        if v > 1 { console.Print("big") } else if v == 0 { break }
    }
    switch x := b[0]; x {
    case 1, 2:
        pt.Move(1, 1)
    default:
    }
    p := &pt
    _ = p
}
"#);
        assert_eq!(f.package, "main");
        assert_eq!(f.imports.len(), 2);
        assert_eq!(f.decls.len(), 4);
    }

    #[test]
    fn composite_literals_are_not_taken_in_headers() {
        p("package main\nfunc f() { if x == y { } ; for i := range n { } }");
        p("package main\nfunc f() { if (T{} == x) { } }");
    }
}
