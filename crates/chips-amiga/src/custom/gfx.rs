//! Graphics data views of Agnus/Alice and Denise/Lisa (see `emu_core::gfx`):
//! the eight hardware sprites, the bitplanes and the colour registers,
//! decoded from chip RAM and the chips' registers, side-effect free.
//!
//! - **Sprites** (HRM chapter 4, "Sprite hardware"; AGA chip set notes):
//!   a sprite in memory is SPRxPOS, SPRxCTL, then DATA/DATB per line from
//!   VSTART to VSTOP, then the next sprite's control words; POS = CTL = 0
//!   ends the list. With the AGA FMODE sprite width of 32 or 64 pixels each
//!   of these is a fetch of two or four words, of which the control words
//!   use the first. Colours: pair `n / 2` uses registers 17–19 … 29–31
//!   (AGA: the bank BPLCON4 ESPRM / OSPRM selects for even / odd sprites,
//!   plus 4 × pair + value); an odd sprite with ATTACH set combines with
//!   its even partner into 15 colours (bank + value, value = odd << 2 |
//!   even). The view draws an attached odd sprite's cell with the combined
//!   colours, line by line from both sprites' first data line (the
//!   hardware combines them where both are on the same beam line).
//! - **Bitplanes** (HRM chapter 3, "Playfield hardware"): depth from
//!   BPLCON0 BPU, words per line from the DDFSTRT/DDFSTOP fetch window and
//!   the fetch unit (low / high / super-high resolution, AGA FMODE fetch
//!   widths), lines from the vertical display window; each plane's line
//!   is followed by its modulo (BPL1MOD odd planes, BPL2MOD even planes).
//!   The colours are what Denise/Lisa makes of each pixel index (the
//!   chip's own colour logic: HAM6/HAM8, extra half-brite, dual playfield
//!   with PF2PRI and PF2OF, the AGA BPLAM XOR), with HAM starting every
//!   line from COLOR00. The view does not scroll (BPLCON1) or clip to
//!   the horizontal window. Interlaced displays show the lines of one
//!   field.
//! - **Palette**: the 32 colour registers of 12 bits (AGA: 256 of 24
//!   bits); with extra half-brite active the 32 halved colours follow.
//!
//! The Copper may rewrite any of these registers during a frame — and
//! the bitplane and sprite pointers advance while the DMA fetches — so by
//! default a view decodes the registers **as they are now** (param
//! `source` 0). Option `copper=1` takes the pointers the Copper list at
//! COP1LC sets before the display window opens instead (`source` 1;
//! MOVEs to BPLxPT/SPRxPT up to the first WAIT for a line at or past
//! DIWSTRT's; jumps and SKIPs are not followed), which is what most
//! programs show at the top of their display.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    grey_palette, palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView,
    GfxViewInfo, OPT_ADDR, OPT_COUNT, OPT_INDEX,
};
use emu_core::MemSpaceId;

use super::Custom;
use crate::chipram::ChipRam;
use crate::denise::{rgb24, Sprite};
use crate::reg;

/// Colour of the gaps between sprite cells.
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};
/// Most lines decoded per sprite (VSTOP − VSTART can reach 511 on the ECS
/// and 1023 on the AGA; a sprite that tall is almost certainly not one).
const MAX_SPRITE_LINES: u16 = 512;
/// Most words per plane line and lines of a bitplane view.
const MAX_WORDS: u32 = 256;
const MAX_LINES: u32 = 2048;
/// Copper instructions scanned for option `copper`.
const COPPER_SCAN: usize = 4096;

pub const OPT_WIDTH: GfxOptionInfo = GfxOptionInfo {
    name: "width",
    help: "picture width in pixels, rounded up to 16 (default: the DDFSTRT/DDFSTOP fetch window)",
};
pub const OPT_HEIGHT: GfxOptionInfo = GfxOptionInfo {
    name: "height",
    help: "lines (default: the vertical display window DIWSTRT/DIWSTOP)",
};
pub const OPT_DEPTH: GfxOptionInfo = GfxOptionInfo {
    name: "depth",
    help: "planes 1-6, AGA 1-8 (default: BPLCON0 BPU)",
};
pub const OPT_MODULO: GfxOptionInfo = GfxOptionInfo {
    name: "modulo",
    help: "bytes skipped after each line, all planes (signed 16 bits; default: BPL1MOD odd, BPL2MOD even planes)",
};
pub const OPT_PLANESTEP: GfxOptionInfo = GfxOptionInfo {
    name: "planestep",
    help: "with addr: bytes from one plane to the next (signed; default: (width / 8 + modulo) x height)",
};
pub const OPT_MODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "0 colour index (extra half-brite with six planes), 1 HAM, 2 dual playfield (default: as BPLCON0 is set)",
};
pub const OPT_COPPER: GfxOptionInfo = GfxOptionInfo {
    name: "copper",
    help: "1: pointers as the Copper list at COP1LC sets them before the display window (default 0: the registers now)",
};

/// What the machine tells the decoder besides the chips.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    pub chip: &'a ChipRam,
    /// The chip-RAM memory space the views report addresses in.
    pub space: MemSpaceId,
}

/// One sprite as stored in memory: control words and its lines.
struct Segment {
    addr: u32,
    pos: u16,
    ctl: u16,
    /// DATA, DATB per line, left-aligned in 64 bits.
    lines: Vec<(u64, u64)>,
}

impl Segment {
    /// Bytes from the control words to the next sprite's.
    fn len(&self, words: u32) -> u32 {
        (2 + 2 * self.lines.len() as u32) * 2 * words
    }

    fn end_of_list(&self) -> bool {
        self.pos == 0 && self.ctl == 0
    }
}

const PT_NAMES: [&str; 8] = [
    "bpl1pt", "bpl2pt", "bpl3pt", "bpl4pt", "bpl5pt", "bpl6pt", "bpl7pt", "bpl8pt",
];
/// Per sprite: pointer, VSTART, VSTOP, HSTART, ATTACH.
const SPRITE_PARAMS: [[&str; 5]; 8] = [
    ["spr0pt", "vstart0", "vstop0", "hstart0", "attach0"],
    ["spr1pt", "vstart1", "vstop1", "hstart1", "attach1"],
    ["spr2pt", "vstart2", "vstop2", "hstart2", "attach2"],
    ["spr3pt", "vstart3", "vstop3", "hstart3", "attach3"],
    ["spr4pt", "vstart4", "vstop4", "hstart4", "attach4"],
    ["spr5pt", "vstart5", "vstop5", "hstart5", "attach5"],
    ["spr6pt", "vstart6", "vstop6", "hstart6", "attach6"],
    ["spr7pt", "vstart7", "vstop7", "hstart7", "attach7"],
];
const COLOR_NAMES: [&str; 32] = [
    "color00", "color01", "color02", "color03", "color04", "color05", "color06", "color07",
    "color08", "color09", "color10", "color11", "color12", "color13", "color14", "color15",
    "color16", "color17", "color18", "color19", "color20", "color21", "color22", "color23",
    "color24", "color25", "color26", "color27", "color28", "color29", "color30", "color31",
];

impl Custom {
    /// The name of the display chip fitted.
    fn display_chip(&self) -> &'static str {
        if self.chipset.aga {
            "Lisa"
        } else if self.chipset.ecs_denise {
            "ECS Denise"
        } else {
            "Denise"
        }
    }

    /// The views the chips offer: `sprites`, `bitplanes`, `palette`.
    pub fn gfx_views(&self) -> Vec<GfxViewInfo> {
        let chip = self.display_chip();
        vec![
            GfxViewInfo {
                id: "sprites",
                kind: GfxKind::Sprites,
                name: format!("Amiga sprites ({chip})"),
                options: vec![OPT_INDEX, OPT_ADDR, OPT_COUNT, OPT_COPPER],
            },
            GfxViewInfo {
                id: "bitplanes",
                kind: GfxKind::Bitmap,
                name: format!("Amiga bitplanes ({chip})"),
                options: vec![
                    OPT_ADDR,
                    OPT_WIDTH,
                    OPT_HEIGHT,
                    OPT_DEPTH,
                    OPT_MODULO,
                    OPT_PLANESTEP,
                    OPT_INDEX,
                    OPT_MODE,
                    OPT_COPPER,
                ],
            },
            GfxViewInfo {
                id: "palette",
                kind: GfxKind::Palette,
                name: format!("Amiga palette ({chip})"),
                options: vec![],
            },
        ]
    }

    /// Decode one view of [`Custom::gfx_views`].
    pub fn gfx_view(
        &self,
        id: &'static str,
        kind: GfxKind,
        name: String,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> GfxView {
        let (addr, params, width, height, pixels, raw) = match kind {
            GfxKind::Sprites => self.sprites_view(ctx, opts),
            GfxKind::Bitmap => self.bitplanes_view(ctx, opts),
            _ => self.palette_view(),
        };
        GfxView {
            id,
            kind,
            name,
            space: ctx.space,
            addr,
            params,
            width,
            height,
            pixels,
            raw,
        }
    }

    /// The bitplane and sprite pointers the Copper list at COP1LC sets
    /// before the display window opens (see the module doc); pointers the
    /// list does not set keep their current values.
    pub fn copper_pointers(&self, chip: &ChipRam) -> ([u32; 8], [u32; 8]) {
        let mut bpl = self.bplpt;
        let mut spr = self.sprpt;
        let first = self.vertical_window().0.min(0xff);
        let mask = self.ptr_mask;
        let mut pc = self.copper.cop1lc;
        for _ in 0..COPPER_SCAN {
            let ir1 = chip.word(pc & mask);
            let ir2 = chip.word(pc.wrapping_add(2) & mask);
            pc = pc.wrapping_add(4);
            if ir1 & 1 == 0 {
                let r = ir1 & 0x1fe;
                let high = r & 2 == 0;
                match r {
                    reg::BPL1PTH..=reg::BPL8PTL => {
                        let p = usize::from((r - reg::BPL1PTH) / 4);
                        if p < 6 || self.chipset.aga {
                            Self::set_ptr(&mut bpl[p], high, ir2, mask);
                        }
                    }
                    0x120..=0x13e => {
                        let s = usize::from((r - reg::SPR0PTH) / 4);
                        Self::set_ptr(&mut spr[s], high, ir2, mask);
                    }
                    _ => {}
                }
            } else if ir2 & 1 == 0 && ir1 >> 8 >= first {
                // A WAIT for the display (the list's end, WAIT $FFFF,$FFFE,
                // included).
                break;
            }
        }
        (bpl, spr)
    }

    /// Pointers per option `copper`, and the `source` param.
    fn view_pointers(&self, chip: &ChipRam, opts: &GfxOptions) -> ([u32; 8], [u32; 8], u32) {
        if opts.flag("copper") == Some(true) {
            let (b, s) = self.copper_pointers(chip);
            (b, s, 1)
        } else {
            (self.bplpt, self.sprpt, 0)
        }
    }

    fn sprite_segment(&self, chip: &ChipRam, addr: u32, words: u32) -> Segment {
        let step = 2 * words;
        let addr = addr & self.ptr_mask;
        let pos = (self.fetch_wide(chip, addr, words) >> 48) as u16;
        let ctl = (self.fetch_wide(chip, addr.wrapping_add(step), words) >> 48) as u16;
        let mut s = Sprite::default();
        s.pos = pos;
        s.ctl = ctl;
        let (vstart, vstop) = if self.chipset.aga {
            (s.vstart_aga(), s.vstop_aga())
        } else {
            (s.vstart(), s.vstop())
        };
        let n = vstop.saturating_sub(vstart).min(MAX_SPRITE_LINES);
        let lines = (0..u32::from(n))
            .map(|i| {
                let a = addr.wrapping_add(2 * step + 2 * step * i);
                (
                    self.fetch_wide(chip, a, words),
                    self.fetch_wide(chip, a.wrapping_add(step), words),
                )
            })
            .collect();
        Segment {
            addr,
            pos,
            ctl,
            lines,
        }
    }

    /// First colour register of sprite `n`'s bank (OCS/ECS: 16; AGA:
    /// BPLCON4 ESPRM for even, OSPRM for odd sprites).
    fn sprite_bank(&self, n: usize) -> usize {
        if !self.chipset.aga {
            return 16;
        }
        let b = if n & 1 == 0 {
            self.denise.bplcon4 >> 4
        } else {
            self.denise.bplcon4
        };
        usize::from(b & 15) << 4
    }

    fn bytes(&self, chip: &ChipRam, addr: u32, len: u32) -> Vec<u8> {
        let mask = self.ptr_mask | 1;
        (0..len)
            .map(|i| chip.byte(addr.wrapping_add(i) & mask))
            .collect()
    }

    /// The eight sprites from their pointers, or a sprite list from
    /// `addr` drawn with sprite `index`'s colours.
    #[allow(clippy::type_complexity)]
    fn sprites_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (u32, Vec<(&'static str, u32)>, u32, u32, GfxPixels, Vec<u8>) {
        let chip = ctx.chip;
        let words = self.sprite_words();
        let width = 16 * words;
        let index = opts.get("index").map(|i| (i & 7) as usize);
        // (segment, sprite whose colours apply, even partner if attached)
        let mut cells: Vec<(Segment, usize, Option<Segment>)> = Vec::new();
        let mut params = Vec::new();
        if let Some(a) = opts.get("addr") {
            let n = index.unwrap_or(0);
            let count = opts.get("count").unwrap_or(16).clamp(1, 64);
            let mut at = a & self.ptr_mask;
            for _ in 0..count {
                let seg = self.sprite_segment(chip, at, words);
                if seg.end_of_list() && !cells.is_empty() {
                    break;
                }
                at = at.wrapping_add(seg.len(words)) & self.ptr_mask;
                let end = seg.end_of_list();
                cells.push((seg, n, None));
                if end {
                    break;
                }
            }
            params.push(("addr", a & self.ptr_mask));
            params.push(("count", cells.len() as u32));
            params.push(("index", n as u32));
        } else {
            let (_, spr, source) = self.view_pointers(chip, opts);
            params.push(("source", source));
            for n in 0..8 {
                let seg = self.sprite_segment(chip, spr[n], words);
                let mut s = Sprite::default();
                s.pos = seg.pos;
                s.ctl = seg.ctl;
                let (vstart, vstop) = if self.chipset.aga {
                    (s.vstart_aga(), s.vstop_aga())
                } else {
                    (s.vstart(), s.vstop())
                };
                let attached = n & 1 == 1 && seg.ctl & 0x80 != 0;
                let [pt, vs, ve, hs, at] = SPRITE_PARAMS[n];
                params.push((pt, spr[n]));
                params.push((vs, u32::from(vstart)));
                params.push((ve, u32::from(vstop)));
                params.push((hs, s.hstart()));
                params.push((at, u32::from(attached)));
                if index.is_some_and(|i| i != n) {
                    continue;
                }
                let partner = attached.then(|| self.sprite_segment(chip, spr[n - 1], words));
                cells.push((seg, n, partner));
            }
        }
        params.push(("width", width));
        params.push(("dmaen", u32::from(self.dma_on(crate::dma::SPREN))));
        if self.chipset.aga {
            params.push(("bank_even", self.sprite_bank(0) as u32));
            params.push(("bank_odd", self.sprite_bank(1) as u32));
        }
        let cell_h = cells
            .iter()
            .map(|c| c.0.lines.len() as u32)
            .max()
            .unwrap_or(0)
            .max(1);
        let cols = if cells.len() <= 8 { 8 } else { 16 };
        let grid = Canvas::grid(cells.len() as u32, cols, width, cell_h, 1, 0);
        let (w, h) = (grid.width, grid.height);
        let mut px = vec![GAP_COLOR; (w * h) as usize];
        let colors = &self.denise.colors;
        let bg = rgb24(colors[0]);
        let mut raw = Vec::new();
        for (i, (seg, n, partner)) in cells.iter().enumerate() {
            let (x0, y0) = Canvas::cell_origin(i as u32, cols, width, cell_h, 1);
            for y in 0..cell_h {
                for x in 0..width {
                    px[((y0 + y) * w + x0 + x) as usize] = bg;
                }
            }
            let bit = |v: u64, x: u32| ((v >> (63 - x)) & 1) as usize;
            for (y, &(a, b)) in seg.lines.iter().enumerate() {
                let even = partner
                    .as_ref()
                    .map(|p| p.lines.get(y).copied().unwrap_or((0, 0)));
                for x in 0..width {
                    let v = bit(a, x) | bit(b, x) << 1;
                    let c = match even {
                        Some((ea, eb)) => {
                            let v = v << 2 | bit(ea, x) | bit(eb, x) << 1;
                            (v != 0).then(|| self.sprite_bank(*n) + v)
                        }
                        None => (v != 0).then(|| self.sprite_bank(*n) + 4 * (n / 2) + v),
                    };
                    if let Some(c) = c {
                        px[((y0 + y as u32) * w + x0 + x) as usize] = rgb24(colors[c & 255]);
                    }
                }
            }
            raw.extend(self.bytes(chip, seg.addr, seg.len(words)));
        }
        let addr = cells.first().map_or(0, |c| c.0.addr);
        (addr, params, w, h, GfxPixels::Rgb(px), raw)
    }

    /// The bitplanes from BPLxPT (or `addr`) as Denise/Lisa colours them,
    /// or one plane in grey.
    #[allow(clippy::type_complexity)]
    fn bitplanes_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (u32, Vec<(&'static str, u32)>, u32, u32, GfxPixels, Vec<u8>) {
        let chip = ctx.chip;
        let mask = self.ptr_mask;
        let aga = self.chipset.aga;
        let max_planes = if aga { 8 } else { 6 };
        let depth = opts
            .get("depth")
            .map_or(self.denise.planes(), |d| (d as usize).clamp(1, max_planes));
        let (start, end) = self.fetch_window();
        let dma_words = u32::from((end - start) / self.fetch_unit()) * self.fetch_words();
        let words = opts
            .get("width")
            .map_or(if dma_words == 0 { 20 } else { dma_words }, |w| {
                w.div_ceil(16)
            })
            .clamp(1, MAX_WORDS);
        let width = 16 * words;
        let (vstart, vstop) = self.vertical_window();
        let window_lines = u32::from(vstop.saturating_sub(vstart));
        let height = opts
            .get("height")
            .unwrap_or(if window_lines == 0 { 256 } else { window_lines })
            .clamp(1, MAX_LINES);
        let rowbytes = 2 * words;
        let modulo_override = opts.get("modulo").map(|m| m as u16);
        let mod1 = modulo_override.unwrap_or(self.bpl1mod);
        let mod2 = modulo_override.unwrap_or(self.bpl2mod);
        let modulo = |p: usize| i32::from(if p & 1 == 0 { mod1 } else { mod2 } as i16);
        let (bpl, _, source) = self.view_pointers(chip, opts);
        let mut params = vec![("source", source)];
        let mut planes = bpl;
        if let Some(a) = opts.get("addr") {
            let step = opts.get("planestep").map_or(
                (rowbytes as i32 + modulo(0)).wrapping_mul(height as i32),
                |s| s as i32,
            );
            for (p, pt) in planes.iter_mut().enumerate() {
                *pt = a.wrapping_add((step.wrapping_mul(p as i32)) as u32) & mask;
            }
            params.push(("planestep", step as u32));
        }
        let single = opts.get("index").map(|i| (i as usize).min(7));
        let shown: Vec<usize> = match single {
            Some(p) => vec![p],
            None => (0..depth).collect(),
        };
        let mut data = vec![Vec::new(); 8];
        let mut raw = Vec::with_capacity(shown.len() * (rowbytes * height) as usize);
        for &p in &shown {
            let mut line = planes[p];
            let d = &mut data[p];
            d.reserve((words * height) as usize);
            for _ in 0..height {
                for w in 0..words {
                    let v = chip.word(line.wrapping_add(2 * w) & mask);
                    d.push(v);
                    raw.extend(v.to_be_bytes());
                }
                line = line.wrapping_add((rowbytes as i32 + modulo(p)) as u32) & mask;
            }
        }
        for &p in &shown {
            params.push((PT_NAMES[p], planes[p]));
        }
        let bit = |p: usize, i: usize, x: u32| -> u8 { ((data[p][i] >> (15 - x)) & 1) as u8 };
        let mut den = self.denise.clone();
        match opts.get("mode") {
            Some(0) => den.bplcon0 &= !0x0c00,
            Some(1) => den.bplcon0 = (den.bplcon0 & !0x0400) | 0x0800,
            Some(2) => den.bplcon0 = (den.bplcon0 & !0x0800) | 0x0400,
            _ => {}
        }
        let ham = den.bplcon0 & 0x0800 != 0 && depth >= 5;
        let dual = den.bplcon0 & 0x0400 != 0 && !ham;
        let ehb = depth == 6 && !ham && !dual && !(aga && den.bplcon2 & 0x0200 != 0);
        let pixels = if let Some(p) = single {
            let mut px = Vec::with_capacity((width * height) as usize);
            for y in 0..height {
                for x in 0..width {
                    px.push(bit(p, (y * words + x / 16) as usize, x % 16));
                }
            }
            params.push(("index", p as u32));
            GfxPixels::Indexed {
                pixels: px,
                palette: grey_palette(2),
            }
        } else {
            let xor = if aga { (den.bplcon4 >> 8) as u8 } else { 0 };
            let reduce = den.shres() && !aga;
            let mut px = Vec::with_capacity((width * height) as usize);
            for y in 0..height {
                den.start_line();
                for x in 0..width {
                    let i = (y * words + x / 16) as usize;
                    let index = (0..depth).fold(0u8, |acc, p| acc | bit(p, i, x % 16) << p);
                    let (mut c, _) = den.color(index ^ xor, depth);
                    if reduce {
                        // Two bits per gun, as Denise shows super-hires.
                        let hi = c & 0xc0c0c0;
                        c = hi | hi >> 2 | hi >> 4 | hi >> 6;
                    }
                    px.push(rgb24(c));
                }
            }
            GfxPixels::Rgb(px)
        };
        let d = &self.denise;
        params.extend([
            ("depth", depth as u32),
            ("width", width),
            ("height", height),
            ("rowbytes", rowbytes),
            ("modulo1", u32::from(mod1)),
            ("modulo2", u32::from(mod2)),
            ("hires", u32::from(d.hires())),
            ("shres", u32::from(d.shres())),
            ("ham", u32::from(ham)),
            ("ehb", u32::from(ehb)),
            ("dualpf", u32::from(dual)),
            ("pf2pri", u32::from(d.bplcon2 & 0x40 != 0)),
            ("lace", u32::from(d.bplcon0 & 0x0004 != 0)),
            ("dmaen", u32::from(self.dma_on(crate::dma::BPLEN))),
            ("bplcon0", u32::from(d.bplcon0)),
            ("bplcon1", u32::from(d.bplcon1)),
            ("bplcon2", u32::from(d.bplcon2)),
            ("bplcon3", u32::from(d.bplcon3)),
            ("bplcon4", u32::from(d.bplcon4)),
            ("ddfstrt", u32::from(self.ddfstrt)),
            ("ddfstop", u32::from(self.ddfstop)),
            ("diwstrt", u32::from(self.diwstrt)),
            ("diwstop", u32::from(self.diwstop)),
            ("fmode", u32::from(self.fmode)),
        ]);
        let addr = planes[shown.first().copied().unwrap_or(0)];
        (addr, params, width, height, pixels, raw)
    }

    /// The colour registers, with the half-brite colours when EHB is on.
    #[allow(clippy::type_complexity)]
    fn palette_view(&self) -> (u32, Vec<(&'static str, u32)>, u32, u32, GfxPixels, Vec<u8>) {
        let d = &self.denise;
        let aga = self.chipset.aga;
        let n = if aga { 256 } else { 32 };
        let mut colors: Vec<Rgb8> = d.colors[..n].iter().map(|&c| rgb24(c)).collect();
        let planes = d.planes();
        let ham = d.bplcon0 & 0x0800 != 0 && planes >= 5;
        let dual = d.bplcon0 & 0x0400 != 0;
        let ehb = planes == 6 && !ham && !dual && !(aga && d.bplcon2 & 0x0200 != 0);
        if ehb {
            colors.extend(d.colors[..32].iter().map(|&c| rgb24((c >> 1) & 0x7f7f7f)));
        }
        let (w, h, px) = palette_canvas(&colors, 16, 16);
        let mut params = vec![
            ("count", colors.len() as u32),
            ("bits", if aga { 24 } else { 12 }),
            ("ehb", u32::from(ehb)),
        ];
        let raw = if aga {
            params.push(("bank", u32::from(d.bplcon3 >> 13)));
            params.push(("loct", u32::from(d.bplcon3 & 0x0200 != 0)));
            d.colors
                .iter()
                .flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8])
                .collect()
        } else {
            for (i, name) in COLOR_NAMES.iter().enumerate() {
                params.push((name, u32::from(d.color12(i))));
            }
            (0..32).flat_map(|i| d.color12(i).to_be_bytes()).collect()
        };
        (0, params, w, h, px, raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom::{Chipset, Standard};
    use crate::dma;

    const SPACE: MemSpaceId = MemSpaceId(1);

    fn ctx(chip: &ChipRam) -> GfxContext<'_> {
        GfxContext { chip, space: SPACE }
    }

    fn rgb(c: u16) -> Rgb8 {
        crate::denise::rgb12(c)
    }

    fn view(c: &Custom, chip: &ChipRam, id: &str, opts: &GfxOptions) -> GfxView {
        let info = c.gfx_views().into_iter().find(|v| v.id == id).unwrap();
        c.gfx_view(info.id, info.kind, info.name, &ctx(chip), opts)
    }

    /// A 320 × 200 low-resolution screen of `depth` planes, plane p at
    /// $10000 + p × $2000.
    fn screen(c: &mut Custom, depth: u16) {
        for (r, v) in [
            (reg::BPLCON0, depth << 12 | 0x0200),
            (reg::DIWSTRT, 0x2c81),
            (reg::DIWSTOP, 0xf4c1),
            (reg::DDFSTRT, 0x38),
            (reg::DDFSTOP, 0xd0),
            (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN),
        ] {
            c.write(r, v);
        }
        for p in 0..depth {
            c.write(reg::BPL1PTH + 4 * p, 0x0001);
            c.write(reg::BPL1PTH + 4 * p + 2, 0x2000 * p);
        }
    }

    #[test]
    fn bitplanes_follow_the_registers() {
        let mut c = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
        let mut chip = ChipRam::new(0x8_0000);
        screen(&mut c, 2);
        c.write(reg::COLOR00, 0x00f);
        c.write(reg::COLOR00 + 2, 0xf00);
        c.write(reg::COLOR00 + 4, 0x0f0);
        c.write(reg::COLOR00 + 6, 0xfff);
        chip.set_word(0x1_0000, 0x8000); // plane 1, pixel 0: colour 1
        chip.set_word(0x1_2000, 0xc000); // plane 2, pixels 0 and 1
        chip.set_word(0x1_0000 + 40, 0x8000); // line 1, pixel 0: colour 1
        let v = view(&c, &chip, "bitplanes", &GfxOptions::new());
        assert_eq!((v.width, v.height), (320, 200));
        assert_eq!(v.kind, GfxKind::Bitmap);
        assert_eq!(v.space, SPACE);
        assert_eq!(v.addr, 0x1_0000);
        assert_eq!(v.param("depth"), Some(2));
        assert_eq!(v.param("bpl2pt"), Some(0x1_2000));
        assert_eq!(v.param("rowbytes"), Some(40));
        assert_eq!(v.param("source"), Some(0));
        assert_eq!(v.raw.len(), 2 * 40 * 200);
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(rgb(0xfff)), "planes 1 + 2: colour 3");
        assert_eq!(f.rgb_at(1, 0), Some(rgb(0x0f0)), "plane 2: colour 2");
        assert_eq!(f.rgb_at(2, 0), Some(rgb(0x00f)));
        assert_eq!(f.rgb_at(0, 1), Some(rgb(0xf00)));
        // One plane in grey, from any address with its own geometry.
        let one = view(
            &c,
            &chip,
            "bitplanes",
            &GfxOptions::new()
                .with("addr", 0x1_2000)
                .with("width", 32)
                .with("height", 4)
                .with("index", 0)
                .with("modulo", 0),
        );
        assert_eq!((one.width, one.height), (32, 4));
        assert_eq!(one.raw.len(), 4 * 4);
        assert!(matches!(one.pixels, GfxPixels::Indexed { .. }));
        let f = one.frame();
        assert_eq!(
            f.rgb_at(1, 0),
            Some(Rgb8 {
                r: 255,
                g: 255,
                b: 255
            })
        );
        assert_eq!(f.rgb_at(2, 0), Some(Rgb8::default()));
    }

    #[test]
    fn hires_and_ham_and_half_brite() {
        let mut c = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
        let mut chip = ChipRam::new(0x8_0000);
        screen(&mut c, 6);
        c.write(reg::COLOR00 + 6, 0xeca);
        // Pixel 0: index 3 + plane 6 = half-brite of colour 3.
        chip.set_word(0x1_0000, 0x8000);
        chip.set_word(0x1_2000, 0x8000);
        chip.set_word(0x1_a000, 0x8000);
        let v = view(&c, &chip, "bitplanes", &GfxOptions::new());
        assert_eq!(v.param("ehb"), Some(1));
        assert_eq!(v.frame().rgb_at(0, 0), Some(rgb(0x765)));
        let p = view(&c, &chip, "palette", &GfxOptions::new());
        assert_eq!(p.param("count"), Some(64), "half-brite colours follow");
        // HAM6: pixel 0 = modify blue with 3 (control 01 in planes 5–6).
        c.write(reg::BPLCON0, 0x6a00);
        chip.set_word(0x1_8000, 0x8000);
        chip.set_word(0x1_a000, 0x0000);
        c.write(reg::COLOR00, 0x12f);
        let v = view(&c, &chip, "bitplanes", &GfxOptions::new());
        assert_eq!(v.param("ham"), Some(1));
        assert!(matches!(v.pixels, GfxPixels::Rgb(_)));
        assert_eq!(v.frame().rgb_at(0, 0), Some(rgb(0x123)));
        // Hires: 640 pixels from the same window ($3C/$D4).
        c.write(reg::BPLCON0, 0x9200);
        c.write(reg::DDFSTRT, 0x3c);
        c.write(reg::DDFSTOP, 0xd4);
        let v = view(&c, &chip, "bitplanes", &GfxOptions::new());
        assert_eq!((v.width, v.param("hires")), (640, Some(1)));
    }

    #[test]
    fn copper_option_takes_the_lists_pointers() {
        let mut c = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
        let mut chip = ChipRam::new(0x8_0000);
        screen(&mut c, 1);
        // The list: BPL1PT = $20000, SPR0PT = $30000, then WAIT for $2C.
        let list = [
            0x00e0, 0x0002, 0x00e2, 0x0000, 0x0120, 0x0003, 0x0122, 0x0000, 0x2c01, 0xfffe, 0x00e0,
            0x0004, 0xffff, 0xfffe,
        ];
        for (i, &w) in list.iter().enumerate() {
            chip.set_word(0x4_0000 + 2 * i as u32, w);
        }
        c.write(reg::COP1LCH, 0x0004);
        c.write(reg::COP1LCL, 0x0000);
        let v = view(&c, &chip, "bitplanes", &GfxOptions::new().with("copper", 1));
        assert_eq!(v.param("source"), Some(1));
        assert_eq!(v.addr, 0x2_0000, "MOVEs after the WAIT do not count");
        let s = view(&c, &chip, "sprites", &GfxOptions::new().with("copper", 1));
        assert_eq!(s.param("spr0pt"), Some(0x3_0000));
    }

    #[test]
    fn sprites_with_colours_and_attachment() {
        let mut c = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
        let mut chip = ChipRam::new(0x8_0000);
        c.write(reg::COLOR00, 0x000);
        c.write(reg::COLOR00 + 2 * 17, 0xf00);
        c.write(reg::COLOR00 + 2 * 19, 0x0f0);
        c.write(reg::COLOR00 + 2 * 21, 0x00f);
        c.write(reg::COLOR00 + 2 * 29, 0xff0);
        // Sprite 0 at $1000: VSTART $40, VSTOP $43, HSTART $90.
        for (i, w) in [
            0x4048u16, 0x4300, 0x8000, 0x0000, 0x0000, 0x8000, 0xc000, 0xc000, 0, 0,
        ]
        .into_iter()
        .enumerate()
        {
            chip.set_word(0x1000 + 2 * i as u32, w);
        }
        // Sprites 2 (even) and 3 (odd, attached) with one line each.
        for (i, w) in [0x4048u16, 0x4100, 0x8000, 0x0000].into_iter().enumerate() {
            chip.set_word(0x2000 + 2 * i as u32, w);
        }
        for (i, w) in [0x4048u16, 0x4180, 0x8000, 0x0000].into_iter().enumerate() {
            chip.set_word(0x3000 + 2 * i as u32, w);
        }
        for (n, a) in [(0u16, 0x1000u16), (2, 0x2000), (3, 0x3000)] {
            c.write(reg::SPR0PTH + 4 * n, 0);
            c.write(reg::SPR0PTH + 4 * n + 2, a);
        }
        let v = view(&c, &chip, "sprites", &GfxOptions::new().with("index", 0));
        assert_eq!((v.width, v.height), (16, 3));
        assert_eq!(v.addr, 0x1000);
        assert_eq!(v.param("vstart0"), Some(0x40));
        assert_eq!(v.param("vstop0"), Some(0x43));
        assert_eq!(v.param("hstart0"), Some(0x90));
        assert_eq!(v.raw.len(), 4 + 3 * 4);
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(rgb(0xf00)), "value 1: COLOR17");
        assert_eq!(f.rgb_at(0, 1), Some(rgb(0x000)), "value 2: COLOR18 is 0");
        assert_eq!(f.rgb_at(0, 2), Some(rgb(0x0f0)), "value 3: COLOR19");
        assert_eq!(f.rgb_at(1, 0), Some(rgb(0x000)), "transparent");
        let all = view(&c, &chip, "sprites", &GfxOptions::new());
        assert_eq!((all.width, all.height), (8 * 17 - 1, 3));
        assert_eq!(all.param("attach3"), Some(1));
        let f = all.frame();
        assert_eq!(f.rgb_at(2 * 17, 0), Some(rgb(0x00f)), "sprite 2: COLOR21");
        // Attached: odd 1, even 1 = value 5 = COLOR21; with odd only: 4.
        assert_eq!(f.rgb_at(3 * 17, 0), Some(rgb(0x00f)));
        // A list from any address.
        let l = view(
            &c,
            &chip,
            "sprites",
            &GfxOptions::new().with("addr", 0x1000),
        );
        assert_eq!(l.param("count"), Some(1), "ends at POS = CTL = 0");
        assert_eq!(l.raw.len(), 16);
    }

    #[test]
    fn aga_palette_and_sprite_banks() {
        let mut c = Custom::new(Standard::Pal, 0x1f_fffe, Chipset::AGA);
        let mut chip = ChipRam::new(0x20_0000);
        c.denise.colors[0x21] = 0x123456;
        c.write(reg::BPLCON4, 0x0021); // even sprites from bank 2
        c.write(reg::FMODE, 0x000c); // 64-pixel sprites
                                     // Sprite 0 at $1000, four-word fetches: POS, CTL, DATA, DATB.
        chip.set_word(0x1000, 0x4048);
        chip.set_word(0x1008, 0x4100);
        chip.set_word(0x1010, 0x8000);
        c.write(reg::SPR0PTH + 2, 0x1000);
        let v = view(&c, &chip, "sprites", &GfxOptions::new().with("index", 0));
        assert_eq!((v.width, v.height), (64, 1));
        assert_eq!(v.param("bank_even"), Some(0x20));
        assert_eq!(v.raw.len(), 4 * 8);
        assert_eq!(
            v.frame().rgb_at(0, 0),
            Some(Rgb8 {
                r: 0x12,
                g: 0x34,
                b: 0x56
            })
        );
        let p = view(&c, &chip, "palette", &GfxOptions::new());
        assert_eq!(p.param("count"), Some(256));
        assert_eq!(p.param("bits"), Some(24));
        assert_eq!(p.raw.len(), 3 * 256);
        assert!(matches!(p.pixels, GfxPixels::Rgb(_)));
        // Eight planes, 64-bit fetches.
        c.write(reg::BPLCON0, 0x0211);
        c.write(reg::FMODE, 0x0003);
        c.write(reg::DDFSTRT, 0x38);
        c.write(reg::DDFSTOP, 0xb8);
        let b = view(&c, &chip, "bitplanes", &GfxOptions::new());
        assert_eq!(b.param("depth"), Some(8));
        assert_eq!(b.width, 320);
    }

    #[test]
    fn ocs_palette_has_the_12_bit_values() {
        let mut c = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
        let chip = ChipRam::new(0x8_0000);
        c.write(reg::COLOR00 + 2 * 5, 0x48c);
        let p = view(&c, &chip, "palette", &GfxOptions::new());
        assert_eq!(p.param("count"), Some(32));
        assert_eq!(p.param("color05"), Some(0x48c));
        assert_eq!(p.raw.len(), 64);
        assert_eq!(&p.raw[10..12], &[0x04, 0x8c]);
        // Swatch 5 (sixth of the first row, 16 pixels + 1 gap each).
        assert_eq!(p.frame().rgb_at(5 * 17, 0), Some(rgb(0x48c)));
    }
}
