//! Packages, declarations and the frames of functions (BRASS.md 2, 6, 11).

use super::*;

/// A function body to translate.
enum Body<'a> {
    Block(&'a Block),
    /// A package's initialisation: its variables, then its `init`s.
    Init(&'a [(String, Ty, Option<Expr>, bool)], &'a [String]),
}

impl Compiler {
    /// Translate the packages (in dependency order: imports first).
    pub fn compile(&mut self, pkgs: &[PackageSrc]) -> CResult<()> {
        for p in pkgs {
            self.package(p)?;
        }
        self.finish_ifaces()
    }

    fn set_file(&mut self, fid: u16) {
        self.file = fid;
    }

    fn package(&mut self, p: &PackageSrc) -> CResult<()> {
        self.pkg = p.path.clone();
        self.pkgs.entry(p.path.clone()).or_default();
        // Imports per file.
        for (fid, file) in &p.files {
            let mut m = HashMap::new();
            for imp in &file.imports {
                if !self.pkgs.contains_key(&imp.path) {
                    return err(imp.pos, format!("package {} not found", imp.path));
                }
                let name = imp.name.clone().unwrap_or_else(|| {
                    imp.path.rsplit('/').next().unwrap_or(&imp.path).to_string()
                });
                m.insert(name, imp.path.clone());
            }
            self.imports.insert(*fid, m);
        }
        // Types: names first (they may refer to each other), then bodies.
        for (fid, file) in &p.files {
            self.set_file(*fid);
            for d in &file.decls {
                if let Decl::Type(specs) = d {
                    for s in specs.iter().filter(|s| !s.alias) {
                        self.types.named.push(Named {
                            pkg: p.path.clone(),
                            name: s.name.clone(),
                            under: Ty::Void,
                            methods: Vec::new(),
                        });
                        let i = self.types.named.len() - 1;
                        self.insert_obj(&s.name, s.pos, Obj::Type(Ty::Named(i)))?;
                    }
                }
            }
        }
        // Type bodies: those that need constants (array lengths) wait until
        // the constants are in; constants need the types they name.
        let mut later = Vec::new();
        for (fid, file) in &p.files {
            self.set_file(*fid);
            for d in &file.decls {
                if let Decl::Type(specs) = d {
                    for s in specs {
                        if self.resolve_type(&s.ty).is_err() {
                            later.push((*fid, s));
                            continue;
                        }
                        self.type_body(s)?;
                    }
                }
            }
        }
        self.constants(p)?;
        for (fid, s) in later {
            self.set_file(fid);
            self.type_body(s)?;
        }
        // Functions and methods: their signatures.
        let mut inits = Vec::new();
        for (fid, file) in &p.files {
            self.set_file(*fid);
            for d in &file.decls {
                let Decl::Func(f) = d else { continue };
                let mut sig = self.resolve_sig(&f.sig)?;
                match &f.recv {
                    None if f.name == "init" => {
                        if !sig.params.is_empty() || !sig.results.is_empty() {
                            return err(f.pos, "func init() takes and returns nothing");
                        }
                        inits.push(format!("{}_{}", mangle(&p.path, "init"), inits.len()));
                    }
                    None => {
                        let sym = mangle(&p.path, &f.name);
                        self.insert_obj(
                            &f.name,
                            f.pos,
                            Obj::Func {
                                sym,
                                sig,
                                asm: f.body.is_none(),
                            },
                        )?;
                    }
                    Some((_, tname, by_ptr)) => {
                        let Some(Obj::Type(Ty::Named(i))) =
                            self.pkgs[&self.pkg].get(tname).cloned()
                        else {
                            return err(f.pos, format!("{tname} is not a type of this package"));
                        };
                        if self.types.is_iface(&Ty::Named(i)) {
                            return err(
                                f.pos,
                                format!("invalid receiver type {tname} (an interface)"),
                            );
                        }
                        let recv_t = if *by_ptr {
                            Ty::Ptr(Box::new(Ty::Named(i)))
                        } else {
                            Ty::Named(i)
                        };
                        sig.params.insert(0, recv_t);
                        if self.types.named[i].methods.iter().any(|m| m.0 == f.name) {
                            return err(f.pos, format!("method {tname}.{} twice", f.name));
                        }
                        if self.types.field(&Ty::Named(i), &f.name).is_some() {
                            return err(f.pos, format!("{tname} has a field {}", f.name));
                        }
                        let sym = mangle(&p.path, &format!("{tname}__{}", f.name));
                        self.types.named[i]
                            .methods
                            .push((f.name.clone(), sym, *by_ptr, sig));
                    }
                }
            }
        }
        // Package variables: their types and storage.
        let mut var_inits: Vec<(String, Ty, Option<Expr>, bool)> = Vec::new();
        for (fid, file) in &p.files {
            self.set_file(*fid);
            for d in &file.decls {
                let Decl::Var(specs) = d else { continue };
                for spec in specs {
                    let declared = match &spec.ty {
                        Some(t) => Some(self.resolve_type(t)?),
                        None => None,
                    };
                    if spec.values.len() > 1 && spec.values.len() != spec.names.len() {
                        return err(spec.names[0].1, "names and values do not match");
                    }
                    if spec.values.len() == 1 && spec.names.len() > 1 {
                        return err(spec.names[0].1, "package variables from one call with several results are not in Brass v1");
                    }
                    for (k, (name, pos)) in spec.names.iter().enumerate() {
                        let v = spec.values.get(k).cloned();
                        let t = match (&declared, &v) {
                            (Some(t), _) => t.clone(),
                            (None, Some(v)) => {
                                let t = self.scratch_type(v)?;
                                if matches!(t, Ty::UntypedNil | Ty::Void | Ty::Tuple(_)) {
                                    return err(
                                        *pos,
                                        format!("cannot declare {name} from this value"),
                                    );
                                }
                                self.types.default_of(&t)
                            }
                            (None, None) => return err(*pos, "a variable needs a type or a value"),
                        };
                        let sym = format!("v_{}", mangle(&p.path, name));
                        let size = self.types.size(&t).max(1);
                        let _ = writeln!(
                            self.data,
                            "        .align  2\n{sym}:\n        .space  {size}"
                        );
                        self.insert_obj(
                            name,
                            *pos,
                            Obj::Var {
                                sym: sym.clone(),
                                ty: t.clone(),
                            },
                        )?;
                        var_inits.push((sym, t, v, true));
                        let _ = fid;
                    }
                }
            }
        }
        // The functions.
        let mut init_k = 0;
        for (fid, file) in &p.files {
            self.set_file(*fid);
            for d in &file.decls {
                let Decl::Func(f) = d else { continue };
                let Some(body) = &f.body else {
                    if f.recv.is_some() || f.name == "init" {
                        return err(f.pos, "only plain functions may be in assembly");
                    }
                    continue;
                };
                let (sym, sig, recv) = match &f.recv {
                    None if f.name == "init" => {
                        let s = inits[init_k].clone();
                        init_k += 1;
                        (
                            s,
                            Sig {
                                params: vec![],
                                results: vec![],
                            },
                            None,
                        )
                    }
                    None => match &self.pkgs[&self.pkg][&f.name] {
                        Obj::Func { sym, sig, .. } => (sym.clone(), sig.clone(), None),
                        _ => unreachable!(),
                    },
                    Some((rname, tname, _)) => {
                        let Some(Obj::Type(Ty::Named(i))) =
                            self.pkgs[&self.pkg].get(tname).cloned()
                        else {
                            unreachable!()
                        };
                        let m = self.types.named[i]
                            .methods
                            .iter()
                            .find(|m| m.0 == f.name)
                            .cloned()
                            .unwrap();
                        let recv_t = m.3.params[0].clone();
                        (m.1, m.3, Some((rname.clone(), recv_t)))
                    }
                };
                self.gen_function(&sym, recv, &f.sig, &sig, body, f.pos)?;
            }
        }
        if p.path == "main" && !matches!(self.pkgs["main"].get("main"), Some(Obj::Func { .. })) {
            return err(Pos::default(), "package main has no func main");
        }
        // The package's initialisation.
        let sym = format!("init__{}", mangle(&p.path, "pkg"));
        self.set_file(p.files.first().map(|f| f.0).unwrap_or(0));
        self.gen_body(
            &sym,
            None,
            &[],
            &Sig {
                params: vec![],
                results: vec![],
            },
            Body::Init(&var_inits, &inits),
            Pos::default(),
        )?;
        self.inits.push(sym);
        let code = std::mem::take(&mut self.code);
        self.pkg_code.push((p.path.clone(), code));
        Ok(())
    }

    /// The type of `v` in a throwaway function (package-level values).
    fn insert_obj(&mut self, name: &str, pos: Pos, o: Obj) -> CResult<()> {
        let scope = self.pkgs.get_mut(&self.pkg).unwrap();
        if scope.contains_key(name) && name != "_" {
            return err(pos, format!("{name} declared twice"));
        }
        scope.insert(name.to_string(), o);
        Ok(())
    }

    /// A type declaration's body (its name is already declared).
    fn type_body(&mut self, s: &TypeSpec) -> CResult<()> {
        let t = self.resolve_type(&s.ty)?;
        if s.alias {
            return self.insert_obj(&s.name, s.pos, Obj::Type(t));
        }
        let Some(Obj::Type(Ty::Named(i))) = self.pkgs[&self.pkg].get(&s.name).cloned() else {
            unreachable!()
        };
        let under = self.types.under(&t).clone();
        if under == Ty::Named(i) || under == Ty::Void {
            return err(s.pos, format!("invalid recursive type {}", s.name));
        }
        self.types.named[i].under = under;
        Ok(())
    }

    /// The package's constants, in order.
    fn constants(&mut self, p: &PackageSrc) -> CResult<()> {
        for (fid, file) in &p.files {
            self.set_file(*fid);
            for d in &file.decls {
                if let Decl::Const(specs) = d {
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
                            self.insert_obj(name, *pos, Obj::Const { ty: t, val: c })?;
                        }
                        self.iota = None;
                    }
                }
            }
        }
        Ok(())
    }

    fn scratch_type(&mut self, v: &Expr) -> CResult<Ty> {
        let saved = self.f.take();
        self.f = Some(Fn {
            scopes: vec![HashMap::new()],
            size: 0,
            max: 0,
            out: 0,
            out_need: 0,
            results: Vec::new(),
            loops: Vec::new(),
            ret_label: String::new(),
            body: String::new(),
            addressed: Default::default(),
        });
        let labels = self.labels;
        let data = self.data.len();
        let code = self.code.len();
        let strings = self.strings.clone();
        let r = self.expr(v, None);
        self.code.truncate(code);
        self.labels = labels;
        self.data.truncate(data);
        self.strings = strings;
        self.f = saved;
        r
    }

    pub(super) fn gen_function(
        &mut self,
        sym: &str,
        recv: Option<(String, Ty)>,
        ast_sig: &FuncType,
        sig: &Sig,
        body: &Block,
        pos: Pos,
    ) -> CResult<()> {
        let mut names: Vec<String> = Vec::new();
        if let Some((n, _)) = &recv {
            names.push(n.clone());
        }
        for p in &ast_sig.params {
            names.push(p.name.clone().unwrap_or_default());
        }
        if !sig.results.is_empty() && !stmt::terminates(&body.stmts) {
            return err(body.end, "missing return at the end of the function");
        }
        self.gen_body(sym, recv.map(|r| r.1), &names, sig, Body::Block(body), pos)
    }

    fn gen_body(
        &mut self,
        sym: &str,
        _recv: Option<Ty>,
        names: &[String],
        sig: &Sig,
        body: Body,
        pos: Pos,
    ) -> CResult<()> {
        // Twice when calls need an outgoing area: its size moves every slot.
        let mut out = 0;
        let code_len = self.code.len();
        loop {
            let labels = self.labels;
            self.f = Some(Fn {
                scopes: vec![HashMap::new()],
                size: 0,
                max: 0,
                out,
                out_need: 0,
                results: sig.results.clone(),
                loops: Vec::new(),
                ret_label: String::new(),
                body: String::new(),
                addressed: match &body {
                    Body::Block(b) => crate::ast::addressed(b),
                    Body::Init(..) => Default::default(),
                },
            });
            let ret = self.new_label();
            self.fnc().ret_label = ret.clone();
            // Parameters: their words come in R1-R4, then the caller's area.
            let mut word = 0u32;
            let mut moves: Vec<(u32, u32)> = Vec::new(); // (argument word, frame offset)
            for (k, t) in sig.params.iter().enumerate() {
                let name = names.get(k).cloned().unwrap_or_default();
                let by_ref = self.types.words(t) == 0;
                let w = if by_ref { 2 } else { self.types.words(t) };
                let off = self.slot(2 * w);
                for j in 0..w {
                    moves.push((word + j, off + 2 * j));
                }
                word += w;
                if !name.is_empty() {
                    self.declare_local(
                        &name,
                        Local::Var {
                            off,
                            ty: t.clone(),
                            by_ref,
                        },
                    );
                }
            }
            match &body {
                Body::Block(b) => {
                    for s in &b.stmts {
                        self.stmt(s)?;
                    }
                }
                Body::Init(vars, inits) => {
                    for (vsym, t, v, _) in vars.iter() {
                        let Some(v) = v else { continue };
                        let m = self.mark();
                        let vt = match &v.kind {
                            ExprKind::Composite(None, el) => self.composite(v.pos, t, el)?,
                            _ => self.expr(v, Some(t))?,
                        };
                        self.coerce(&vt, t, v.pos)?;
                        if self.types.words(t) == 0 {
                            self.emit("mov     r3, r1");
                            self.emit("mov     r4, r2");
                            self.emit(format!("li      r1, #seg({vsym})"));
                            self.emit(format!("li      r2, #off({vsym})"));
                            let size = self.types.size(t);
                            self.memcpy(size);
                        } else {
                            self.emit(format!("li      r10, #seg({vsym})"));
                            self.emit("mts     s3, r10");
                            self.emit(format!("li      r9, #off({vsym})"));
                            self.store_x(t);
                        }
                        self.release(m);
                    }
                    for i in inits.iter() {
                        self.emit(format!("li      r9, #seg({i})"));
                        self.emit(format!("li      r10, #off({i})"));
                        self.emit("callf   r9:r10");
                    }
                }
            }
            let f = self.f.take().unwrap();
            if f.out_need > out {
                out = f.out_need;
                self.labels = labels;
                self.code.truncate(code_len);
                continue;
            }
            let frame = (f.out + f.max).next_multiple_of(2);
            let mut code = String::new();
            let _ = writeln!(code, "\n; {sym}\n{sym}:");
            let line = |c: &mut String, s: &str| {
                let _ = writeln!(c, "        {s}");
            };
            line(&mut code, "addi    r12, #-6");
            line(&mut code, "sw      r9, 0(r12)");
            line(&mut code, "sw      r10, 2(r12)");
            line(&mut code, "sw      r11, 4(r12)");
            if frame > 0 {
                if frame <= 127 {
                    line(&mut code, &format!("addi    r12, #-{frame}"));
                } else {
                    line(&mut code, &format!("li      r9, #{frame}"));
                    line(&mut code, "sub     r12, r9");
                }
            }
            // Parameter words into their slots.
            self.f = Some(Fn {
                scopes: vec![],
                size: 0,
                max: 0,
                out: 0,
                out_need: 0,
                results: vec![],
                loops: vec![],
                ret_label: String::new(),
                body: String::new(),
                addressed: Default::default(),
            });
            for &(w, off) in &moves {
                if w < 4 {
                    self.st(w as u8 + 1, off);
                } else {
                    self.ld(10, frame + 6 + 2 * (w - 4));
                    self.st(10, off);
                }
            }
            let moves_code = self.f.take().unwrap().body;
            code.push_str(&moves_code);
            code.push_str(&f.body);
            let _ = writeln!(code, "{ret}:");
            if frame > 0 {
                if frame <= 127 {
                    line(&mut code, &format!("addi    r12, #{frame}"));
                } else {
                    line(&mut code, &format!("li      r9, #{frame}"));
                    line(&mut code, "add     r12, r9");
                }
            }
            line(&mut code, "lw      r9, 0(r12)");
            line(&mut code, "lw      r10, 2(r12)");
            line(&mut code, "lw      r11, 4(r12)");
            line(&mut code, "addi    r12, #6");
            line(&mut code, "retf");
            self.code.push_str(&code);
            let _ = pos;
            return Ok(());
        }
    }
}
