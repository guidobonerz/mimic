//! The linker: places the objects' sections (fixed ones at their `.org`,
//! the others one after another from a base address), resolves symbols
//! and relocations, and gives the memory image, the symbol table and a
//! listing.

use std::collections::BTreeMap;

use crate::asm::branch_offset;
use crate::expr::Sel;
use crate::object::{Field, Object, Target};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Linear address for the sections without `.org`.
    pub base: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { base: 0x0040_0000 }
    }
}

/// The linked program: blocks of bytes at linear addresses (sorted, not
/// overlapping), the global symbols and a listing.
#[derive(Clone, Debug, Default)]
pub struct Image {
    pub blocks: Vec<(u32, Vec<u8>)>,
    pub symbols: BTreeMap<String, u32>,
    pub listing: String,
}

impl Image {
    /// Lowest address.
    pub fn start(&self) -> Option<u32> {
        self.blocks.first().map(|(a, _)| *a)
    }

    /// The bytes from `start` for `len`, gaps and the rest `fill`.
    pub fn flat(&self, start: u32, len: usize, fill: u8) -> Vec<u8> {
        let mut out = vec![fill; len];
        for (a, b) in &self.blocks {
            for (i, v) in b.iter().enumerate() {
                let at = (*a as u64 + i as u64).wrapping_sub(u64::from(start));
                if let Some(slot) = out.get_mut(at as usize).filter(|_| at < len as u64) {
                    *slot = *v;
                }
            }
        }
        out
    }

    /// Everything from the lowest to the highest address, gaps `fill`;
    /// `None` when that spans more than `limit` bytes.
    pub fn contiguous(&self, fill: u8, limit: usize) -> Option<(u32, Vec<u8>)> {
        let start = self.start()?;
        let end = self
            .blocks
            .iter()
            .map(|(a, b)| u64::from(*a) + b.len() as u64)
            .max()?;
        let len = (end - u64::from(start)) as usize;
        (len <= limit).then(|| (start, self.flat(start, len, fill)))
    }
}

/// Link `objects`.
pub fn link(objects: &[Object], opts: Options) -> Result<Image, Vec<String>> {
    let mut errors = Vec::new();
    // Placement: (object, section) → address.
    let mut place: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    let mut cursor = u64::from(opts.base);
    for (oi, o) in objects.iter().enumerate() {
        for (si, s) in o.sections.iter().enumerate() {
            if let Some(org) = s.origin {
                place.insert((oi, si), org);
            }
        }
    }
    for (oi, o) in objects.iter().enumerate() {
        for (si, s) in o.sections.iter().enumerate() {
            if s.origin.is_none() {
                cursor = (cursor + 1) & !1;
                place.insert((oi, si), cursor as u32);
                cursor += s.bytes.len() as u64;
            }
        }
    }
    // Ranges: no overlap, no section across a segment boundary.
    let mut ranges: Vec<(u64, u64, String)> = Vec::new();
    for (&(oi, si), &at) in &place {
        let s = &objects[oi].sections[si];
        if s.bytes.is_empty() {
            continue;
        }
        let end = u64::from(at) + s.bytes.len() as u64;
        let name = format!("{}:{}", objects[oi].name, s.name);
        if end > 1 << 32 {
            errors.push(format!("{name} runs past the end of the address space"));
        } else if (end - 1) >> 16 != u64::from(at) >> 16 {
            errors.push(format!(
                "{name} (${at:08x}, {} bytes) crosses a segment boundary",
                s.bytes.len()
            ));
        }
        ranges.push((u64::from(at), end, name));
    }
    ranges.sort();
    for w in ranges.windows(2) {
        if w[1].0 < w[0].1 {
            errors.push(format!(
                "{} and {} overlap at ${:08x}",
                w[0].2, w[1].2, w[1].0
            ));
        }
    }
    // Global symbols.
    let mut symbols = BTreeMap::new();
    for (oi, o) in objects.iter().enumerate() {
        for (name, si, off) in &o.globals {
            let Some(base) = place.get(&(oi, *si)) else {
                errors.push(format!("{}: symbol {name} in a missing section", o.name));
                continue;
            };
            if symbols.insert(name.clone(), base + off).is_some() {
                errors.push(format!("{name} is defined in more than one object"));
            }
        }
    }
    // Relocations.
    let mut out: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let mut listing = String::new();
    for (oi, o) in objects.iter().enumerate() {
        for (si, s) in o.sections.iter().enumerate() {
            let at = place[&(oi, si)];
            let mut bytes = s.bytes.clone();
            for r in &s.relocs {
                let value = match &r.target {
                    Target::Section(i) => place.get(&(oi, *i)).map(|b| i64::from(*b)),
                    Target::Symbol(n) => symbols.get(n).map(|v| i64::from(*v)),
                    Target::Absolute => Some(0),
                };
                let Some(value) = value else {
                    if let Target::Symbol(n) = &r.target {
                        errors.push(format!("{}: undefined symbol `{n}`", r.line));
                    }
                    continue;
                };
                let v = (value + r.add) as u32;
                let i = r.at as usize;
                if let Err(e) = patch(&mut bytes, i, r.field, r.sel, v, at + r.at) {
                    errors.push(format!("{}: {e}", r.line));
                }
            }
            for l in &s.listing {
                let a = at + l.at;
                let mut hex = String::new();
                let end = (l.at + l.len) as usize;
                for (k, chunk) in bytes[l.at as usize..end].chunks(2).enumerate() {
                    if k == 3 {
                        hex.push('…');
                        break;
                    }
                    if chunk.len() == 2 {
                        hex.push_str(&format!(
                            "{:04x} ",
                            u16::from_le_bytes([chunk[0], chunk[1]])
                        ));
                    } else {
                        hex.push_str(&format!("{:02x} ", chunk[0]));
                    }
                }
                listing.push_str(&format!(
                    "{:04x}:{:04x}  {hex:<16} {}\n",
                    a >> 16,
                    a & 0xffff,
                    l.text
                ));
            }
            if !bytes.is_empty() {
                out.insert(at, bytes);
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // Merge adjacent blocks.
    let mut blocks: Vec<(u32, Vec<u8>)> = Vec::new();
    for (a, b) in out {
        match blocks.last_mut() {
            Some((la, lb)) if u64::from(*la) + lb.len() as u64 == u64::from(a) => lb.extend(b),
            _ => blocks.push((a, b)),
        }
    }
    Ok(Image {
        blocks,
        symbols,
        listing,
    })
}

fn patch(
    bytes: &mut [u8],
    i: usize,
    field: Field,
    sel: Sel,
    v: u32,
    pc: u32,
) -> Result<(), String> {
    match field {
        // A linear address in a word is its offset (the low 16 bits).
        Field::Word => bytes[i..i + 2].copy_from_slice(&(sel.apply(v) as u16).to_le_bytes()),
        Field::Byte => bytes[i] = sel.apply(v) as u8,
        Field::Imm8 => bytes[i] = sel.apply(v) as u8,
        Field::Branch8 | Field::Branch12 => {
            let off = branch_offset(field, pc, v)?;
            let w = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
            let w = if field == Field::Branch8 {
                (w & 0xff00) | (off as u8 as u16)
            } else {
                (w & 0xf000) | (off as u16 & 0x0fff)
            };
            bytes[i..i + 2].copy_from_slice(&w.to_le_bytes());
        }
    }
    Ok(())
}
