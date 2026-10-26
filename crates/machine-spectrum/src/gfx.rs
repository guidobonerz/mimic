//! Graphics data views (see `emu_core::gfx`), decoded side-effect free.
//!
//! - **screen:** the 6912-byte screen of a RAM bank (default: the one
//!   the ULA shows) with its attributes — 256 × 192, ink and paper with
//!   bright, flash not applied (see `chips_sinclair::spectrum` for the
//!   layout).
//! - **bitmap:** the same bitmap without attributes (black on white).
//! - **charset:** the 96 characters (space to ©) of the character set at
//!   the system variable CHARS (`$5C36`, the address of the set minus
//!   256), or at the address option, in the CPU's view.
//!
//! Sources: ZX Spectrum BASIC Programming manual, chapter 24 (the screen
//! layout) and chapter 25 (CHARS); Logan/O'Hara, *The Complete Spectrum
//! ROM Disassembly* (the character set at $3D00).

use chips_sinclair::spectrum::{attr_row, bitmap_row, PALETTE};
use emu_core::frame::Rgb8;
use emu_core::gfx::{Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo, OPT_ADDR};

use crate::memory::BANK;
use crate::{Spectrum, CPU_SPACE, RAM_SPACE};

/// System variable CHARS.
pub const CHARS: u16 = 0x5c36;

const OPT_BANK: GfxOptionInfo = GfxOptionInfo {
    name: "bank",
    help: "RAM bank of the screen (default: the one shown)",
};

const WHITE: u8 = 16;
const BLACK: u8 = 17;
const GAP: u8 = 18;

fn palette() -> Vec<Rgb8> {
    let mut p = PALETTE.to_vec();
    p.push(Rgb8 {
        r: 255,
        g: 255,
        b: 255,
    });
    p.push(Rgb8 { r: 0, g: 0, b: 0 });
    p.push(Rgb8 {
        r: 0x80,
        g: 0x80,
        b: 0x80,
    });
    p
}

impl Spectrum {
    pub(crate) fn views(&self) -> Vec<GfxViewInfo> {
        vec![
            GfxViewInfo {
                id: "screen",
                kind: GfxKind::Screen,
                name: "Spectrum screen".into(),
                options: vec![OPT_BANK],
            },
            GfxViewInfo {
                id: "bitmap",
                kind: GfxKind::Bitmap,
                name: "Spectrum bitmap".into(),
                options: vec![OPT_BANK],
            },
            GfxViewInfo {
                id: "charset",
                kind: GfxKind::Charset,
                name: "Spectrum character set".into(),
                options: vec![OPT_ADDR],
            },
        ]
    }

    pub(crate) fn view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        let info = self.views().into_iter().find(|v| v.id == id)?;
        let (space, addr, params, c, raw) = if id == "charset" {
            let chars = u32::from(self.mem.read(CHARS)) | u32::from(self.mem.read(CHARS + 1)) << 8;
            let base = opts.get("addr").map_or(chars + 256, |a| a & 0xffff);
            let mut c = Canvas::grid(96, 16, 8, 8, 1, GAP);
            for i in 0..96 {
                let (x, y) = Canvas::cell_origin(i, 16, 8, 8, 1);
                for row in 0..8 {
                    let v = self.mem.read((base + i * 8 + row) as u16);
                    c.row_1bpp(x, y + row, v, BLACK, Some(WHITE), 1);
                }
            }
            let raw = (0..768).map(|i| self.mem.read((base + i) as u16)).collect();
            (
                CPU_SPACE,
                base,
                vec![("base", base), ("chars", chars)],
                c,
                raw,
            )
        } else {
            let bank = opts
                .get("bank")
                .map_or(self.mem.screen_bank(), |b| (b & 7) as usize);
            let s = &self.mem.bank(bank)[..crate::memory::SCREEN_LEN];
            let mut c = Canvas::new(256, 192, WHITE);
            for y in 0..192 {
                for col in 0..32 {
                    let bits = s[bitmap_row(y) + col];
                    let (ink, paper) = if id == "screen" {
                        let a = s[attr_row(y) + col];
                        let bright = (a >> 3) & 8;
                        ((a & 7) | bright, ((a >> 3) & 7) | bright)
                    } else {
                        (BLACK, WHITE)
                    };
                    c.row_1bpp(col as u32 * 8, y, bits, ink, Some(paper), 1);
                }
            }
            let addr = (bank * BANK) as u32;
            (RAM_SPACE, addr, vec![("bank", bank as u32)], c, s.to_vec())
        };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space,
            addr,
            params,
            width: c.width,
            height: c.height,
            pixels: c.into_pixels(palette()),
            raw,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;
    use emu_core::Machine;

    fn stub(model: &str) -> Spectrum {
        let mut config = MachineConfig {
            model: Some(model.into()),
            ..MachineConfig::default()
        };
        config.options.insert("roms".into(), "stub".into());
        Spectrum::new(&config).unwrap()
    }

    #[test]
    fn screen_with_attributes_and_bank() {
        let mut m = stub("128k");
        m.mem.bank_mut(5)[0] = 0x80;
        m.mem.bank_mut(5)[0x1800] = 0x42; // bright red ink, black paper
        m.mem.bank_mut(7)[0] = 0xff;
        let v = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!((v.width, v.height), (256, 192));
        assert_eq!(v.param("bank"), Some(5));
        assert_eq!(v.space, RAM_SPACE);
        assert_eq!(v.addr, 5 * BANK as u32);
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(PALETTE[10]));
        assert_eq!(f.rgb_at(1, 0), Some(PALETTE[8]));
        let b = m
            .gfx_view("bitmap", &GfxOptions::new().with("bank", 7))
            .unwrap();
        assert_eq!(b.frame().rgb_at(7, 0), Some(Rgb8 { r: 0, g: 0, b: 0 }));
        assert_eq!(b.raw.len(), 6912);
    }

    #[test]
    fn charset_from_chars() {
        let mut m = stub("48k");
        // CHARS = $8000 - 256: the set at $8000.
        m.mem.write(CHARS, 0x00);
        m.mem.write(CHARS + 1, 0x7f);
        m.mem.write(0x8008, 0x80); // "!" row 0
        let v = m.gfx_view("charset", &GfxOptions::new()).unwrap();
        assert_eq!(v.addr, 0x8000);
        assert_eq!(v.raw.len(), 768);
        let (x, y) = Canvas::cell_origin(1, 16, 8, 8, 1);
        assert_eq!(v.frame().rgb_at(x, y), Some(Rgb8 { r: 0, g: 0, b: 0 }));
    }
}
