//! The syntax tree of a Brass source file.

use crate::lexer::Pos;

#[derive(Clone, Debug)]
pub struct File {
    pub package: String,
    pub imports: Vec<Import>,
    pub decls: Vec<Decl>,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub name: Option<String>,
    pub path: String,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub enum Decl {
    Const(Vec<ConstSpec>),
    Var(Vec<VarSpec>),
    Type(Vec<TypeSpec>),
    Func(Box<FuncDecl>),
}

#[derive(Clone, Debug)]
pub struct ConstSpec {
    pub names: Vec<(String, Pos)>,
    pub ty: Option<TypeExpr>,
    pub values: Vec<Expr>,
    /// `iota` for this line.
    pub iota: i64,
}

#[derive(Clone, Debug)]
pub struct VarSpec {
    pub names: Vec<(String, Pos)>,
    pub ty: Option<TypeExpr>,
    pub values: Vec<Expr>,
}

#[derive(Clone, Debug)]
pub struct TypeSpec {
    pub name: String,
    pub pos: Pos,
    pub alias: bool,
    pub ty: TypeExpr,
}

#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub name: String,
    pub pos: Pos,
    /// Method receiver: name (may be empty), type name, by pointer.
    pub recv: Option<(String, String, bool)>,
    pub sig: FuncType,
    pub body: Option<Block>,
}

#[derive(Clone, Debug, Default)]
pub struct FuncType {
    pub params: Vec<Field>,
    pub results: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: Option<String>,
    pub ty: TypeExpr,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub enum TypeExpr {
    /// `name` or `pkg.name`.
    Name(Option<String>, String, Pos),
    Pointer(Box<TypeExpr>),
    Slice(Box<TypeExpr>),
    Array(Box<Expr>, Box<TypeExpr>),
    Struct(Vec<Field>),
    Func(Box<FuncType>),
    /// Methods, then embedded interfaces (`io.Reader`).
    Interface(Vec<(String, FuncType, Pos)>, Vec<TypeExpr>),
    Chan(Box<TypeExpr>),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub end: Pos,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Expr(Expr),
    VarDecl(Vec<VarSpec>, Pos),
    ConstDecl(Vec<ConstSpec>, Pos),
    TypeDecl(Vec<TypeSpec>, Pos),
    /// `a, b := …` (define) or `a, b = …`.
    Assign {
        lhs: Vec<Expr>,
        rhs: Vec<Expr>,
        define: bool,
        pos: Pos,
    },
    /// `x op= y`.
    OpAssign {
        lhs: Expr,
        op: &'static str,
        rhs: Expr,
        pos: Pos,
    },
    IncDec(Expr, bool, Pos),
    Block(Block),
    If {
        init: Option<Box<Stmt>>,
        cond: Expr,
        then: Block,
        els: Option<Box<Stmt>>,
        pos: Pos,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        post: Option<Box<Stmt>>,
        body: Block,
        label: Option<String>,
        pos: Pos,
    },
    /// `for k, v := range x` (define) or `=`.
    Range {
        key: Option<Expr>,
        value: Option<Expr>,
        define: bool,
        over: Expr,
        body: Block,
        label: Option<String>,
        pos: Pos,
    },
    Switch {
        init: Option<Box<Stmt>>,
        tag: Option<Expr>,
        cases: Vec<Case>,
        label: Option<String>,
        pos: Pos,
    },
    TypeSwitch {
        init: Option<Box<Stmt>>,
        bind: Option<String>,
        x: Expr,
        cases: Vec<TypeCase>,
        label: Option<String>,
        pos: Pos,
    },
    Return(Vec<Expr>, Pos),
    Break(Option<String>, Pos),
    Continue(Option<String>, Pos),
    Go(Expr, Pos),
    Send(Expr, Expr, Pos),
    Select(Vec<SelectCase>, Option<String>, Pos),
    Empty,
}

#[derive(Clone, Debug)]
pub struct SelectCase {
    pub comm: Comm,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub enum Comm {
    /// `case v, ok := <-c` / `case v = <-c` / `case <-c`.
    Recv {
        lhs: Vec<Expr>,
        define: bool,
        chan: Expr,
    },
    Send(Expr, Expr),
    Default,
}

#[derive(Clone, Debug)]
pub struct Case {
    /// Empty: `default`.
    pub exprs: Vec<Expr>,
    pub default: bool,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub struct TypeCase {
    /// `None` in the list: `nil`.
    pub types: Vec<Option<TypeExpr>>,
    pub default: bool,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

pub type ExprId = u32;

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: ExprId,
    pub pos: Pos,
    pub kind: ExprKind,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Ident(String),
    Int(i64),
    Char(u8),
    Str(Vec<u8>),
    /// `pkg.Name`, `x.field`, `x.Method`.
    Selector(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),
    Call(Box<Expr>, Vec<Expr>, bool),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// `T{…}`: elements with optional keys.
    Composite(Option<TypeExpr>, Vec<(Option<Expr>, Expr)>),
    FuncLit(Box<FuncType>, Block),
    /// `x.(T)`.
    Assert(Box<Expr>, TypeExpr),
    /// A type in expression position (conversions, `make`, `new`).
    Type(TypeExpr),
    Paren(Box<Expr>),
}

// ---- names whose address is taken ----

/// The names `&name` takes the address of somewhere in a body (scopes
/// aside: a name shadowed elsewhere is counted too, which costs nothing
/// but a pointer).
pub fn addressed(b: &Block) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    block(b, &mut out);
    out
}

type Names = std::collections::BTreeSet<String>;

fn block(b: &Block, out: &mut Names) {
    for s in &b.stmts {
        stmt(s, out);
    }
}

fn opt_stmt(s: &Option<Box<Stmt>>, out: &mut Names) {
    if let Some(s) = s {
        stmt(s, out);
    }
}

fn stmt(s: &Stmt, out: &mut Names) {
    match s {
        Stmt::Expr(e) | Stmt::IncDec(e, _, _) | Stmt::Go(e, _) => expr(e, out),
        Stmt::VarDecl(specs, _) => {
            for sp in specs {
                sp.values.iter().for_each(|e| expr(e, out));
            }
        }
        Stmt::Assign { lhs, rhs, .. } => lhs.iter().chain(rhs).for_each(|e| expr(e, out)),
        Stmt::OpAssign { lhs, rhs, .. } | Stmt::Send(lhs, rhs, _) => {
            expr(lhs, out);
            expr(rhs, out);
        }
        Stmt::Block(b) => block(b, out),
        Stmt::If {
            init,
            cond,
            then,
            els,
            ..
        } => {
            opt_stmt(init, out);
            expr(cond, out);
            block(then, out);
            opt_stmt(els, out);
        }
        Stmt::For {
            init,
            cond,
            post,
            body,
            ..
        } => {
            opt_stmt(init, out);
            cond.iter().for_each(|e| expr(e, out));
            opt_stmt(post, out);
            block(body, out);
        }
        Stmt::Range {
            key,
            value,
            over,
            body,
            ..
        } => {
            key.iter().chain(value).for_each(|e| expr(e, out));
            expr(over, out);
            block(body, out);
        }
        Stmt::Switch {
            init, tag, cases, ..
        } => {
            opt_stmt(init, out);
            tag.iter().for_each(|e| expr(e, out));
            for c in cases {
                c.exprs.iter().for_each(|e| expr(e, out));
                c.body.iter().for_each(|s| stmt(s, out));
            }
        }
        Stmt::TypeSwitch { init, x, cases, .. } => {
            opt_stmt(init, out);
            expr(x, out);
            for c in cases {
                c.body.iter().for_each(|s| stmt(s, out));
            }
        }
        Stmt::Return(es, _) => es.iter().for_each(|e| expr(e, out)),
        Stmt::Select(cases, _, _) => {
            for c in cases {
                match &c.comm {
                    Comm::Recv { lhs, chan, .. } => {
                        lhs.iter().for_each(|e| expr(e, out));
                        expr(chan, out);
                    }
                    Comm::Send(a, b) => {
                        expr(a, out);
                        expr(b, out);
                    }
                    Comm::Default => {}
                }
                c.body.iter().for_each(|s| stmt(s, out));
            }
        }
        Stmt::ConstDecl(..)
        | Stmt::TypeDecl(..)
        | Stmt::Break(..)
        | Stmt::Continue(..)
        | Stmt::Empty => {}
    }
}

fn expr(e: &Expr, out: &mut Names) {
    match &e.kind {
        ExprKind::Unary(op, x) => {
            if *op == "&" {
                if let ExprKind::Ident(n) = &x.kind {
                    out.insert(n.clone());
                }
            }
            expr(x, out);
        }
        ExprKind::Selector(x, _) | ExprKind::Assert(x, _) | ExprKind::Paren(x) => expr(x, out),
        ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) => {
            expr(a, out);
            expr(b, out);
        }
        ExprKind::Slice(a, b, c) => {
            expr(a, out);
            b.iter().chain(c).for_each(|e| expr(e, out));
        }
        ExprKind::Call(f, args, _) => {
            expr(f, out);
            args.iter().for_each(|e| expr(e, out));
        }
        ExprKind::Composite(_, els) => {
            for (k, v) in els {
                k.iter().for_each(|e| expr(e, out));
                expr(v, out);
            }
        }
        ExprKind::FuncLit(_, b) => block(b, out),
        ExprKind::Ident(_)
        | ExprKind::Int(_)
        | ExprKind::Char(_)
        | ExprKind::Str(_)
        | ExprKind::Type(_) => {}
    }
}
