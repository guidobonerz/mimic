//! Graphics data views of the Atari 8-bit computers' ANTIC and GTIA (see
//! `emu_core::gfx`): players and missiles, the character set, the
//! playfield as the display list builds it, and the GTIA palette. Decoded
//! from the chips' registers and ANTIC's view of memory, side-effect free.
//!
//! Sources: Altirra Hardware Reference Manual, "ANTIC" (display list
//! instructions, LMS/JMP/JVB, the 1 KB display-list and 4 KB memory-scan
//! counters, playfield widths, character modes and CHACTL) and "GTIA"
//! (player/missile graphics, PRIOR, colour registers); Atari 400/800
//! hardware manual (PMBASE layout: double-line missiles at +$180, players
//! at +$200 + $80·n; single-line missiles at +$300, players at +$400 +
//! $100·n).
//!
//! Approximations (a memory view, not the beam): the screen view draws
//! every mode line in full (no vertical or horizontal scroll offsets; a
//! horizontally scrolled line shows the wider fetch it makes), ignores
//! GTIA modes 9–11 and players, and stops at 240 scan lines like the
//! display does.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
    OPT_ADDR, OPT_INDEX,
};
use emu_core::MemSpaceId;

use super::{char_data, pixel, width_edges, Antic, AnticMemory, Playfield, MODES};
use crate::gtia::Gtia;

/// Colour register value of the gaps between cells (black).
const GAP: u8 = 0;
/// Scan lines ANTIC can display (8–247).
const DISPLAY_LINES: u32 = 240;
/// Output width in hires pixels (colour clocks 32–223, as GTIA's output).
const SCREEN_WIDTH: u32 = 384;
/// Display-list instructions walked at most (a JMP loop never ends).
const MAX_INSTR: u32 = 1024;

const OPT_SOURCE: GfxOptionInfo = GfxOptionInfo {
    name: "source",
    help: "0 = P/M memory at PMBASE, 1 = the GRAFPx/GRAFM registers (default: memory while P/M DMA is on)",
};
const OPT_SINGLE: GfxOptionInfo = GfxOptionInfo {
    name: "single",
    help: "1 = single-line, 0 = double-line player resolution (default: DMACTL bit 4)",
};
const OPT_CHARMODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "ANTIC mode 2 (hires text), 4 (4-colour text) or 6 (20-column text); 3/5/7 count as 2/4/6 (default: the current mode line's)",
};

/// What the machine tells the decoder besides the chips.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    /// Memory as ANTIC sees it (the 130XE banks it apart from the CPU).
    pub mem: &'a dyn AnticMemory,
    /// The space ANTIC's addresses are reported in.
    pub space: MemSpaceId,
    /// The CPU's space, for views taken from the GTIA registers.
    pub io_space: MemSpaceId,
    /// The 256 GTIA colours, indexed by colour register value.
    pub palette: &'a [Rgb8],
}

/// The views ANTIC and GTIA offer (players, charset, screen, palette).
pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "players",
            kind: GfxKind::Sprites,
            name: "GTIA players and missiles".into(),
            options: vec![OPT_INDEX, OPT_ADDR, OPT_SOURCE, OPT_SINGLE],
        },
        GfxViewInfo {
            id: "charset",
            kind: GfxKind::Charset,
            name: "ANTIC character set".into(),
            options: vec![OPT_ADDR, OPT_CHARMODE],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: "ANTIC playfield (display list)".into(),
            options: vec![OPT_ADDR],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: "GTIA palette".into(),
            options: vec![],
        },
    ]
}

/// A decoded picture before it becomes a [`GfxView`].
struct Parts {
    space: MemSpaceId,
    addr: u32,
    params: Vec<(&'static str, u32)>,
    canvas: Canvas,
    raw: Vec<u8>,
}

const HPOSP: [&str; 4] = ["hposp0", "hposp1", "hposp2", "hposp3"];
const HPOSM: [&str; 4] = ["hposm0", "hposm1", "hposm2", "hposm3"];
const SIZEP: [&str; 4] = ["sizep0", "sizep1", "sizep2", "sizep3"];
const COLPM: [&str; 4] = ["colpm0", "colpm1", "colpm2", "colpm3"];
const COLPF: [&str; 4] = ["colpf0", "colpf1", "colpf2", "colpf3"];

impl Antic {
    /// Decode one view (`kind` from [`views`]).
    pub fn gfx_view(
        &self,
        gtia: &Gtia,
        id: &'static str,
        kind: GfxKind,
        name: String,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> GfxView {
        let mut palette = ctx.palette.to_vec();
        palette.resize(256, Rgb8::default());
        let parts = match kind {
            GfxKind::Palette => {
                let (width, height, pixels) = palette_canvas(&palette, 16, 8);
                return GfxView {
                    id,
                    kind,
                    name,
                    space: ctx.io_space,
                    addr: 0,
                    params: vec![("count", 256)],
                    width,
                    height,
                    pixels,
                    raw: (0..=255).collect(),
                };
            }
            GfxKind::Sprites => self.players_view(gtia, ctx, opts),
            GfxKind::Charset => self.charset_view(gtia, ctx, opts),
            _ => self.screen_view(gtia, ctx, opts),
        };
        GfxView {
            id,
            kind,
            name,
            space: parts.space,
            addr: parts.addr,
            params: parts.params,
            width: parts.canvas.width,
            height: parts.canvas.height,
            pixels: GfxPixels::Indexed {
                pixels: parts.canvas.pixels,
                palette,
            },
            raw: parts.raw,
        }
    }

    /// The missiles (one column, missile n in bits 2n and 2n + 1) and
    /// players 0–3 side by side in memory order, 8 pixels wide each at
    /// normal size; `raw` holds their bytes in the same order (from P/M
    /// memory one block from `addr`). From P/M
    /// memory: 256 rows (double-line bytes drawn two rows high); from the
    /// GRAFPx/GRAFM registers: the byte repeated over 8 rows.
    fn players_view(&self, gtia: &Gtia, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Parts {
        let from_regs = match opts.get("source") {
            Some(s) => s != 0,
            None => opts.get("addr").is_none() && self.dmactl & 0x0c == 0,
        };
        let single = opts.flag("single").unwrap_or(self.dmactl & 0x10 != 0);
        let base = opts
            .get("addr")
            .map_or(u16::from(self.pmbase) << 8, |a| a as u16)
            & if single { 0xf800 } else { 0xfc00 };
        let only = opts.get("index").map(|i| i.min(4) as usize);
        // Memory order: missiles, then players 0-3.
        let objects: Vec<usize> = [4, 0, 1, 2, 3]
            .into_iter()
            .filter(|&n| only.is_none_or(|i| i == n))
            .collect();
        let fifth = gtia.prior & 0x10 != 0;
        let missile_colors: [u8; 4] =
            std::array::from_fn(|n| if fifth { gtia.colpf[3] } else { gtia.colpm[n] });
        let rows: u16 = if single { 256 } else { 128 };
        let row_h: u32 = if from_regs {
            8
        } else if single {
            1
        } else {
            2
        };
        let cell_h = if from_regs { 8 } else { 256 };
        let count = objects.len() as u32;
        let mut c = Canvas::grid(count, count, 8, cell_h, 1, GAP);
        let mut raw = Vec::new();
        let offset = |n: usize| -> u16 {
            match (n, single) {
                (4, true) => 0x300,
                (4, false) => 0x180,
                (n, true) => 0x400 + 0x100 * n as u16,
                (n, false) => 0x200 + 0x80 * n as u16,
            }
        };
        for (cell, &n) in objects.iter().enumerate() {
            let (x0, y0) = Canvas::cell_origin(cell as u32, count, 8, cell_h, 1);
            c.fill(x0, y0, 8, cell_h, gtia.colbk);
            let bytes: Vec<u8> = if from_regs {
                vec![if n == 4 { gtia.grafm } else { gtia.grafp[n] }]
            } else {
                (0..rows)
                    .map(|r| ctx.mem.dma_read(base.wrapping_add(offset(n) + r)))
                    .collect()
            };
            for (r, &v) in bytes.iter().enumerate() {
                for dy in 0..row_h {
                    let y = y0 + r as u32 * row_h + dy;
                    if n == 4 {
                        for b in 0..8u32 {
                            if v & (0x80 >> b) != 0 {
                                c.set(x0 + b, y, missile_colors[(7 - b as usize) / 2]);
                            }
                        }
                    } else {
                        c.row_1bpp(x0, y, v, gtia.colpm[n], None, 1);
                    }
                }
            }
            raw.extend_from_slice(&bytes);
        }
        let (space, addr) = if from_regs {
            let first = objects.first().copied().unwrap_or(4);
            // GRAFP0–3 at $D00D–$D010, GRAFM at $D011.
            (ctx.io_space, 0xd00d + first as u32)
        } else {
            let first = objects.first().copied().unwrap_or(4);
            (ctx.space, u32::from(base.wrapping_add(offset(first))))
        };
        let mut params = vec![
            ("source", u32::from(from_regs)),
            ("pmbase", u32::from(base)),
            ("single", u32::from(single)),
            ("dmactl", u32::from(self.dmactl)),
            ("gractl", u32::from(gtia.gractl)),
            ("prior", u32::from(gtia.prior)),
            ("vdelay", u32::from(gtia.vdelay)),
        ];
        params.extend(
            HPOSP
                .iter()
                .zip(gtia.hposp)
                .map(|(&k, v)| (k, u32::from(v))),
        );
        params.extend(
            SIZEP
                .iter()
                .zip(gtia.sizep)
                .map(|(&k, v)| (k, u32::from(v))),
        );
        params.extend(
            HPOSM
                .iter()
                .zip(gtia.hposm)
                .map(|(&k, v)| (k, u32::from(v))),
        );
        params.push(("sizem", u32::from(gtia.sizem)));
        params.extend(
            COLPM
                .iter()
                .zip(gtia.colpm)
                .map(|(&k, v)| (k, u32::from(v))),
        );
        params.push(("colpf3", u32::from(gtia.colpf[3])));
        params.push(("colbk", u32::from(gtia.colbk)));
        Parts {
            space,
            addr,
            params,
            canvas: c,
            raw,
        }
    }

    /// The character set at CHBASE: 128 characters (64 in modes 6/7), 16
    /// to a row, in hires pixels as the mode draws them: mode 2 in PF2
    /// with PF1's luminance, mode 4 in BK/PF0/PF1/PF2, mode 6 double wide
    /// in PF0 on BK. The patterns are shown as stored; CHACTL's reflection
    /// and inversion only apply on the screen (see the params).
    fn charset_view(&self, gtia: &Gtia, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Parts {
        let chip_mode = self.ir.map(|ir| ir & 0x0f).filter(|m| (2..=7).contains(m));
        let mode: u8 = match opts.get("mode").map_or(chip_mode.unwrap_or(2), |m| m as u8) {
            4 | 5 => 4,
            6 | 7 => 6,
            _ => 2,
        };
        let mask = if mode == 6 { 0xfe00 } else { 0xfc00 };
        let base = opts
            .get("addr")
            .map_or(u16::from(self.chbase) << 8, |a| a as u16)
            & mask;
        let count: u16 = if mode == 6 { 64 } else { 128 };
        let cell_w = if mode == 6 { 16 } else { 8 };
        let mut c = Canvas::grid(u32::from(count), 16, cell_w, 8, 1, GAP);
        let lum = (gtia.colpf[2] & 0xf0) | (gtia.colpf[1] & 0x0f);
        let mut raw = Vec::with_capacity(usize::from(count) * 8);
        for ch in 0..count {
            let (x0, y0) = Canvas::cell_origin(u32::from(ch), 16, cell_w, 8, 1);
            for row in 0..8u16 {
                let v = ctx.mem.dma_read(base.wrapping_add(ch * 8 + row));
                raw.push(v);
                let y = y0 + u32::from(row);
                match mode {
                    2 => c.row_1bpp(x0, y, v, lum, Some(gtia.colpf[2]), 1),
                    4 => c.row_2bpp(
                        x0,
                        y,
                        v,
                        [
                            Some(gtia.colbk),
                            Some(gtia.colpf[0]),
                            Some(gtia.colpf[1]),
                            Some(gtia.colpf[2]),
                        ],
                        1,
                    ),
                    _ => c.row_1bpp(x0, y, v, gtia.colpf[0], Some(gtia.colbk), 2),
                }
            }
        }
        let mut params = vec![
            ("charbase", u32::from(base)),
            ("chbase", u32::from(self.chbase)),
            ("mode", u32::from(mode)),
            ("count", u32::from(count)),
            ("chactl", u32::from(self.chactl)),
            ("blank", u32::from(self.chactl & 0x01 != 0)),
            ("invert", u32::from(self.chactl & 0x02 != 0)),
            ("reflect", u32::from(self.chactl & 0x04 != 0)),
            ("colbk", u32::from(gtia.colbk)),
        ];
        params.extend(
            COLPF
                .iter()
                .zip(gtia.colpf)
                .map(|(&k, v)| (k, u32::from(v))),
        );
        Parts {
            space: ctx.space,
            addr: u32::from(base),
            params,
            canvas: c,
            raw,
        }
    }

    /// The playfield as the display list from `addr` (default: where the
    /// field's display list began) builds it: every mode line at its own
    /// resolution, 384 hires pixels wide (colour clocks 32–223) with the
    /// playfield placed for its width, one picture row per scan line.
    fn screen_view(&self, gtia: &Gtia, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Parts {
        let start = opts.get("addr").map_or(self.dlist_start, |a| a as u16);
        let width_sel = match self.dmactl & 3 {
            0 => 2,
            w => w,
        };
        let lum = (gtia.colpf[2] & 0xf0) | (gtia.colpf[1] & 0x0f);
        let colour = |pf: Playfield| -> [u8; 2] {
            match pf {
                Playfield::Background => [gtia.colbk; 2],
                Playfield::Pf(n) => [gtia.colpf[usize::from(n & 3)]; 2],
                Playfield::Hires(bits) => [
                    if bits & 2 != 0 { lum } else { gtia.colpf[2] },
                    if bits & 1 != 0 { lum } else { gtia.colpf[2] },
                ],
            }
        };
        let mut c = Canvas::new(SCREEN_WIDTH, DISPLAY_LINES, gtia.colbk);
        let mut dl = start;
        let next = |dl: &mut u16| -> u8 {
            let v = ctx.mem.dma_read(*dl);
            // The display-list counter counts within a 1 KB block.
            *dl = (*dl & 0xfc00) | (dl.wrapping_add(1) & 0x03ff);
            v
        };
        let (mut y, mut dl_bytes, mut mode_lines, mut lms_count) = (0u32, 0u32, 0u32, 0u32);
        let mut first_lms = None;
        let mut memscan = 0u16;
        let mut jvb = false;
        let mut raw = Vec::new();
        for _ in 0..MAX_INSTR {
            if y >= DISPLAY_LINES {
                break;
            }
            let ir = next(&mut dl);
            dl_bytes += 1;
            let mode = ir & 0x0f;
            match mode {
                0 => y += u32::from((ir >> 4) & 7) + 1,
                1 => {
                    let lo = next(&mut dl);
                    let hi = next(&mut dl);
                    dl_bytes += 2;
                    if ir & 0x40 != 0 {
                        jvb = true;
                        break;
                    }
                    dl = u16::from_le_bytes([lo, hi]);
                    // A JMP takes one blank scan line.
                    y += 1;
                }
                _ => {
                    if ir & 0x40 != 0 {
                        let lo = next(&mut dl);
                        let hi = next(&mut dl);
                        dl_bytes += 2;
                        memscan = u16::from_le_bytes([lo, hi]);
                        first_lms.get_or_insert(memscan);
                        lms_count += 1;
                    }
                    mode_lines += 1;
                    let info = MODES[usize::from(mode)];
                    let width = if ir & 0x10 != 0 {
                        (width_sel + 1).min(3)
                    } else {
                        width_sel
                    };
                    let bytes = u16::from(info.bytes) * [0, 4, 5, 6][usize::from(width)] / 5;
                    let cc_per_byte = 160 / u16::from(info.bytes);
                    let x0 = u32::from(width_edges(width).0 - 32) * 2;
                    let line: Vec<u8> = (0..bytes)
                        .map(|i| {
                            let a = (memscan & 0xf000) | (memscan.wrapping_add(i) & 0x0fff);
                            ctx.mem.dma_read(a)
                        })
                        .collect();
                    raw.extend_from_slice(&line);
                    for row in 0..info.lines {
                        if y >= DISPLAY_LINES {
                            break;
                        }
                        for (i, &name) in line.iter().enumerate() {
                            let d = if info.chars {
                                char_data(ctx.mem, mode, self.chbase, self.chactl, name, row)
                            } else {
                                name
                            };
                            for sub in 0..cc_per_byte {
                                let [a, b] = colour(pixel(mode, d, name, sub));
                                let x =
                                    x0 + (i as u32 * u32::from(cc_per_byte) + u32::from(sub)) * 2;
                                c.set(x, y, a);
                                c.set(x + 1, y, b);
                            }
                        }
                        y += 1;
                    }
                    memscan = (memscan & 0xf000) | (memscan.wrapping_add(bytes) & 0x0fff);
                }
            }
        }
        let lines = y.min(DISPLAY_LINES);
        c.height = lines.max(1);
        c.pixels.truncate((c.width * c.height) as usize);
        let params = vec![
            ("dlist", u32::from(start)),
            ("dlist_bytes", dl_bytes),
            ("lines", lines),
            ("mode_lines", mode_lines),
            ("first_lms", u32::from(first_lms.unwrap_or(0))),
            ("lms_count", lms_count),
            ("jvb", u32::from(jvb)),
            ("dmactl", u32::from(self.dmactl)),
            ("width", u32::from(width_sel)),
            ("chbase", u32::from(self.chbase)),
            ("chactl", u32::from(self.chactl)),
            ("prior", u32::from(gtia.prior)),
            ("colbk", u32::from(gtia.colbk)),
        ];
        Parts {
            space: ctx.space,
            addr: u32::from(first_lms.unwrap_or(start)),
            params,
            canvas: c,
            raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette;
    use emu_core::frame::VideoStandard;

    struct Ram(Vec<u8>);

    impl AnticMemory for Ram {
        fn dma_read(&self, addr: u16) -> u8 {
            self.0[usize::from(addr)]
        }
    }

    fn ctx<'a>(ram: &'a Ram, pal: &'a [Rgb8]) -> GfxContext<'a> {
        GfxContext {
            mem: ram,
            space: MemSpaceId(1),
            io_space: MemSpaceId(0),
            palette: pal,
        }
    }

    fn view(antic: &Antic, gtia: &Gtia, ram: &Ram, kind: GfxKind, opts: GfxOptions) -> GfxView {
        let pal = palette::palette(VideoStandard::Pal);
        antic.gfx_view(gtia, "v", kind, "v".into(), &ctx(ram, &pal), &opts)
    }

    fn colour_at(v: &GfxView, x: u32, y: u32) -> u8 {
        match &v.pixels {
            GfxPixels::Indexed { pixels, .. } => pixels[(y * v.width + x) as usize],
            GfxPixels::Rgb(_) => panic!("indexed expected"),
        }
    }

    #[test]
    fn players_come_from_pm_memory_or_the_registers() {
        let mut ram = Ram(vec![0; 0x10000]);
        let mut antic = Antic::new(VideoStandard::Pal);
        let mut gtia = Gtia::new(VideoStandard::Pal);
        antic.write(0x07, 0x30); // PMBASE $3000
        antic.write(0x00, 0x3e); // P/M DMA, single line
        gtia.write(0x14, 0x46); // COLPM2
        gtia.write(0x1a, 0x02); // COLBK
        gtia.write(0x02, 0x80); // HPOSP2
        ram.0[0x3000 + 0x600 + 10] = 0x80; // player 2, row 10
        ram.0[0x3000 + 0x300 + 5] = 0x03; // missile 0, row 5
        let v = view(&antic, &gtia, &ram, GfxKind::Sprites, GfxOptions::new());
        assert_eq!((v.width, v.height), (5 * 9 - 1, 256));
        assert_eq!(v.raw.len(), 1280);
        assert_eq!(v.addr, 0x3300, "missiles first");
        assert_eq!(v.param("single"), Some(1));
        assert_eq!(v.param("hposp2"), Some(0x80));
        assert_eq!(colour_at(&v, 27, 10), 0x46);
        assert_eq!(colour_at(&v, 28, 10), 0x02);
        assert_eq!(colour_at(&v, 7, 5), gtia.colpm[0], "missile 0 is bits 0-1");
        assert_eq!(v.raw[0x300 + 10], 0x80);

        // Double-line, one player: 128 bytes drawn two rows high.
        antic.write(0x00, 0x2e);
        let one = view(
            &antic,
            &gtia,
            &ram,
            GfxKind::Sprites,
            GfxOptions::new().with("index", 2),
        );
        assert_eq!((one.width, one.height, one.raw.len()), (8, 256, 128));
        assert_eq!(one.addr, 0x3000 + 0x300);
        // $3305 (single-line missile row 5) is double-line player 2's byte 5.
        assert_eq!(one.raw[5], 0x03);
        assert_eq!(
            colour_at(&one, 7, 10),
            0x46,
            "byte 5 drawn on rows 10 and 11"
        );
        assert_eq!(colour_at(&one, 7, 11), 0x46);
        assert_eq!(colour_at(&one, 7, 12), 0x02);

        // Without P/M DMA the GRAF registers show.
        antic.write(0x00, 0x22);
        gtia.write(0x0d, 0xf0); // GRAFP0
        let regs = view(&antic, &gtia, &ram, GfxKind::Sprites, GfxOptions::new());
        assert_eq!(regs.param("source"), Some(1));
        assert_eq!((regs.width, regs.height), (44, 8));
        assert_eq!(regs.raw, vec![0, 0xf0, 0, 0, 0]);
        assert_eq!((regs.space, regs.addr), (MemSpaceId(0), 0xd011));
        assert_eq!(colour_at(&regs, 9 + 3, 7), gtia.colpm[0]);
    }

    #[test]
    fn charset_decodes_the_three_text_modes() {
        let mut ram = Ram(vec![0; 0x10000]);
        let mut antic = Antic::new(VideoStandard::Pal);
        let mut gtia = Gtia::new(VideoStandard::Pal);
        antic.write(0x09, 0xe0); // CHBASE $E000
        antic.write(0x01, 0x04); // reflect
        gtia.write(0x17, 0x0a); // COLPF1 luma A
        gtia.write(0x18, 0x94); // COLPF2
        gtia.write(0x16, 0x26); // COLPF0
        ram.0[0xe000 + 8] = 0x80; // char 1 row 0
        let v = view(&antic, &gtia, &ram, GfxKind::Charset, GfxOptions::new());
        assert_eq!((v.width, v.height), (16 * 9 - 1, 8 * 9 - 1));
        assert_eq!(v.raw.len(), 1024);
        assert_eq!(v.param("reflect"), Some(1));
        assert_eq!(colour_at(&v, 9, 0), 0x9a, "PF2 hue, PF1 luma");
        assert_eq!(colour_at(&v, 10, 0), 0x94);
        let m4 = view(
            &antic,
            &gtia,
            &ram,
            GfxKind::Charset,
            GfxOptions::new().with("mode", 4),
        );
        assert_eq!(colour_at(&m4, 9, 0), gtia.colpf[1], "pair 10 = PF1");
        let m6 = view(
            &antic,
            &gtia,
            &ram,
            GfxKind::Charset,
            GfxOptions::new().with("mode", 7),
        );
        assert_eq!(m6.param("mode"), Some(6));
        assert_eq!(
            (m6.width, m6.height, m6.raw.len()),
            (16 * 17 - 1, 4 * 9 - 1, 512)
        );
        assert_eq!(colour_at(&m6, 17, 0), 0x26);
        assert_eq!(colour_at(&m6, 18, 0), 0x26, "double-wide pixel");
        assert_eq!(colour_at(&m6, 19, 0), gtia.colbk);
    }

    #[test]
    fn screen_walks_the_display_list() {
        let mut ram = Ram(vec![0; 0x10000]);
        let mut antic = Antic::new(VideoStandard::Pal);
        let mut gtia = Gtia::new(VideoStandard::Pal);
        antic.write(0x00, 0x22); // DL DMA, normal width
        antic.write(0x09, 0xe0);
        gtia.write(0x17, 0x0e);
        gtia.write(0x18, 0x94);
        gtia.write(0x1a, 0x00);
        gtia.write(0x16, 0x46);
        // 8 blank lines, mode 2 LMS $4000, mode $0D, JVB.
        let dl = [0x70, 0x42, 0x00, 0x40, 0x0d, 0x41, 0x00, 0x20];
        ram.0[0x2000..0x2008].copy_from_slice(&dl);
        ram.0[0x4000] = 1; // char 1
        ram.0[0xe008] = 0xc0; // char 1, row 0: two pixels
        ram.0[0x4000 + 40] = 0x40; // mode D byte 0: pixel 0 = PF0
        let v = view(
            &antic,
            &gtia,
            &ram,
            GfxKind::Screen,
            GfxOptions::new().with("addr", 0x2000),
        );
        assert_eq!((v.width, v.height), (384, 8 + 8 + 2));
        assert_eq!(v.param("first_lms"), Some(0x4000));
        assert_eq!(v.param("mode_lines"), Some(2));
        assert_eq!(v.param("jvb"), Some(1));
        assert_eq!(v.param("dlist_bytes"), Some(8));
        assert_eq!(v.addr, 0x4000);
        assert_eq!(v.raw.len(), 80);
        // Normal width starts at hires pixel 32.
        assert_eq!(colour_at(&v, 32, 8), 0x9e);
        assert_eq!(colour_at(&v, 34, 8), 0x94);
        assert_eq!(colour_at(&v, 31, 8), 0x00, "border");
        assert_eq!(colour_at(&v, 32, 16), 0x46);
        assert_eq!(colour_at(&v, 33, 17), 0x46);
        assert_eq!(colour_at(&v, 34, 17), 0x00);
        // The default start is the one latched at line 8.
        antic.dlist_start = 0x2000;
        let d = view(&antic, &gtia, &ram, GfxKind::Screen, GfxOptions::new());
        assert_eq!(d.param("dlist"), Some(0x2000));
    }

    #[test]
    fn palette_has_256_colours() {
        let ram = Ram(vec![0; 0x10000]);
        let antic = Antic::new(VideoStandard::Ntsc);
        let gtia = Gtia::new(VideoStandard::Ntsc);
        let v = view(&antic, &gtia, &ram, GfxKind::Palette, GfxOptions::new());
        let pal = palette::palette(VideoStandard::Pal);
        assert_eq!(v.param("count"), Some(256));
        assert_eq!(v.raw.len(), 256);
        assert_eq!(v.frame().rgb_at(9, 0), Some(pal[1]));
    }
}
