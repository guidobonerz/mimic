//! Statements (BRASS.md 7).

use super::*;

impl Compiler {
    pub(super) fn block(&mut self, b: &Block) -> CResult<()> {
        self.fnc().scopes.push(HashMap::new());
        for s in &b.stmts {
            self.stmt(s)?;
        }
        self.fnc().scopes.pop();
        Ok(())
    }

    pub(super) fn stmts(&mut self, v: &[Stmt]) -> CResult<()> {
        self.fnc().scopes.push(HashMap::new());
        for s in v {
            self.stmt(s)?;
        }
        self.fnc().scopes.pop();
        Ok(())
    }

    pub(super) fn synth(&mut self, pos: Pos, kind: ExprKind) -> Expr {
        // Synthetic expressions take ids far above the parser's.
        self.labels += 1;
        Expr {
            id: u32::MAX - self.labels,
            pos,
            kind,
        }
    }

    pub(super) fn stmt(&mut self, s: &Stmt) -> CResult<()> {
        match s {
            Stmt::Empty => Ok(()),
            Stmt::Expr(e) => {
                match &e.kind {
                    ExprKind::Call(f, args, spread) => {
                        self.call(e, f, args, *spread)?;
                    }
                    _ => return err(e.pos, "this value is not used"),
                }
                Ok(())
            }
            Stmt::Block(b) => self.block(b),
            Stmt::VarDecl(specs, _) => {
                for spec in specs {
                    self.var_spec(spec)?;
                }
                Ok(())
            }
            Stmt::ConstDecl(specs, _) => {
                for spec in specs {
                    self.iota = Some(spec.iota);
                    for (k, (name, pos)) in spec.names.iter().enumerate() {
                        let v = spec
                            .values
                            .get(k)
                            .ok_or((*pos, "a constant needs a value".to_string()))?;
                        let (t, c) = self.const_expr(v)?;
                        let (t, c) = match &spec.ty {
                            Some(te) => {
                                let ty = self.resolve_type(te)?;
                                let c = self.convert_const(&t, c, &ty, *pos)?;
                                (ty, c)
                            }
                            None => (t, c),
                        };
                        self.declare_local(name, Local::Const { ty: t, val: c });
                    }
                    self.iota = None;
                }
                Ok(())
            }
            Stmt::TypeDecl(specs, pos) => {
                for spec in specs {
                    let t = self.resolve_type(&spec.ty)?;
                    let t = if spec.alias {
                        t
                    } else {
                        let under = self.types.under(&t).clone();
                        self.types.named.push(Named {
                            pkg: self.pkg.clone(),
                            name: spec.name.clone(),
                            under,
                            methods: Vec::new(),
                        });
                        Ty::Named(self.types.named.len() - 1)
                    };
                    let _ = pos;
                    self.declare_local(&spec.name, Local::Type(t));
                }
                Ok(())
            }
            Stmt::Assign {
                lhs,
                rhs,
                define,
                pos,
            } => self.assign_stmt(lhs, rhs, *define, *pos),
            Stmt::OpAssign { lhs, op, rhs, pos } => {
                let bin = self.synth(
                    *pos,
                    ExprKind::Binary(op, Box::new(lhs.clone()), Box::new(rhs.clone())),
                );
                self.assign(lhs, &bin)
            }
            Stmt::IncDec(x, inc, pos) => {
                let one = self.synth(*pos, ExprKind::Int(1));
                let bin = self.synth(
                    *pos,
                    ExprKind::Binary(
                        if *inc { "+" } else { "-" },
                        Box::new(x.clone()),
                        Box::new(one),
                    ),
                );
                self.assign(x, &bin)
            }
            Stmt::If {
                init,
                cond,
                then,
                els,
                ..
            } => {
                self.fnc().scopes.push(HashMap::new());
                if let Some(i) = init {
                    self.stmt(i)?;
                }
                let else_l = self.new_label();
                let end = self.new_label();
                let t = self.expr(cond, Some(&Ty::Bool))?;
                if !matches!(self.types.under(&t), Ty::Bool) {
                    return err(cond.pos, "an if condition is a bool");
                }
                self.jump_if_false(&else_l);
                self.block(then)?;
                self.jump(&end);
                self.label(&else_l);
                if let Some(e) = els {
                    self.stmt(e)?;
                }
                self.label(&end);
                self.fnc().scopes.pop();
                Ok(())
            }
            Stmt::For {
                init,
                cond,
                post,
                body,
                label,
                ..
            } => {
                self.fnc().scopes.push(HashMap::new());
                if let Some(i) = init {
                    self.stmt(i)?;
                }
                let top = self.new_label();
                let cont = self.new_label();
                let brk = self.new_label();
                self.label(&top);
                if let Some(c) = cond {
                    let t = self.expr(c, Some(&Ty::Bool))?;
                    if !matches!(self.types.under(&t), Ty::Bool) {
                        return err(c.pos, "a for condition is a bool");
                    }
                    self.jump_if_false(&brk);
                }
                self.fnc().loops.push(LoopLabels {
                    name: label.clone(),
                    brk: brk.clone(),
                    cont: Some(cont.clone()),
                });
                self.block(body)?;
                self.fnc().loops.pop();
                self.label(&cont);
                if let Some(p) = post {
                    self.stmt(p)?;
                }
                self.jump(&top);
                self.label(&brk);
                self.fnc().scopes.pop();
                Ok(())
            }
            Stmt::Range {
                key,
                value,
                define,
                over,
                body,
                label,
                pos,
            } => self.range(
                key.as_ref(),
                value.as_ref(),
                *define,
                over,
                body,
                label,
                *pos,
            ),
            Stmt::Switch {
                init,
                tag,
                cases,
                label,
                ..
            } => {
                self.fnc().scopes.push(HashMap::new());
                if let Some(i) = init {
                    self.stmt(i)?;
                }
                let brk = self.new_label();
                let (tag_ty, tag_off) = match tag {
                    Some(t) => {
                        let ty = self.expr(t, None)?;
                        if self.types.words(&ty) == 0 {
                            return err(t.pos, "switch on a struct or array");
                        }
                        (Some(ty.clone()), self.spill(&ty))
                    }
                    None => (None, 0),
                };
                let mut bodies = Vec::new();
                let mut default = None;
                for c in cases {
                    let l = self.new_label();
                    if c.default {
                        default = Some(l.clone());
                    }
                    for x in &c.exprs {
                        match &tag_ty {
                            Some(tt) => {
                                let mut xt = self.expr(x, Some(tt))?;
                                self.coerce(&xt, tt, x.pos)?;
                                if self.types.is_iface(tt) {
                                    xt = tt.clone();
                                }
                                let ut = self.types.under(tt).clone();
                                self.compare(x.pos, "==", &ut, tag_off, &xt)?;
                            }
                            None => {
                                let xt = self.expr(x, Some(&Ty::Bool))?;
                                if !matches!(self.types.under(&xt), Ty::Bool) {
                                    return err(x.pos, "a case of a switch without tag is a bool");
                                }
                            }
                        }
                        self.jump_if_true(&l);
                    }
                    bodies.push((l, &c.body));
                }
                self.jump(default.as_deref().unwrap_or(&brk));
                self.fnc().loops.push(LoopLabels {
                    name: label.clone(),
                    brk: brk.clone(),
                    cont: None,
                });
                for (l, b) in bodies {
                    self.label(&l);
                    self.stmts(b)?;
                    self.jump(&brk);
                }
                self.fnc().loops.pop();
                self.label(&brk);
                self.fnc().scopes.pop();
                Ok(())
            }
            Stmt::TypeSwitch {
                init,
                bind,
                x,
                cases,
                label,
                pos,
            } => self.type_switch(init.as_deref(), bind.as_deref(), x, cases, label, *pos),
            Stmt::Return(vals, pos) => self.ret(vals, *pos),
            Stmt::Break(l, pos) => {
                let target = self.loop_label(l.as_deref(), true, *pos)?;
                self.jump(&target);
                Ok(())
            }
            Stmt::Continue(l, pos) => {
                let target = self.loop_label(l.as_deref(), false, *pos)?;
                self.jump(&target);
                Ok(())
            }
            Stmt::Go(call, pos) => self.go(call, *pos),
            Stmt::Send(c, v, pos) => self.send(c, v, *pos),
            Stmt::Select(cases, label, pos) => self.select(cases, label, *pos),
        }
    }

    fn loop_label(&mut self, name: Option<&str>, brk: bool, pos: Pos) -> CResult<String> {
        let f = self.fnc();
        for l in f.loops.iter().rev() {
            if name.is_some() && l.name.as_deref() != name {
                continue;
            }
            if brk {
                return Ok(l.brk.clone());
            }
            if let Some(c) = &l.cont {
                return Ok(c.clone());
            }
            if name.is_some() {
                return err(pos, "continue needs a loop");
            }
        }
        err(
            pos,
            if brk {
                "break outside a loop or switch"
            } else {
                "continue outside a loop"
            },
        )
    }

    fn ret(&mut self, vals: &[Expr], pos: Pos) -> CResult<()> {
        let results = self.fnc().results.clone();
        if vals.len() != results.len() {
            // one call returning all of them
            if vals.len() == 1 && results.len() > 1 {
                if let ExprKind::Call(f, args, sp) = &vals[0].kind {
                    let t = self.call(&vals[0], f, args, *sp)?;
                    if t != Ty::Tuple(results.clone()) {
                        return err(pos, "wrong result types");
                    }
                    let l = self.fnc().ret_label.clone();
                    self.jump(&l);
                    return Ok(());
                }
            }
            return err(
                pos,
                format!(
                    "{} values to return, the function has {}",
                    vals.len(),
                    results.len()
                ),
            );
        }
        let m = self.mark();
        let mut spills = Vec::new();
        for (v, t) in vals.iter().zip(&results) {
            let vt = match &v.kind {
                ExprKind::Composite(None, el) => self.composite(v.pos, t, el)?,
                _ => self.expr(v, Some(t))?,
            };
            self.coerce(&vt, t, v.pos)?;
            if results.len() > 1 {
                spills.push(self.spill(t));
            }
        }
        if results.len() > 1 {
            let mut r = 1u8;
            for (off, t) in spills.into_iter().zip(&results) {
                self.unspill(t, off, r);
                r += self.types.words(t) as u8;
            }
        }
        self.release(m);
        let l = self.fnc().ret_label.clone();
        self.jump(&l);
        Ok(())
    }

    fn var_spec(&mut self, spec: &VarSpec) -> CResult<()> {
        let declared = match &spec.ty {
            Some(t) => Some(self.resolve_type(t)?),
            None => None,
        };
        if spec.values.is_empty() {
            let t = declared.ok_or((
                spec.names[0].1,
                "a variable needs a type or a value".to_string(),
            ))?;
            for (n, _) in &spec.names {
                self.new_var(n, &t);
            }
            return Ok(());
        }
        if spec.values.len() == 1 && spec.names.len() > 1 {
            return self.define_multi(&spec.names, &spec.values[0], declared.as_ref());
        }
        if spec.values.len() != spec.names.len() {
            return err(spec.names[0].1, "names and values do not match");
        }
        for ((n, pos), v) in spec.names.iter().zip(&spec.values) {
            self.define_one(n, *pos, v, declared.as_ref())?;
        }
        Ok(())
    }

    /// `name := v` (or `var name T = v`).
    fn define_one(&mut self, name: &str, pos: Pos, v: &Expr, declared: Option<&Ty>) -> CResult<()> {
        let t = match declared {
            Some(t) => t.clone(),
            None => {
                let t = self.type_peek(v)?;
                match t {
                    Ty::Void => return err(pos, "the call has no value"),
                    Ty::Tuple(_) => return err(pos, "the call has several values"),
                    Ty::UntypedNil => return err(pos, "nil needs a type"),
                    t => self.types.default_of(&t),
                }
            }
        };
        let size = self.types.size(&t).max(1);
        let off = self.slot(size.next_multiple_of(2));
        let vt = match &v.kind {
            ExprKind::Composite(None, el) => self.composite(v.pos, &t, el)?,
            _ => self.expr(v, Some(&t))?,
        };
        self.coerce(&vt, &t, v.pos)?;
        self.store_slot(off, &t);
        self.declare_local(
            name,
            Local::Var {
                off,
                ty: t,
                by_ref: false,
            },
        );
        Ok(())
    }

    /// The value in R1.. (or a struct's address) into frame slot `off`.
    pub(super) fn store_slot_pub(&mut self, off: u32, t: &Ty) {
        self.store_slot(off, t)
    }

    pub(super) fn assign_from_pub(&mut self, lhs: &Expr, t: &Ty, off: u32) -> CResult<()> {
        self.assign_from(lhs, t, off)
    }

    fn store_slot(&mut self, off: u32, t: &Ty) {
        if self.types.words(t) == 0 {
            self.emit("mov     r3, r1");
            self.emit("mov     r4, r2");
            self.frame_addr(off);
            let size = self.types.size(t);
            self.memcpy(size);
        } else if self.types.is_byte_sized(t) {
            self.x_frame(off);
            self.store_x(t);
        } else {
            for k in 0..self.types.words(t) {
                self.st(k as u8 + 1, off + 2 * k);
            }
        }
    }

    pub(super) fn zero_slot(&mut self, off: u32, t: &Ty) {
        let size = self.types.size(t).max(1);
        if size <= 8 {
            self.emit("movi    r1, #0");
            for k in 0..size.div_ceil(2) {
                self.st(1, off + 2 * k);
            }
        } else {
            self.frame_addr(off);
            self.li(9, i64::from(size));
            self.rt("rt_memzero");
        }
    }

    /// `a, b := f()` and `var a, b = f()`.
    fn define_multi(
        &mut self,
        names: &[(String, Pos)],
        v: &Expr,
        declared: Option<&Ty>,
    ) -> CResult<()> {
        if let (ExprKind::Assert(x, te), 2) = (&v.kind, names.len()) {
            if declared.is_some() {
                return err(v.pos, "v, ok := x.(T) without a declared type");
            }
            let (t, val, ok) = self.assert_ok(v.pos, x, te)?;
            for ((n, _), (off, ty)) in names.iter().zip([(val, t), (ok, Ty::Bool)]) {
                if n != "_" {
                    self.declare_local(
                        n,
                        Local::Var {
                            off,
                            ty,
                            by_ref: false,
                        },
                    );
                }
            }
            return Ok(());
        }
        let t = match &v.kind {
            ExprKind::Call(f, args, sp) => self.call(v, f, args, *sp)?,
            ExprKind::Unary("<-", c) if names.len() == 2 => self.recv(c, true)?,
            _ => return err(v.pos, "names and values do not match"),
        };
        let Ty::Tuple(ts) = t else {
            return err(v.pos, "the call has one value");
        };
        if ts.len() != names.len() {
            return err(v.pos, format!("the call has {} values", ts.len()));
        }
        let mut r = 1u8;
        for ((n, pos), t) in names.iter().zip(&ts) {
            let t = declared.cloned().unwrap_or_else(|| t.clone());
            let words = self.types.words(&t) as u8;
            let size = self.types.size(&t).max(1);
            let off = self.slot(size.next_multiple_of(2));
            for k in 0..words {
                // results stay in their registers until stored
                self.st(r + k, off + 2 * u32::from(k));
            }
            let _ = pos;
            r += words;
            if n != "_" {
                self.declare_local(
                    n,
                    Local::Var {
                        off,
                        ty: t,
                        by_ref: false,
                    },
                );
            }
        }
        Ok(())
    }

    fn assign_stmt(&mut self, lhs: &[Expr], rhs: &[Expr], define: bool, pos: Pos) -> CResult<()> {
        if define {
            let names: Vec<(String, Pos)> = lhs
                .iter()
                .map(|e| match &e.kind {
                    ExprKind::Ident(n) => Ok((n.clone(), e.pos)),
                    _ => err(e.pos, "a name on the left of :="),
                })
                .collect::<CResult<_>>()?;
            // Names already in this scope are assigned (at least one new).
            let scope_has = |c: &Self, n: &str| {
                c.f.as_ref()
                    .and_then(|f| f.scopes.last())
                    .is_some_and(|s| s.contains_key(n))
            };
            if names.iter().all(|(n, _)| n == "_" || scope_has(self, n)) {
                return err(pos, "no new variables on the left of :=");
            }
            if let (ExprKind::Assert(x, te), 1, 2) = (&rhs[0].kind, rhs.len(), lhs.len()) {
                // `v, ok := x.(T)`, either name may exist already
                let (t, val, ok) = self.assert_ok(pos, x, te)?;
                for (k, ((n, _), (off, ty))) in
                    names.iter().zip([(val, t), (ok, Ty::Bool)]).enumerate()
                {
                    if n == "_" {
                        continue;
                    }
                    if scope_has(self, n) {
                        let s = if self.types.words(&ty) == 0 {
                            self.frame_addr(off);
                            self.spill_addr()
                        } else {
                            off
                        };
                        self.assign_from(&lhs[k], &ty, s)?;
                    } else {
                        self.declare_local(
                            n,
                            Local::Var {
                                off,
                                ty,
                                by_ref: false,
                            },
                        );
                    }
                }
                return Ok(());
            }
            if rhs.len() == 1 && lhs.len() > 1 {
                if names.iter().any(|(n, _)| scope_has(self, n)) {
                    return err(pos, "redeclaring with several results is not in Brass v1");
                }
                return self.define_multi(&names, &rhs[0], None);
            }
            if rhs.len() != lhs.len() {
                return err(pos, "names and values do not match");
            }
            // Evaluate everything first: `a, b := b, a`.
            if lhs.len() == 1 {
                let (n, p) = &names[0];
                if scope_has(self, n) {
                    return self.assign(&lhs[0], &rhs[0]);
                }
                return self.define_one(n, *p, &rhs[0], None);
            }
            let vals = self.eval_all(rhs, &lhs.iter().map(|_| None).collect::<Vec<_>>())?;
            for ((n, p), (t, off)) in names.iter().zip(vals) {
                if n == "_" {
                    continue;
                }
                if scope_has(self, n) {
                    let target = lhs
                        .iter()
                        .find(|e| matches!(&e.kind, ExprKind::Ident(x) if x == n))
                        .unwrap()
                        .clone();
                    self.assign_from(&target, &t, off)?;
                } else {
                    let _ = p;
                    let t = self.types.default_of(&t);
                    let size = self.types.size(&t).max(1);
                    let slot = self.slot(size.next_multiple_of(2));
                    self.unspill(&t, off, 1);
                    self.store_slot(slot, &t);
                    self.declare_local(
                        n,
                        Local::Var {
                            off: slot,
                            ty: t,
                            by_ref: false,
                        },
                    );
                }
            }
            return Ok(());
        }
        if let (ExprKind::Assert(x, te), 1, 2) = (&rhs[0].kind, rhs.len(), lhs.len()) {
            let m = self.mark();
            let (t, val, ok) = self.assert_ok(pos, x, te)?;
            let val = if self.types.words(&t) == 0 {
                self.frame_addr(val);
                self.spill_addr()
            } else {
                val
            };
            self.assign_from(&lhs[0], &t, val)?;
            self.assign_from(&lhs[1], &Ty::Bool, ok)?;
            self.release(m);
            return Ok(());
        }
        if rhs.len() == 1 && lhs.len() > 1 {
            let t = match &rhs[0].kind {
                ExprKind::Call(f, args, sp) => self.call(&rhs[0], f, args, *sp)?,
                ExprKind::Unary("<-", c) if lhs.len() == 2 => self.recv(c, true)?,
                _ => return err(pos, "names and values do not match"),
            };
            let Ty::Tuple(ts) = t else {
                return err(pos, "the call has one value");
            };
            let all = Ty::Tuple(ts.clone());
            let w: u32 = ts.iter().map(|t| self.types.words(t)).sum();
            let off = self.slot(2 * w);
            for k in 0..w {
                self.st(k as u8 + 1, off + 2 * k);
            }
            let _ = all;
            let mut o = off;
            for (l, t) in lhs.iter().zip(&ts) {
                self.assign_from(l, t, o)?;
                o += 2 * self.types.words(t);
            }
            return Ok(());
        }
        if rhs.len() != lhs.len() {
            return err(pos, "names and values do not match");
        }
        if lhs.len() == 1 {
            return self.assign(&lhs[0], &rhs[0]);
        }
        let wants: Vec<Option<Ty>> = lhs
            .iter()
            .map(|l| match &l.kind {
                ExprKind::Ident(n) if n == "_" => Ok(None),
                _ => self.type_peek(l).map(Some),
            })
            .collect::<CResult<_>>()?;
        let vals = self.eval_all(rhs, &wants)?;
        for (l, (t, off)) in lhs.iter().zip(vals) {
            self.assign_from(l, &t, off)?;
        }
        Ok(())
    }

    /// Evaluate values into temporaries (scalars; struct addresses).
    fn eval_all(&mut self, vals: &[Expr], wants: &[Option<Ty>]) -> CResult<Vec<(Ty, u32)>> {
        let mut out = Vec::new();
        for (v, w) in vals.iter().zip(wants) {
            let t = self.expr(v, w.as_ref())?;
            let t = match w {
                Some(w) => {
                    self.coerce(&t, w, v.pos)?;
                    w.clone()
                }
                None => t,
            };
            let off = if self.types.words(&t) == 0 {
                // copy the struct: the original may change before it is used
                let size = self.types.size(&t);
                let copy = self.slot(size.next_multiple_of(2));
                self.emit("mov     r3, r1");
                self.emit("mov     r4, r2");
                self.frame_addr(copy);
                self.memcpy(size);
                self.frame_addr(copy);
                self.spill_addr()
            } else {
                self.spill(&t)
            };
            out.push((t, off));
        }
        Ok(out)
    }

    /// `lhs = rhs`.
    pub(super) fn assign(&mut self, lhs: &Expr, rhs: &Expr) -> CResult<()> {
        if let ExprKind::Ident(n) = &lhs.kind {
            if n == "_" {
                let t = self.expr(rhs, None)?;
                if matches!(t, Ty::Void) {
                    return err(rhs.pos, "the call has no value");
                }
                return Ok(());
            }
            // A local scalar: straight into its slot.
            if let Some(Local::Var {
                off,
                ty,
                by_ref: false,
            }) = self.local(n)
            {
                if self.types.words(&ty) > 0 {
                    let vt = self.expr(rhs, Some(&ty))?;
                    self.coerce(&vt, &ty, rhs.pos)?;
                    self.store_slot(off, &ty);
                    return Ok(());
                }
            }
        }
        let lt = self.type_peek(lhs)?;
        let m = self.mark();
        let vt = match &rhs.kind {
            ExprKind::Composite(None, el) => self.composite(rhs.pos, &lt, el)?,
            _ => self.expr(rhs, Some(&lt))?,
        };
        self.coerce(&vt, &lt, rhs.pos)?;
        let v = if self.types.words(&lt) == 0 {
            self.spill_addr()
        } else {
            self.spill(&lt)
        };
        self.assign_from(lhs, &lt, v)?;
        self.release(m);
        Ok(())
    }

    /// Store the value spilled at `off` (a struct: its address) into `lhs`.
    fn assign_from(&mut self, lhs: &Expr, t: &Ty, off: u32) -> CResult<()> {
        if let ExprKind::Ident(n) = &lhs.kind {
            if n == "_" {
                return Ok(());
            }
        }
        // A value for an interface variable: converted first.
        let (t, off) = {
            let lt = self.type_peek(lhs)?;
            if self.types.is_iface(&lt) && *t != lt {
                if self.types.words(t) == 0 {
                    self.ld(1, off);
                    self.ld(2, off + 2);
                } else {
                    self.unspill(t, off, 1);
                }
                self.coerce(t, &lt, lhs.pos)?;
                let o = self.spill(&lt);
                (lt, o)
            } else {
                (t.clone(), off)
            }
        };
        let t = &t;
        // A local scalar keeps its words in register order in its slot
        // (memory through a pointer has them low word first): straight in.
        if let ExprKind::Ident(n) = &lhs.kind {
            if let Some(Local::Var {
                off: slot,
                ty,
                by_ref: false,
            }) = self.local(n)
            {
                if self.types.words(&ty) > 0 {
                    self.check_assign(t, &ty, lhs.pos)?;
                    self.unspill(&ty, off, 1);
                    self.store_slot(slot, &ty);
                    return Ok(());
                }
            }
        }
        let lt = self.addr(lhs)?;
        self.check_assign(t, &lt, lhs.pos)?;
        if self.types.words(&lt) == 0 {
            self.ld(3, off);
            self.ld(4, off + 2);
            let size = self.types.size(&lt);
            self.memcpy(size);
        } else {
            let a = self.spill_addr();
            self.unspill(&lt, off, 1);
            self.addr_to_x(a);
            self.store_x(&lt);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn range(
        &mut self,
        key: Option<&Expr>,
        value: Option<&Expr>,
        define: bool,
        over: &Expr,
        body: &Block,
        label: &Option<String>,
        pos: Pos,
    ) -> CResult<()> {
        self.fnc().scopes.push(HashMap::new());
        let ot = self.type_peek(over)?;
        if let Ty::Chan(el) = self.types.under(&ot).clone() {
            if value.is_some() {
                return err(pos, "range over a channel gives one value");
            }
            self.expr(over, None)?;
            let ch = self.spill_addr();
            let size = self.types.size(&el).max(1);
            let buf = self.slot(size.next_multiple_of(2));
            let top = self.new_label();
            let brk = self.new_label();
            self.label(&top);
            self.ld(1, ch);
            self.ld(2, ch + 2);
            self.frame_addr_pub(buf, 3);
            self.rt("rt_chan_recv");
            self.jump_if_false(&brk);
            if let Some(k) = key {
                let s = if self.types.words(&el) == 0 {
                    self.frame_addr(buf);
                    self.spill_addr()
                } else {
                    self.x_frame(buf);
                    self.load_x(&el);
                    self.spill(&el)
                };
                self.range_var(k, &el, s, define)?;
            }
            self.fnc().loops.push(LoopLabels {
                name: label.clone(),
                brk: brk.clone(),
                cont: Some(top.clone()),
            });
            self.block(body)?;
            self.fnc().loops.pop();
            self.jump(&top);
            self.label(&brk);
            self.fnc().scopes.pop();
            return Ok(());
        }
        // The collection, its length; the index.
        let (elem, is_str, base, len) = match self.types.under(&ot).clone() {
            Ty::UntypedInt | Ty::I8 | Ty::U8 | Ty::I16 | Ty::U16 => {
                self.expr(over, Some(&Ty::I16))?;
                let len = self.spill(&Ty::I16);
                (None, false, 0, len)
            }
            Ty::Slice(el) => {
                self.expr(over, None)?;
                let b = self.spill(&ot);
                (Some(*el), false, b, b + 4)
            }
            Ty::Str => {
                self.expr(over, None)?;
                let b = self.spill(&ot);
                (Some(Ty::U8), true, b, b + 4)
            }
            Ty::Array(n, el) => {
                self.addr(over)?;
                let b = self.slot(8);
                self.st(1, b);
                self.st(2, b + 2);
                self.li(1, i64::from(n));
                self.st(1, b + 4);
                (Some(*el), false, b, b + 4)
            }
            Ty::Ptr(inner) if matches!(self.types.under(&inner), Ty::Array(..)) => {
                let Ty::Array(n, el) = self.types.under(&inner).clone() else {
                    unreachable!()
                };
                self.expr(over, None)?;
                let b = self.slot(8);
                self.st(1, b);
                self.st(2, b + 2);
                self.li(1, i64::from(n));
                self.st(1, b + 4);
                (Some(*el), false, b, b + 4)
            }
            _ => return err(pos, format!("cannot range over {}", self.types.name(&ot))),
        };
        let i = self.slot(2);
        self.emit("movi    r1, #0");
        self.st(1, i);
        let top = self.new_label();
        let cont = self.new_label();
        let brk = self.new_label();
        self.label(&top);
        self.ld(1, i);
        self.ld(3, len);
        self.emit("cmp     r1, r3");
        let go = self.new_label();
        self.emit(format!("blt     {go}"));
        self.jump(&brk);
        self.label(&go);
        // key = i
        let key_ty = if elem.is_none() && !matches!(self.types.under(&ot), Ty::UntypedInt) {
            ot.clone()
        } else {
            Ty::I16
        };
        if let Some(k) = key {
            self.ld(1, i);
            let s = self.spill(&key_ty);
            self.range_var(k, &key_ty, s, define)?;
        }
        if let (Some(v), Some(el)) = (value, &elem) {
            // element i: base pointer + i * size
            let size = if is_str { 1 } else { self.types.size(el) };
            self.ld(1, i);
            self.li(3, i64::from(size));
            self.emit("mov     r4, r1");
            self.emit("mul     r4, r3");
            self.emit("mulhu   r1, r3");
            self.emit("mov     r3, r1");
            self.ld(1, base);
            self.ld(2, base + 2);
            self.emit("add     r2, r4");
            self.emit("adc     r1, r3");
            let s = if self.types.words(el) == 0 {
                self.spill_addr()
            } else {
                self.addr_load(el);
                self.spill(el)
            };
            self.range_var(v, el, s, define)?;
        }
        self.fnc().loops.push(LoopLabels {
            name: label.clone(),
            brk: brk.clone(),
            cont: Some(cont.clone()),
        });
        self.block(body)?;
        self.fnc().loops.pop();
        self.label(&cont);
        self.ld(1, i);
        self.emit("addi    r1, #1");
        self.st(1, i);
        self.jump(&top);
        self.label(&brk);
        self.fnc().scopes.pop();
        Ok(())
    }

    /// The key or value variable of a range loop from the temporary `s`.
    fn range_var(&mut self, x: &Expr, t: &Ty, s: u32, define: bool) -> CResult<()> {
        if let ExprKind::Ident(n) = &x.kind {
            if n == "_" {
                return Ok(());
            }
            if define {
                let size = self.types.size(t).max(1);
                let off = match self.local(n) {
                    Some(Local::Var { off, .. })
                        if self
                            .f
                            .as_ref()
                            .unwrap()
                            .scopes
                            .last()
                            .unwrap()
                            .contains_key(n) =>
                    {
                        off
                    }
                    _ => {
                        let off = self.slot(size.next_multiple_of(2));
                        self.declare_local(
                            n,
                            Local::Var {
                                off,
                                ty: t.clone(),
                                by_ref: false,
                            },
                        );
                        off
                    }
                };
                if self.types.words(t) == 0 {
                    self.ld(3, s);
                    self.ld(4, s + 2);
                    self.frame_addr(off);
                    self.memcpy(size);
                } else {
                    self.unspill(t, s, 1);
                    self.store_slot(off, t);
                }
                return Ok(());
            }
        }
        self.assign_from(x, t, s)
    }
}

/// Does the statement list end in a statement that never falls through?
pub(super) fn terminates(stmts: &[Stmt]) -> bool {
    match stmts.last() {
        Some(Stmt::Return(..)) => true,
        Some(Stmt::Block(b)) => terminates(&b.stmts),
        Some(Stmt::If {
            then, els: Some(e), ..
        }) => terminates(&then.stmts) && terminates(std::slice::from_ref(e)),
        Some(Stmt::For { cond: None, .. }) => true,
        // Go: a switch with a default whose every case ends in a
        // terminating statement (a `break` out of it would be one that
        // does not; such a body ends in the break, not a return).
        Some(Stmt::Switch { cases, .. }) => {
            cases.iter().any(|c| c.default) && cases.iter().all(|c| terminates(&c.body))
        }
        Some(Stmt::TypeSwitch { cases, .. }) => {
            cases.iter().any(|c| c.default) && cases.iter().all(|c| terminates(&c.body))
        }
        Some(Stmt::Expr(Expr {
            kind: ExprKind::Call(f, _, _),
            ..
        })) => matches!(&f.kind, ExprKind::Ident(n) if n == "panic"),
        _ => false,
    }
}
