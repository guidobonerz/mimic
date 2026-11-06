//! Brass, the Go-like programming language of the Logophor 16 (a
//! fictional home computer of this project): the compiler `brass`.
//!
//! Language: `docs/logophor16/BRASS.md`. Brass source → typed program →
//! Ada assembly → `asm-ada` → a binary for 0040:0000: code segments from
//! $0040 (packages packed into 64 KiB segments, each with its own copy of
//! the runtime's code), then the data segment (S1), the heap after it to
//! the end of fast RAM, the stack 003F:FFFE (S2).
#![forbid(unsafe_code)]

pub mod ast;
pub mod compile;
pub mod lexer;
pub mod parser;
pub mod types;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use compile::{Compiler, PackageSrc};

/// Where the program is loaded and starts.
pub const LOAD: u32 = 0x0040_0000;
/// Code bytes a segment takes at most (the estimate's margin below 64 KiB).
const SEGMENT_LIMIT: u32 = 60 * 1024;

/// The runtime and the library packages, built in.
const RT: &str = include_str!("../lib/rt.s");
/// The runtime's interface section (left out when unused).
const RT_IFACE: &str = "; ---- interfaces ----";
const RT_IFACE_END: &str = "; ---- end of interfaces ----\n";
const LIBRARY: &[(&str, &str, &str)] = &[
    (
        "console",
        "console.brass",
        include_str!("../lib/console/console.brass"),
    ),
    (
        "console",
        "console.s",
        include_str!("../lib/console/console.s"),
    ),
    ("strs", "strs.brass", include_str!("../lib/strs/strs.brass")),
];

/// Where packages come from: a package path → its files (name, text).
pub trait Sources {
    fn package(&self, path: &str) -> Option<Vec<(String, String)>>;
}

/// The program's folder: `main` is the folder itself, `a/b` its
/// subfolder; anything else from the built-in library.
pub struct DirSources {
    pub root: PathBuf,
}

fn read_dir(dir: &Path) -> Option<Vec<(String, String)>> {
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        if ext == "brass" || ext == "s" {
            files.push((p.display().to_string(), std::fs::read_to_string(&p).ok()?));
        }
    }
    files.sort();
    (!files.is_empty()).then_some(files)
}

impl Sources for DirSources {
    fn package(&self, path: &str) -> Option<Vec<(String, String)>> {
        let dir = if path == "main" {
            self.root.clone()
        } else {
            self.root.join(path)
        };
        read_dir(&dir).or_else(|| library(path))
    }
}

/// A program in memory: package path → files (tests).
pub struct MemSources(pub BTreeMap<String, Vec<(String, String)>>);

impl Sources for MemSources {
    fn package(&self, path: &str) -> Option<Vec<(String, String)>> {
        self.0.get(path).cloned().or_else(|| library(path))
    }
}

fn library(path: &str) -> Option<Vec<(String, String)>> {
    let v: Vec<(String, String)> = LIBRARY
        .iter()
        .filter(|(p, _, _)| *p == path)
        .map(|(p, n, t)| (format!("<lib>/{p}/{n}"), t.to_string()))
        .collect();
    (!v.is_empty()).then_some(v)
}

/// The compiled program.
pub struct Output {
    pub binary: Vec<u8>,
    pub asm: String,
    pub listing: String,
}

/// Compile the program whose `main` package `src` gives.
pub fn build(src: &dyn Sources) -> Result<Output, Vec<String>> {
    // Load the packages, imports first (depth first, cycles refused).
    let mut names: Vec<String> = Vec::new();
    let mut order: Vec<PackageSrc> = Vec::new();
    let mut asm_files: Vec<(String, String, String)> = Vec::new();
    let mut state: HashMap<String, bool> = HashMap::new(); // false: in progress
    let mut next_id = 0;
    #[allow(clippy::too_many_arguments)]
    fn visit(
        path: &str,
        src: &dyn Sources,
        names: &mut Vec<String>,
        order: &mut Vec<PackageSrc>,
        asm_files: &mut Vec<(String, String, String)>,
        state: &mut HashMap<String, bool>,
        next_id: &mut u32,
        from: &str,
    ) -> Result<(), String> {
        match state.get(path) {
            Some(true) => return Ok(()),
            Some(false) => return Err(format!("import cycle through {path}")),
            None => {}
        }
        state.insert(path.to_string(), false);
        let files = src
            .package(path)
            .ok_or_else(|| format!("{from}: package {path} not found"))?;
        let mut parsed = Vec::new();
        for (name, text) in files {
            if name.ends_with(".s") {
                asm_files.push((path.to_string(), name, text));
                continue;
            }
            let fid = names.len() as u16;
            names.push(name.clone());
            let toks = lexer::lex(fid, &text)
                .map_err(|(p, e)| format!("{name}:{}:{}: {e}", p.line, p.col))?;
            let file = parser::parse(&toks, next_id)
                .map_err(|(p, e)| format!("{name}:{}:{}: {e}", p.line, p.col))?;
            let want = path.rsplit('/').next().unwrap_or(path);
            if file.package != want {
                return Err(format!(
                    "{name}: package {} in the folder of package {want}",
                    file.package
                ));
            }
            parsed.push((fid, file));
        }
        if parsed.is_empty() {
            return Err(format!("package {path} has no .brass file"));
        }
        let imports: Vec<String> = parsed
            .iter()
            .flat_map(|(_, f)| f.imports.iter().map(|i| i.path.clone()))
            .collect();
        for i in imports {
            visit(&i, src, names, order, asm_files, state, next_id, path)?;
        }
        state.insert(path.to_string(), true);
        order.push(PackageSrc {
            path: path.to_string(),
            files: parsed,
        });
        Ok(())
    }
    visit(
        "main",
        src,
        &mut names,
        &mut order,
        &mut asm_files,
        &mut state,
        &mut next_id,
        "main",
    )
    .map_err(|e| vec![e])?;
    let mut c = Compiler::new(names.clone());
    if let Err((p, e)) = c.compile(&order) {
        let file = names.get(usize::from(p.file)).cloned().unwrap_or_default();
        return Err(vec![format!("{file}:{}:{}: {e}", p.line, p.col)]);
    }
    // The program's text: segments of packages, each with the runtime.
    // The interface routines only when they are called.
    let rt = if c.uses_interfaces() {
        RT.to_string()
    } else {
        match (RT.find(RT_IFACE), RT.find(RT_IFACE_END)) {
            (Some(a), Some(b)) => format!("{}{}", &RT[..a], &RT[b + RT_IFACE_END.len()..]),
            _ => RT.to_string(),
        }
    };
    let (rt_all, rt_data) = rt.split_once("; ---- data ----").unwrap_or((&rt, ""));
    // The runtime's constants once, its code in every segment.
    let is_const = |l: &str| l.trim_start().starts_with("RT_") && l.contains('=');
    let mut rt_consts = String::new();
    let mut rt_code = String::new();
    for l in rt_all.lines() {
        let target = if is_const(l) {
            &mut rt_consts
        } else {
            &mut rt_code
        };
        target.push_str(l);
        target.push('\n');
    }
    let line = |t: &mut String, l: &str| {
        t.push_str(l);
        t.push('\n');
    };
    let mut start = String::new();
    line(&mut start, "_start:");
    line(&mut start, "        li      r1, #seg(rtd_data_start)");
    line(&mut start, "        mts     s1, r1");
    line(&mut start, "        li      r1, #$003f");
    line(&mut start, "        mts     s2, r1");
    line(&mut start, "        li      r12, #$fffe");
    line(&mut start, "        li      r10, #@rt_heap_init");
    line(&mut start, "        jalr    r11, r10");
    let main = compile::mangle("main", "main");
    for f in c.inits.iter().chain(std::iter::once(&main)) {
        line(&mut start, &format!("        li      r9, #seg({f})"));
        line(&mut start, &format!("        li      r10, #off({f})"));
        line(&mut start, "        callf   r9:r10");
    }
    line(&mut start, "        movi    r1, #0");
    line(&mut start, "        li      r10, #@rt_exit");
    line(&mut start, "        jr      r10");
    let rt_size = estimate(&rt_code);
    let mut segments: Vec<(String, u32)> = vec![(start.clone(), rt_size + estimate(&start))];
    for (pkg, code) in c.code() {
        let mut text = code.clone();
        for (_, name, t) in asm_files.iter().filter(|(p, _, _)| p == pkg) {
            line(&mut text, &format!("; ---- {name}"));
            line(&mut text, t);
        }
        let size = estimate(&text);
        // A package that fits goes into one segment whole; a bigger one is
        // spread over several at its functions (every call is far).
        let pieces = if size + rt_size > SEGMENT_LIMIT {
            functions(&text)
        } else {
            vec![text]
        };
        for piece in pieces {
            let size = estimate(&piece);
            if size + rt_size > SEGMENT_LIMIT {
                return Err(vec![format!(
                    "a function of package {pkg} has more than 60 KiB of code: split it"
                )]);
            }
            if segments.last().unwrap().1 + size > SEGMENT_LIMIT {
                segments.push((String::new(), rt_size));
            }
            let last = segments.last_mut().unwrap();
            last.0.push_str(&piece);
            last.1 += size;
        }
    }
    let mut asm = String::new();
    line(&mut asm, "; generated by brass");
    asm.push_str(&rt_consts);
    for (i, (text, _)) in segments.iter().enumerate() {
        let prefix = format!("rt{i}_");
        line(&mut asm, &format!("        .section code{i}"));
        line(
            &mut asm,
            &format!("        .org    ${:08x}", LOAD + ((i as u32) << 16)),
        );
        asm.push_str(&text.replace("@rt_", &prefix));
        asm.push_str(&rt_code.replace("@rt_", &prefix));
    }
    let data_org = LOAD + ((segments.len() as u32) << 16);
    line(&mut asm, "        .section data");
    line(&mut asm, &format!("        .org    ${data_org:08x}"));
    line(&mut asm, "rtd_data_start:");
    line(&mut asm, rt_data);
    asm.push_str(c.data());
    line(&mut asm, "        .align  2");
    line(&mut asm, "rtd_data_end:");
    line(&mut asm, "        .word   0");
    let obj = asm_ada::assemble("program.s", &asm, &NoFiles).map_err(|e| {
        let mut v = vec!["the generated assembly did not assemble (a compiler bug):".to_string()];
        v.extend(e);
        v
    })?;
    let img = asm_ada::link(&[obj], asm_ada::Options { base: LOAD })?;
    let (start, binary) = img
        .contiguous(0, 16 << 20)
        .ok_or_else(|| vec!["the program is too large".to_string()])?;
    if start != LOAD {
        return Err(vec![format!("the program starts at {start:08x}")]);
    }
    Ok(Output {
        binary,
        asm,
        listing: img.listing,
    })
}

/// A package's assembly cut before each global label (a function, or a
/// routine of its assembly part): local labels stay with their function.
fn functions(text: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for l in text.lines() {
        let global = l
            .split(';')
            .next()
            .and_then(|c| c.split_once(':'))
            .is_some_and(|(label, _)| {
                !label.is_empty()
                    && !label.starts_with(['.', ' ', '\t'])
                    && !label.contains([' ', '\t', '"'])
            });
        if global && !out.last().unwrap().trim().is_empty() {
            out.push(String::new());
        }
        let last = out.last_mut().unwrap();
        last.push_str(l);
        last.push('\n');
    }
    out
}

/// Bytes the assembly text takes (a safe estimate: `li` four bytes).
fn estimate(text: &str) -> u32 {
    let mut n = 0u32;
    for line in text.lines() {
        let l = line.split(';').next().unwrap_or("").trim();
        let l = match l.split_once(':') {
            Some((label, rest)) if !label.contains(' ') && !label.contains('"') => rest.trim(),
            _ => l,
        };
        if l.is_empty() {
            continue;
        }
        let (op, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        n += match op {
            "li" => 4,
            ".byte" => rest.split(',').count() as u32,
            ".word" => 2 * rest.split(',').count() as u32,
            ".ascii" | ".asciz" => rest.len() as u32,
            ".space" => rest.trim().parse().unwrap_or(0),
            ".align" => 1,
            _ if op.starts_with('.') => 0,
            _ => 2,
        };
    }
    n
}

struct NoFiles;

impl asm_ada::Files for NoFiles {
    fn read(&self, path: &Path) -> Result<String, String> {
        Err(format!("{}: no includes in generated code", path.display()))
    }
}

#[cfg(test)]
mod tests;
