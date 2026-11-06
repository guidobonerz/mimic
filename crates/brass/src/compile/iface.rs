//! Interfaces (BRASS.md 4): conversions, calls through the method table,
//! type assertions, type switches, comparison.
//!
//! **[B]** An interface value is two far pointers, 8 bytes: the value's
//! address ("data") and the address of the dynamic type's descriptor (0:0
//! for a nil interface). In registers R1:R2 = data (segment, offset),
//! R3:R4 = descriptor (segment, offset); spilled in the frame in that
//! order; in memory each pointer offset first, as every pointer
//! (data offset, data segment, descriptor offset, descriptor segment).
//!
//! A type converted to an interface (or named in an assertion or a type
//! switch case) gets one descriptor `tdesc_…` in the data segment; its
//! address is the type's identity:
//!
//! ```text
//! .word count, size         ; methods; bytes of a boxed value (0: a pointer type)
//! .word name, off(f), seg(f) ; per method, sorted by name
//! ```
//!
//! `name` is the program's number for a method name, given in order of
//! first use (deterministic). Every entry takes the receiver as a pointer
//! (the data pointer): pointer-receiver methods are listed themselves,
//! value-receiver methods through a wrapper `func (p *T) M(…) { return
//! (*p).M(…) }` the compiler adds (`iw_` + the method's symbol). The
//! descriptor of `*T` lists both kinds, the one of `T` the value
//! methods only (Go's method sets). A pointer value is stored as data
//! itself; any other value is copied to a new heap block.
//!
//! `x.M(a)` looks the method up at run time (`rt_ifind`, linear, panics
//! for a nil interface) and calls it with the data pointer as receiver.
//! `==` is true for the same descriptor and either the same data pointer
//! or, for a non-pointer dynamic type, the same bytes (`rt_memeq`; a
//! string compares its pointer and length, not its bytes).

use super::expr::CallTarget;
use super::*;

#[derive(Default, Debug)]
pub(super) struct IfaceState {
    /// Method names by number, in order of first use.
    names: Vec<String>,
    /// The types with a descriptor, in order of first use, and its label.
    descs: Vec<(Ty, String)>,
    /// The runtime's interface routines are called.
    used: bool,
}

/// A descriptor entry: method name, signature (no receiver), symbol, and
/// for a value method the named type and the value method's symbol (the
/// entry is then a wrapper).
type Entry = (String, Sig, String, Option<(usize, String)>);

impl Compiler {
    /// Does the program call the runtime's interface routines?
    pub fn uses_interfaces(&self) -> bool {
        self.ifc.used
    }

    /// `interface { … }`: methods sorted by name, embedded ones merged.
    pub(super) fn resolve_iface(
        &mut self,
        ms: &[(String, FuncType, Pos)],
        embedded: &[TypeExpr],
    ) -> CResult<Ty> {
        let mut v: Vec<(String, Sig)> = Vec::new();
        for (n, ft, p) in ms {
            let s = self.resolve_sig(ft)?;
            if v.iter().any(|m| m.0 == *n) {
                return err(*p, format!("duplicate method {n}"));
            }
            v.push((n.clone(), s));
        }
        for e in embedded {
            let t = self.resolve_type(e)?;
            let Ty::Iface(more) = self.types.under(&t).clone() else {
                return err(
                    t_pos(e),
                    format!("{} is not an interface", self.types.name(&t)),
                );
            };
            for (n, s) in more {
                match v.iter().find(|m| m.0 == n) {
                    Some(m) if m.1 == s => {}
                    Some(_) => return err(t_pos(e), format!("duplicate method {n}")),
                    None => v.push((n, s)),
                }
            }
        }
        v.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Ty::Iface(v))
    }

    /// The number of a method name.
    fn method_num(&mut self, name: &str) -> u16 {
        match self.ifc.names.iter().position(|n| n == name) {
            Some(k) => k as u16,
            None => {
                self.ifc.names.push(name.to_string());
                (self.ifc.names.len() - 1) as u16
            }
        }
    }

    /// A runtime routine of the interface section.
    fn rt_iface(&mut self, name: &str) {
        self.ifc.used = true;
        self.rt(name);
    }

    /// The label of `t`'s descriptor (emitted at the end).
    fn desc_label(&mut self, t: &Ty) -> String {
        if let Some((_, l)) = self.ifc.descs.iter().find(|(d, _)| d == t) {
            return l.clone();
        }
        let mut l = format!("tdesc_{}", self.type_mangle(t));
        if self.ifc.descs.iter().any(|(_, x)| *x == l) {
            l = format!("{l}_{}", self.ifc.descs.len());
        }
        self.ifc.descs.push((t.clone(), l.clone()));
        l
    }

    /// A type as a part of a symbol.
    fn type_mangle(&self, t: &Ty) -> String {
        match t {
            Ty::Named(i) => {
                let n = &self.types.named[*i];
                mangle(&n.pkg, &n.name)
            }
            Ty::Ptr(e) => format!("p_{}", self.type_mangle(e)),
            Ty::Slice(e) => format!("s_{}", self.type_mangle(e)),
            Ty::Chan(e) => format!("c_{}", self.type_mangle(e)),
            Ty::Array(n, e) => format!("a{n}_{}", self.type_mangle(e)),
            Ty::Struct(_) => "struct".into(),
            Ty::Func(_) => "func".into(),
            Ty::Iface(_) => "iface".into(),
            t => self.types.name(t),
        }
    }

    /// The methods of `t` (receiver left out): an interface's, or the
    /// method set of a named type or a pointer to one.
    fn methods_of(&self, t: &Ty) -> Vec<(String, Sig)> {
        if let Ty::Iface(ms) = self.types.under(t) {
            return ms.clone();
        }
        self.entries(t)
            .into_iter()
            .map(|(n, s, _, _)| (n, s))
            .collect()
    }

    /// A concrete type's descriptor entries, sorted by name.
    fn entries(&self, t: &Ty) -> Vec<Entry> {
        let (i, ptr) = match t {
            Ty::Named(i) => (*i, false),
            Ty::Ptr(inner) => match **inner {
                Ty::Named(i) => (i, true),
                _ => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        if self.types.is_iface(&Ty::Named(i)) {
            return Vec::new();
        }
        let mut v: Vec<Entry> = self.types.named[i]
            .methods
            .iter()
            .filter(|(_, _, by_ptr, _)| ptr || !by_ptr)
            .map(|(n, sym, by_ptr, sig)| {
                let s = Sig {
                    params: sig.params[1..].to_vec(),
                    results: sig.results.clone(),
                };
                if *by_ptr {
                    (n.clone(), s, sym.clone(), None)
                } else {
                    (n.clone(), s, format!("iw_{sym}"), Some((i, sym.clone())))
                }
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Why `t` does not implement the interface methods `ms` (None: it does).
    fn missing(&self, t: &Ty, ms: &[(String, Sig)]) -> Option<String> {
        let have = self.methods_of(t);
        for (n, s) in ms {
            match have.iter().find(|m| m.0 == *n) {
                Some(m) if m.1 == *s => {}
                Some(_) => return Some(format!("wrong type for method {n}")),
                None => {
                    let by_ptr = match t {
                        Ty::Named(i) => self.types.named[*i]
                            .methods
                            .iter()
                            .any(|m| m.0 == *n && m.2),
                        _ => false,
                    };
                    return Some(if by_ptr {
                        format!("method {n} has pointer receiver")
                    } else {
                        format!("missing method {n}")
                    });
                }
            }
        }
        None
    }

    /// Check that the value of type `from` in R1.. (a struct: its address)
    /// may be used as `to`, and convert it to an interface when `to` is one.
    pub(super) fn coerce(&mut self, from: &Ty, to: &Ty, pos: Pos) -> CResult<()> {
        let Ty::Iface(ms) = self.types.under(to).clone() else {
            return self.check_assign(from, to, pos);
        };
        if matches!(from, Ty::UntypedNil) || from == to {
            return Ok(());
        }
        let from = self.types.default_of(from);
        if matches!(from, Ty::Void | Ty::Tuple(_) | Ty::UntypedNil) {
            return self.check_assign(&from, to, pos);
        }
        if let Some(why) = self.missing(&from, &ms) {
            return err(
                pos,
                format!(
                    "{} does not implement {} ({why})",
                    self.types.name(&from),
                    self.types.name(to)
                ),
            );
        }
        if !self.types.is_iface(&from) {
            self.box_value(&from);
        }
        Ok(())
    }

    /// The value of `from` in R1.. (a struct: its address) as an interface
    /// in R1..R4.
    fn box_value(&mut self, from: &Ty) {
        let m = self.mark();
        if !matches!(self.types.under(from), Ty::Ptr(_)) {
            let words = self.types.words(from);
            let v = if words == 0 {
                self.spill_addr()
            } else {
                self.spill(from)
            };
            let size = self.types.size(from).max(1);
            self.li(1, i64::from(size >> 16));
            self.li(2, i64::from(size & 0xffff));
            self.rt("rt_alloc");
            let h = self.spill_addr();
            if words == 0 {
                self.ld(3, v);
                self.ld(4, v + 2);
                self.memcpy(size);
            } else {
                self.unspill(from, v, 1);
                self.addr_to_x(h);
                self.store_x(from);
            }
            self.ld(1, h);
            self.ld(2, h + 2);
        }
        let l = self.desc_label(from);
        self.emit(format!("li      r3, #seg({l})"));
        self.emit(format!("li      r4, #off({l})"));
        self.release(m);
    }

    /// `x.M(args)` with `x` an interface: the target (the method found at
    /// run time) and the argument words, the data pointer first.
    pub(super) fn iface_call_args(
        &mut self,
        pos: Pos,
        x: &Expr,
        name: &str,
        sig: &Sig,
        args: &[Expr],
    ) -> CResult<(CallTarget, Vec<u32>)> {
        let xt = self.expr(x, None)?;
        let iv = self.spill(&xt);
        let num = self.method_num(name);
        self.ld(3, iv + 4);
        self.ld(4, iv + 6);
        self.li(1, i64::from(num));
        self.rt_iface("rt_ifind");
        let fv = self.spill_addr();
        let mut words = vec![iv, iv + 2];
        words.extend(self.arg_words(pos, sig, None, args)?);
        Ok((CallTarget::Value(fv), words))
    }

    /// Compare the interface spilled at `left` with the one in R1..R4;
    /// R1 = the result of `op` (== or !=).
    pub(super) fn compare_iface(&mut self, op: &str, left: u32) {
        let right = self.spill(&Ty::Iface(Vec::new()));
        let yes = self.new_label();
        let no = self.new_label();
        let diff = self.new_label();
        let done = self.new_label();
        // the same descriptor?
        self.ld(1, left + 4);
        self.ld(3, right + 4);
        self.emit("cmp     r1, r3");
        self.branch_far("bne", &no);
        self.ld(1, left + 6);
        self.ld(3, right + 6);
        self.emit("cmp     r1, r3");
        self.branch_far("bne", &no);
        // both nil
        self.ld(2, left + 4);
        self.emit("or      r1, r2");
        self.branch_far("beq", &yes);
        // the same data
        self.ld(1, left);
        self.ld(3, right);
        self.emit("cmp     r1, r3");
        self.branch_far("bne", &diff);
        self.ld(1, left + 2);
        self.ld(3, right + 2);
        self.emit("cmp     r1, r3");
        self.branch_far("beq", &yes);
        // a value: its bytes (a pointer type has size 0: different)
        self.label(&diff);
        self.ld(1, left + 4);
        self.ld(2, left + 6);
        self.emit("mts     s3, r1");
        self.emit("addi    r2, #2");
        self.emit("lwx     r10, (r2)");
        self.emit("cmpi    r10, #0");
        self.emit(format!("beq     {no}"));
        self.ld(1, left);
        self.ld(2, left + 2);
        self.ld(3, right);
        self.ld(4, right + 2);
        self.emit("mov     r9, r10");
        self.rt_iface("rt_memeq");
        self.jump(&done);
        self.label(&yes);
        self.emit("movi    r1, #1");
        self.jump(&done);
        self.label(&no);
        self.emit("movi    r1, #0");
        self.label(&done);
        if op == "!=" {
            self.emit("movi    r9, #1");
            self.emit("xor     r1, r9");
        }
    }

    /// Branch on `cond` (beq or bne) to `l` at any distance.
    fn branch_far(&mut self, cond: &str, l: &str) {
        let skip = self.new_label();
        let inverse = if cond == "beq" { "bne" } else { "beq" };
        self.emit(format!("{inverse}     {skip}"));
        self.jump(l);
        self.label(&skip);
    }

    /// A compile-time check of `x.(t)` for `x` of interface type `xt`.
    fn check_assertable(&self, xt: &Ty, t: &Ty, pos: Pos, what: &str) -> CResult<()> {
        let Ty::Iface(ms) = self.types.under(xt).clone() else {
            return err(pos, format!("{} is not an interface", self.types.name(xt)));
        };
        if self.types.is_iface(t) {
            return Ok(());
        }
        if let Some(why) = self.missing(t, &ms) {
            return err(
                pos,
                format!(
                    "{what}: {} does not implement {} ({why})",
                    self.types.name(t),
                    self.types.name(xt)
                ),
            );
        }
        Ok(())
    }

    /// R1 = 1 when the interface spilled at `iv` holds a `t` (an
    /// interface type: when its dynamic type has `t`'s methods).
    fn type_test(&mut self, iv: u32, t: &Ty) {
        let no = self.new_label();
        let done = self.new_label();
        if let Ty::Iface(ms) = self.types.under(t).clone() {
            let some = self.new_label();
            self.ld(3, iv + 4);
            self.ld(4, iv + 6);
            self.emit("mov     r9, r3");
            self.emit("or      r9, r4");
            self.emit(format!("bne     {some}"));
            self.jump(&no);
            self.label(&some);
            for (n, _) in &ms {
                let num = self.method_num(n);
                let has = self.new_label();
                self.ld(3, iv + 4);
                self.ld(4, iv + 6);
                self.li(1, i64::from(num));
                self.rt_iface("rt_ilookup");
                self.emit("cmpi    r9, #0");
                self.emit(format!("bne     {has}"));
                self.jump(&no);
                self.label(&has);
            }
        } else {
            let l = self.desc_label(t);
            self.ld(3, iv + 4);
            self.ld(4, iv + 6);
            self.emit(format!("li      r1, #seg({l})"));
            self.emit(format!("li      r2, #off({l})"));
            self.emit("cmp     r1, r3");
            self.emit(format!("bne     {no}"));
            self.emit("cmp     r2, r4");
            self.emit(format!("bne     {no}"));
        }
        self.emit("movi    r1, #1");
        self.emit(format!("bra     {done}"));
        self.label(&no);
        self.emit("movi    r1, #0");
        self.label(&done);
    }

    /// The value of a successful `x.(t)` from the interface spilled at
    /// `iv`: in R1.. (a struct: its address, in the heap block).
    fn assert_value(&mut self, iv: u32, t: &Ty) {
        if self.types.is_iface(t) {
            self.unspill(&Ty::Iface(Vec::new()), iv, 1);
            return;
        }
        self.ld(1, iv);
        self.ld(2, iv + 2);
        if !matches!(self.types.under(t), Ty::Ptr(_)) && self.types.words(t) > 0 {
            self.addr_load(t);
        }
    }

    /// The type of an assertion `x.(te)`.
    fn assert_type(&mut self, te: &TypeExpr) -> CResult<Ty> {
        if let TypeExpr::Name(None, n, p) = te {
            if n == "type" {
                return err(*p, "use of .(type) outside a type switch");
            }
        }
        self.resolve_type(te)
    }

    /// `x.(T)`: the value in R1.. (a struct: its address); a panic when
    /// `x` does not hold a `T`.
    pub(super) fn assert_expr(&mut self, pos: Pos, x: &Expr, te: &TypeExpr) -> CResult<Ty> {
        let t = self.assert_type(te)?;
        let m = self.mark();
        let xt = self.expr(x, None)?;
        self.check_assertable(&xt, &t, pos, "impossible type assertion")?;
        let iv = self.spill(&xt);
        self.type_test(iv, &t);
        let ok = self.new_label();
        self.emit("cmpi    r1, #0");
        self.emit(format!("bne     {ok}"));
        let name = self.types.name(&t);
        let not_nil = self.new_label();
        self.ld(3, iv + 4);
        self.ld(4, iv + 6);
        self.emit("or      r3, r4");
        self.emit(format!("bne     {not_nil}"));
        self.load_const(
            &Ty::Str,
            &Const::Str(format!("interface conversion: interface is nil, not {name}").into_bytes()),
        );
        self.rt("rt_panic");
        self.label(&not_nil);
        self.load_const(
            &Ty::Str,
            &Const::Str(format!("interface conversion: interface is not {name}").into_bytes()),
        );
        self.rt("rt_panic");
        self.label(&ok);
        self.assert_value(iv, &t);
        self.release(m);
        Ok(t)
    }

    /// `v, ok := x.(T)`: the type, the frame slot with the value (zero on
    /// failure; scalars as spilled, a struct in place) and the slot of ok.
    pub(super) fn assert_ok(
        &mut self,
        pos: Pos,
        x: &Expr,
        te: &TypeExpr,
    ) -> CResult<(Ty, u32, u32)> {
        let t = self.assert_type(te)?;
        let xt = self.expr(x, None)?;
        self.check_assertable(&xt, &t, pos, "impossible type assertion")?;
        let iv = self.spill(&xt);
        self.type_test(iv, &t);
        let ok = self.slot(2);
        self.st(1, ok);
        let size = self.types.size(&t).max(1);
        let v = self.slot(size.next_multiple_of(2));
        self.zero_slot(v, &t);
        let done = self.new_label();
        self.ld(1, ok);
        self.jump_if_false(&done);
        self.assert_value(iv, &t);
        let words = self.types.words(&t);
        if words == 0 {
            self.emit("mov     r3, r1");
            self.emit("mov     r4, r2");
            self.frame_addr(v);
            self.memcpy(size);
        } else {
            for k in 0..words {
                self.st(k as u8 + 1, v + 2 * k);
            }
        }
        self.label(&done);
        Ok((t, v, ok))
    }

    /// `switch v := x.(type) { … }`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn type_switch(
        &mut self,
        init: Option<&Stmt>,
        bind: Option<&str>,
        x: &Expr,
        cases: &[TypeCase],
        label: &Option<String>,
        pos: Pos,
    ) -> CResult<()> {
        self.fnc().scopes.push(HashMap::new());
        if let Some(i) = init {
            self.stmt(i)?;
        }
        let xt = self.expr(x, None)?;
        if !self.types.is_iface(&xt) {
            return err(pos, format!("{} is not an interface", self.types.name(&xt)));
        }
        let iv = self.spill(&xt);
        let brk = self.new_label();
        let mut bodies = Vec::new();
        let mut default = None;
        for c in cases {
            let l = self.new_label();
            if c.default {
                if default.is_some() {
                    return err(c.pos, "several defaults in a type switch");
                }
                default = Some(l.clone());
            }
            let mut single = None;
            for te in &c.types {
                match te {
                    None => {
                        self.ld(3, iv + 4);
                        self.ld(4, iv + 6);
                        self.emit("mov     r1, r3");
                        self.emit("or      r1, r4");
                        self.jump_if_false(&l);
                    }
                    Some(te) => {
                        let t = self.resolve_type(te)?;
                        self.check_assertable(&xt, &t, c.pos, "impossible type switch case")?;
                        self.type_test(iv, &t);
                        self.jump_if_true(&l);
                        single = Some(t);
                    }
                }
            }
            if c.types.len() != 1 {
                single = None;
            }
            bodies.push((l, &c.body, single));
        }
        self.jump(default.as_deref().unwrap_or(&brk));
        self.fnc().loops.push(LoopLabels {
            name: label.clone(),
            brk: brk.clone(),
            cont: None,
        });
        for (l, body, single) in bodies {
            self.label(&l);
            self.fnc().scopes.push(HashMap::new());
            if let Some(name) = bind {
                let vt = single.unwrap_or_else(|| xt.clone());
                self.assert_value(iv, &vt);
                let size = self.types.size(&vt).max(1);
                let off = self.slot(size.next_multiple_of(2));
                self.store_slot_pub(off, &vt);
                self.declare_local(
                    name,
                    Local::Var {
                        off,
                        ty: vt,
                        by_ref: false,
                    },
                );
            }
            self.stmts(body)?;
            self.fnc().scopes.pop();
            self.jump(&brk);
        }
        self.fnc().loops.pop();
        self.label(&brk);
        self.fnc().scopes.pop();
        Ok(())
    }

    /// After every package: the wrappers of value methods and the
    /// descriptors (nothing for a program without interfaces).
    pub(super) fn finish_ifaces(&mut self) -> CResult<()> {
        let mut wrapped: Vec<String> = Vec::new();
        let mut k = 0;
        while k < self.ifc.descs.len() {
            let (t, label) = self.ifc.descs[k].clone();
            k += 1;
            let entries = self.entries(&t);
            let size = if matches!(self.types.under(&t), Ty::Ptr(_)) {
                0
            } else {
                self.types.size(&t).max(1)
            };
            let mut text = format!(
                "        .align  2\n{label}:\n        .word   {}, {size}\n",
                entries.len()
            );
            for (name, sig, sym, wrap) in entries {
                if let Some((i, vsym)) = wrap {
                    if !wrapped.contains(&sym) {
                        wrapped.push(sym.clone());
                        self.wrapper(i, &name, &sym, &vsym, &sig)?;
                    }
                }
                let num = self.method_num(&name);
                let _ = writeln!(text, "        .word   {num}, off({sym}), seg({sym})");
            }
            self.data.push_str(&text);
        }
        if !self.code.is_empty() {
            let code = std::mem::take(&mut self.code);
            self.pkg_code.push(("<interfaces>".into(), code));
        }
        Ok(())
    }

    /// `func (p *T) M(a…) … { return (*p).M(a…) }` for the value method M
    /// of the named type `i`, as symbol `sym`.
    fn wrapper(&mut self, i: usize, name: &str, sym: &str, _vsym: &str, sig: &Sig) -> CResult<()> {
        let pos = Pos::default();
        let recv_t = Ty::Ptr(Box::new(Ty::Named(i)));
        let mut full = sig.clone();
        full.params.insert(0, recv_t.clone());
        let params: Vec<Field> = (0..sig.params.len())
            .map(|k| Field {
                name: Some(format!("a·{k}")),
                ty: TypeExpr::Name(None, "_".into(), pos),
                pos,
            })
            .collect();
        let p = self.synth(pos, ExprKind::Ident("r·".into()));
        let deref = self.synth(pos, ExprKind::Unary("*", Box::new(p)));
        let paren = self.synth(pos, ExprKind::Paren(Box::new(deref)));
        let callee = self.synth(pos, ExprKind::Selector(Box::new(paren), name.to_string()));
        let args: Vec<Expr> = (0..sig.params.len())
            .map(|k| self.synth(pos, ExprKind::Ident(format!("a·{k}"))))
            .collect();
        let call = self.synth(pos, ExprKind::Call(Box::new(callee), args, false));
        let stmt = if sig.results.is_empty() {
            Stmt::Expr(call)
        } else {
            Stmt::Return(vec![call], pos)
        };
        let body = Block {
            stmts: vec![stmt],
            end: pos,
        };
        let ast_sig = FuncType {
            params,
            results: Vec::new(),
        };
        let saved = std::mem::replace(&mut self.pkg, self.types.named[i].pkg.clone());
        let r = self.gen_function(
            sym,
            Some(("r·".into(), recv_t)),
            &ast_sig,
            &full,
            &body,
            pos,
        );
        self.pkg = saved;
        r
    }
}
