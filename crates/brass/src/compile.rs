//! Type checking and code generation for Brass (BRASS.md), in one walk:
//! every expression is typed as it is translated into Ada assembly.
//!
//! **[B]** Code (BRASS.md 11, details decided here): R1–R4 carry
//! arguments and results; a function keeps R9, R10, R11 in its frame
//! and uses R9/R10 as scratch; R5–R8 are never touched (assembly may rely
//! on them). Frame, addressed from R12 (segment S2), which does not move
//! inside a function body: outgoing arguments beyond R4 at 0, then locals
//! and temporaries, then the saved R9, R10, R11, then the caller's
//! outgoing arguments (this function's 5th word on). Every temporary has
//! its own slot, so nothing is ever below R12 (interrupts are safe).
//! Pointers are (segment, offset) pairs: loads and stores through them use
//! S3 and the X instructions; package variables and string literals live
//! in the data segment S1. Every jump is long (`li r10, #L` / `jr r10`).

use std::collections::HashMap;
use std::fmt::Write;

use crate::ast::*;
use crate::lexer::Pos;
use crate::types::{int_range, Const, Named, Sig, Ty, Types};

mod conc;
mod decls;
mod expr;
mod iface;
mod stmt;

pub type CResult<T> = Result<T, (Pos, String)>;

fn err<T>(pos: Pos, msg: impl Into<String>) -> CResult<T> {
    Err((pos, msg.into()))
}

/// Something a name stands for.
#[derive(Clone, Debug)]
pub enum Obj {
    Var { sym: String, ty: Ty },
    Const { ty: Ty, val: Const },
    Func { sym: String, sig: Sig, asm: bool },
    Type(Ty),
    Builtin(&'static str),
    Package(String),
    Nil,
}

/// A package as the driver hands it over: its path and parsed files.
pub struct PackageSrc {
    pub path: String,
    pub files: Vec<(u16, File)>,
}

#[derive(Clone, Debug)]
enum Local {
    /// A variable in the frame; `by_ref`: the slot holds a pointer to it
    /// (a struct or array parameter).
    Var {
        off: u32,
        ty: Ty,
        by_ref: bool,
    },
    Const {
        ty: Ty,
        val: Const,
    },
    Type(Ty),
}

struct LoopLabels {
    name: Option<String>,
    brk: String,
    cont: Option<String>,
}

/// The function being translated.
struct Fn {
    scopes: Vec<HashMap<String, Local>>,
    /// Bytes of locals and temporaries so far, the most ever.
    size: u32,
    max: u32,
    /// Bytes for outgoing arguments beyond R4 (this pass), the most a
    /// call needed.
    out: u32,
    out_need: u32,
    results: Vec<Ty>,
    loops: Vec<LoopLabels>,
    ret_label: String,
    body: String,
    /// Names whose address the body takes (`&name`): such a local of
    /// several words lives in a cell in memory order (`declare_local`).
    addressed: std::collections::BTreeSet<String>,
}

pub struct Compiler {
    pub types: Types,
    /// Package path → name → object.
    pkgs: HashMap<String, HashMap<String, Obj>>,
    /// Per source file: import name → package path.
    imports: HashMap<u16, HashMap<String, String>>,
    pkg: String,
    file: u16,
    data: String,
    strings: HashMap<Vec<u8>, String>,
    labels: u32,
    code: String,
    pkg_code: Vec<(String, String)>,
    f: Option<Fn>,
    /// Untyped constants and `iota` while evaluating a const spec.
    iota: Option<i64>,
    /// Package init functions, in order.
    pub inits: Vec<String>,
    pub files: Vec<String>,
    /// Interfaces: method-name numbers, type descriptors.
    ifc: iface::IfaceState,
}

pub fn mangle(pkg: &str, name: &str) -> String {
    let p: String = pkg
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{p}__{name}")
}

/// What `print` and friends call in the runtime.
const RT_PRINT_STR: &str = "rt_print_str";

impl Compiler {
    pub fn new(files: Vec<String>) -> Self {
        Self {
            types: Types::default(),
            pkgs: HashMap::new(),
            imports: HashMap::new(),
            pkg: String::new(),
            file: 0,
            data: String::new(),
            strings: HashMap::new(),
            labels: 0,
            code: String::new(),
            pkg_code: Vec::new(),
            f: None,
            iota: None,
            inits: Vec::new(),
            files,
            ifc: iface::IfaceState::default(),
        }
    }

    /// The code of each package, in order.
    pub fn code(&self) -> &[(String, String)] {
        &self.pkg_code
    }

    pub fn data(&self) -> &str {
        &self.data
    }

    // ---- output ----

    fn out(&mut self) -> &mut String {
        match &mut self.f {
            Some(f) => &mut f.body,
            None => &mut self.code,
        }
    }

    fn emit(&mut self, s: impl AsRef<str>) {
        let o = self.out();
        o.push_str("        ");
        o.push_str(s.as_ref());
        o.push('\n');
    }

    fn label(&mut self, l: &str) {
        let o = self.out();
        o.push_str(l);
        o.push_str(":\n");
    }

    fn new_label(&mut self) -> String {
        self.labels += 1;
        format!(".L{}", self.labels)
    }

    fn jump(&mut self, l: &str) {
        self.emit(format!("li      r10, #{l}"));
        self.emit("jr      r10");
    }

    /// Jump to `l` when R1 is 0 (false).
    fn jump_if_false(&mut self, l: &str) {
        let skip = self.new_label();
        self.emit("cmpi    r1, #0");
        self.emit(format!("bne     {skip}"));
        self.jump(l);
        self.label(&skip);
    }

    fn jump_if_true(&mut self, l: &str) {
        let skip = self.new_label();
        self.emit("cmpi    r1, #0");
        self.emit(format!("beq     {skip}"));
        self.jump(l);
        self.label(&skip);
    }

    /// R = v (16 bits).
    fn li(&mut self, r: u8, v: i64) {
        let v = v as u16;
        if v < 256 {
            self.emit(format!("movi    r{r}, #{v}"));
        } else {
            self.emit(format!("li      r{r}, #${v:04x}"));
        }
    }

    /// A string literal in the data segment: its label.
    fn string_label(&mut self, s: &[u8]) -> String {
        if let Some(l) = self.strings.get(s) {
            return l.clone();
        }
        let l = format!("str_{}", self.strings.len());
        let mut line = format!("{l}:");
        if s.is_empty() {
            line.push('\n');
        } else {
            for chunk in s.chunks(16) {
                let b: Vec<String> = chunk.iter().map(|c| format!("${c:02x}")).collect();
                let _ = write!(line, "\n        .byte   {}", b.join(","));
            }
            line.push('\n');
        }
        self.data.push_str(&line);
        self.strings.insert(s.to_vec(), l.clone());
        l
    }

    // ---- the frame ----

    fn fnc(&mut self) -> &mut Fn {
        self.f.as_mut().expect("in a function")
    }

    /// A slot of `bytes` for a temporary or local (even offsets).
    fn slot(&mut self, bytes: u32) -> u32 {
        let f = self.fnc();
        let off = f.out + f.size.next_multiple_of(2);
        f.size = f.size.next_multiple_of(2) + bytes.max(1);
        f.max = f.max.max(f.size);
        off
    }

    /// Free temporaries back to a mark.
    fn mark(&mut self) -> u32 {
        self.fnc().size
    }

    fn release(&mut self, m: u32) {
        self.fnc().size = m;
    }

    /// Frame offsets are known only at the end (the outgoing area); slots
    /// are written as `OUT+n` and fixed up then.
    fn frame_ref(off: u32) -> String {
        format!("{off}")
    }

    /// R = word at frame offset `off`.
    fn ld(&mut self, r: u8, off: u32) {
        if off <= 30 {
            self.emit(format!("lw      r{r}, {}(r12)", Self::frame_ref(off)));
        } else {
            self.x_frame(off);
            self.emit(format!("lwx     r{r}, (r9)"));
        }
    }

    fn st(&mut self, r: u8, off: u32) {
        if off <= 30 {
            self.emit(format!("sw      r{r}, {}(r12)", Self::frame_ref(off)));
        } else {
            self.x_frame(off);
            self.emit(format!("swx     r{r}, (r9)"));
        }
    }

    /// S3:R9 = the frame slot at `off`.
    fn x_frame(&mut self, off: u32) {
        self.emit("mfs     r9, s2");
        self.emit("mts     s3, r9");
        self.li(9, i64::from(off));
        self.emit("add     r9, r12");
    }

    /// R1:R2 = the address of frame offset `off`.
    fn frame_addr(&mut self, off: u32) {
        self.emit("mfs     r1, s2");
        self.li(2, i64::from(off));
        self.emit("add     r2, r12");
    }

    /// Store the scalar in R1.. at S3:R9 (memory order).
    fn store_x(&mut self, ty: &Ty) {
        let words = self.types.words(ty);
        if self.types.is_byte_sized(ty) {
            self.emit("sbx     r1, (r9)");
            return;
        }
        if self.types.is_iface(ty) {
            // two pointers, each offset first
            self.emit("swx     r2, (r9)+");
            self.emit("swx     r1, (r9)+");
            self.emit("swx     r4, (r9)+");
            self.emit("swx     r3, (r9)");
            return;
        }
        match words {
            1 => self.emit("swx     r1, (r9)"),
            2 => {
                self.emit("swx     r2, (r9)+");
                self.emit("swx     r1, (r9)");
            }
            3 => {
                self.emit("swx     r2, (r9)+");
                self.emit("swx     r1, (r9)+");
                self.emit("swx     r3, (r9)");
            }
            _ => {
                self.emit("swx     r2, (r9)+");
                self.emit("swx     r1, (r9)+");
                self.emit("swx     r3, (r9)+");
                self.emit("swx     r4, (r9)");
            }
        }
    }

    /// Load the scalar at S3:R9 into R1.. (memory order).
    fn load_x(&mut self, ty: &Ty) {
        let words = self.types.words(ty);
        if self.types.is_byte_sized(ty) {
            self.emit("lbx     r1, (r9)");
            if matches!(self.types.under(ty), Ty::I8) {
                self.emit("sxb     r1, r1");
            }
            return;
        }
        if self.types.is_iface(ty) {
            self.emit("lwx     r2, (r9)+");
            self.emit("lwx     r1, (r9)+");
            self.emit("lwx     r4, (r9)+");
            self.emit("lwx     r3, (r9)");
            return;
        }
        match words {
            1 => self.emit("lwx     r1, (r9)"),
            2 => {
                self.emit("lwx     r2, (r9)+");
                self.emit("lwx     r1, (r9)");
            }
            3 => {
                self.emit("lwx     r2, (r9)+");
                self.emit("lwx     r1, (r9)+");
                self.emit("lwx     r3, (r9)");
            }
            _ => {
                self.emit("lwx     r2, (r9)+");
                self.emit("lwx     r1, (r9)+");
                self.emit("lwx     r3, (r9)+");
                self.emit("lwx     r4, (r9)");
            }
        }
    }

    /// Spill the scalar in R1.. to a new temporary; its offset.
    fn spill(&mut self, ty: &Ty) -> u32 {
        let w = self.types.words(ty).max(1);
        let off = self.slot(2 * w);
        for k in 0..w {
            self.st(k as u8 + 1, off + 2 * k);
        }
        off
    }

    /// Reload a spilled scalar into registers from `base` (R`base`..).
    fn unspill(&mut self, ty: &Ty, off: u32, base: u8) {
        let w = self.types.words(ty).max(1);
        for k in 0..w {
            self.ld(base + k as u8, off + 2 * k);
        }
    }

    /// Save R1:R2 (an address) in a temporary.
    fn spill_addr(&mut self) -> u32 {
        let off = self.slot(4);
        self.st(1, off);
        self.st(2, off + 2);
        off
    }

    /// S3:R9 = a spilled address (segment at `off`, offset at `off + 2`).
    /// Uses R10.
    fn addr_to_x(&mut self, off: u32) {
        if off + 2 <= 30 {
            self.ld(10, off);
            self.ld(9, off + 2);
        } else {
            self.x_frame(off);
            self.emit("lwx     r10, (r9)+");
            self.emit("lwx     r9, (r9)");
        }
        self.emit("mts     s3, r10");
    }

    /// Copy `size` bytes from the address in R3:R4 to the address in R1:R2.
    fn memcpy(&mut self, size: u32) {
        self.li(9, i64::from(size));
        self.rt("rt_memcpy");
    }

    /// Call a runtime routine (near: every code segment has its copy;
    /// the driver turns `@rt_` into that copy's prefix).
    fn rt(&mut self, name: &str) {
        self.emit(format!("li      r10, #@{name}"));
        self.emit("jalr    r11, r10");
    }

    // ---- names ----

    fn lookup(&self, name: &str) -> Option<Obj> {
        if let Some(f) = &self.f {
            for s in f.scopes.iter().rev() {
                if let Some(l) = s.get(name) {
                    return Some(match l {
                        Local::Var { ty, .. } => Obj::Var {
                            sym: String::new(),
                            ty: ty.clone(),
                        },
                        Local::Const { ty, val } => Obj::Const {
                            ty: ty.clone(),
                            val: val.clone(),
                        },
                        Local::Type(t) => Obj::Type(t.clone()),
                    });
                }
            }
        }
        if name == "iota" {
            if let Some(i) = self.iota {
                return Some(Obj::Const {
                    ty: Ty::UntypedInt,
                    val: Const::Int(i),
                });
            }
        }
        if let Some(o) = self.pkgs.get(&self.pkg).and_then(|p| p.get(name)) {
            return Some(o.clone());
        }
        if let Some(p) = self.imports.get(&self.file).and_then(|m| m.get(name)) {
            return Some(Obj::Package(p.clone()));
        }
        universe(name)
    }

    fn local(&self, name: &str) -> Option<Local> {
        let f = self.f.as_ref()?;
        for s in f.scopes.iter().rev() {
            if let Some(l) = s.get(name) {
                return Some(l.clone());
            }
        }
        None
    }

    fn declare_local(&mut self, name: &str, l: Local) {
        if name == "_" {
            return;
        }
        // A scalar of several words keeps them in register order in its
        // slot, memory (and so a pointer to it) low word first: when its
        // address is taken it moves to a cell of its own in memory order
        // and the slot holds the cell's address (as for struct parameters).
        let l = match l {
            Local::Var {
                off,
                ty,
                by_ref: false,
            } if self.types.words(&ty) >= 2 && self.fnc().addressed.contains(name) => {
                let size = self.types.size(&ty);
                let cell = self.slot(size.next_multiple_of(2));
                self.unspill(&ty, off, 1);
                self.x_frame(cell);
                self.store_x(&ty);
                self.frame_addr(cell);
                self.st(1, off);
                self.st(2, off + 2);
                Local::Var {
                    off,
                    ty,
                    by_ref: true,
                }
            }
            l => l,
        };
        let f = self.fnc();
        f.scopes.last_mut().unwrap().insert(name.to_string(), l);
    }

    /// A new local variable of `ty`, zeroed.
    fn new_var(&mut self, name: &str, ty: &Ty) -> u32 {
        let size = self.types.size(ty).max(1);
        let off = self.slot(size.next_multiple_of(2));
        self.zero_slot(off, ty);
        self.declare_local(
            name,
            Local::Var {
                off,
                ty: ty.clone(),
                by_ref: false,
            },
        );
        off
    }

    fn pkg_obj(&self, pkg: &str, name: &str, pos: Pos) -> CResult<Obj> {
        let p = self
            .pkgs
            .get(pkg)
            .ok_or((pos, format!("no package {pkg}")))?;
        if !name.starts_with(|c: char| c.is_ascii_uppercase()) && pkg != self.pkg {
            return err(pos, format!("{pkg}.{name} is not exported"));
        }
        p.get(name)
            .cloned()
            .ok_or((pos, format!("{pkg}.{name} not declared")))
    }

    // ---- types from syntax ----

    pub fn resolve_type(&mut self, t: &TypeExpr) -> CResult<Ty> {
        Ok(match t {
            TypeExpr::Name(pkg, name, pos) => {
                let obj = match pkg {
                    Some(p) => match self.lookup(p) {
                        Some(Obj::Package(path)) => self.pkg_obj(&path, name, *pos)?,
                        _ => return err(*pos, format!("{p} is not a package")),
                    },
                    None => self
                        .lookup(name)
                        .ok_or((*pos, format!("{name} not declared")))?,
                };
                match obj {
                    Obj::Type(t) => t,
                    _ => return err(*pos, format!("{name} is not a type")),
                }
            }
            TypeExpr::Pointer(e) => Ty::Ptr(Box::new(self.resolve_type(e)?)),
            TypeExpr::Slice(e) => Ty::Slice(Box::new(self.resolve_type(e)?)),
            TypeExpr::Array(n, e) => {
                let (_, c) = self.const_expr(n)?;
                let Const::Int(n) = c else {
                    return err(n.pos, "an array length is an integer constant");
                };
                if !(0..=65535).contains(&n) {
                    return err(t_pos(t), "array length out of range");
                }
                Ty::Array(n as u32, Box::new(self.resolve_type(e)?))
            }
            TypeExpr::Struct(fields) => {
                let mut v = Vec::new();
                for f in fields {
                    let ty = self.resolve_type(&f.ty)?;
                    let n = f.name.clone().unwrap_or_default();
                    if v.iter().any(|(m, _): &(String, Ty)| *m == n) {
                        return err(f.pos, format!("field {n} twice"));
                    }
                    v.push((n, ty));
                }
                Ty::Struct(v)
            }
            TypeExpr::Func(sig) => Ty::Func(Box::new(self.resolve_sig(sig)?)),
            TypeExpr::Interface(ms, embedded) => self.resolve_iface(ms, embedded)?,
            TypeExpr::Chan(e) => Ty::Chan(Box::new(self.resolve_type(e)?)),
        })
    }

    fn resolve_sig(&mut self, s: &FuncType) -> CResult<Sig> {
        let mut params = Vec::new();
        for p in &s.params {
            params.push(self.resolve_type(&p.ty)?);
        }
        let mut results = Vec::new();
        for r in &s.results {
            let t = self.resolve_type(&r.ty)?;
            // (a named type still being declared, as in `Next() Node`, passes)
            if self.types.words(&t) == 0 && self.types.under(&t) != &Ty::Void {
                return err(
                    r.pos,
                    "a result is a scalar; return a pointer to a struct or array",
                );
            }
            results.push(t);
        }
        let words: u32 = results.iter().map(|t| self.types.words(t)).sum();
        if words > 4 {
            return err(
                s.results.first().map(|r| r.pos).unwrap_or_default(),
                "results take at most 4 words (R1-R4)",
            );
        }
        Ok(Sig { params, results })
    }

    // ---- constants ----

    /// A constant expression: its type (maybe untyped) and value.
    pub fn const_expr(&mut self, e: &Expr) -> CResult<(Ty, Const)> {
        self.try_const(e)?
            .ok_or((e.pos, "a constant expression".to_string()))
    }

    /// `Some` when `e` is constant.
    fn try_const(&mut self, e: &Expr) -> CResult<Option<(Ty, Const)>> {
        Ok(Some(match &e.kind {
            ExprKind::Int(v) => (Ty::UntypedInt, Const::Int(*v)),
            ExprKind::Char(c) => (Ty::UntypedInt, Const::Int(i64::from(*c))),
            ExprKind::Str(s) => (Ty::UntypedStr, Const::Str(s.clone())),
            ExprKind::Paren(x) => return self.try_const(x),
            ExprKind::Ident(n) => match self.lookup(n) {
                Some(Obj::Const { ty, val }) => (ty, val),
                Some(Obj::Nil) => (Ty::UntypedNil, Const::Nil),
                _ => return Ok(None),
            },
            ExprKind::Selector(x, n) => match &x.kind {
                ExprKind::Ident(p) if matches!(self.lookup(p), Some(Obj::Package(_))) => {
                    let Some(Obj::Package(path)) = self.lookup(p) else {
                        unreachable!()
                    };
                    match self.pkg_obj(&path, n, e.pos)? {
                        Obj::Const { ty, val } => (ty, val),
                        _ => return Ok(None),
                    }
                }
                _ => return Ok(None),
            },
            ExprKind::Unary(op, x) => {
                let Some((t, v)) = self.try_const(x)? else {
                    return Ok(None);
                };
                let v = match (*op, v) {
                    ("-", Const::Int(i)) => Const::Int(-i),
                    ("+", Const::Int(i)) => Const::Int(i),
                    ("^", Const::Int(i)) => Const::Int(match self.types.under(&t) {
                        Ty::U8 => !i & 0xff,
                        Ty::U16 => !i & 0xffff,
                        Ty::U32 | Ty::Uintptr => !i & 0xffff_ffff,
                        _ => !i,
                    }),
                    ("!", Const::Bool(b)) => Const::Bool(!b),
                    _ => return err(e.pos, format!("bad constant operand for {op}")),
                };
                self.check_range(&t, &v, e.pos)?;
                (t, v)
            }
            ExprKind::Binary(op, a, b) => {
                let (Some((ta, va)), Some((tb, vb))) = (self.try_const(a)?, self.try_const(b)?)
                else {
                    return Ok(None);
                };
                return self.fold(op, ta, va, tb, vb, e.pos).map(Some);
            }
            ExprKind::Call(f, args, false) if args.len() == 1 => {
                // A conversion of a constant, len of a constant string.
                if let ExprKind::Ident(n) = &f.kind {
                    if n == "len" {
                        if let Some((_, Const::Str(s))) = self.try_const(&args[0])? {
                            return Ok(Some((Ty::UntypedInt, Const::Int(s.len() as i64))));
                        }
                        return Ok(None);
                    }
                }
                let Some(t) = self.type_of_callee(f)? else {
                    return Ok(None);
                };
                // Only conversions to basic types stay constant.
                if !matches!(
                    self.types.under(&t),
                    Ty::Bool
                        | Ty::I8
                        | Ty::U8
                        | Ty::I16
                        | Ty::U16
                        | Ty::I32
                        | Ty::U32
                        | Ty::Uintptr
                        | Ty::Fixed
                        | Ty::Str
                ) {
                    return Ok(None);
                }
                let Some((ta, va)) = self.try_const(&args[0])? else {
                    return Ok(None);
                };
                let v = self.convert_const(&ta, va, &t, e.pos)?;
                (t, v)
            }
            _ => return Ok(None),
        }))
    }

    /// When `f` names a type (a conversion), that type.
    fn type_of_callee(&mut self, f: &Expr) -> CResult<Option<Ty>> {
        Ok(match &f.kind {
            ExprKind::Ident(n) => match self.lookup(n) {
                Some(Obj::Type(t)) => Some(t),
                _ => None,
            },
            ExprKind::Selector(x, n) => match &x.kind {
                ExprKind::Ident(p) => match self.lookup(p) {
                    Some(Obj::Package(path)) => match self.pkg_obj(&path, n, f.pos)? {
                        Obj::Type(t) => Some(t),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            },
            ExprKind::Type(t) => Some(self.resolve_type(t)?),
            ExprKind::Paren(x) => match &x.kind {
                ExprKind::Unary("*", inner) => {
                    self.type_of_callee(inner)?.map(|t| Ty::Ptr(Box::new(t)))
                }
                _ => self.type_of_callee(x)?,
            },
            _ => None,
        })
    }

    fn convert_const(&mut self, from: &Ty, v: Const, to: &Ty, pos: Pos) -> CResult<Const> {
        let ut = self.types.under(to).clone();
        let fixed_from = matches!(self.types.under(from), Ty::Fixed);
        Ok(match (v, &ut) {
            (Const::Int(i), Ty::Fixed) if !fixed_from => {
                let raw = i << 16;
                self.check_range(to, &Const::Int(raw), pos)?;
                Const::Int(raw)
            }
            (Const::Int(i), _) if fixed_from && self.types.is_int(to) => {
                let v = Const::Int(i >> 16);
                self.check_range(to, &v, pos)?;
                v
            }
            (Const::Int(i), _) if self.types.is_numeric(to) => {
                // Conversions of typed constants wrap like the CPU does.
                let v = if matches!(from, Ty::UntypedInt) {
                    i
                } else {
                    wrap(&ut, i)
                };
                let v = Const::Int(v);
                self.check_range(to, &v, pos)?;
                v
            }
            (Const::Bool(b), Ty::Bool) => Const::Bool(b),
            (Const::Str(s), Ty::Str) => Const::Str(s),
            (Const::Nil, _) => Const::Nil,
            (v, _) => {
                return err(
                    pos,
                    format!("cannot convert constant {v} to {}", self.types.name(to)),
                )
            }
        })
    }

    fn check_range(&self, t: &Ty, v: &Const, pos: Pos) -> CResult<()> {
        if let (Const::Int(i), Some((lo, hi))) = (v, int_range(self.types.under(t))) {
            let (lo, hi) = if matches!(self.types.under(t), Ty::Fixed) {
                (lo << 16, (hi << 16) | 0xffff)
            } else {
                (lo, hi)
            };
            if *i < lo || *i > hi {
                return err(
                    pos,
                    format!("constant {i} does not fit {}", self.types.name(t)),
                );
            }
        }
        Ok(())
    }

    fn fold(
        &mut self,
        op: &str,
        ta: Ty,
        va: Const,
        tb: Ty,
        vb: Const,
        pos: Pos,
    ) -> CResult<(Ty, Const)> {
        // The result type: the typed side, else untyped.
        let t = if op == "<<" || op == ">>" {
            ta.clone()
        } else if !is_untyped(&ta) {
            if !is_untyped(&tb) && ta != tb {
                return err(
                    pos,
                    format!(
                        "mismatched types {} and {}",
                        self.types.name(&ta),
                        self.types.name(&tb)
                    ),
                );
            }
            ta.clone()
        } else {
            tb.clone()
        };
        let fixed = matches!(self.types.under(&t), Ty::Fixed);
        let to_raw = |c: &Const, ty: &Ty| match (c, fixed && is_untyped(ty)) {
            (Const::Int(i), true) => Const::Int(i << 16),
            (c, _) => c.clone(),
        };
        let (va, vb) = if op == "<<" || op == ">>" {
            (va, vb)
        } else {
            (to_raw(&va, &ta), to_raw(&vb, &tb))
        };
        let cmp = |o: std::cmp::Ordering| -> Const {
            use std::cmp::Ordering::*;
            Const::Bool(match op {
                "==" => o == Equal,
                "!=" => o != Equal,
                "<" => o == Less,
                "<=" => o != Greater,
                ">" => o == Greater,
                _ => o != Less,
            })
        };
        let is_cmp = matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=");
        let v = match (va, vb) {
            (Const::Int(a), Const::Int(b)) if is_cmp => cmp(a.cmp(&b)),
            (Const::Int(a), Const::Int(b)) => Const::Int(match op {
                "+" => a + b,
                "-" => a - b,
                "*" if fixed => (a * b) >> 16,
                "*" => a * b,
                "/" | "%" if b == 0 => return err(pos, "division by zero"),
                "/" if fixed => (a << 16) / b,
                "/" => a / b,
                "%" => a % b,
                "&" => a & b,
                "|" => a | b,
                "^" => a ^ b,
                "&^" => a & !b,
                "<<" => a.checked_shl(b as u32).unwrap_or(0),
                ">>" => a >> b.min(63),
                _ => return err(pos, format!("operator {op} on integers")),
            }),
            (Const::Bool(a), Const::Bool(b)) => Const::Bool(match op {
                "&&" => a && b,
                "||" => a || b,
                "==" => a == b,
                "!=" => a != b,
                _ => return err(pos, format!("operator {op} on booleans")),
            }),
            (Const::Str(a), Const::Str(b)) if is_cmp => cmp(a.cmp(&b)),
            (Const::Str(mut a), Const::Str(b)) if op == "+" => {
                a.extend(b);
                Const::Str(a)
            }
            (a, b) => return err(pos, format!("invalid constant operation {a} {op} {b}")),
        };
        let t = if is_cmp {
            if is_untyped(&ta) && is_untyped(&tb) {
                Ty::UntypedBool
            } else {
                Ty::Bool
            }
        } else {
            t
        };
        let v = match (&v, self.types.under(&t)) {
            (Const::Int(i), ut) if !is_untyped(&t) && !is_cmp => {
                Const::Int(if fixed { *i } else { wrap(ut, *i) })
            }
            _ => v,
        };
        if !is_cmp {
            self.check_range(&t, &v, pos)?;
        }
        Ok((t, v))
    }
}

fn wrap(t: &Ty, i: i64) -> i64 {
    match t {
        Ty::I8 => i64::from(i as i8),
        Ty::U8 => i64::from(i as u8),
        Ty::I16 => i64::from(i as i16),
        Ty::U16 => i64::from(i as u16),
        Ty::I32 => i64::from(i as i32),
        Ty::U32 | Ty::Uintptr => i64::from(i as u32),
        _ => i,
    }
}

pub fn is_untyped(t: &Ty) -> bool {
    matches!(
        t,
        Ty::UntypedInt | Ty::UntypedBool | Ty::UntypedStr | Ty::UntypedNil
    )
}

fn t_pos(t: &TypeExpr) -> Pos {
    match t {
        TypeExpr::Name(_, _, p) => *p,
        TypeExpr::Pointer(e) | TypeExpr::Slice(e) | TypeExpr::Chan(e) => t_pos(e),
        TypeExpr::Array(n, _) => n.pos,
        TypeExpr::Struct(f) => f.first().map(|f| f.pos).unwrap_or_default(),
        TypeExpr::Func(s) => s.params.first().map(|f| f.pos).unwrap_or_default(),
        TypeExpr::Interface(m, e) => m
            .first()
            .map(|m| m.2)
            .or_else(|| e.first().map(t_pos))
            .unwrap_or_default(),
    }
}

/// Predeclared names (BRASS.md 4, 8).
fn universe(name: &str) -> Option<Obj> {
    Some(match name {
        "bool" => Obj::Type(Ty::Bool),
        "int8" => Obj::Type(Ty::I8),
        "uint8" | "byte" => Obj::Type(Ty::U8),
        "int16" | "int" => Obj::Type(Ty::I16),
        "uint16" | "uint" => Obj::Type(Ty::U16),
        "int32" => Obj::Type(Ty::I32),
        "uint32" => Obj::Type(Ty::U32),
        "uintptr" => Obj::Type(Ty::Uintptr),
        "fixed" => Obj::Type(Ty::Fixed),
        "string" => Obj::Type(Ty::Str),
        "any" => Obj::Type(Ty::Iface(Vec::new())),
        "true" => Obj::Const {
            ty: Ty::UntypedBool,
            val: Const::Bool(true),
        },
        "false" => Obj::Const {
            ty: Ty::UntypedBool,
            val: Const::Bool(false),
        },
        "nil" => Obj::Nil,
        "len" | "cap" | "new" | "make" | "append" | "copy" | "free" | "panic" | "print"
        | "println" | "min" | "max" | "close" => Obj::Builtin(match name {
            "len" => "len",
            "cap" => "cap",
            "new" => "new",
            "make" => "make",
            "append" => "append",
            "copy" => "copy",
            "free" => "free",
            "panic" => "panic",
            "print" => "print",
            "println" => "println",
            "min" => "min",
            "close" => "close",
            _ => "max",
        }),
        _ => return None,
    })
}

#[allow(dead_code)]
fn named_stub(pkg: &str, name: &str) -> Named {
    Named {
        pkg: pkg.into(),
        name: name.into(),
        under: Ty::Void,
        methods: Vec::new(),
    }
}

#[allow(dead_code)]
const _PRINT: &str = RT_PRINT_STR;
