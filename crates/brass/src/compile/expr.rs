//! Expressions: typed and translated in one go. A scalar result ends up
//! in R1.. (see `types.rs`); a struct or array result is an address in
//! R1:R2 (`words == 0`).

use super::*;

/// What a callee is.
enum Callee {
    Builtin(&'static str),
    Convert(Ty),
    /// A function by symbol, the receiver (expression, by pointer) of a
    /// method call.
    Static {
        sym: String,
        sig: Sig,
        recv: Option<(Expr, bool)>,
    },
    /// A function value.
    Value(Sig),
    /// A method of an interface value.
    Iface {
        x: Expr,
        name: String,
        sig: Sig,
    },
}

impl Compiler {
    /// Translate `e`; untyped constants become `want` (or their default).
    pub(super) fn expr(&mut self, e: &Expr, want: Option<&Ty>) -> CResult<Ty> {
        if let Some((t, v)) = self.try_const(e)? {
            let t = match (is_untyped(&t), want) {
                // a constant becomes an interface from its default type
                (true, Some(w)) if self.types.is_iface(w) && t != Ty::UntypedNil => {
                    self.types.default_of(&t)
                }
                (true, Some(w)) if !matches!(w, Ty::Void) => {
                    if !self.types.assignable(&t, w) {
                        return err(
                            e.pos,
                            format!(
                                "cannot use {} as {}",
                                self.types.name(&t),
                                self.types.name(w)
                            ),
                        );
                    }
                    w.clone()
                }
                _ => self.types.default_of(&t),
            };
            let is_fixed = matches!(self.types.under(&t), Ty::Fixed);
            let v = match &v {
                Const::Int(i) if is_fixed && !self.const_is_fixed(e) => Const::Int(i << 16),
                _ => v,
            };
            self.check_range(&t, &v, e.pos)?;
            self.load_const(&t, &v);
            return Ok(t);
        }
        match &e.kind {
            ExprKind::Paren(x) => self.expr(x, want),
            ExprKind::Ident(n) => self.ident(n, e.pos),
            ExprKind::Selector(x, n) => self.selector(e, x, n),
            ExprKind::Index(x, i) => self.index(e, x, i),
            ExprKind::Slice(x, lo, hi) => self.slice_expr(e, x, lo.as_deref(), hi.as_deref()),
            ExprKind::Call(f, args, spread) => {
                let t = self.call(e, f, args, *spread)?;
                if let Ty::Tuple(_) = t {
                    return err(
                        e.pos,
                        "a call with several results in a single-value context",
                    );
                }
                Ok(t)
            }
            ExprKind::Unary(op, x) => self.unary(e, op, x, want),
            ExprKind::Binary(op, a, b) => self.binary(e, op, a, b, want),
            ExprKind::Composite(t, elems) => {
                let ty = match (t, want) {
                    (Some(t), _) => self.resolve_type(t)?,
                    (None, Some(w)) => w.clone(),
                    (None, None) => return err(e.pos, "a composite literal needs its type"),
                };
                self.composite(e.pos, &ty, elems)
            }
            ExprKind::FuncLit(sig, body) => self.func_lit(e.pos, sig, body),
            ExprKind::Assert(x, te) => self.assert_expr(e.pos, x, te),
            ExprKind::Type(_) => err(e.pos, "a type is not a value"),
            ExprKind::Int(_) | ExprKind::Char(_) | ExprKind::Str(_) => unreachable!("constants"),
        }
    }

    fn const_is_fixed(&mut self, e: &Expr) -> bool {
        matches!(self.try_const(e), Ok(Some((t, _))) if matches!(self.types.under(&t), Ty::Fixed))
    }

    /// A constant into registers.
    pub(super) fn load_const(&mut self, t: &Ty, v: &Const) {
        match v {
            Const::Int(i) => {
                if self.types.is_wide(t) {
                    self.li(1, i >> 16);
                    self.li(2, *i);
                } else {
                    self.li(1, *i);
                }
            }
            Const::Bool(b) => self.li(1, i64::from(*b)),
            Const::Str(s) => {
                let l = self.string_label(s);
                self.emit(format!("li      r1, #seg({l})"));
                self.emit(format!("li      r2, #off({l})"));
                self.li(3, s.len() as i64);
            }
            Const::Nil => {
                for r in 1..=self.types.words(t).max(2) {
                    self.li(r as u8, 0);
                }
            }
        }
    }

    fn ident(&mut self, n: &str, pos: Pos) -> CResult<Ty> {
        if let Some(Local::Var { off, ty, by_ref }) = self.local(n) {
            if by_ref {
                self.ld(1, off);
                self.ld(2, off + 2);
                if self.types.words(&ty) > 0 {
                    self.addr_load(&ty);
                }
            } else if self.types.words(&ty) == 0 {
                self.frame_addr(off);
            } else if self.types.is_byte_sized(&ty) {
                self.x_frame(off);
                self.load_x(&ty);
            } else {
                self.unspill(&ty, off, 1);
            }
            return Ok(ty);
        }
        match self.lookup(n) {
            Some(Obj::Var { sym, ty }) => {
                self.global_addr(&sym);
                if self.types.words(&ty) > 0 {
                    self.addr_load(&ty);
                }
                Ok(ty)
            }
            Some(Obj::Func { sym, sig, .. }) => {
                self.emit(format!("li      r1, #seg({sym})"));
                self.emit(format!("li      r2, #off({sym})"));
                Ok(Ty::Func(Box::new(sig)))
            }
            Some(Obj::Type(_)) => err(pos, format!("{n} is a type, not a value")),
            Some(Obj::Package(_)) => err(pos, format!("{n} is a package, not a value")),
            Some(Obj::Builtin(_)) => err(pos, format!("{n} must be called")),
            Some(Obj::Nil) | Some(Obj::Const { .. }) => unreachable!("constants"),
            None => err(pos, format!("{n} not declared")),
        }
    }

    /// R1:R2 = a package variable's address.
    fn global_addr(&mut self, sym: &str) {
        self.emit(format!("li      r1, #seg({sym})"));
        self.emit(format!("li      r2, #off({sym})"));
    }

    /// Load the scalar at the address in R1:R2.
    pub(super) fn addr_load(&mut self, ty: &Ty) {
        self.emit("mts     s3, r1");
        self.emit("mov     r9, r2");
        self.load_x(ty);
    }

    /// Add the constant `k` to the address in R1:R2 (with the carry into
    /// the segment).
    fn addr_add(&mut self, k: u32) {
        if k == 0 {
            return;
        }
        self.li(10, i64::from(k & 0xffff));
        self.emit("add     r2, r10");
        self.li(10, i64::from(k >> 16));
        self.emit("adc     r1, r10");
    }

    /// The address of an lvalue (or of a struct/array value) in R1:R2.
    pub(super) fn addr(&mut self, e: &Expr) -> CResult<Ty> {
        match &e.kind {
            ExprKind::Paren(x) => self.addr(x),
            ExprKind::Ident(n) => {
                if let Some(Local::Var { off, ty, by_ref }) = self.local(n) {
                    if by_ref {
                        self.ld(1, off);
                        self.ld(2, off + 2);
                    } else {
                        self.frame_addr(off);
                    }
                    return Ok(ty);
                }
                match self.lookup(n) {
                    Some(Obj::Var { sym, ty }) => {
                        self.global_addr(&sym);
                        Ok(ty)
                    }
                    _ => err(e.pos, format!("cannot take the address of {n}")),
                }
            }
            ExprKind::Selector(x, n) => {
                if let ExprKind::Ident(p) = &x.kind {
                    if let Some(Obj::Package(path)) = self.lookup(p) {
                        return match self.pkg_obj(&path, n, e.pos)? {
                            Obj::Var { sym, ty } => {
                                self.global_addr(&sym);
                                Ok(ty)
                            }
                            _ => err(e.pos, format!("cannot take the address of {p}.{n}")),
                        };
                    }
                }
                let base = self.struct_base(x)?;
                let (off, fty) = self.types.field(&base, n).ok_or((
                    e.pos,
                    format!("{} has no field {n}", self.types.name(&base)),
                ))?;
                self.addr_add(off);
                Ok(fty)
            }
            ExprKind::Index(x, i) => self.elem_addr(e, x, i),
            ExprKind::Unary("*", x) => {
                let t = self.expr(x, None)?;
                match self.types.under(&t).clone() {
                    Ty::Ptr(inner) => {
                        self.nil_check();
                        Ok(*inner)
                    }
                    _ => err(e.pos, format!("cannot dereference {}", self.types.name(&t))),
                }
            }
            ExprKind::Composite(..) | ExprKind::Call(..) | ExprKind::Assert(..) => {
                let t = self.expr(e, None)?;
                if self.types.words(&t) > 0 {
                    // A scalar: give it a home.
                    let off = self.spill(&t);
                    self.frame_addr(off);
                }
                Ok(t)
            }
            _ => err(e.pos, "cannot take the address of this expression"),
        }
    }

    /// The struct an `x.field` reads from: its address in R1:R2 (through a
    /// pointer automatically); its type.
    fn struct_base(&mut self, x: &Expr) -> CResult<Ty> {
        let t = self.type_peek(x)?;
        if let Ty::Ptr(inner) = self.types.under(&t).clone() {
            self.expr(x, None)?;
            self.nil_check();
            return Ok(*inner);
        }
        self.addr(x)
    }

    /// The type of `x` without its code (translated into a scratch buffer
    /// that is thrown away).
    pub(super) fn type_peek(&mut self, x: &Expr) -> CResult<Ty> {
        let f = self.f.as_mut().expect("in a function");
        let saved_body = std::mem::take(&mut f.body);
        let (size, max, labels) = (f.size, f.max, self.labels);
        let saved_data = self.data.len();
        let saved_code = self.code.len();
        let strings = self.strings.clone();
        let r = self.expr_or_addr(x);
        self.code.truncate(saved_code);
        let f = self.f.as_mut().unwrap();
        f.body = saved_body;
        f.size = size;
        f.max = max.max(f.max);
        self.labels = labels;
        self.data.truncate(saved_data);
        self.strings = strings;
        r
    }

    /// `expr`, but also accepts what only has an address.
    fn expr_or_addr(&mut self, x: &Expr) -> CResult<Ty> {
        self.expr(x, None)
    }

    /// Panic when the pointer in R1:R2 is nil.
    fn nil_check(&mut self) {
        let ok = self.new_label();
        self.emit("mov     r9, r1");
        self.emit("or      r9, r2");
        self.emit(format!("bne     {ok}"));
        self.rt("rt_panic_nil");
        self.label(&ok);
    }

    fn selector(&mut self, e: &Expr, x: &Expr, n: &str) -> CResult<Ty> {
        if let ExprKind::Ident(p) = &x.kind {
            if let Some(Obj::Package(path)) = self.lookup(p) {
                return match self.pkg_obj(&path, n, e.pos)? {
                    Obj::Var { sym, ty } => {
                        self.global_addr(&sym);
                        if self.types.words(&ty) > 0 {
                            self.addr_load(&ty);
                        }
                        Ok(ty)
                    }
                    Obj::Func { sym, sig, .. } => {
                        self.emit(format!("li      r1, #seg({sym})"));
                        self.emit(format!("li      r2, #off({sym})"));
                        Ok(Ty::Func(Box::new(sig)))
                    }
                    _ => err(e.pos, format!("{p}.{n} is not a value")),
                };
            }
        }
        let t = self.addr(e)?;
        if self.types.words(&t) > 0 {
            self.addr_load(&t);
        }
        Ok(t)
    }

    /// The address of `x[i]` in R1:R2 (bounds checked); the element type.
    fn elem_addr(&mut self, e: &Expr, x: &Expr, i: &Expr) -> CResult<Ty> {
        let xt = self.type_peek(x)?;
        let m = self.mark();
        let (elem, base_off, len_off) = match self.types.under(&xt).clone() {
            Ty::Array(n, el) => {
                self.addr(x)?;
                let b = self.spill_addr();
                let l = self.slot(2);
                self.li(1, i64::from(n));
                self.st(1, l);
                (*el, b, l)
            }
            Ty::Ptr(inner) if matches!(self.types.under(&inner), Ty::Array(..)) => {
                let Ty::Array(n, el) = self.types.under(&inner).clone() else {
                    unreachable!()
                };
                self.expr(x, None)?;
                self.nil_check();
                let b = self.spill_addr();
                let l = self.slot(2);
                self.li(1, i64::from(n));
                self.st(1, l);
                (*el, b, l)
            }
            Ty::Slice(el) => {
                self.expr(x, None)?;
                let b = self.spill_addr();
                let l = self.slot(2);
                self.st(3, l);
                (*el, b, l)
            }
            Ty::Str => return err(e.pos, "a string's bytes cannot be changed or addressed"),
            _ => return err(e.pos, format!("cannot index {}", self.types.name(&xt))),
        };
        let it = self.expr(i, Some(&Ty::I16))?;
        if !self.types.is_int(&it) || self.types.words(&it) != 1 {
            return err(i.pos, "an index is a 16-bit integer");
        }
        self.bounds(len_off, false);
        // R1:R2 = base + i * size
        let size = self.types.size(&elem);
        self.li(3, i64::from(size));
        self.emit("mov     r4, r1");
        self.emit("mul     r4, r3"); // low word of i * size
        self.emit("mulhu   r1, r3"); // high word
        self.emit("mov     r3, r1");
        self.ld(1, base_off);
        self.ld(2, base_off + 2);
        self.emit("add     r2, r4");
        self.emit("adc     r1, r3");
        self.release(m);
        Ok(elem)
    }

    /// Panic unless 0 <= R1 < the length at `len_off` (`incl`: <=).
    fn bounds(&mut self, len_off: u32, incl: bool) {
        let ok = self.new_label();
        let bad = self.new_label();
        self.ld(9, len_off);
        self.emit("cmp     r1, r9");
        self.emit(format!("{}     {ok}", if incl { "bls" } else { "bcc" }));
        self.label(&bad);
        self.rt("rt_panic_index");
        self.label(&ok);
    }

    fn index(&mut self, e: &Expr, x: &Expr, i: &Expr) -> CResult<Ty> {
        let xt = self.type_peek(x)?;
        if matches!(self.types.under(&xt), Ty::Str) {
            let m = self.mark();
            self.expr(x, None)?;
            let s = self.spill(&Ty::Str);
            let it = self.expr(i, Some(&Ty::I16))?;
            if !self.types.is_int(&it) || self.types.words(&it) != 1 {
                return err(i.pos, "an index is a 16-bit integer");
            }
            self.bounds(s + 4, false);
            self.emit("mov     r3, r1");
            self.ld(1, s);
            self.ld(2, s + 2);
            self.emit("add     r2, r3");
            self.emit("movi    r3, #0");
            self.emit("adc     r1, r3");
            self.addr_load(&Ty::U8);
            self.release(m);
            return Ok(Ty::U8);
        }
        let t = self.elem_addr(e, x, i)?;
        if self.types.words(&t) > 0 {
            self.addr_load(&t);
        }
        Ok(t)
    }

    /// `x[lo:hi]`: a slice (or string) in R1..
    fn slice_expr(
        &mut self,
        e: &Expr,
        x: &Expr,
        lo: Option<&Expr>,
        hi: Option<&Expr>,
    ) -> CResult<Ty> {
        let xt = self.type_peek(x)?;
        let m = self.mark();
        // ptr, len, cap of the base into a temporary.
        let (res, elem) = match self.types.under(&xt).clone() {
            Ty::Str => {
                self.expr(x, None)?;
                self.emit("mov     r4, r3");
                (Ty::Str, Ty::U8)
            }
            Ty::Slice(el) => {
                self.expr(x, None)?;
                (xt.clone(), *el)
            }
            Ty::Array(n, el) => {
                self.addr(x)?;
                self.li(3, i64::from(n));
                self.li(4, i64::from(n));
                (Ty::Slice(el.clone()), *el)
            }
            Ty::Ptr(inner) if matches!(self.types.under(&inner), Ty::Array(..)) => {
                let Ty::Array(n, el) = self.types.under(&inner).clone() else {
                    unreachable!()
                };
                self.expr(x, None)?;
                self.li(3, i64::from(n));
                self.li(4, i64::from(n));
                (Ty::Slice(el.clone()), *el)
            }
            _ => return err(e.pos, format!("cannot slice {}", self.types.name(&xt))),
        };
        let base = self.spill(&Ty::Slice(Box::new(Ty::U8)));
        // lo (default 0), hi (default len)
        let lo_off = self.slot(2);
        match lo {
            Some(l) => {
                self.expr(l, Some(&Ty::I16))?;
            }
            None => self.li(1, 0),
        }
        self.st(1, lo_off);
        let hi_off = self.slot(2);
        match hi {
            Some(h) => {
                self.expr(h, Some(&Ty::I16))?;
            }
            None => self.ld(1, base + 4),
        }
        self.st(1, hi_off);
        // checks: hi <= cap (len for strings), lo <= hi
        let limit = if matches!(res, Ty::Str) {
            base + 4
        } else {
            base + 6
        };
        self.bounds(limit, true);
        self.ld(1, lo_off);
        self.bounds(hi_off, true);
        // new ptr = ptr + lo * size; len = hi - lo; cap = cap - lo
        let size = self.types.size(&elem);
        self.ld(1, lo_off);
        self.li(3, i64::from(size));
        self.emit("mov     r4, r1");
        self.emit("mul     r4, r3");
        self.emit("mulhu   r1, r3");
        self.emit("mov     r3, r1");
        self.ld(1, base);
        self.ld(2, base + 2);
        self.emit("add     r2, r4");
        self.emit("adc     r1, r3");
        self.ld(3, hi_off);
        self.ld(9, lo_off);
        self.emit("sub     r3, r9");
        self.ld(4, base + 6);
        self.emit("sub     r4, r9");
        self.release(m);
        Ok(res)
    }

    fn unary(&mut self, e: &Expr, op: &str, x: &Expr, want: Option<&Ty>) -> CResult<Ty> {
        match op {
            "&" => {
                // &T{…}: on the heap.
                if let ExprKind::Composite(t, elems) = &x.kind {
                    let ty = match (t, want) {
                        (Some(t), _) => self.resolve_type(t)?,
                        (None, Some(Ty::Ptr(w))) => (**w).clone(),
                        _ => return err(e.pos, "a composite literal needs its type"),
                    };
                    return self.heap_composite(e.pos, &ty, elems);
                }
                let t = self.addr(x)?;
                Ok(Ty::Ptr(Box::new(t)))
            }
            "*" => {
                if self.type_of_callee(x)?.is_some() {
                    return err(e.pos, "a type is not a value");
                }
                let t = self.addr(e)?;
                if self.types.words(&t) > 0 {
                    self.addr_load(&t);
                }
                Ok(t)
            }
            "-" | "+" | "^" => {
                let t = self.expr(x, want)?;
                if !self.types.is_numeric(&t) {
                    return err(e.pos, format!("operator {op} on {}", self.types.name(&t)));
                }
                if op == "+" {
                    return Ok(t);
                }
                if self.types.is_wide(&t) {
                    self.emit("not     r1, r1");
                    self.emit("not     r2, r2");
                    if op == "-" {
                        self.emit("movi    r9, #1");
                        self.emit("add     r2, r9");
                        self.emit("movi    r9, #0");
                        self.emit("adc     r1, r9");
                    }
                } else if op == "-" {
                    self.emit("neg     r1, r1");
                } else {
                    self.emit("not     r1, r1");
                }
                self.narrow(&t);
                Ok(t)
            }
            "!" => {
                let t = self.expr(x, want)?;
                if !matches!(self.types.under(&t), Ty::Bool) {
                    return err(e.pos, "! needs a bool");
                }
                self.emit("movi    r9, #1");
                self.emit("xor     r1, r9");
                Ok(t)
            }
            "<-" => self.recv(x, false),
            _ => err(e.pos, format!("unknown operator {op}")),
        }
    }

    /// Bring an 8-bit result back into its range.
    fn narrow(&mut self, t: &Ty) {
        match self.types.under(t) {
            Ty::U8 => {
                self.emit("li      r9, #$ff");
                self.emit("and     r1, r9");
            }
            Ty::I8 => self.emit("sxb     r1, r1"),
            _ => {}
        }
    }

    fn binary(
        &mut self,
        e: &Expr,
        op: &'static str,
        a: &Expr,
        b: &Expr,
        want: Option<&Ty>,
    ) -> CResult<Ty> {
        if op == "&&" || op == "||" {
            let end = self.new_label();
            let ta = self.expr(a, Some(&Ty::Bool))?;
            if !matches!(self.types.under(&ta), Ty::Bool) {
                return err(a.pos, format!("{op} needs bools"));
            }
            if op == "&&" {
                self.jump_if_false(&end);
            } else {
                self.jump_if_true(&end);
            }
            let tb = self.expr(b, Some(&Ty::Bool))?;
            if !matches!(self.types.under(&tb), Ty::Bool) {
                return err(b.pos, format!("{op} needs bools"));
            }
            self.label(&end);
            return Ok(Ty::Bool);
        }
        let cmp = matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=");
        let shift = op == "<<" || op == ">>";
        // The operand type: a typed side decides for untyped constants.
        let a_const = self.try_const(a)?;
        let b_const = self.try_const(b)?;
        let ta_peek = match &a_const {
            Some((t, _)) => t.clone(),
            None => self.type_peek(a)?,
        };
        let tb_peek = match &b_const {
            Some((t, _)) => t.clone(),
            None => self.type_peek(b)?,
        };
        let operand = if shift {
            match (is_untyped(&ta_peek), want) {
                (true, Some(w)) if !cmp => w.clone(),
                (true, _) => Ty::I16,
                _ => ta_peek.clone(),
            }
        } else if !is_untyped(&ta_peek) {
            ta_peek.clone()
        } else if !is_untyped(&tb_peek) {
            tb_peek.clone()
        } else if let (Some(w), false) = (want, cmp) {
            w.clone()
        } else {
            self.types.default_of(&ta_peek)
        };
        // A value compared with an interface becomes one.
        let iface_cmp = cmp
            && (self.types.is_iface(&ta_peek) || self.types.is_iface(&tb_peek))
            && !(self.types.is_iface(&ta_peek) && self.types.is_iface(&tb_peek));
        let operand = if iface_cmp && !self.types.is_iface(&operand) {
            tb_peek.clone()
        } else {
            operand
        };
        let both_iface = self.types.is_iface(&ta_peek) && self.types.is_iface(&tb_peek);
        if !shift
            && !iface_cmp
            && !both_iface
            && !is_untyped(&ta_peek)
            && !is_untyped(&tb_peek)
            && ta_peek != tb_peek
        {
            return err(
                e.pos,
                format!(
                    "mismatched types {} and {}",
                    self.types.name(&ta_peek),
                    self.types.name(&tb_peek)
                ),
            );
        }
        let m = self.mark();
        let mut ta = self.expr(a, Some(&operand))?;
        if iface_cmp && !self.types.is_iface(&ta) {
            self.coerce(&ta, &operand, a.pos)?;
            ta = operand.clone();
        }
        let left = self.spill(&ta);
        let count_ty = if shift { Ty::U16 } else { operand.clone() };
        let mut tb = self.expr(b, Some(&count_ty))?;
        if iface_cmp && !self.types.is_iface(&tb) && tb != Ty::UntypedNil {
            self.coerce(&tb, &operand, b.pos)?;
            tb = operand.clone();
        }
        if shift && (!self.types.is_int(&tb)) {
            return err(b.pos, "a shift count is an integer");
        }
        let ut = self.types.under(&operand).clone();
        if cmp {
            let r = self.compare(e.pos, op, &ut, left, &tb)?;
            self.release(m);
            return Ok(r);
        }
        let wide = self.types.is_wide(&operand);
        match ut {
            Ty::Str => {
                return err(
                    e.pos,
                    "strings cannot be added at run time (use package strs)",
                )
            }
            Ty::Bool => return err(e.pos, format!("operator {op} on bool")),
            _ if !self.types.is_numeric(&operand) => {
                return err(
                    e.pos,
                    format!("operator {op} on {}", self.types.name(&operand)),
                )
            }
            _ => {}
        }
        let signed = self.types.is_signed(&operand);
        let fixed = matches!(ut, Ty::Fixed);
        if shift {
            // count in R1 (one word; a wide count uses its low word)
            if self.types.is_wide(&tb) {
                self.emit("mov     r1, r2");
            }
            self.emit("mov     r3, r1");
            self.unspill(&ta, left, 1);
            if wide {
                self.rt(match (op, signed) {
                    ("<<", _) => "rt_shl32",
                    (_, true) => "rt_sar32",
                    _ => "rt_shr32",
                });
            } else {
                // A count of 16 or more: 0 (or the sign).
                let small = self.new_label();
                let done = self.new_label();
                self.emit("cmpi    r3, #16");
                self.emit(format!("bcc     {small}"));
                if op == ">>" && signed {
                    self.emit("movi    r3, #15");
                    self.emit("asr     r1, r3");
                } else {
                    self.emit("movi    r1, #0");
                }
                self.emit(format!("bra     {done}"));
                self.label(&small);
                self.emit(match (op, signed) {
                    ("<<", _) => "shl     r1, r3",
                    (_, true) => "asr     r1, r3",
                    _ => "shr     r1, r3",
                });
                self.label(&done);
                self.narrow(&operand);
            }
            self.release(m);
            return Ok(operand);
        }
        // right operand to R3(:R4), left back to R1(:R2)
        if wide {
            self.emit("mov     r3, r1");
            self.emit("mov     r4, r2");
        } else {
            self.emit("mov     r3, r1");
        }
        self.unspill(&ta, left, 1);
        if wide {
            match op {
                "+" => {
                    self.emit("add     r2, r4");
                    self.emit("adc     r1, r3");
                }
                "-" => {
                    self.emit("sub     r2, r4");
                    self.emit("sbc     r1, r3");
                }
                "&" | "|" | "^" => {
                    let o = match op {
                        "&" => "and",
                        "|" => "or",
                        _ => "xor",
                    };
                    self.emit(format!("{o}     r1, r3"));
                    self.emit(format!("{o}     r2, r4"));
                }
                "&^" => {
                    self.emit("not     r3, r3");
                    self.emit("not     r4, r4");
                    self.emit("and     r1, r3");
                    self.emit("and     r2, r4");
                }
                "*" => self.rt(if fixed { "rt_fixmul" } else { "rt_mul32" }),
                "/" => self.rt(match (fixed, signed) {
                    (true, _) => "rt_fixdiv",
                    (_, true) => "rt_divs32",
                    _ => "rt_divu32",
                }),
                "%" if !fixed => self.rt(if signed { "rt_mods32" } else { "rt_modu32" }),
                _ => {
                    return err(
                        e.pos,
                        format!("operator {op} on {}", self.types.name(&operand)),
                    )
                }
            }
        } else {
            let ins = match (op, signed) {
                ("+", _) => "add",
                ("-", _) => "sub",
                ("*", _) => "mul",
                ("/", true) => "divs",
                ("/", false) => "divu",
                ("%", true) => "rems",
                ("%", false) => "remu",
                ("&", _) => "and",
                ("|", _) => "or",
                ("^", _) => "xor",
                ("&^", _) => {
                    self.emit("not     r3, r3");
                    "and"
                }
                _ => return err(e.pos, format!("operator {op}")),
            };
            self.emit(format!("{ins:<7} r1, r3"));
            self.narrow(&operand);
        }
        self.release(m);
        Ok(operand)
    }

    /// Compare the spilled left operand with the right one in R1..; R1 = bool.
    pub(super) fn compare(
        &mut self,
        pos: Pos,
        op: &str,
        ut: &Ty,
        left: u32,
        tb: &Ty,
    ) -> CResult<Ty> {
        let lt = ut.clone();
        let yes = self.new_label();
        let done = self.new_label();
        let eq_only = matches!(
            lt,
            Ty::Bool | Ty::Ptr(_) | Ty::Func(_) | Ty::Slice(_) | Ty::Iface(_)
        );
        if eq_only && !(op == "==" || op == "!=") {
            return err(pos, format!("operator {op} on {}", self.types.name(&lt)));
        }
        match lt {
            Ty::Iface(_) => {
                if !self.types.is_iface(tb) && *tb != Ty::UntypedNil {
                    return err(
                        pos,
                        format!(
                            "mismatched types {} and {}",
                            self.types.name(&lt),
                            self.types.name(tb)
                        ),
                    );
                }
                self.compare_iface(op, left);
                return Ok(Ty::Bool);
            }
            Ty::Str => {
                // both strings into temporaries; rt_strcmp(&a, &b) -> -1/0/1
                let b = self.spill(&Ty::Str);
                self.frame_addr(b);
                self.emit("mov     r3, r1");
                self.emit("mov     r4, r2");
                self.frame_addr(left);
                self.rt("rt_strcmp");
                self.emit("cmpi    r1, #0");
                let br = match op {
                    "==" => "beq",
                    "!=" => "bne",
                    "<" => "blt",
                    "<=" => "ble",
                    ">" => "bgt",
                    _ => "bge",
                };
                self.emit("movi    r1, #1");
                self.emit(format!("{br}     {yes}"));
                self.emit("movi    r1, #0");
                self.label(&yes);
                let _ = done;
                return Ok(Ty::Bool);
            }
            Ty::Slice(_) => {
                // only against nil: the pointer
                let _ = tb;
                self.emit("mov     r3, r1");
                self.emit("mov     r4, r2");
                self.ld(1, left);
                self.ld(2, left + 2);
            }
            Ty::Struct(_) | Ty::Array(..) => {
                return err(
                    pos,
                    "structs and arrays are compared field by field (not in Brass v1)",
                )
            }
            _ => {
                if self.types.is_wide(&lt) || matches!(lt, Ty::Func(_)) {
                    self.emit("mov     r3, r1");
                    self.emit("mov     r4, r2");
                    self.ld(1, left);
                    self.ld(2, left + 2);
                } else {
                    self.emit("mov     r3, r1");
                    self.ld(1, left);
                }
            }
        }
        let wide = self.types.words(&lt) >= 2;
        let signed = self.types.is_signed(&lt);
        let cond = |op: &str, signed: bool| -> &'static str {
            match (op, signed) {
                ("==", _) => "beq",
                ("!=", _) => "bne",
                ("<", true) => "blt",
                ("<=", true) => "ble",
                (">", true) => "bgt",
                (">=", true) => "bge",
                ("<", false) => "bcc",
                ("<=", false) => "bls",
                (">", false) => "bhi",
                _ => "bcs",
            }
        };
        if !wide {
            self.emit("cmp     r1, r3");
            self.emit("movi    r1, #1");
            self.emit(format!("{}     {yes}", cond(op, signed)));
            self.emit("movi    r1, #0");
            self.label(&yes);
            return Ok(Ty::Bool);
        }
        let no = self.new_label();
        let lo = self.new_label();
        match op {
            "==" | "!=" => {
                self.emit("cmp     r1, r3");
                self.emit(format!("bne     {no}"));
                self.emit("cmp     r2, r4");
                self.emit(format!("bne     {no}"));
                self.emit(format!("movi    r1, #{}", u8::from(op == "==")));
                self.emit(format!("bra     {done}"));
                self.label(&no);
                self.emit(format!("movi    r1, #{}", u8::from(op == "!=")));
            }
            _ => {
                // the high words decide unless equal; then the low words, unsigned
                let strict = match op {
                    "<=" => "<",
                    ">=" => ">",
                    o => o,
                };
                self.emit("cmp     r1, r3");
                self.emit(format!("beq     {lo}"));
                self.emit("movi    r1, #1");
                self.emit(format!("{}     {done}", cond(strict, signed)));
                self.emit("movi    r1, #0");
                self.emit(format!("bra     {done}"));
                self.label(&lo);
                self.emit("cmp     r2, r4");
                self.emit("movi    r1, #1");
                self.emit(format!("{}     {done}", cond(op, false)));
                self.emit("movi    r1, #0");
            }
        }
        self.label(&done);
        let _ = yes;
        Ok(Ty::Bool)
    }

    // ---- composite literals ----

    /// A struct or array literal in a temporary: its address in R1:R2; a
    /// slice literal on the heap: the slice in R1..R4.
    pub(super) fn composite(
        &mut self,
        pos: Pos,
        ty: &Ty,
        elems: &[(Option<Expr>, Expr)],
    ) -> CResult<Ty> {
        match self.types.under(ty).clone() {
            Ty::Slice(el) => {
                let n = elems.len() as u32;
                let size = self.types.size(&el);
                let total = n * size;
                self.li(1, i64::from(total >> 16));
                self.li(2, i64::from(total & 0xffff));
                self.rt("rt_alloc");
                let base = self.spill_addr();
                self.fill_array(&el, base, elems, n)?;
                self.ld(1, base);
                self.ld(2, base + 2);
                self.li(3, i64::from(n));
                self.li(4, i64::from(n));
                Ok(ty.clone())
            }
            Ty::Struct(_) | Ty::Array(..) => {
                let size = self.types.size(ty);
                let off = self.slot(size.next_multiple_of(2));
                self.frame_addr(off);
                self.li(9, i64::from(size));
                self.rt("rt_memzero");
                self.frame_addr(off);
                let base = self.spill_addr();
                self.fill(pos, ty, base, elems)?;
                self.frame_addr(off);
                Ok(ty.clone())
            }
            _ => err(
                pos,
                format!("no composite literal for {}", self.types.name(ty)),
            ),
        }
    }

    /// `&T{…}`: on the heap (zeroed); the pointer in R1:R2.
    fn heap_composite(&mut self, pos: Pos, ty: &Ty, elems: &[(Option<Expr>, Expr)]) -> CResult<Ty> {
        let size = self.types.size(ty);
        self.li(1, i64::from(size >> 16));
        self.li(2, i64::from(size & 0xffff));
        self.rt("rt_alloc");
        let m = self.mark();
        let base = self.spill_addr();
        self.fill(pos, ty, base, elems)?;
        self.ld(1, base);
        self.ld(2, base + 2);
        self.release(m);
        Ok(Ty::Ptr(Box::new(ty.clone())))
    }

    /// Store the elements of a struct or array literal at the address in
    /// the temporary `base`.
    fn fill(
        &mut self,
        pos: Pos,
        ty: &Ty,
        base: u32,
        elems: &[(Option<Expr>, Expr)],
    ) -> CResult<()> {
        match self.types.under(ty).clone() {
            Ty::Struct(fields) => {
                let keyed = elems.first().is_some_and(|(k, _)| k.is_some());
                for (k, (key, val)) in elems.iter().enumerate() {
                    let name = match (keyed, key) {
                        (
                            true,
                            Some(Expr {
                                kind: ExprKind::Ident(n),
                                ..
                            }),
                        ) => n.clone(),
                        (false, None) => fields
                            .get(k)
                            .map(|f| f.0.clone())
                            .ok_or((val.pos, "too many values".to_string()))?,
                        _ => return err(val.pos, "mixed keyed and positional fields"),
                    };
                    let (off, fty) = self
                        .types
                        .field(ty, &name)
                        .ok_or((val.pos, format!("no field {name}")))?;
                    self.store_at(base, off, &fty, val)?;
                }
                Ok(())
            }
            Ty::Array(n, el) => self.fill_array(&el, base, elems, n),
            _ => err(
                pos,
                format!("no composite literal for {}", self.types.name(ty)),
            ),
        }
    }

    fn fill_array(
        &mut self,
        el: &Ty,
        base: u32,
        elems: &[(Option<Expr>, Expr)],
        n: u32,
    ) -> CResult<()> {
        let size = self.types.size(el);
        let mut idx = 0u32;
        for (key, val) in elems {
            if let Some(k) = key {
                let (_, c) = self.const_expr(k)?;
                let Const::Int(i) = c else {
                    return err(k.pos, "an index is an integer constant");
                };
                idx = i as u32;
            }
            if idx >= n {
                return err(val.pos, "index out of range of the literal");
            }
            self.store_at(base, idx * size, el, val)?;
            idx += 1;
        }
        Ok(())
    }

    /// Evaluate `val` (of type `ty`) into the address in temporary `base`
    /// plus `off`.
    pub(super) fn store_at(&mut self, base: u32, off: u32, ty: &Ty, val: &Expr) -> CResult<()> {
        let want = ty.clone();
        let vt = match &val.kind {
            ExprKind::Composite(None, elems) => self.composite(val.pos, &want, elems)?,
            _ => self.expr(val, Some(&want))?,
        };
        self.coerce(&vt, ty, val.pos)?;
        if self.types.words(ty) == 0 {
            // R1:R2 = source address
            self.emit("mov     r3, r1");
            self.emit("mov     r4, r2");
            self.ld(1, base);
            self.ld(2, base + 2);
            self.addr_add(off);
            let size = self.types.size(ty);
            self.memcpy(size);
        } else {
            self.addr_to_x(base);
            if off > 0 {
                self.li(10, i64::from(off));
                self.emit("add     r9, r10");
                // a carry into the segment
                let nc = self.new_label();
                self.emit(format!("bcc     {nc}"));
                self.emit("mfs     r10, s3");
                self.emit("addi    r10, #1");
                self.emit("mts     s3, r10");
                self.label(&nc);
            }
            self.store_x(ty);
        }
        Ok(())
    }

    pub(super) fn check_assign(&self, from: &Ty, to: &Ty, pos: Pos) -> CResult<()> {
        if self.types.assignable(from, to) {
            Ok(())
        } else {
            err(
                pos,
                format!(
                    "cannot use {} as {}",
                    self.types.name(from),
                    self.types.name(to)
                ),
            )
        }
    }

    // ---- calls ----

    fn callee(&mut self, f: &Expr) -> CResult<Callee> {
        if let Some(t) = self.type_of_callee(f)? {
            return Ok(Callee::Convert(t));
        }
        match &f.kind {
            ExprKind::Paren(x) => self.callee(x),
            ExprKind::Ident(n) if self.local(n).is_none() => match self.lookup(n) {
                Some(Obj::Builtin(b)) => Ok(Callee::Builtin(b)),
                Some(Obj::Func { sym, sig, .. }) => Ok(Callee::Static {
                    sym,
                    sig,
                    recv: None,
                }),
                _ => self.callee_value(f),
            },
            ExprKind::Selector(x, n) => {
                if let ExprKind::Ident(p) = &x.kind {
                    if let Some(Obj::Package(path)) = self.lookup(p) {
                        return match self.pkg_obj(&path, n, f.pos)? {
                            Obj::Func { sym, sig, .. } => Ok(Callee::Static {
                                sym,
                                sig,
                                recv: None,
                            }),
                            _ => self.callee_value(f),
                        };
                    }
                }
                // A method?
                let xt = self.type_peek(x)?;
                if let Ty::Iface(ms) = self.types.under(&xt) {
                    let Some((_, sig)) = ms.iter().find(|m| m.0 == *n) else {
                        return err(f.pos, format!("{} has no method {n}", self.types.name(&xt)));
                    };
                    return Ok(Callee::Iface {
                        x: (**x).clone(),
                        name: n.clone(),
                        sig: sig.clone(),
                    });
                }
                let (named, via_ptr) = match &xt {
                    Ty::Named(i) => (Some(*i), false),
                    Ty::Ptr(inner) => match **inner {
                        Ty::Named(i) => (Some(i), true),
                        _ => (None, false),
                    },
                    _ => (None, false),
                };
                if let Some(i) = named {
                    if let Some((_, sym, by_ptr, sig)) = self.types.named[i]
                        .methods
                        .iter()
                        .find(|m| m.0 == *n)
                        .cloned()
                    {
                        let _ = via_ptr;
                        return Ok(Callee::Static {
                            sym,
                            sig,
                            recv: Some(((**x).clone(), by_ptr)),
                        });
                    }
                }
                self.callee_value(f)
            }
            _ => self.callee_value(f),
        }
    }

    fn callee_value(&mut self, f: &Expr) -> CResult<Callee> {
        let t = self.type_peek(f)?;
        match self.types.under(&t) {
            Ty::Func(sig) => Ok(Callee::Value((**sig).clone())),
            _ => err(f.pos, format!("cannot call {}", self.types.name(&t))),
        }
    }

    /// A call; results in R1.. (a Tuple for several).
    pub(super) fn call(&mut self, e: &Expr, f: &Expr, args: &[Expr], spread: bool) -> CResult<Ty> {
        match self.callee(f)? {
            Callee::Builtin(b) => self.builtin(e, b, args, spread),
            Callee::Convert(t) => {
                if args.len() != 1 {
                    return err(e.pos, "a conversion takes one value");
                }
                self.convert(&args[0], &t)
            }
            Callee::Static { sym, sig, recv } => {
                let target = CallTarget::Static(sym);
                self.call_with(e.pos, target, &sig, recv, args, spread)
            }
            Callee::Value(sig) => {
                let m = self.mark();
                self.expr(f, None)?;
                let fv = self.spill_addr();
                let r = self.call_with(e.pos, CallTarget::Value(fv), &sig, None, args, spread);
                self.release(m);
                r
            }
            Callee::Iface { x, name, sig } => {
                if spread {
                    return err(e.pos, "... only with append");
                }
                let m = self.mark();
                let (target, words) = self.iface_call_args(e.pos, &x, &name, &sig, args)?;
                self.call_emit(target, words);
                self.release(m);
                Ok(match sig.results.len() {
                    0 => Ty::Void,
                    1 => sig.results[0].clone(),
                    _ => Ty::Tuple(sig.results.clone()),
                })
            }
        }
    }

    fn call_with(
        &mut self,
        pos: Pos,
        target: CallTarget,
        sig: &Sig,
        recv: Option<(Expr, bool)>,
        args: &[Expr],
        spread: bool,
    ) -> CResult<Ty> {
        if spread {
            return err(
                pos,
                "... only with append (variadic functions are not in Brass v1)",
            );
        }
        let m = self.mark();
        let words = self.arg_words(pos, sig, recv, args)?;
        self.call_emit(target, words);
        self.release(m);
        Ok(match sig.results.len() {
            0 => Ty::Void,
            1 => sig.results[0].clone(),
            _ => Ty::Tuple(sig.results.clone()),
        })
    }

    /// Every argument word into a temporary, in order: their offsets.
    pub(super) fn arg_words(
        &mut self,
        pos: Pos,
        sig: &Sig,
        recv: Option<(Expr, bool)>,
        args: &[Expr],
    ) -> CResult<Vec<u32>> {
        let nparams = sig.params.len() - usize::from(recv.is_some());
        if args.len() != nparams {
            return err(
                pos,
                format!("{} arguments, the function takes {nparams}", args.len()),
            );
        }
        let mut words: Vec<u32> = Vec::new();
        let mut params = sig.params.iter();
        if let Some((rx, by_ptr)) = &recv {
            let pt = params.next().unwrap().clone();
            let xt = self.type_peek(rx)?;
            let x_is_ptr = matches!(self.types.under(&xt), Ty::Ptr(_));
            if *by_ptr {
                if x_is_ptr {
                    self.expr(rx, None)?;
                } else {
                    self.addr(rx)?;
                }
                let o = self.spill_addr();
                words.extend([o, o + 2]);
            } else {
                // by value: a scalar, or a struct by address (a copy)
                if x_is_ptr {
                    self.expr(rx, None)?;
                    self.nil_check();
                    if self.types.words(&pt) > 0 {
                        self.addr_load(&pt);
                    }
                } else {
                    self.expr(rx, None)?;
                }
                self.push_arg(&pt, &mut words)?;
            }
        }
        for (a, pt) in args.iter().zip(params) {
            let at = match &a.kind {
                ExprKind::Composite(None, el) => self.composite(a.pos, pt, el)?,
                _ => self.expr(a, Some(pt))?,
            };
            self.coerce(&at, pt, a.pos)?;
            self.push_arg(pt, &mut words)?;
        }
        Ok(words)
    }

    /// The words of a call's arguments, and its target, without the call
    /// (`go`).
    pub(super) fn call_args(
        &mut self,
        pos: Pos,
        f: &Expr,
        args: &[Expr],
        spread: bool,
    ) -> CResult<(CallTarget, Vec<u32>)> {
        if spread {
            return err(pos, "... only with append");
        }
        match self.callee(f)? {
            Callee::Static { sym, sig, recv } => {
                let w = self.arg_words(pos, &sig, recv, args)?;
                Ok((CallTarget::Static(sym), w))
            }
            Callee::Value(sig) => {
                self.expr(f, None)?;
                let fv = self.spill_addr();
                let w = self.arg_words(pos, &sig, None, args)?;
                Ok((CallTarget::Value(fv), w))
            }
            Callee::Iface { x, name, sig } => self.iface_call_args(pos, &x, &name, &sig, args),
            _ => err(pos, "go needs a function or method"),
        }
    }

    pub(super) fn call_emit(&mut self, target: CallTarget, words: Vec<u32>) {
        // Words 5.. into the outgoing area, 1-4 into R1-R4.
        let extra = words.len().saturating_sub(4) as u32;
        {
            let f = self.fnc();
            f.out_need = f.out_need.max(2 * extra);
        }
        for (k, &w) in words.iter().enumerate().skip(4) {
            self.ld(10, w);
            self.st(10, 2 * (k as u32 - 4));
        }
        for (k, &w) in words.iter().enumerate().take(4) {
            self.ld(k as u8 + 1, w);
        }
        match target {
            CallTarget::Static(sym) => {
                self.emit(format!("li      r9, #seg({sym})"));
                self.emit(format!("li      r10, #off({sym})"));
                self.emit("callf   r9:r10");
            }
            CallTarget::Value(fv) => {
                // segment into R10, offset into R9
                self.x_frame(fv);
                self.emit("lwx     r10, (r9)+");
                self.emit("lwx     r9, (r9)");
                self.emit("callf   r10:r9");
            }
        }
    }

    /// The value in R1.. (or a struct's address) as argument words.
    fn push_arg(&mut self, pt: &Ty, words: &mut Vec<u32>) -> CResult<()> {
        if self.types.words(pt) == 0 {
            // a copy, passed by its address
            let size = self.types.size(pt);
            let copy = self.slot(size.next_multiple_of(2));
            self.emit("mov     r3, r1");
            self.emit("mov     r4, r2");
            self.frame_addr(copy);
            self.memcpy(size);
            self.frame_addr(copy);
            let o = self.spill_addr();
            words.extend([o, o + 2]);
        } else {
            let o = self.spill(pt);
            for k in 0..self.types.words(pt) {
                words.push(o + 2 * k);
            }
        }
        Ok(())
    }

    fn func_lit(&mut self, pos: Pos, sig: &FuncType, body: &Block) -> CResult<Ty> {
        let s = self.resolve_sig(sig)?;
        self.labels += 1;
        let sym = format!("lit_{}_{}", mangle(&self.pkg, "f"), self.labels);
        // Translate it as its own function; it sees no locals of ours.
        let saved = self.f.take();
        let r = self.gen_function(&sym, None, sig, &s, body, pos);
        self.f = saved;
        r?;
        self.emit(format!("li      r1, #seg({sym})"));
        self.emit(format!("li      r2, #off({sym})"));
        Ok(Ty::Func(Box::new(s)))
    }

    // ---- conversions ----

    fn convert(&mut self, x: &Expr, to: &Ty) -> CResult<Ty> {
        let mut from = self.type_peek(x)?;
        if matches!(from, Ty::UntypedStr) && matches!(self.types.under(to), Ty::Slice(_)) {
            from = Ty::Str;
        }
        if matches!(from, Ty::UntypedNil) {
            if !self.types.assignable(&from, to) {
                return err(
                    x.pos,
                    format!("cannot convert nil to {}", self.types.name(to)),
                );
            }
            self.load_const(to, &Const::Nil);
            return Ok(to.clone());
        }
        if self.types.is_iface(to) {
            let t = self.expr(x, Some(to))?;
            self.coerce(&t, to, x.pos)?;
            return Ok(to.clone());
        }
        if is_untyped(&from) {
            // constants were folded by try_const; a non-constant untyped
            // value (a comparison) is a bool
            self.expr(x, Some(to))?;
            return Ok(to.clone());
        }
        let uf = self.types.under(&from).clone();
        let ut = self.types.under(to).clone();
        match (&uf, &ut) {
            (Ty::Slice(e), Ty::Str) if matches!(self.types.under(e), Ty::U8) => {
                self.expr(x, None)?;
                self.rt("rt_str_from_bytes");
                return Ok(to.clone());
            }
            (Ty::Str, Ty::Slice(e)) if matches!(self.types.under(e), Ty::U8) => {
                self.expr(x, None)?;
                self.rt("rt_bytes_from_str");
                return Ok(to.clone());
            }
            _ => {}
        }
        self.expr(x, None)?;
        let num = |t: &Ty| {
            matches!(
                t,
                Ty::I8 | Ty::U8 | Ty::I16 | Ty::U16 | Ty::I32 | Ty::U32 | Ty::Uintptr
            )
        };
        let ptrish = |t: &Ty| matches!(t, Ty::Ptr(_) | Ty::Uintptr);
        if uf == ut
            || (ptrish(&uf) && ptrish(&ut))
            || (matches!(uf, Ty::Uintptr) && matches!(ut, Ty::I32 | Ty::U32))
            || (matches!(ut, Ty::Uintptr) && matches!(uf, Ty::I32 | Ty::U32))
        {
            return Ok(to.clone());
        }
        let fw = self.types.is_wide(&from);
        let tw = self.types.is_wide(to);
        match (&uf, &ut) {
            (f, Ty::Fixed) if num(f) => {
                if fw {
                    self.emit("mov     r1, r2");
                }
                self.emit("movi    r2, #0");
            }
            (Ty::Fixed, t) if num(t) => {
                // the integer part (rounded down)
                if tw {
                    self.emit("mov     r2, r1");
                    self.emit("movi    r9, #15");
                    self.emit("asr     r1, r9");
                } else {
                    self.narrow(to);
                }
            }
            (f, t) if num(f) && num(t) => {
                match (fw, tw) {
                    (false, true) => {
                        self.emit("mov     r2, r1");
                        if self.types.is_signed(&from) {
                            self.emit("movi    r9, #15");
                            self.emit("asr     r1, r9");
                        } else {
                            self.emit("movi    r1, #0");
                        }
                    }
                    (true, false) => self.emit("mov     r1, r2"),
                    _ => {}
                }
                if !tw {
                    self.narrow(to);
                }
            }
            _ => {
                return err(
                    x.pos,
                    format!(
                        "cannot convert {} to {}",
                        self.types.name(&from),
                        self.types.name(to)
                    ),
                )
            }
        }
        Ok(to.clone())
    }

    // ---- built-in functions ----

    fn builtin(&mut self, e: &Expr, b: &str, args: &[Expr], spread: bool) -> CResult<Ty> {
        let n = |k: usize| -> CResult<()> {
            if args.len() == k {
                Ok(())
            } else {
                err(e.pos, format!("{b} takes {k} argument(s)"))
            }
        };
        match b {
            "len" | "cap" => {
                n(1)?;
                let t = self.type_peek(&args[0])?;
                match self.types.under(&t).clone() {
                    Ty::Array(k, _) => self.li(1, i64::from(k)),
                    Ty::Ptr(inner) if matches!(self.types.under(&inner), Ty::Array(..)) => {
                        let Ty::Array(k, _) = self.types.under(&inner).clone() else {
                            unreachable!()
                        };
                        self.li(1, i64::from(k));
                    }
                    Ty::Slice(_) => {
                        self.expr(&args[0], None)?;
                        self.emit(if b == "len" {
                            "mov     r1, r3"
                        } else {
                            "mov     r1, r4"
                        });
                    }
                    Ty::Str if b == "len" => {
                        self.expr(&args[0], None)?;
                        self.emit("mov     r1, r3");
                    }
                    _ => return err(e.pos, format!("{b} of {}", self.types.name(&t))),
                }
                Ok(Ty::I16)
            }
            "new" => {
                n(1)?;
                let t = self
                    .type_of_callee(&args[0])?
                    .ok_or((args[0].pos, "new takes a type".to_string()))?;
                let size = self.types.size(&t).max(1);
                self.li(1, i64::from(size >> 16));
                self.li(2, i64::from(size & 0xffff));
                self.rt("rt_alloc");
                Ok(Ty::Ptr(Box::new(t)))
            }
            "make" => {
                if args.is_empty() || args.len() > 3 {
                    return err(e.pos, "make([]T, len[, cap])");
                }
                let t = self
                    .type_of_callee(&args[0])?
                    .ok_or((args[0].pos, "make takes a type".to_string()))?;
                if let Ty::Chan(el) = self.types.under(&t).clone() {
                    if args.len() > 2 {
                        return err(e.pos, "make(chan T[, cap])");
                    }
                    return self.make_chan(&t, &el, args.get(1));
                }
                let Ty::Slice(el) = self.types.under(&t).clone() else {
                    return err(e.pos, "make makes slices and channels");
                };
                if args.len() < 2 {
                    return err(e.pos, "make([]T, len[, cap])");
                }
                let m = self.mark();
                self.expr(&args[1], Some(&Ty::I16))?;
                let len = self.spill(&Ty::I16);
                if args.len() == 3 {
                    self.expr(&args[2], Some(&Ty::I16))?;
                }
                let cap = self.spill(&Ty::I16);
                // bytes = cap * size (32 bits)
                let size = self.types.size(&el);
                self.li(3, i64::from(size));
                self.emit("mov     r2, r1");
                self.emit("mul     r2, r3");
                self.emit("mulhu   r1, r3");
                self.rt("rt_alloc");
                self.ld(3, len);
                self.ld(4, cap);
                self.release(m);
                Ok(t)
            }
            "append" => {
                if args.is_empty() {
                    return err(e.pos, "append needs a slice");
                }
                let st = self.type_peek(&args[0])?;
                let Ty::Slice(el) = self.types.under(&st).clone() else {
                    return err(e.pos, "append needs a slice");
                };
                let size = self.types.size(&el);
                let m = self.mark();
                self.expr(&args[0], None)?;
                let s = self.spill(&st);
                if spread {
                    if args.len() != 2 {
                        return err(e.pos, "append(s, t...) takes two slices");
                    }
                    let tt = self.expr(&args[1], Some(&st))?;
                    let src = self.spill(&tt);
                    // grow by len(t), then copy t to s[len:]
                    self.unspill(&st, s, 1);
                    let par = self.slot(2);
                    self.li(10, i64::from(size));
                    self.st(10, par);
                    self.ld(10, src + 4);
                    self.st(10, par + 2);
                    self.li(9, i64::from(par));
                    self.rt("rt_grow");
                    for k in 0..4 {
                        self.st(k + 1, s + 2 * u32::from(k));
                    }
                    // dst = ptr + len*size; src ptr; bytes = len(t)*size
                    self.elem_ptr(s, size);
                    self.ld(3, src);
                    self.ld(4, src + 2);
                    self.ld(9, src + 4);
                    self.li(10, i64::from(size));
                    self.emit("mul     r9, r10");
                    self.rt("rt_memcpy");
                    self.ld(9, src + 4);
                    self.ld(10, s + 4);
                    self.emit("add     r10, r9");
                    self.st(10, s + 4);
                } else {
                    let k = args.len() - 1;
                    if k > 0 {
                        self.unspill(&st, s, 1);
                        let par = self.slot(4);
                        self.li(10, i64::from(size));
                        self.st(10, par);
                        self.li(10, k as i64);
                        self.st(10, par + 2);
                        self.li(9, i64::from(par));
                        self.rt("rt_grow");
                        for r in 0..4u32 {
                            self.st(r as u8 + 1, s + 2 * r);
                        }
                    }
                    for a in &args[1..] {
                        // address of s[len], store, len++
                        self.elem_ptr(s, size);
                        let at = self.spill_addr();
                        self.store_at(at, 0, &el, a)?;
                        self.ld(10, s + 4);
                        self.emit("addi    r10, #1");
                        self.st(10, s + 4);
                    }
                }
                self.unspill(&st, s, 1);
                self.release(m);
                Ok(st)
            }
            "copy" => {
                n(2)?;
                let dt = self.type_peek(&args[0])?;
                let Ty::Slice(el) = self.types.under(&dt).clone() else {
                    return err(e.pos, "copy(dst, src []T)");
                };
                let m = self.mark();
                self.expr(&args[0], None)?;
                let d = self.spill(&dt);
                let srct = self.expr(&args[1], None)?;
                let s = self.spill(&srct);
                // n = min(len d, len s)
                let nn = self.slot(2);
                self.ld(1, d + 4);
                self.ld(3, s + 4);
                let ok = self.new_label();
                self.emit("cmp     r1, r3");
                self.emit(format!("bls     {ok}"));
                self.emit("mov     r1, r3");
                self.label(&ok);
                self.st(1, nn);
                self.ld(1, d);
                self.ld(2, d + 2);
                self.ld(3, s);
                self.ld(4, s + 2);
                self.ld(9, nn);
                self.li(10, i64::from(self.types.size(&el)));
                self.emit("mul     r9, r10");
                self.rt("rt_memcpy");
                self.ld(1, nn);
                self.release(m);
                Ok(Ty::I16)
            }
            "free" => {
                n(1)?;
                let t = self.expr(&args[0], None)?;
                match self.types.under(&t) {
                    Ty::Ptr(_) | Ty::Slice(_) | Ty::Str => self.rt("rt_free"),
                    _ => return err(e.pos, "free takes a pointer, slice or string"),
                }
                Ok(Ty::Void)
            }
            "close" => {
                n(1)?;
                let t = self.expr(&args[0], None)?;
                if !matches!(self.types.under(&t), Ty::Chan(_)) {
                    return err(e.pos, "close takes a channel");
                }
                self.rt("rt_chan_close");
                Ok(Ty::Void)
            }
            "panic" => {
                n(1)?;
                let t = self.expr(&args[0], Some(&Ty::Str))?;
                if !matches!(self.types.under(&t), Ty::Str) {
                    return err(e.pos, "panic takes a string");
                }
                self.rt("rt_panic");
                Ok(Ty::Void)
            }
            "print" | "println" => {
                for (k, a) in args.iter().enumerate() {
                    if k > 0 && b == "println" {
                        self.li(1, i64::from(b' '));
                        self.rt("rt_print_char");
                    }
                    let t = self.expr(a, None)?;
                    let r = match self.types.under(&t) {
                        Ty::Str => "rt_print_str",
                        Ty::Bool => "rt_print_bool",
                        Ty::I8 | Ty::I16 => "rt_print_int",
                        Ty::U8 | Ty::U16 => "rt_print_uint",
                        Ty::I32 => "rt_print_int32",
                        Ty::U32 => "rt_print_uint32",
                        Ty::Fixed => "rt_print_fixed",
                        Ty::Ptr(_) | Ty::Uintptr | Ty::Func(_) => "rt_print_hex32",
                        _ => return err(a.pos, format!("cannot print {}", self.types.name(&t))),
                    };
                    self.rt(r);
                }
                if b == "println" {
                    self.li(1, 10);
                    self.rt("rt_print_char");
                }
                Ok(Ty::Void)
            }
            "min" | "max" => {
                if args.is_empty() {
                    return err(e.pos, format!("{b} needs values"));
                }
                let m = self.mark();
                let t = self.expr(&args[0], None)?;
                if !self.types.is_numeric(&t) {
                    return err(e.pos, format!("{b} of {}", self.types.name(&t)));
                }
                let acc = self.spill(&t);
                for a in &args[1..] {
                    self.expr(a, Some(&t))?;
                    let keep = self.new_label();
                    let wide = self.types.is_wide(&t);
                    // R1.. = candidate; replace acc when better
                    let cand = self.spill(&t);
                    let r = self.compare(
                        a.pos,
                        if b == "min" { ">" } else { "<" },
                        &self.types.under(&t).clone(),
                        acc,
                        &t,
                    );
                    // compare() takes the right side from R1..: reload it
                    let _ = r;
                    let _ = wide;
                    self.jump_if_false(&keep);
                    self.unspill(&t, cand, 1);
                    for k in 0..self.types.words(&t) {
                        self.st(k as u8 + 1, acc + 2 * k);
                    }
                    self.label(&keep);
                }
                self.unspill(&t, acc, 1);
                self.release(m);
                Ok(t)
            }
            _ => err(e.pos, format!("{b} is not built yet")),
        }
    }

    /// R1:R2 = the address of slice `s` (spilled) element `len`.
    fn elem_ptr(&mut self, s: u32, size: u32) {
        self.ld(1, s + 4);
        self.li(3, i64::from(size));
        self.emit("mov     r4, r1");
        self.emit("mul     r4, r3");
        self.emit("mulhu   r1, r3");
        self.emit("mov     r3, r1");
        self.ld(1, s);
        self.ld(2, s + 2);
        self.emit("add     r2, r4");
        self.emit("adc     r1, r3");
    }
}

pub(super) enum CallTarget {
    Static(String),
    /// The function value spilled at this offset (segment, offset).
    Value(u32),
}
