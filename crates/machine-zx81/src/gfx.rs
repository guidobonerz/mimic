//! Graphics data views (see `emu_core::gfx`): the character set and the
//! display file, decoded from memory as the CPU sees it, side-effect free.
//!
//! **Character set** (Sinclair ZX81 BASIC Programming manual, appendix A,
//! "The character set"; Logan/O'Hara, *The Complete ZX81 ROM Disassembly*,
//! the character patterns at $1E00): 64 patterns of eight bytes, the most
//! significant bit left; the ULA fetches the row `LCNTR` of code `c` from
//! `I·256 + (c & $3F)·8`, and bit 7 of the code inverts it
//! (`chips_sinclair::ula`). The ROM sets I to $1E; the views use the I
//! register as it is now, so the stub ROM's patterns show the same way.
//!
//! **Display file** (Logan/O'Hara, system variable D_FILE at $400C and the
//! CLS routine): a NEWLINE ($76), then 24 lines of up to 32 character
//! codes, each ending in a NEWLINE. With less than 3¼ KB of RAM the ROM
//! keeps the display collapsed: a line ends at its last non-space
//! character, an empty line is the NEWLINE alone. The views follow the
//! file the same way the display routine does (a line also ends after 32
//! codes).

use emu_core::frame::Rgb8;
use emu_core::gfx::{Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo, OPT_ADDR};

use crate::{Zx81, CPU_SPACE};

/// System variable D_FILE: the display file's address.
pub const D_FILE: u16 = 0x400c;
/// The NEWLINE code ending each line (and executed as HALT).
pub const NEWLINE: u8 = 0x76;

const WHITE: u8 = 0;
const BLACK: u8 = 1;
const GAP: u8 = 2;

const OPT_CHARSET: GfxOptionInfo = GfxOptionInfo {
    name: "charset",
    help: "address of the character patterns for the screen (default: I·256)",
};

/// Parameter names of the line lengths.
const LINE_PARAMS: [&str; 24] = [
    "line0", "line1", "line2", "line3", "line4", "line5", "line6", "line7", "line8", "line9",
    "line10", "line11", "line12", "line13", "line14", "line15", "line16", "line17", "line18",
    "line19", "line20", "line21", "line22", "line23",
];

impl Zx81 {
    pub(crate) fn views(&self) -> Vec<GfxViewInfo> {
        vec![
            GfxViewInfo {
                id: "charset",
                kind: GfxKind::Charset,
                name: "ZX81 character set".into(),
                options: vec![OPT_ADDR],
            },
            GfxViewInfo {
                id: "screen",
                kind: GfxKind::Screen,
                name: "ZX81 display file".into(),
                options: vec![OPT_ADDR, OPT_CHARSET],
            },
        ]
    }

    fn byte(&self, addr: u32) -> u8 {
        self.mem.read(addr as u16)
    }

    /// Draw code `code` at `(x, y)` from the patterns at `base`.
    fn draw_char(&self, c: &mut Canvas, x: u32, y: u32, code: u8, base: u32) {
        for row in 0..8 {
            let mut v = self.byte(base + u32::from(code & 0x3f) * 8 + row);
            if code & 0x80 != 0 {
                v = !v;
            }
            c.row_1bpp(x, y + row, v, BLACK, Some(WHITE), 1);
        }
    }

    pub(crate) fn view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        let info = self.views().into_iter().find(|v| v.id == id)?;
        let patterns = u32::from(self.cpu.i) << 8;
        let (addr, params, c, raw) = if id == "charset" {
            let base = opts.get("addr").map_or(patterns, |a| a & 0xffff);
            // Codes $00–$3F in the upper four rows, $80–$BF below.
            let mut c = Canvas::grid(128, 16, 8, 8, 1, GAP);
            for i in 0..128u32 {
                let code = if i < 64 { i } else { i + 64 } as u8;
                let (x, y) = Canvas::cell_origin(i, 16, 8, 8, 1);
                self.draw_char(&mut c, x, y, code, base);
            }
            let raw = (0..512).map(|i| self.byte(base + i)).collect();
            let params = vec![("base", base), ("i", u32::from(self.cpu.i))];
            (base, params, c, raw)
        } else {
            let d_file = opts.get("addr").map_or_else(
                || {
                    u32::from(self.byte(u32::from(D_FILE)))
                        | u32::from(self.byte(u32::from(D_FILE) + 1)) << 8
                },
                |a| a & 0xffff,
            );
            let charset = opts.get("charset").map_or(patterns, |a| a & 0xffff);
            let mut c = Canvas::new(256, 192, WHITE);
            let mut lens = [0u32; 24];
            // Skip the leading NEWLINE.
            let mut p = d_file + u32::from(self.byte(d_file) == NEWLINE);
            for (line, len) in lens.iter_mut().enumerate() {
                let mut col = 0;
                loop {
                    let code = self.byte(p);
                    if code == NEWLINE {
                        p += 1;
                        break;
                    }
                    if col == 32 {
                        break;
                    }
                    self.draw_char(&mut c, col * 8, line as u32 * 8, code, charset);
                    p += 1;
                    col += 1;
                }
                *len = col;
            }
            let raw = (d_file..p).map(|a| self.byte(a)).collect();
            let mut params = vec![
                ("d_file", d_file),
                ("charset", charset),
                ("bytes", p - d_file),
            ];
            params.extend(LINE_PARAMS.iter().zip(lens).map(|(&n, l)| (n, l)));
            (d_file, params, c, raw)
        };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space: CPU_SPACE,
            addr,
            params,
            width: c.width,
            height: c.height,
            pixels: c.into_pixels(vec![
                Rgb8 {
                    r: 255,
                    g: 255,
                    b: 255,
                },
                Rgb8 { r: 0, g: 0, b: 0 },
                Rgb8 {
                    r: 0x80,
                    g: 0x80,
                    b: 0x80,
                },
            ]),
            raw,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;
    use emu_core::Machine;

    const W: Rgb8 = Rgb8 {
        r: 255,
        g: 255,
        b: 255,
    };
    const B: Rgb8 = Rgb8 { r: 0, g: 0, b: 0 };

    fn stub() -> Zx81 {
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let mut m = Zx81::new(&config).unwrap();
        m.cpu.i = 0x1e;
        m
    }

    #[test]
    fn charset_shows_the_patterns_and_their_inverse() {
        let m = stub();
        let v = m.gfx_view("charset", &GfxOptions::new()).unwrap();
        assert_eq!((v.width, v.height), (16 * 9 - 1, 8 * 9 - 1));
        assert_eq!(v.addr, 0x1e00);
        assert_eq!(v.param("i"), Some(0x1e));
        assert_eq!(v.raw.len(), 512);
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(W), "code 0 blank");
        assert_eq!(f.rgb_at(9, 0), Some(B), "code 1: the stub's box");
        assert_eq!(f.rgb_at(10, 1), Some(W));
        let (x, y) = Canvas::cell_origin(64, 16, 8, 8, 1);
        assert_eq!(f.rgb_at(x, y), Some(B), "code $80: inverse blank");
    }

    #[test]
    fn screen_follows_the_collapsed_display_file() {
        let mut m = stub();
        // D_FILE at $4100: NEWLINE, line 0 = codes 1 and $81, line 1
        // empty, line 2 = 32 codes, then the rest empty.
        let d = 0x4100u16;
        m.mem.write(D_FILE, d as u8);
        m.mem.write(D_FILE + 1, (d >> 8) as u8);
        let mut bytes = vec![NEWLINE, 1, 0x81, NEWLINE, NEWLINE];
        bytes.extend([1u8; 32]);
        bytes.push(NEWLINE);
        bytes.extend([NEWLINE; 21]);
        for (i, &b) in bytes.iter().enumerate() {
            m.mem.write(d + i as u16, b);
        }
        let v = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!((v.width, v.height), (256, 192));
        assert_eq!(v.space, CPU_SPACE);
        assert_eq!(v.addr, u32::from(d));
        assert_eq!(v.param("d_file"), Some(u32::from(d)));
        assert_eq!(v.param("line0"), Some(2));
        assert_eq!(v.param("line1"), Some(0));
        assert_eq!(v.param("line2"), Some(32));
        assert_eq!(v.param("line23"), Some(0));
        assert_eq!(v.raw.len(), bytes.len());
        assert_eq!(v.raw, bytes);
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(B), "box corner");
        assert_eq!(f.rgb_at(9, 1), Some(B), "inverse box inside");
        assert_eq!(f.rgb_at(1, 1), Some(W), "box inside");
        assert_eq!(f.rgb_at(0, 8), Some(W), "empty line");
        assert_eq!(f.rgb_at(255, 16), Some(B), "line 2, column 31");
        // An address option reads another file.
        let o = m
            .gfx_view("screen", &GfxOptions::new().with("addr", u32::from(d) + 4))
            .unwrap();
        assert_eq!(
            o.param("line0"),
            Some(32),
            "line 1's NEWLINE taken as the leading one"
        );
    }
}
