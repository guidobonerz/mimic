//! Symbol tables from assembler output, so the debugger shows and accepts
//! label names.
//!
//! Formats:
//!
//! - **VICE labels** (`-moncommands` files, KickAss `-vicesymbols`, ACME,
//!   cc65 `ld65 -Ln`): `al C:0810 .start`. The memory prefix (`C:`) and
//!   the leading dot are optional; other lines are ignored.
//! - **KickAssembler `.sym`** (`-symbolfile`): `.label start=$0810`, with
//!   `.namespace name {` … `}` blocks giving `name.start`. `.const` and
//!   `.var` lines are values, not addresses, and are ignored.
//!
//! Tables are ordered (`BTreeMap`) so listings and lookups are
//! deterministic. A name maps to one address (the last definition wins);
//! an address may carry several names.

use std::collections::BTreeMap;

use emu_core::MemSpaceId;

/// Largest accepted symbol file.
pub const MAX_FILE: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Vice,
    KickAss,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Vice => "vice",
            Format::KickAss => "kickass",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "vice" => Some(Format::Vice),
            "kickass" | "kick" | "sym" => Some(Format::KickAss),
            _ => None,
        }
    }

    /// Guess from the content: VICE files are made of `al` lines.
    pub fn detect(text: &str) -> Option<Self> {
        for line in text.lines().map(str::trim) {
            if line.starts_with("al ") {
                return Some(Format::Vice);
            }
            if line.starts_with(".label") || line.starts_with(".namespace") {
                return Some(Format::KickAss);
            }
        }
        None
    }
}

/// Parse a symbol file into (name, address) pairs; returns them and the
/// number of lines that looked like definitions but could not be read.
pub fn parse(text: &str, format: Format) -> (Vec<(String, u32)>, usize) {
    match format {
        Format::Vice => parse_vice(text),
        Format::KickAss => parse_kickass(text),
    }
}

fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    let s = s
        .strip_prefix('$')
        .or_else(|| s.strip_prefix("0x"))
        .unwrap_or(s);
    u32::from_str_radix(s, 16).ok()
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | ':'))
}

fn parse_vice(text: &str) -> (Vec<(String, u32)>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("al") {
            continue;
        }
        let (Some(addr), Some(name)) = (words.next(), words.next()) else {
            bad += 1;
            continue;
        };
        // "C:0810": the memory prefix names the bank, not part of the address.
        let addr = addr.rsplit(':').next().and_then(parse_hex);
        let name = name.strip_prefix('.').unwrap_or(name);
        match addr {
            Some(a) if valid_name(name) => out.push((name.to_string(), a)),
            _ => bad += 1,
        }
    }
    (out, bad)
}

fn parse_kickass(text: &str) -> (Vec<(String, u32)>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    let mut namespaces: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix(".namespace") {
            let name = rest.trim().trim_end_matches('{').trim();
            namespaces.push(name.to_string());
            continue;
        }
        if line.starts_with('}') {
            namespaces.pop();
            continue;
        }
        let Some(def) = line.strip_prefix(".label") else {
            continue;
        };
        let Some((name, value)) = def.split_once('=') else {
            bad += 1;
            continue;
        };
        let name = name.trim();
        let full = if namespaces.is_empty() {
            name.to_string()
        } else {
            format!("{}.{name}", namespaces.join("."))
        };
        match parse_hex(value) {
            Some(a) if valid_name(&full) => out.push((full, a)),
            _ => bad += 1,
        }
    }
    (out, bad)
}

/// Symbols of all memory spaces.
#[derive(Clone, Debug, Default)]
pub struct SymbolTable {
    by_name: BTreeMap<String, (MemSpaceId, u32)>,
    by_addr: BTreeMap<(u16, u32), Vec<String>>,
}

impl SymbolTable {
    pub fn insert(&mut self, space: MemSpaceId, name: String, addr: u32) {
        if let Some((old_space, old)) = self.by_name.insert(name.clone(), (space, addr)) {
            if let Some(names) = self.by_addr.get_mut(&(old_space.0, old)) {
                names.retain(|n| *n != name);
                if names.is_empty() {
                    self.by_addr.remove(&(old_space.0, old));
                }
            }
        }
        self.by_addr.entry((space.0, addr)).or_default().push(name);
    }

    pub fn clear(&mut self) {
        self.by_name.clear();
        self.by_addr.clear();
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn lookup(&self, name: &str) -> Option<(MemSpaceId, u32)> {
        self.by_name.get(name).copied()
    }

    /// The first name defined for an address.
    pub fn name_at(&self, space: MemSpaceId, addr: u32) -> Option<&str> {
        self.by_addr
            .get(&(space.0, addr))
            .and_then(|names| names.first())
            .map(String::as_str)
    }

    /// All symbols ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, MemSpaceId, u32)> {
        self.by_name
            .iter()
            .map(|(n, (space, addr))| (n.as_str(), *space, *addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vice_labels() {
        let text = "al C:0810 .start\nal 0820 .loop\nbreak 0810\nal C:zz .bad\nal $1000 .data\n";
        assert_eq!(Format::detect(text), Some(Format::Vice));
        let (syms, bad) = parse(text, Format::Vice);
        assert_eq!(
            syms,
            [
                ("start".to_string(), 0x0810),
                ("loop".to_string(), 0x0820),
                ("data".to_string(), 0x1000)
            ]
        );
        assert_eq!(bad, 1);
    }

    #[test]
    fn kickass_sym_with_namespaces() {
        let text = "\
.label start=$0810
.const SCREEN=$0400
.namespace sprites {
  .label init = $1000
  .namespace anim {
    .label step=$1010
  }
}
.label end=$1100
.label broken
";
        assert_eq!(Format::detect(text), Some(Format::KickAss));
        let (syms, bad) = parse(text, Format::KickAss);
        assert_eq!(
            syms,
            [
                ("start".to_string(), 0x0810),
                ("sprites.init".to_string(), 0x1000),
                ("sprites.anim.step".to_string(), 0x1010),
                ("end".to_string(), 0x1100),
            ]
        );
        assert_eq!(bad, 1);
        assert_eq!(Format::detect("nothing here"), None);
    }

    #[test]
    fn table_lookups_both_ways() {
        let mut t = SymbolTable::default();
        let cpu = MemSpaceId(0);
        t.insert(cpu, "start".into(), 0x0810);
        t.insert(cpu, "entry".into(), 0x0810);
        t.insert(MemSpaceId(3), "drive".into(), 0x0810);
        assert_eq!(t.lookup("entry"), Some((cpu, 0x0810)));
        assert_eq!(t.name_at(cpu, 0x0810), Some("start"));
        assert_eq!(t.name_at(MemSpaceId(3), 0x0810), Some("drive"));
        // Redefinition moves the name.
        t.insert(cpu, "start".into(), 0x0900);
        assert_eq!(t.name_at(cpu, 0x0810), Some("entry"));
        assert_eq!(t.name_at(cpu, 0x0900), Some("start"));
        assert_eq!(t.len(), 3);
        let names: Vec<&str> = t.iter().map(|(n, _, _)| n).collect();
        assert_eq!(names, ["drive", "entry", "start"]);
        t.clear();
        assert!(t.is_empty());
    }
}
