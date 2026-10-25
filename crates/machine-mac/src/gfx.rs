//! Graphics data view (see `emu_core::gfx`): the screen buffer, decoded
//! from RAM as stored, side-effect free.
//!
//! The compact Macintosh shows 512 × 342 pixels of one bit each, 64 bytes
//! a line, most significant bit left, 1 = black (Inside Macintosh III,
//! "The Macintosh Hardware", "The Video Interface"; Guide to the Macintosh
//! Family Hardware, 2nd ed., chapter 3). The main buffer starts $5900
//! below the top of RAM, the alternate one $D900 below it; VIA PA6 = 1
//! selects the main buffer ([`crate::video`]). The view follows the same
//! rule as the video circuit for every model and RAM size.

use emu_core::frame::Rgb8;
use emu_core::gfx::{Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo, OPT_ADDR};

use crate::{video, Mac, RAM_SPACE};

const OPT_BUFFER: GfxOptionInfo = GfxOptionInfo {
    name: "index",
    help: "0 main, 1 alternate screen buffer (default: as VIA PA6 selects)",
};

/// Bytes of one screen buffer.
pub const SCREEN_BYTES: u32 = video::WIDTH / 8 * video::HEIGHT;

const WHITE: Rgb8 = Rgb8 {
    r: 255,
    g: 255,
    b: 255,
};
const BLACK: Rgb8 = Rgb8 { r: 0, g: 0, b: 0 };

impl Mac {
    pub(crate) fn views(&self) -> Vec<GfxViewInfo> {
        vec![GfxViewInfo {
            id: "screen",
            kind: GfxKind::Bitmap,
            name: "Macintosh screen buffer".into(),
            options: vec![OPT_BUFFER, OPT_ADDR],
        }]
    }

    /// The 512 × 342 bitmap of the buffer VIA PA6 selects, option `index`
    /// picks or `addr` names (an address in RAM, wrapped by its size).
    pub(crate) fn view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        let info = self.views().into_iter().find(|v| v.id == id)?;
        let b = &self.board;
        let top = b.ram_size();
        let mask = b.ram.len() as u32 - 1;
        let pa6 = b.via.port_a_out() & 0x40 != 0;
        let alternate = opts.get("index").map_or(!pa6, |i| i != 0);
        let base = match opts.get("addr") {
            Some(a) => a & mask,
            None => {
                top - if alternate {
                    video::ALT_SCREEN
                } else {
                    video::MAIN_SCREEN
                }
            }
        };
        let raw: Vec<u8> = (0..SCREEN_BYTES)
            .map(|i| b.ram[((base + i) & mask) as usize])
            .collect();
        let mut c = Canvas::new(video::WIDTH, video::HEIGHT, 0);
        for (i, &v) in raw.iter().enumerate() {
            let (x, y) = ((i % 64) as u32 * 8, (i / 64) as u32);
            c.row_1bpp(x, y, v, 1, None, 1);
        }
        let params = vec![
            ("base", base),
            (
                "alternate",
                u32::from(opts.get("addr").is_none() && alternate),
            ),
            ("pa6", u32::from(pa6)),
            ("ram", top),
            ("bytes_per_line", video::WIDTH / 8),
        ];
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space: RAM_SPACE,
            addr: base,
            params,
            width: video::WIDTH,
            height: video::HEIGHT,
            pixels: c.into_pixels(vec![WHITE, BLACK]),
            raw,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;
    use emu_core::Machine;

    fn mac(model: &str) -> Mac {
        let mut config = MachineConfig {
            model: Some(model.into()),
            ..MachineConfig::default()
        };
        config.options.insert("roms".into(), "stub".into());
        Mac::new(&config).unwrap()
    }

    #[test]
    fn screen_buffers_sit_below_the_top_of_ram() {
        let mut m = mac("plus");
        let top = m.board.ram_size();
        let main = top - video::MAIN_SCREEN;
        let alt = top - video::ALT_SCREEN;
        m.board.ram[main as usize] = 0x80; // top-left pixel
        m.board.ram[(main + 64 + 1) as usize] = 0x01; // line 1, pixel 15
        m.board.ram[alt as usize] = 0x40;
        let v = m
            .gfx_view("screen", &GfxOptions::new().with("index", 0))
            .unwrap();
        assert_eq!((v.width, v.height), (512, 342));
        assert_eq!(v.space, RAM_SPACE);
        assert_eq!(v.addr, main);
        assert_eq!(v.param("base"), Some(main));
        assert_eq!(v.param("alternate"), Some(0));
        assert_eq!(v.raw.len(), 21_888);
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(BLACK));
        assert_eq!(f.rgb_at(1, 0), Some(WHITE));
        assert_eq!(f.rgb_at(15, 1), Some(BLACK));
        let a = m
            .gfx_view("screen", &GfxOptions::new().with("index", 1))
            .unwrap();
        assert_eq!(a.addr, alt);
        assert_eq!(a.frame().rgb_at(1, 0), Some(BLACK));
        let by_addr = m
            .gfx_view("screen", &GfxOptions::new().with("addr", alt))
            .unwrap();
        assert_eq!(by_addr.raw, a.raw);
        assert!(m.gfx_view("charset", &GfxOptions::new()).is_none());
    }

    #[test]
    fn default_follows_via_pa6() {
        let mut m = mac("128k");
        let top = m.board.ram_size();
        m.board.via.ddra = 0xff;
        m.board.via.ora = 0x40;
        let v = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!(
            (v.addr, v.param("pa6")),
            (top - video::MAIN_SCREEN, Some(1))
        );
        m.board.via.ora = 0x00;
        let v = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!(v.addr, top - video::ALT_SCREEN);
        assert_eq!(v.param("ram"), Some(128 * 1024));
    }
}
