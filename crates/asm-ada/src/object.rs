//! Object files: what the assembler leaves for the linker. Sections of
//! bytes (placed at a fixed address with `.org`, or by the linker),
//! symbols, relocations and listing lines; stored as `.ao` files.
//!
//! File format (little-endian): "ADAO", version u16, then the sections,
//! the global symbols and the externals as length-prefixed records.

use crate::expr::Sel;

/// What a relocation patches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// A data word (16 bits of the selected value).
    Word,
    /// A data byte.
    Byte,
    /// The 8-bit immediate of an instruction (bits 7–0).
    Imm8,
    /// The 8-bit word offset of a conditional branch to the value.
    Branch8,
    /// The 12-bit word offset of BL to the value.
    Branch12,
}

impl Field {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Option<Self> {
        [
            Field::Word,
            Field::Byte,
            Field::Imm8,
            Field::Branch8,
            Field::Branch12,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// What the value is relative to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Section(usize),
    Symbol(String),
    /// No base: `add` is the value.
    Absolute,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reloc {
    /// Byte offset in the section.
    pub at: u32,
    pub field: Field,
    pub target: Target,
    pub add: i64,
    pub sel: Sel,
    /// Where it came from (for messages).
    pub line: String,
}

/// One source line's output, for the listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListLine {
    pub at: u32,
    pub len: u32,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    /// Fixed linear address (from `.org`), else the linker places it.
    pub origin: Option<u32>,
    pub bytes: Vec<u8>,
    pub relocs: Vec<Reloc>,
    pub listing: Vec<ListLine>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Object {
    pub name: String,
    pub sections: Vec<Section>,
    /// Exported symbols: name → (section, offset).
    pub globals: Vec<(String, usize, u32)>,
}

const MAGIC: &[u8; 4] = b"ADAO";
const VERSION: u16 = 1;

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
}

struct R<'a>(&'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("object file ends early".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().unwrap_or_default(),
        ))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().unwrap_or_default(),
        ))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().unwrap_or_default(),
        ))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?).map_err(|_| "bad text in object file".into())
    }
}

impl Object {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = W(Vec::new());
        w.0.extend_from_slice(MAGIC);
        w.u16(VERSION);
        w.str(&self.name);
        w.u32(self.sections.len() as u32);
        for s in &self.sections {
            w.str(&s.name);
            w.u8(u8::from(s.origin.is_some()));
            w.u32(s.origin.unwrap_or(0));
            w.bytes(&s.bytes);
            w.u32(s.relocs.len() as u32);
            for r in &s.relocs {
                w.u32(r.at);
                w.u8(r.field.to_u8());
                match &r.target {
                    Target::Section(i) => {
                        w.u8(0);
                        w.u32(*i as u32);
                    }
                    Target::Symbol(n) => {
                        w.u8(1);
                        w.str(n);
                    }
                    Target::Absolute => w.u8(2),
                }
                w.i64(r.add);
                w.u8(r.sel.to_u8());
                w.str(&r.line);
            }
            w.u32(s.listing.len() as u32);
            for l in &s.listing {
                w.u32(l.at);
                w.u32(l.len);
                w.str(&l.text);
            }
        }
        w.u32(self.globals.len() as u32);
        for (n, s, at) in &self.globals {
            w.str(n);
            w.u32(*s as u32);
            w.u32(*at);
        }
        w.0
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, String> {
        let mut r = R(b);
        if r.take(4)? != MAGIC {
            return Err("not an Ada object file".into());
        }
        if r.u16()? != VERSION {
            return Err("unknown object file version".into());
        }
        let name = r.str()?;
        let n = r.u32()?;
        let mut sections = Vec::new();
        for _ in 0..n {
            let sname = r.str()?;
            let fixed = r.u8()? != 0;
            let org = r.u32()?;
            let bytes = r.bytes()?;
            let mut relocs = Vec::new();
            for _ in 0..r.u32()? {
                let at = r.u32()?;
                let field = Field::from_u8(r.u8()?).ok_or("bad relocation field")?;
                let target = match r.u8()? {
                    0 => Target::Section(r.u32()? as usize),
                    1 => Target::Symbol(r.str()?),
                    2 => Target::Absolute,
                    _ => return Err("bad relocation target".into()),
                };
                let add = r.i64()?;
                let sel = Sel::from_u8(r.u8()?).ok_or("bad relocation selector")?;
                let line = r.str()?;
                relocs.push(Reloc {
                    at,
                    field,
                    target,
                    add,
                    sel,
                    line,
                });
            }
            let mut listing = Vec::new();
            for _ in 0..r.u32()? {
                listing.push(ListLine {
                    at: r.u32()?,
                    len: r.u32()?,
                    text: r.str()?,
                });
            }
            sections.push(Section {
                name: sname,
                origin: fixed.then_some(org),
                bytes,
                relocs,
                listing,
            });
        }
        let mut globals = Vec::new();
        for _ in 0..r.u32()? {
            globals.push((r.str()?, r.u32()? as usize, r.u32()?));
        }
        let obj = Object {
            name,
            sections,
            globals,
        };
        for s in &obj.sections {
            for rel in &s.relocs {
                if matches!(rel.target, Target::Section(i) if i >= obj.sections.len()) {
                    return Err("relocation to a missing section".into());
                }
            }
        }
        Ok(obj)
    }
}
