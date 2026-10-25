//! CRT/monitor emulation as post-processing (golden rule 9): the signal
//! stage ([`signal`]: composite PAL/NTSC, S-Video, PAL delay line,
//! monochrome), the field stage ([`field`]: interlace weave/bob), then
//! the CRT stage: aspect correction, scaling, curvature, scanlines, bloom,
//! phosphor mask, vignette, gamma and phosphor persistence (see [`cpu`]).
//!
//! The signal stage runs on the CPU for every frontend. The CRT stage has
//! two paths with the same model: [`cpu::render`] (deterministic, for
//! headless, preview snapshots and CI) and the WGSL shader [`gpu::SHADER`]
//! used by the desktop. Persistence needs the previous output: [`Crt`] keeps
//! it on the CPU, the desktop keeps a ping-pong pair of render targets.

#![forbid(unsafe_code)]

pub mod cpu;
pub mod field;
pub mod profile;
pub mod signal;

pub use cpu::{render, render_size, Crt, Image};
pub use profile::{Mask, Profile, Signal};

pub mod gpu {
    //! GPU path: the shader source and its uniform block.

    use crate::Profile;

    /// WGSL source of the CRT stage (vertex `vs_main`, fragment `fs_main`).
    pub const SHADER: &str = include_str!("shaders/crt.wgsl");

    /// Size of the `Params` uniform in bytes.
    pub const PARAMS_SIZE: u64 = 80;

    /// The `Params` uniform of [`SHADER`], laid out for WGSL (80 bytes;
    /// `tint` is a `vec3` at offset 16, `origin` a `vec2` at 40). `origin`
    /// is the top-left corner of the output rectangle (`out`) in the
    /// render target, for letterboxing. Binding 2 (`previous`) is read
    /// only when the profile's persistence is above 0; it holds the last
    /// output at (0, 0) with the size of `out`.
    pub fn params(
        profile: &Profile,
        src: (u32, u32),
        out: (u32, u32),
        origin: (u32, u32),
    ) -> [f32; 20] {
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
            profile.persistence,
            origin.0 as f32,
            origin.1 as f32,
            profile.mask.code(),
            profile.mask_strength,
            profile.mask_pitch,
            profile.bloom,
            profile.curvature,
            profile.vignette,
            0.0,
            0.0,
        ]
    }

    /// [`params`] as the bytes to upload.
    pub fn params_bytes(
        profile: &Profile,
        src: (u32, u32),
        out: (u32, u32),
        origin: (u32, u32),
    ) -> Vec<u8> {
        params(profile, src, out, origin)
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect()
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
        assert_eq!(&p[10..12], &[10.0, 20.0]);
        assert_eq!(p[7], 0.35);
        assert_eq!(p[12], Mask::Slot.code());
        assert_eq!(p[16], 0.04);
        assert_eq!(std::mem::size_of_val(&p) as u64, gpu::PARAMS_SIZE);
    }

    /// The WGSL struct's member offsets (as naga lays them out for the
    /// uniform) match the indices `gpu::params` writes.
    #[test]
    fn params_layout_matches_the_wgsl_struct() {
        let module = naga::front::wgsl::parse_str(gpu::SHADER).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Params"))
            .expect("Params struct");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("Params is a struct");
        };
        assert_eq!(u64::from(*span), gpu::PARAMS_SIZE);
        let offset = |name: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("member {name}"))
                .offset
                / 4
        };
        let expect = [
            ("src_size", 0),
            ("out_size", 2),
            ("tint", 4),
            ("scanlines", 7),
            ("gamma", 8),
            ("persistence", 9),
            ("origin", 10),
            ("mask_kind", 12),
            ("mask_strength", 13),
            ("mask_pitch", 14),
            ("bloom", 15),
            ("curvature", 16),
            ("vignette", 17),
        ];
        for (name, index) in expect {
            assert_eq!(offset(name), index, "{name}");
        }
        let p = Profile {
            persistence: 0.25,
            mask: Mask::Aperture,
            mask_strength: 0.5,
            mask_pitch: 2.5,
            bloom: 0.75,
            curvature: 0.125,
            vignette: 0.375,
            ..Profile::default()
        };
        let v = gpu::params(&p, (1, 1), (1, 1), (0, 0));
        assert_eq!(&v[12..18], &[2.0, 0.5, 2.5, 0.75, 0.125, 0.375]);
        assert_eq!(v[9], 0.25);
        assert_eq!(gpu::params_bytes(&p, (1, 1), (1, 1), (0, 0)).len(), 80);
    }
}
