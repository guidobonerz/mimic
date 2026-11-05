//! The assembler: source text (with `.include` and macros) to an
//! [`Object`]. Two passes: the first lays out sections and defines the
//! symbols, the second encodes (through `cpu_ada::isa`) and leaves what
//! only the linker knows as relocations.
//!
//! Syntax (SPEC.md section 12): one statement a line, `;` comments,
//! `label:` (labels starting with `.` are local to the last other label),
//! instructions as the disassembler prints them (`add r1, r2`,
//! `addi r1, #-1`, `lw r1, 4(r2)`, `lwx r1, (r2)+`, `beq label`,
//! `jf r1:r2`, `mfsc r1, #3`), pseudo-instructions `li rd, #value`
//! (MOVI + LUI), `ret`, `retf`, `push rx`, `pop rx`, and the directives
//! `.section .org .align .word .byte .far .ascii .asciz .space .equ .set
//! .global .include .macro/.endm` (`\name` parameters, `\@` a counter).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cpu_ada::isa::{self, Imm, Mem, Op, Xop};

use crate::expr::{self, Base, Sel, Symbols, Val};
use crate::object::{Field, ListLine, Object, Reloc, Section, Target};

/// Reads included files.
pub trait Files {
    fn read(&self, path: &Path) -> Result<String, String>;
}

/// The host's file system.
pub struct HostFiles;

impl Files for HostFiles {
    fn read(&self, path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// A source line after includes and macros.
#[derive(Clone, Debug)]
struct Line {
    file: String,
    num: usize,
    text: String,
}

impl Line {
    fn at(&self) -> String {
        format!("{}:{}", self.file, self.num)
    }
}

struct Macro {
    params: Vec<String>,
    body: Vec<Line>,
}

/// Assemble `src` (named `name`, includes relative to its folder).
pub fn assemble(name: &str, src: &str, files: &dyn Files) -> Result<Object, Vec<String>> {
    let mut errors = Vec::new();
    let mut lines = Vec::new();
    let mut macros = BTreeMap::new();
    let mut counter = 0;
    expand(
        Path::new(name),
        src,
        files,
        &mut macros,
        &mut counter,
        &mut lines,
        &mut errors,
        0,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut a = Asm {
        object: Object {
            name: name.to_string(),
            ..Object::default()
        },
        current: None,
        labels: BTreeMap::new(),
        equs: BTreeMap::new(),
        globals: BTreeSet::new(),
        scope: String::new(),
        errors,
        pass: 1,
        evaluating: std::cell::RefCell::new(Vec::new()),
    };
    for pass in 1..=2 {
        a.pass = pass;
        a.current = None;
        a.scope.clear();
        for s in &mut a.object.sections {
            s.bytes.clear();
            s.relocs.clear();
            s.listing.clear();
        }
        for l in &lines {
            if let Err(e) = a.statement(l) {
                a.errors.push(format!("{}: {e}", l.at()));
            }
        }
        if !a.errors.is_empty() {
            return Err(a.errors);
        }
    }
    for g in &a.globals {
        match a.labels.get(g) {
            Some(&(sec, off)) => a.object.globals.push((g.clone(), sec, off)),
            None => a.errors.push(format!("{name}: .global {g}: no such label")),
        }
    }
    if a.errors.is_empty() {
        Ok(a.object)
    } else {
        Err(a.errors)
    }
}

/// Strip a `;` comment (not inside quotes).
fn strip_comment(s: &str) -> &str {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, ';') => return &s[..i],
            _ => {}
        }
    }
    s
}

/// Split operands at top-level commas.
fn split_operands(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut cur) = (0, None, String::new());
    for c in s.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                cur.push(c);
            }
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (None, '(') => {
                depth += 1;
                cur.push(c);
            }
            (None, ')') => {
                depth -= 1;
                cur.push(c);
            }
            (None, ',') if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() || !out.is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Head word (mnemonic) and the rest.
fn head(s: &str) -> (String, String) {
    let s = s.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (s[..i].to_ascii_lowercase(), s[i..].trim().to_string()),
        None => (s.to_ascii_lowercase(), String::new()),
    }
}

/// Split `label:` off a line.
fn split_label(s: &str) -> (Option<String>, String) {
    let t = s.trim_start();
    let end = t
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '@'))
        .unwrap_or(t.len());
    if end > 0 && t[end..].starts_with(':') && !t[..end].starts_with(|c: char| c.is_ascii_digit()) {
        (Some(t[..end].to_string()), t[end + 1..].to_string())
    } else {
        (None, t.to_string())
    }
}

#[allow(clippy::too_many_arguments)]
fn expand(
    path: &Path,
    src: &str,
    files: &dyn Files,
    macros: &mut BTreeMap<String, Macro>,
    counter: &mut usize,
    out: &mut Vec<Line>,
    errors: &mut Vec<String>,
    depth: usize,
) {
    if depth > 32 {
        errors.push(format!(
            "{}: includes or macros nested too deep",
            path.display()
        ));
        return;
    }
    let file = path.display().to_string();
    let lines: Vec<Line> = src
        .lines()
        .enumerate()
        .map(|(i, t)| Line {
            file: file.clone(),
            num: i + 1,
            text: t.to_string(),
        })
        .collect();
    expand_lines(path, &lines, files, macros, counter, out, errors, depth);
}

#[allow(clippy::too_many_arguments)]
fn expand_lines(
    path: &Path,
    lines: &[Line],
    files: &dyn Files,
    macros: &mut BTreeMap<String, Macro>,
    counter: &mut usize,
    out: &mut Vec<Line>,
    errors: &mut Vec<String>,
    depth: usize,
) {
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        i += 1;
        let code = strip_comment(&l.text);
        let (label, rest) = split_label(code);
        let (word, args) = head(&rest);
        match word.as_str() {
            ".macro" => {
                if let Some(lab) = &label {
                    out.push(Line {
                        text: format!("{lab}:"),
                        ..l.clone()
                    });
                }
                let ops = split_operands(&args);
                let Some((name, params)) = ops.split_first().and_then(|(f, p)| {
                    let (n, first) = head(f);
                    let mut ps: Vec<String> = Vec::new();
                    if !first.is_empty() {
                        ps.push(first);
                    }
                    ps.extend(p.iter().cloned());
                    (!n.is_empty()).then_some((n, ps))
                }) else {
                    errors.push(format!("{}: .macro needs a name", l.at()));
                    continue;
                };
                let mut body = Vec::new();
                let mut closed = false;
                while i < lines.len() {
                    let b = &lines[i];
                    i += 1;
                    if head(strip_comment(&b.text)).0 == ".endm" {
                        closed = true;
                        break;
                    }
                    body.push(b.clone());
                }
                if !closed {
                    errors.push(format!("{}: .macro {name} without .endm", l.at()));
                }
                macros.insert(name, Macro { params, body });
            }
            ".include" => {
                let file = args.trim().trim_matches('"');
                let p: PathBuf = path.parent().unwrap_or(Path::new("")).join(file);
                match files.read(&p) {
                    Ok(src) => {
                        if let Some(lab) = label {
                            out.push(Line {
                                text: format!("{lab}:"),
                                ..l.clone()
                            });
                        }
                        expand(&p, &src, files, macros, counter, out, errors, depth + 1);
                    }
                    Err(e) => errors.push(format!("{}: {e}", l.at())),
                }
            }
            w if macros.contains_key(w) => {
                let m = &macros[w];
                let actual = split_operands(&args);
                if actual.len() > m.params.len() {
                    errors.push(format!(
                        "{}: {w}: {} arguments, {} expected",
                        l.at(),
                        actual.len(),
                        m.params.len()
                    ));
                    continue;
                }
                *counter += 1;
                let n = *counter;
                let body: Vec<Line> = m
                    .body
                    .iter()
                    .map(|b| {
                        let mut t = b.text.replace("\\@", &n.to_string());
                        // longest names first, so \ab is not replaced as \a
                        let mut ps: Vec<(usize, &String)> = m.params.iter().enumerate().collect();
                        ps.sort_by_key(|(_, p)| std::cmp::Reverse(p.len()));
                        for (k, p) in ps {
                            let v = actual.get(k).map(String::as_str).unwrap_or("");
                            t = t.replace(&format!("\\{p}"), v);
                        }
                        Line {
                            file: l.file.clone(),
                            num: l.num,
                            text: t,
                        }
                    })
                    .collect();
                if let Some(lab) = label {
                    out.push(Line {
                        text: format!("{lab}:"),
                        ..l.clone()
                    });
                }
                expand_lines(path, &body, files, macros, counter, out, errors, depth + 1);
            }
            _ => out.push(l.clone()),
        }
    }
}

/// An `.equ`: its expression and the local-label scope it was written in.
enum Equ {
    Text(String, String),
}

struct Asm {
    object: Object,
    current: Option<usize>,
    /// Label → (section, offset).
    labels: BTreeMap<String, (usize, u32)>,
    equs: BTreeMap<String, Equ>,
    globals: BTreeSet<String>,
    /// The last label not starting with `.` (scope of local labels).
    scope: String,
    errors: Vec<String>,
    pass: u8,
    evaluating: std::cell::RefCell<Vec<String>>,
}

impl Symbols for Asm {
    fn lookup(&self, name: &str) -> Result<Val, String> {
        let name = self.qualify(name);
        // Labels stay relative to their section, also in fixed ones: the
        // linker puts the address in (so `li`, `.word` take its offset).
        if let Some(&(sec, off)) = self.labels.get(&name) {
            return Ok(Val::Rel {
                base: Base::Section(sec),
                add: i64::from(off),
                sel: Sel::Lin,
            });
        }
        if let Some(Equ::Text(text, scope)) = self.equs.get(&name) {
            if self.evaluating.borrow().contains(&name) {
                return Err(format!("`{name}` refers to itself"));
            }
            self.evaluating.borrow_mut().push(name.clone());
            let saved = scope.clone();
            let r = {
                let sub = ScopedLookup {
                    asm: self,
                    scope: saved,
                };
                expr::eval_str(text, &sub)
            };
            self.evaluating.borrow_mut().pop();
            return r;
        }
        if self.pass == 1 {
            return Err(format!("`{name}` is not defined (yet)"));
        }
        Ok(Val::Rel {
            base: Base::Extern(name),
            add: 0,
            sel: Sel::Lin,
        })
    }
}

/// Looks symbols up with another local-label scope (for `.equ` bodies).
struct ScopedLookup<'a> {
    asm: &'a Asm,
    scope: String,
}

impl Symbols for ScopedLookup<'_> {
    fn lookup(&self, name: &str) -> Result<Val, String> {
        if name.starts_with('.') {
            let q = format!("{}{name}", self.scope);
            return self.asm.lookup(&q);
        }
        self.asm.lookup(name)
    }
}

fn reg(s: &str) -> Result<u8, String> {
    let t = s.trim().to_ascii_lowercase();
    match t.as_str() {
        "sp" => return Ok(12),
        "lr" => return Ok(11),
        _ => {}
    }
    t.strip_prefix('r')
        .and_then(|n| n.parse::<u8>().ok())
        .filter(|n| *n < isa::REGS)
        .ok_or_else(|| format!("register expected, got `{s}`"))
}

fn segreg(s: &str) -> Result<u8, String> {
    let t = s.trim().to_ascii_lowercase();
    t.strip_prefix('s')
        .and_then(|n| n.parse::<u8>().ok())
        .filter(|n| *n < 4)
        .ok_or_else(|| format!("segment register expected, got `{s}`"))
}

fn want(ops: &[String], n: usize, m: &str) -> Result<(), String> {
    if ops.len() == n {
        Ok(())
    } else {
        Err(format!("{m}: {n} operand(s) expected, {} given", ops.len()))
    }
}

fn hash(s: &str) -> Result<&str, String> {
    s.trim()
        .strip_prefix('#')
        .ok_or_else(|| format!("`#value` expected, got `{s}`"))
}

/// Where an instruction's immediate comes from.
enum ImmVal {
    Value(u8),
    Reloc(Val),
}

impl Asm {
    fn qualify(&self, name: &str) -> String {
        if name.starts_with('.') && !self.scope.is_empty() {
            format!("{}{name}", self.scope)
        } else {
            name.to_string()
        }
    }

    fn section(&mut self) -> usize {
        if let Some(c) = self.current {
            return c;
        }
        self.switch_section("text")
    }

    fn switch_section(&mut self, name: &str) -> usize {
        let i = match self.object.sections.iter().position(|s| s.name == name) {
            Some(i) => i,
            None => {
                self.object.sections.push(Section {
                    name: name.to_string(),
                    ..Section::default()
                });
                self.object.sections.len() - 1
            }
        };
        self.current = Some(i);
        i
    }

    fn here(&mut self) -> (usize, u32) {
        let s = self.section();
        (s, self.object.sections[s].bytes.len() as u32)
    }

    fn emit(&mut self, bytes: &[u8]) {
        let s = self.section();
        self.object.sections[s].bytes.extend_from_slice(bytes);
    }

    fn word(&mut self, w: u16) {
        self.emit(&w.to_le_bytes());
    }

    fn reloc(&mut self, at: u32, field: Field, v: Val, line: &Line) -> Result<(), String> {
        let s = self.section();
        let (target, add, sel) = match v {
            Val::Abs(x) => (Target::Absolute, x, Sel::Lin),
            Val::Rel { base, add, sel } => (
                match base {
                    Base::Section(i) => Target::Section(i),
                    Base::Extern(n) => Target::Symbol(n),
                },
                add,
                sel,
            ),
        };
        self.object.sections[s].relocs.push(Reloc {
            at,
            field,
            target,
            add,
            sel,
            line: format!("{}: {}", line.at(), line.text.trim()),
        });
        Ok(())
    }

    fn value(&self, s: &str) -> Result<Val, String> {
        expr::eval_str(s, self)
    }

    fn absolute(&self, s: &str) -> Result<i64, String> {
        self.value(s)?
            .abs()
            .ok_or_else(|| format!("`{s}` must be a constant here"))
    }

    fn statement(&mut self, l: &Line) -> Result<(), String> {
        let code = strip_comment(&l.text).to_string();
        let (label, rest) = split_label(&code);
        if let Some(lab) = label {
            self.define_label(&lab)?;
        }
        let rest = rest.trim();
        if rest.is_empty() {
            return Ok(());
        }
        // name = expr
        if let Some((n, e)) = rest.split_once('=') {
            let n = n.trim();
            if !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                && !n.contains(char::is_whitespace)
            {
                return self.define_equ(n, e.trim());
            }
        }
        let (word, args) = head(rest);
        let start = self.here();
        let r = if word.starts_with('.') {
            self.directive(&word, &args, l)
        } else {
            self.instruction(&word, &args, l)
        };
        let end = self.here();
        if self.pass == 2 && end.0 == start.0 && end.1 > start.1 {
            self.object.sections[end.0].listing.push(ListLine {
                at: start.1,
                len: end.1 - start.1,
                text: l.text.trim_end().to_string(),
            });
        }
        r
    }

    fn define_label(&mut self, lab: &str) -> Result<(), String> {
        if !lab.starts_with('.') {
            self.scope = lab.to_string();
        }
        let name = self.qualify(lab);
        let at = self.here();
        if self.pass == 1 {
            if self.labels.contains_key(&name) || self.equs.contains_key(&name) {
                return Err(format!("`{name}` defined twice"));
            }
            self.labels.insert(name, at);
        } else if self.labels.get(&name) != Some(&at) {
            return Err(format!("`{name}` moved between passes"));
        }
        Ok(())
    }

    fn define_equ(&mut self, n: &str, e: &str) -> Result<(), String> {
        if self.pass == 1 {
            let name = self.qualify(n);
            if self.labels.contains_key(&name) || self.equs.contains_key(&name) {
                return Err(format!("`{name}` defined twice"));
            }
            self.equs
                .insert(name, Equ::Text(e.to_string(), self.scope.clone()));
        }
        Ok(())
    }

    fn directive(&mut self, word: &str, args: &str, l: &Line) -> Result<(), String> {
        let ops = split_operands(args);
        match word {
            ".section" => {
                want(&ops, 1, word)?;
                self.switch_section(&ops[0]);
            }
            ".org" => {
                want(&ops, 1, word)?;
                let target = parse_address(&ops[0], self)?;
                let s = self.section();
                let sec = &mut self.object.sections[s];
                match sec.origin {
                    None if sec.bytes.is_empty() => sec.origin = Some(target),
                    None => return Err(".org after data in a section the linker places".into()),
                    Some(o) => {
                        let at = o + sec.bytes.len() as u32;
                        if target < at {
                            return Err(format!(
                                ".org ${target:08x} goes backwards (at ${at:08x})"
                            ));
                        }
                        sec.bytes.resize((target - o) as usize, 0xff);
                    }
                }
            }
            ".align" => {
                want(&ops, 1, word)?;
                let n = self.absolute(&ops[0])?;
                if !(1..=65536).contains(&n) || n & (n - 1) != 0 {
                    return Err(".align needs a power of two".into());
                }
                let (_, at) = self.here();
                let pad = (n as u32 - at % n as u32) % n as u32;
                self.emit(&vec![0; pad as usize]);
            }
            ".space" => {
                if ops.is_empty() || ops.len() > 2 {
                    return Err(".space n [, fill]".into());
                }
                let n = self.absolute(&ops[0])?;
                let fill = if ops.len() == 2 {
                    self.absolute(&ops[1])?
                } else {
                    0
                };
                if !(0..=0x100_0000).contains(&n) {
                    return Err(format!(".space {n}: out of range"));
                }
                self.emit(&vec![fill as u8; n as usize]);
            }
            ".word" | ".dw" => {
                for o in &ops {
                    let (_, at) = self.here();
                    if self.pass == 1 {
                        self.word(0);
                        continue;
                    }
                    match self.value(o)? {
                        Val::Abs(x) => {
                            if !(-32768..=65535).contains(&x) {
                                return Err(format!("{x} does not fit a word"));
                            }
                            self.word(x as u16);
                        }
                        v => {
                            self.word(0);
                            self.reloc(at, Field::Word, v, l)?;
                        }
                    }
                }
            }
            ".byte" | ".db" => {
                for o in &ops {
                    if o.starts_with('"') {
                        self.emit(&parse_string(o)?);
                        continue;
                    }
                    let (_, at) = self.here();
                    if self.pass == 1 {
                        self.emit(&[0]);
                        continue;
                    }
                    match self.value(o)? {
                        Val::Abs(x) => {
                            if !(-128..=255).contains(&x) {
                                return Err(format!("{x} does not fit a byte"));
                            }
                            self.emit(&[x as u8]);
                        }
                        v @ Val::Rel {
                            sel: Sel::OffLo | Sel::OffHi | Sel::SegLo | Sel::SegHi,
                            ..
                        } => {
                            self.emit(&[0]);
                            self.reloc(at, Field::Byte, v, l)?;
                        }
                        _ => return Err("an address in a byte: use lo() or hi()".into()),
                    }
                }
            }
            ".far" => {
                for o in &ops {
                    let (_, at) = self.here();
                    self.word(0);
                    self.word(0);
                    if self.pass == 1 {
                        continue;
                    }
                    match self.value(o)? {
                        Val::Abs(x) => {
                            let s = self.section();
                            let b = &mut self.object.sections[s].bytes;
                            let a = at as usize;
                            b[a..a + 2].copy_from_slice(&((x >> 16) as u16).to_le_bytes());
                            b[a + 2..a + 4].copy_from_slice(&(x as u16).to_le_bytes());
                        }
                        Val::Rel {
                            base,
                            add,
                            sel: Sel::Lin,
                        } => {
                            let mk = |sel| Val::Rel {
                                base: base.clone(),
                                add,
                                sel,
                            };
                            self.reloc(at, Field::Word, mk(Sel::Seg), l)?;
                            self.reloc(at + 2, Field::Word, mk(Sel::Off), l)?;
                        }
                        _ => return Err(".far needs an address".into()),
                    }
                }
            }
            ".ascii" | ".asciz" => {
                want(&ops, 1, word)?;
                let mut b = parse_string(&ops[0])?;
                if word == ".asciz" {
                    b.push(0);
                }
                self.emit(&b);
            }
            ".equ" | ".set" => {
                want(&ops, 2, word)?;
                self.define_equ(&ops[0], &ops[1])?;
            }
            ".global" | ".globl" => {
                for o in ops {
                    self.globals.insert(o);
                }
            }
            ".extern" => {}
            _ => return Err(format!("unknown directive `{word}`")),
        }
        Ok(())
    }

    /// An 8-bit immediate: a constant in `lo..=hi`, or a byte of an
    /// address for the linker.
    fn imm8(&self, s: &str, lo: i64, hi: i64) -> Result<ImmVal, String> {
        if self.pass == 1 {
            return Ok(ImmVal::Value(0));
        }
        match self.value(hash(s)?)? {
            Val::Abs(x) if (lo..=hi).contains(&x) => Ok(ImmVal::Value(x as u8)),
            Val::Abs(x) => Err(format!("#{x} out of range {lo}..{hi}")),
            v @ Val::Rel {
                sel: Sel::OffLo | Sel::OffHi | Sel::SegLo | Sel::SegHi,
                ..
            } => Ok(ImmVal::Reloc(v)),
            _ => Err("an address as an 8-bit immediate: use lo(), hi() or li".into()),
        }
    }

    fn op(&mut self, op: Op) {
        self.word(isa::encode(&op));
    }

    fn op_imm(&mut self, f: Imm, rd: u8, v: ImmVal, l: &Line) -> Result<(), String> {
        let (_, at) = self.here();
        match v {
            ImmVal::Value(imm) => self.op(Op::Imm { f, rd, imm }),
            ImmVal::Reloc(v) => {
                self.op(Op::Imm { f, rd, imm: 0 });
                self.reloc(at, Field::Imm8, v, l)?;
            }
        }
        Ok(())
    }

    fn instruction(&mut self, m: &str, args: &str, l: &Line) -> Result<(), String> {
        let (_, at) = self.here();
        if at % 2 != 0 {
            return Err("instruction at an odd address (use .align 2)".into());
        }
        let ops = split_operands(args);
        let r = |i: usize| reg(&ops[i]);
        if let Some((f, _)) = isa::ALU.iter().find(|(_, n)| *n == m) {
            want(&ops, 2, m)?;
            self.op(Op::Alu {
                f: *f,
                rd: r(0)?,
                rs: r(1)?,
            });
            return Ok(());
        }
        if let Some((f, _)) = isa::MD.iter().find(|(_, n)| *n == m) {
            want(&ops, 2, m)?;
            self.op(Op::Md {
                f: *f,
                rd: r(0)?,
                rs: r(1)?,
            });
            return Ok(());
        }
        if let Some((f, _)) = isa::IMM.iter().find(|(_, n)| *n == m) {
            want(&ops, 2, m)?;
            let rd = r(0)?;
            let v = if *f == Imm::Addi {
                self.imm8(&ops[1], -128, 127)?
            } else {
                self.imm8(&ops[1], -128, 255)?
            };
            return self.op_imm(*f, rd, v, l);
        }
        if let Some((f, _)) = isa::MEM.iter().find(|(_, n)| *n == m) {
            want(&ops, 2, m)?;
            let rd = r(0)?;
            let (off, rb) = self.mem_operand(&ops[1])?;
            let word = matches!(f, Mem::Lw | Mem::Sw);
            let max = if word { 30 } else { 15 };
            if !(0..=max).contains(&off) || word && off % 2 != 0 {
                return Err(format!(
                    "offset {off} out of range (0..{max}{})",
                    if word { ", even" } else { "" }
                ));
            }
            self.op(Op::Mem {
                f: *f,
                rd,
                rb,
                off: off as u8,
            });
            return Ok(());
        }
        if let Some((f, _)) = isa::XOP.iter().find(|(_, n)| *n == m) {
            want(&ops, 2, m)?;
            let rd = r(0)?;
            let t = ops[1].trim();
            let (inner, post) = match t.strip_suffix('+') {
                Some(x) => (x.trim(), true),
                None => (t, false),
            };
            let rs = inner
                .strip_prefix('(')
                .and_then(|x| x.strip_suffix(')'))
                .ok_or_else(|| format!("`(rs)` or `(rs)+` expected, got `{t}`"))
                .and_then(reg)?;
            let f: Xop = *f;
            self.op(Op::X { f, post, rd, rs });
            return Ok(());
        }
        let cond = isa::COND
            .iter()
            .position(|c| *c == m)
            .map(|c| c as u8)
            .or_else(|| {
                isa::COND_ALIASES
                    .iter()
                    .find(|(n, _)| *n == m)
                    .map(|(_, c)| *c)
            });
        if let Some(cond) = cond {
            want(&ops, 1, m)?;
            return self.branch(Field::Branch8, cond, &ops[0], l);
        }
        match m {
            "bl" => {
                want(&ops, 1, m)?;
                self.branch(Field::Branch12, 0, &ops[0], l)?;
            }
            "jr" => {
                want(&ops, 1, m)?;
                self.op(Op::Jr { rs: r(0)? });
            }
            "ret" => {
                want(&ops, 0, m)?;
                self.op(Op::Jr { rs: 11 });
            }
            "retf" => {
                want(&ops, 0, m)?;
                self.op(Op::Jf { rd: 10, rs: 11 });
            }
            "jalr" => {
                want(&ops, 2, m)?;
                self.op(Op::Jalr {
                    rd: r(0)?,
                    rs: r(1)?,
                });
            }
            "mts" => {
                want(&ops, 2, m)?;
                let sn = segreg(&ops[0])?;
                if sn == 0 {
                    return Err("mts s0 does not exist: use jf".into());
                }
                self.op(Op::Mts { sn, rs: r(1)? });
            }
            "mfs" => {
                want(&ops, 2, m)?;
                self.op(Op::Mfs {
                    rd: r(0)?,
                    sn: segreg(&ops[1])?,
                });
            }
            "jf" | "callf" => {
                want(&ops, 1, m)?;
                let (a, b) = ops[0]
                    .split_once(':')
                    .ok_or_else(|| format!("{m} rseg:roff expected"))?;
                let (rd, rs) = (reg(a)?, reg(b)?);
                self.op(if m == "jf" {
                    Op::Jf { rd, rs }
                } else {
                    Op::Callf { rd, rs }
                });
            }
            "trap" => {
                want(&ops, 1, m)?;
                let n = self.absolute(hash(&ops[0])?)?;
                if !(0..=255).contains(&n) {
                    return Err(format!("trap #{n}: 0..255"));
                }
                self.op(Op::Trap { n: n as u8 });
            }
            "rti" | "ei" | "di" | "halt" | "nop" | "brk" => {
                want(&ops, 0, m)?;
                self.op(match m {
                    "rti" => Op::Rti,
                    "ei" => Op::Ei,
                    "di" => Op::Di,
                    "halt" => Op::Halt,
                    "nop" => Op::Nop,
                    _ => Op::Brk,
                });
            }
            "mtsr" => {
                want(&ops, 1, m)?;
                self.op(Op::Mtsr { rs: r(0)? });
            }
            "mfsr" => {
                want(&ops, 1, m)?;
                self.op(Op::Mfsr { rd: r(0)? });
            }
            "mfsc" | "mtsc" => {
                want(&ops, 2, m)?;
                let (ri, ci) = if m == "mfsc" { (0, 1) } else { (1, 0) };
                let c = self.absolute(hash(&ops[ci])?)?;
                if !(0..=15).contains(&c) {
                    return Err(format!("system register #{c}: 0..15"));
                }
                let x = r(ri)?;
                self.op(if m == "mfsc" {
                    Op::Mfsc { rd: x, c: c as u8 }
                } else {
                    Op::Mtsc { c: c as u8, rs: x }
                });
            }
            "li" => {
                want(&ops, 2, m)?;
                let rd = r(0)?;
                if self.pass == 1 {
                    self.word(0);
                    self.word(0);
                    return Ok(());
                }
                let v = self.value(hash(&ops[1])?)?;
                let (lo, hi) = match v {
                    Val::Abs(x) => {
                        if !(-32768..=65535).contains(&x) {
                            return Err(format!("li #{x}: does not fit 16 bits"));
                        }
                        let x = x as u16;
                        (ImmVal::Value(x as u8), ImmVal::Value((x >> 8) as u8))
                    }
                    Val::Rel { base, add, sel } => {
                        let (a, b) = match sel {
                            Sel::Lin | Sel::Off => (Sel::OffLo, Sel::OffHi),
                            Sel::Seg => (Sel::SegLo, Sel::SegHi),
                            _ => return Err("li of a byte: use movi".into()),
                        };
                        let mk = |sel| {
                            ImmVal::Reloc(Val::Rel {
                                base: base.clone(),
                                add,
                                sel,
                            })
                        };
                        (mk(a), mk(b))
                    }
                };
                self.op_imm(Imm::Movi, rd, lo, l)?;
                self.op_imm(Imm::Lui, rd, hi, l)?;
            }
            "push" => {
                want(&ops, 1, m)?;
                let x = r(0)?;
                self.op(Op::Imm {
                    f: Imm::Addi,
                    rd: 12,
                    imm: (-2i8) as u8,
                });
                self.op(Op::Mem {
                    f: Mem::Sw,
                    rd: x,
                    rb: 12,
                    off: 0,
                });
            }
            "pop" => {
                want(&ops, 1, m)?;
                let x = r(0)?;
                self.op(Op::Mem {
                    f: Mem::Lw,
                    rd: x,
                    rb: 12,
                    off: 0,
                });
                self.op(Op::Imm {
                    f: Imm::Addi,
                    rd: 12,
                    imm: 2,
                });
            }
            _ => return Err(format!("unknown instruction `{m}`")),
        }
        Ok(())
    }

    fn mem_operand(&self, s: &str) -> Result<(i64, u8), String> {
        let t = s.trim();
        let open = t
            .rfind('(')
            .ok_or_else(|| format!("`off(rb)` expected, got `{t}`"))?;
        let inner = t[open + 1..]
            .strip_suffix(')')
            .ok_or_else(|| format!("`)` expected in `{t}`"))?;
        let rb = reg(inner)?;
        let off_text = t[..open].trim();
        let off = if off_text.is_empty() {
            0
        } else {
            self.absolute(off_text)?
        };
        Ok((off, rb))
    }

    fn branch(&mut self, field: Field, cond: u8, target: &str, l: &Line) -> Result<(), String> {
        let (sec, at) = self.here();
        let placeholder = match field {
            Field::Branch8 => Op::Branch { cond, off: 0 },
            _ => Op::Bl { off: 0 },
        };
        self.op(placeholder);
        if self.pass == 1 {
            return Ok(());
        }
        let v = self.value(target)?;
        if let (Val::Abs(t), Some(o)) = (&v, self.object.sections[sec].origin) {
            // Both ends known: encode now.
            let pc = o + at;
            let off = branch_offset(field, pc, *t as u32)?;
            let w = match field {
                Field::Branch8 => isa::encode(&Op::Branch {
                    cond,
                    off: off as i8,
                }),
                _ => isa::encode(&Op::Bl { off: off as i16 }),
            };
            let b = &mut self.object.sections[sec].bytes;
            b[at as usize..at as usize + 2].copy_from_slice(&w.to_le_bytes());
            return Ok(());
        }
        match v {
            Val::Abs(_) | Val::Rel { sel: Sel::Lin, .. } => self.reloc(at, field, v, l),
            _ => Err("a branch needs an address".into()),
        }
    }
}

/// Word offset of a branch at linear `pc` to linear `target`.
pub fn branch_offset(field: Field, pc: u32, target: u32) -> Result<i32, String> {
    let next = pc.wrapping_add(2);
    let (range, name) = match field {
        Field::Branch8 => (128, "conditional branch"),
        _ => (2048, "bl"),
    };
    if target >> 16 != pc >> 16 && target > 0xffff {
        return Err(format!(
            "{name} to ${target:08x} leaves segment ${:04x}",
            pc >> 16
        ));
    }
    let t = if target <= 0xffff && pc > 0xffff {
        (pc & 0xffff_0000) | target
    } else {
        target
    };
    if t & 1 != 0 {
        return Err(format!("{name} to an odd address ${t:08x}"));
    }
    let delta = (i64::from(t) - i64::from(next)) / 2;
    if delta < -range || delta >= range {
        return Err(format!(
            "{name} to ${t:08x} is out of range ({} bytes)",
            delta * 2
        ));
    }
    Ok(delta as i32)
}

/// `seg:off` or an expression, as a linear address.
fn parse_address(s: &str, syms: &dyn Symbols) -> Result<u32, String> {
    if let Some((a, b)) = s.split_once(':') {
        let seg = expr::eval_str(a, syms)?
            .abs()
            .ok_or("segment must be a constant")?;
        let off = expr::eval_str(b, syms)?
            .abs()
            .ok_or("offset must be a constant")?;
        return Ok(((seg as u32) << 16) | (off as u32 & 0xffff));
    }
    let v = expr::eval_str(s, syms)?
        .abs()
        .ok_or("address must be a constant")?;
    u32::try_from(v).map_err(|_| format!("address {v} out of range"))
}

/// A `"…"` string with `\n \r \t \0 \\ \" \xNN`.
fn parse_string(s: &str) -> Result<Vec<u8>, String> {
    let inner = s
        .trim()
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .ok_or_else(|| format!("string expected, got `{s}`"))?;
    let mut out = Vec::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('0') => out.push(0),
            Some('\\') => out.push(b'\\'),
            Some('"') => out.push(b'"'),
            Some('x') => {
                let h: String = chars.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&h, 16).map_err(|_| format!("bad \\x{h}"))?);
            }
            other => return Err(format!("bad escape \\{}", other.unwrap_or(' '))),
        }
    }
    Ok(out)
}

/// Parse `seg:off` (hex without `$` allowed) for command lines.
pub fn parse_seg_off(s: &str) -> Result<u32, String> {
    let (a, b) = s
        .split_once(':')
        .ok_or_else(|| format!("seg:off expected, got `{s}`"))?;
    let h = |x: &str| {
        u32::from_str_radix(
            x.trim().trim_start_matches('$').trim_start_matches("0x"),
            16,
        )
        .map_err(|_| format!("hex number expected, got `{x}`"))
    };
    let (seg, off) = (h(a)?, h(b)?);
    if seg > 0xffff || off > 0xffff {
        return Err(format!("`{s}` out of range"));
    }
    Ok(seg << 16 | off)
}
