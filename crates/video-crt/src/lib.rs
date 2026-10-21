//! CRT/monitor emulation as post-processing (golden rule 9): the signal
//! stage ([`signal`]: composite PAL/NTSC, S-Video, PAL delay line,
//! monochrome), the field stage ([`field`]: interlace weave/bob), then
//! aspect correction, scaling, scanlines, gamma; later the full CRT stage.
//!
//! The signal stage runs on the CPU for every frontend. The CRT stage has
//! two paths with the same model: [`cpu::render`] (deterministic, for
//! headless, preview snapshots and CI) and the WGSL shader [`gpu::SHADER`]
//! used by the desktop and web frontends.

#![forbid(unsafe_code)]

pub mod cpu;
pub mod field;
pub mod profile;
pub mod signal;

pub use cpu::{render, Image};
pub use profile::{Profile, Signal};

pub mod gpu {
    //! GPU path: the shader source and its uniform block.

    use crate::Profile;

    /// WGSL source of the CRT stage (vertex `vs_main`, fragment `fs_main`).
    pub const SHADER: &str = include_str!("shaders/crt.wgsl");

    /// The `Params` uniform of [`SHADER`], laid out for WGSL (48 bytes).
    /// `origin` is the top-left corner of the output rectangle (`out`) in
    /// the render target, for letterboxing.
    pub fn params(
        profile: &Profile,
        src: (u32, u32),
        out: (u32, u32),
        origin: (u32, u32),
    ) -> [f32; 12] {
        [
            src.0 as f32,
            src.1 as f32,
            out.0 as f32,
            out.1 as f32,
            profile.tint[0],
            profile.tint[1],
            profile.tint[2],
            profile.scanlines,
            profile.gamma,
            0.0,
            origin.0 as f32,
            origin.1 as f32,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_compiles_and_validates() {
        let module = naga::front::wgsl::parse_str(gpu::SHADER).expect("WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("WGSL validates");
    }

    #[test]
    fn shader_declares_its_entry_points_and_params() {
        assert!(gpu::SHADER.contains("fn vs_main"));
        assert!(gpu::SHADER.contains("fn fs_main"));
        let p = gpu::params(
            &Profile::preset("1702").unwrap(),
            (404, 284),
            (756, 568),
            (10, 20),
        );
        assert_eq!(&p[10..], &[10.0, 20.0]);
        assert_eq!(p[7], 0.35);
        assert_eq!(std::mem::size_of_val(&p), 48);
    }
}
