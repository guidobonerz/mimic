//! Assembler with macros and linker for Ada, the CPU of the Logophor 16
//! (a fictional home computer of this project). Used by the tests, to
//! build the Foundry ROM and as the host tool `ada-asm`.
//!
//! Specification: `docs/logophor16/SPEC.md` sections 3 and 12; the
//! encodings come from `cpu_ada::isa`.
//!
//! ```text
//! ada-asm build prog.s [more.s …] -o prog.bin [--base 0040:0000] [--listing prog.lst]
//! ada-asm compile prog.s -o prog.ao        (object file)
//! ada-asm link a.ao b.ao -o prog.bin [--base …] [--listing …]
//! ada-asm run prog.bin [--load 0040:0000] [--entry seg:off] [--cycles N]
//! ada-asm sd card.img [--size MIB] [--boot prog.bin] [--add CARD/PATH=host-file …]
//! ada-asm hub [--port 6516] [--no-gateway]   (relay + gateway 10.16.0.1)
//! ```
#![forbid(unsafe_code)]

pub mod asm;
pub mod expr;
pub mod hub;
pub mod link;
pub mod object;
pub mod run;
pub mod sdimage;

#[cfg(test)]
mod tests;

pub use asm::{assemble, Files, HostFiles};
pub use link::{link, Image, Options};
pub use object::Object;

/// Assemble and link sources from memory (name, text) in one go.
pub fn build(
    sources: &[(&str, &str)],
    files: &dyn Files,
    opts: Options,
) -> Result<Image, Vec<String>> {
    let mut objects = Vec::new();
    let mut errors = Vec::new();
    for (name, text) in sources {
        match assemble(name, text, files) {
            Ok(o) => objects.push(o),
            Err(e) => errors.extend(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    link(&objects, opts)
}
