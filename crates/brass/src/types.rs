//! Brass types (BRASS.md 4) and their layout in memory and registers.
//!
//! **[B]** Layout: `bool`, `int8`, `uint8` one byte; 16-bit types two
//! bytes; 32-bit integers, `fixed`, `uintptr`, pointers and functions four
//! bytes, the low word first (a pointer: offset, then segment); a string
//! is pointer + length (6 bytes), a slice pointer + length + capacity (8),
//! an interface a pointer to the value + a pointer to the dynamic type's
//! descriptor (8; `compile/iface.rs`).
//! Struct fields follow each other, a field of two bytes or more on an
//! even offset; structs and arrays of more than one byte have even sizes.
//! In registers ("scalars"): one word in R1; 32-bit values R1 = high,
//! R2 = low; a string R1:R2 pointer, R3 length; a slice also R4 capacity;
//! an interface R1:R2 the value's pointer, R3:R4 the descriptor's.
//! Structs and arrays are never in registers: they move by address.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    Uintptr,
    Fixed,
    Str,
    Ptr(Box<Ty>),
    Slice(Box<Ty>),
    Array(u32, Box<Ty>),
    Struct(Vec<(String, Ty)>),
    Func(Box<Sig>),
    /// A channel: a pointer to Boiler's channel object.
    Chan(Box<Ty>),
    /// An interface: its methods (receiver left out), sorted by name, so
    /// identical method sets are equal types.
    Iface(Vec<(String, Sig)>),
    /// A named type: index into the program's table.
    Named(usize),
    /// Constants without a type yet.
    UntypedInt,
    UntypedBool,
    UntypedStr,
    UntypedNil,
    /// Several results of a call.
    Tuple(Vec<Ty>),
    /// No value (a call of a function without results).
    Void,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sig {
    pub params: Vec<Ty>,
    pub results: Vec<Ty>,
}

/// A named type: its package, name, underlying type, methods.
#[derive(Clone, Debug)]
pub struct Named {
    pub pkg: String,
    pub name: String,
    pub under: Ty,
    /// Method name → (assembly symbol, by pointer, signature without the
    /// receiver).
    pub methods: Vec<(String, String, bool, Sig)>,
}

/// The program's named types.
#[derive(Default, Debug)]
pub struct Types {
    pub named: Vec<Named>,
}

impl Types {
    /// The type under any names.
    pub fn under<'a>(&'a self, t: &'a Ty) -> &'a Ty {
        let mut t = t;
        while let Ty::Named(i) = t {
            t = &self.named[*i].under;
        }
        t
    }

    pub fn size(&self, t: &Ty) -> u32 {
        match self.under(t) {
            Ty::Bool | Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32
            | Ty::U32
            | Ty::Uintptr
            | Ty::Fixed
            | Ty::Ptr(_)
            | Ty::Func(_)
            | Ty::Chan(_) => 4,
            Ty::Str => 6,
            Ty::Slice(_) | Ty::Iface(_) => 8,
            Ty::Array(n, e) => {
                let s = n * self.size(e);
                if s > 1 {
                    s.next_multiple_of(2)
                } else {
                    s
                }
            }
            Ty::Struct(fields) => {
                let mut off = 0u32;
                for (_, f) in fields {
                    let s = self.size(f);
                    if s >= 2 {
                        off = off.next_multiple_of(2);
                    }
                    off += s;
                }
                if off > 1 {
                    off.next_multiple_of(2)
                } else {
                    off
                }
            }
            _ => 0,
        }
    }

    /// A struct field's offset and type.
    pub fn field(&self, t: &Ty, name: &str) -> Option<(u32, Ty)> {
        let Ty::Struct(fields) = self.under(t) else {
            return None;
        };
        let mut off = 0u32;
        for (n, f) in fields {
            let s = self.size(f);
            if s >= 2 {
                off = off.next_multiple_of(2);
            }
            if n == name {
                return Some((off, f.clone()));
            }
            off += s;
        }
        None
    }

    /// Registers a scalar takes (0: moves by address).
    pub fn words(&self, t: &Ty) -> u32 {
        match self.under(t) {
            Ty::Bool | Ty::I8 | Ty::U8 | Ty::I16 | Ty::U16 => 1,
            Ty::I32
            | Ty::U32
            | Ty::Uintptr
            | Ty::Fixed
            | Ty::Ptr(_)
            | Ty::Func(_)
            | Ty::Chan(_) => 2,
            Ty::Str => 3,
            Ty::Slice(_) | Ty::Iface(_) => 4,
            Ty::UntypedInt | Ty::UntypedBool => 1,
            Ty::UntypedNil => 2,
            _ => 0,
        }
    }

    pub fn is_int(&self, t: &Ty) -> bool {
        matches!(
            self.under(t),
            Ty::I8 | Ty::U8 | Ty::I16 | Ty::U16 | Ty::I32 | Ty::U32 | Ty::Uintptr | Ty::UntypedInt
        )
    }

    pub fn is_signed(&self, t: &Ty) -> bool {
        matches!(
            self.under(t),
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::Fixed | Ty::UntypedInt
        )
    }

    pub fn is_numeric(&self, t: &Ty) -> bool {
        self.is_int(t) || matches!(self.under(t), Ty::Fixed)
    }

    /// 32 bits wide in registers (two words, high first).
    pub fn is_wide(&self, t: &Ty) -> bool {
        matches!(
            self.under(t),
            Ty::I32 | Ty::U32 | Ty::Uintptr | Ty::Fixed | Ty::Ptr(_) | Ty::Func(_) | Ty::Chan(_)
        )
    }

    /// An interface type (under its names).
    pub fn is_iface(&self, t: &Ty) -> bool {
        matches!(self.under(t), Ty::Iface(_))
    }

    pub fn is_byte_sized(&self, t: &Ty) -> bool {
        matches!(self.under(t), Ty::Bool | Ty::I8 | Ty::U8)
    }

    pub fn name(&self, t: &Ty) -> String {
        match t {
            Ty::Bool => "bool".into(),
            Ty::I8 => "int8".into(),
            Ty::U8 => "uint8".into(),
            Ty::I16 => "int16".into(),
            Ty::U16 => "uint16".into(),
            Ty::I32 => "int32".into(),
            Ty::U32 => "uint32".into(),
            Ty::Uintptr => "uintptr".into(),
            Ty::Fixed => "fixed".into(),
            Ty::Str => "string".into(),
            Ty::Ptr(e) => format!("*{}", self.name(e)),
            Ty::Chan(e) => format!("chan {}", self.name(e)),
            Ty::Slice(e) => format!("[]{}", self.name(e)),
            Ty::Array(n, e) => format!("[{n}]{}", self.name(e)),
            Ty::Struct(f) => format!(
                "struct {{ {} }}",
                f.iter()
                    .map(|(n, t)| format!("{n} {}", self.name(t)))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Ty::Func(s) => format!(
                "func({}) ({})",
                s.params
                    .iter()
                    .map(|t| self.name(t))
                    .collect::<Vec<_>>()
                    .join(", "),
                s.results
                    .iter()
                    .map(|t| self.name(t))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Ty::Iface(ms) if ms.is_empty() => "interface {}".into(),
            Ty::Iface(ms) => format!(
                "interface {{ {} }}",
                ms.iter()
                    .map(|(n, s)| {
                        let f = self.name(&Ty::Func(Box::new(s.clone())));
                        format!("{n}{}", &f[4..])
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Ty::Named(i) => {
                let n = &self.named[*i];
                if n.pkg.is_empty() || n.pkg == "main" {
                    n.name.clone()
                } else {
                    format!("{}.{}", n.pkg, n.name)
                }
            }
            Ty::UntypedInt => "untyped int".into(),
            Ty::UntypedBool => "untyped bool".into(),
            Ty::UntypedStr => "untyped string".into(),
            Ty::UntypedNil => "nil".into(),
            Ty::Tuple(v) => format!(
                "({})",
                v.iter()
                    .map(|t| self.name(t))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Ty::Void => "no value".into(),
        }
    }

    /// Go's identity of types (names by index, others by structure).
    pub fn identical(&self, a: &Ty, b: &Ty) -> bool {
        a == b
    }

    /// The default type of an untyped constant.
    pub fn default_of(&self, t: &Ty) -> Ty {
        match t {
            Ty::UntypedInt => Ty::I16,
            Ty::UntypedBool => Ty::Bool,
            Ty::UntypedStr => Ty::Str,
            t => t.clone(),
        }
    }

    /// Can a value of type `from` be assigned to `to`?
    pub fn assignable(&self, from: &Ty, to: &Ty) -> bool {
        if from == to {
            return true;
        }
        let (uf, ut) = (self.under(from), self.under(to));
        match from {
            Ty::UntypedInt => self.is_numeric(to),
            Ty::UntypedBool => matches!(ut, Ty::Bool),
            Ty::UntypedStr => matches!(ut, Ty::Str),
            Ty::UntypedNil => matches!(
                ut,
                Ty::Ptr(_) | Ty::Slice(_) | Ty::Func(_) | Ty::Chan(_) | Ty::Iface(_)
            ),
            // One side unnamed with identical underlying types.
            _ => {
                (!matches!(from, Ty::Named(_)) || !matches!(to, Ty::Named(_)))
                    && uf == ut
                    && !matches!(
                        uf,
                        Ty::Bool
                            | Ty::I8
                            | Ty::U8
                            | Ty::I16
                            | Ty::U16
                            | Ty::I32
                            | Ty::U32
                            | Ty::Fixed
                            | Ty::Str
                            | Ty::Uintptr
                    )
            }
        }
    }
}

/// A constant's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Const {
    Int(i64),
    Bool(bool),
    Str(Vec<u8>),
    Nil,
}

impl fmt::Display for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Const::Int(v) => write!(f, "{v}"),
            Const::Bool(b) => write!(f, "{b}"),
            Const::Str(s) => write!(f, "{:?}", String::from_utf8_lossy(s)),
            Const::Nil => write!(f, "nil"),
        }
    }
}

/// The range of an integer type (for constant checks).
pub fn int_range(t: &Ty) -> Option<(i64, i64)> {
    Some(match t {
        Ty::I8 => (-128, 127),
        Ty::U8 => (0, 255),
        Ty::I16 => (-32768, 32767),
        Ty::U16 => (0, 65535),
        Ty::I32 => (i64::from(i32::MIN), i64::from(i32::MAX)),
        Ty::U32 | Ty::Uintptr => (0, i64::from(u32::MAX)),
        Ty::Fixed => (-32768, 32767),
        _ => return None,
    })
}
