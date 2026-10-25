//! Graphics data views of the TIA (see `emu_core::gfx`). The TIA has no
//! frame memory, so the views show what its registers hold now: the
//! movable objects and the playfield laid out on one 160-pixel line, and
//! the palette. Side-effect free.
//!
//! Sources: Steve Wright, "Stella Programmer's Guide" (1979): GRP0/1,
//! ENAM0/1, ENABL, NUSIZ0/1 copies and sizes, REFP0/1, CTRLPF (reflect,
//! score, priority, ball size), PF0–PF2 bit order, VDELP0/1/VDELBL and
//! the colour registers; Andrew Towers, "Atari 2600 TIA Hardware Notes"
//! (start delays of the position counters: players 5 clocks, 6 when
//! stretched, missiles and the ball 4).
//!
//! Positions are where each object's main copy starts on a line without
//! HMOVE, derived from its position counter and the beam (pending HMOVE
//! motion is not included).

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
};
use emu_core::MemSpaceId;

use super::*;

/// Palette index of the gaps between strips and of disabled objects (one
/// past the 128 TIA colours).
const GAP: u8 = 128;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x40,
    g: 0x40,
    b: 0x40,
};
/// Height of one strip.
const STRIP: u32 = 8;
/// Strips of the objects view, top to bottom.
const STRIPS: [&str; 9] = [
    "P0",
    "P0 (VDEL copy)",
    "P1",
    "P1 (VDEL copy)",
    "M0",
    "M1",
    "BL",
    "PF",
    "line",
];

const OPT_STRIP: GfxOptionInfo = GfxOptionInfo {
    name: "index",
    help: "show one strip: 0 P0, 1 P0 delayed copy, 2 P1, 3 P1 delayed copy, 4 M0, 5 M1, 6 ball, 7 playfield, 8 the line as shown",
};

/// What the machine tells the decoder besides the chip.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    /// The CPU's space (the TIA's registers are at `$00`).
    pub space: MemSpaceId,
    /// The 128 TIA colours (register value ÷ 2).
    pub palette: &'a [Rgb8],
}

/// The views the TIA offers (objects, palette).
pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "objects",
            kind: GfxKind::Sprites,
            name: "TIA objects and playfield".into(),
            options: vec![OPT_STRIP],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: "TIA palette".into(),
            options: vec![],
        },
    ]
}

const POS: [&str; 5] = ["pos_p0", "pos_p1", "pos_m0", "pos_m1", "pos_bl"];

/// Registers listed in the params, by name and address.
const PARAM_REGS: [(&str, u8); 27] = [
    ("nusiz0", NUSIZ0),
    ("nusiz1", NUSIZ1),
    ("colup0", COLUP0),
    ("colup1", COLUP1),
    ("colupf", COLUPF),
    ("colubk", COLUBK),
    ("ctrlpf", CTRLPF),
    ("refp0", REFP0),
    ("refp1", REFP1),
    ("pf0", PF0),
    ("pf1", PF1),
    ("pf2", PF2),
    ("grp0", GRP0),
    ("grp1", GRP1),
    ("enam0", ENAM0),
    ("enam1", ENAM1),
    ("enabl", ENABL),
    ("hmp0", HMP0),
    ("hmp1", HMP0 + 1),
    ("hmm0", HMP0 + 2),
    ("hmm1", HMP0 + 3),
    ("hmbl", HMBL),
    ("vdelp0", VDELP0),
    ("vdelp1", VDELP1),
    ("vdelbl", VDELBL),
    ("resmp0", RESMP0),
    ("resmp1", RESMP1),
];

impl Tia {
    /// Pixel (0–159) at which object `i`'s main copy starts on a line
    /// without HMOVE: its counter wraps `160 − counter` motion clocks after
    /// the last one, and graphics follow after the start delay.
    pub fn object_position(&self, i: usize) -> u8 {
        let m = &self.movers[i];
        let last_cc = if self.cc == 0 {
            CLOCKS_PER_LINE - 1
        } else {
            self.cc - 1
        };
        let moved = last_cc >= HBLANK && !(self.hmove_blank && last_cc < HBLANK_HMOVE);
        let x_last = if moved {
            u16::from(last_cc - HBLANK)
        } else {
            159
        };
        let delay = match i {
            P0 | P1 if player_width(self.reg(NUSIZ0 + i as u8)) > 1 => 6,
            P0 | P1 => 5,
            _ => 4,
        };
        ((x_last + 160 - u16::from(m.counter) + delay + u16::from(m.skip)) % 160) as u8
    }

    /// Copy start pixels of object `i` (main copy first).
    fn copy_starts(&self, i: usize) -> Vec<u16> {
        let main = u16::from(self.object_position(i));
        let copies = match i {
            P0 | P1 => copies(self.reg(NUSIZ0 + i as u8)),
            M0 | M1 => copies(self.reg(NUSIZ0 + (i - M0) as u8)),
            _ => 0,
        };
        let mut starts = vec![main];
        for (bit, off) in [(1, 16), (2, 32), (4, 64)] {
            if copies & bit != 0 {
                starts.push((main + off) % 160);
            }
        }
        starts
    }

    /// Does player `n` with graphics `g` cover pixel `x`?
    fn player_at(&self, n: usize, g: u8, starts: &[u16], x: u16) -> bool {
        let w = u16::from(player_width(self.reg(NUSIZ0 + n as u8)));
        starts.iter().any(|&s| {
            let off = (x + 160 - s) % 160;
            if off >= 8 * w {
                return false;
            }
            let bit = off / w;
            let bit = if self.reg(REFP0 + n as u8) & 8 != 0 {
                bit
            } else {
                7 - bit
            };
            g >> bit & 1 != 0
        })
    }

    /// Does a missile or the ball (`len` pixels from each start) cover `x`?
    fn block_at(starts: &[u16], len: u16, x: u16) -> bool {
        starts.iter().any(|&s| (x + 160 - s) % 160 < len)
    }

    fn missile_len(&self, n: usize) -> u16 {
        1 << ((self.reg(NUSIZ0 + n as u8) >> 4) & 3)
    }

    fn ball_len(&self) -> u16 {
        1 << ((self.reg(CTRLPF) >> 4) & 3)
    }

    /// Decode one view (`kind` from [`views`]).
    pub fn gfx_view(
        &self,
        id: &'static str,
        kind: GfxKind,
        name: String,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> GfxView {
        let mut palette = ctx.palette.to_vec();
        palette.resize(128, Rgb8::default());
        if kind == GfxKind::Palette {
            let (width, height, pixels) = palette_canvas(&palette, 16, 8);
            return GfxView {
                id,
                kind,
                name,
                space: ctx.space,
                addr: 0,
                params: vec![("count", 128)],
                width,
                height,
                pixels,
                raw: (0..128).map(|i| i * 2).collect(),
            };
        }
        palette.push(GAP_COLOR);
        let strips: Vec<usize> = match opts.get("index") {
            Some(i) => vec![(i as usize).min(STRIPS.len() - 1)],
            None => (0..STRIPS.len()).collect(),
        };
        let bk = self.reg(COLUBK) >> 1;
        let n = strips.len() as u32;
        let mut c = Canvas::grid(n, 1, WIDTH, STRIP, 1, GAP);
        let starts: [Vec<u16>; 5] = std::array::from_fn(|i| self.copy_starts(i));
        let vdel_p = [self.reg(VDELP0) & 1 != 0, self.reg(VDELP1) & 1 != 0];
        let grp = |n: usize| {
            if vdel_p[n] {
                self.grp_old[n]
            } else {
                self.grp_new[n]
            }
        };
        let ball_on = if self.reg(VDELBL) & 1 != 0 {
            self.enabl_old
        } else {
            self.enabl_new
        };
        let ctrl = self.reg(CTRLPF);
        let score = ctrl & 2 != 0 && ctrl & 4 == 0;
        for (row, &strip) in strips.iter().enumerate() {
            let (_, y0) = Canvas::cell_origin(row as u32, 1, WIDTH, STRIP, 1);
            c.fill(0, y0, WIDTH, STRIP, bk);
            for x in 0..WIDTH as u16 {
                let colour = match strip {
                    0..=3 => {
                        let p = strip / 2;
                        let g = if strip & 1 == 0 {
                            self.grp_new[p]
                        } else {
                            self.grp_old[p]
                        };
                        self.player_at(p, g, &starts[p], x)
                            .then(|| self.reg(COLUP0 + p as u8) >> 1)
                    }
                    4 | 5 => {
                        let m = strip - 4;
                        Self::block_at(&starts[M0 + m], self.missile_len(m), x).then(|| {
                            let on = self.reg(ENAM0 + m as u8) & 2 != 0
                                && self.reg(RESMP0 + m as u8) & 2 == 0;
                            if on {
                                self.reg(COLUP0 + m as u8) >> 1
                            } else {
                                GAP
                            }
                        })
                    }
                    6 => Self::block_at(&starts[BL], self.ball_len(), x).then(|| {
                        if ball_on {
                            self.reg(COLUPF) >> 1
                        } else {
                            GAP
                        }
                    }),
                    7 => self.pf_bit((x / 4) as u8).then(|| {
                        if score && x >= 80 {
                            self.reg(COLUP1) >> 1
                        } else if score {
                            self.reg(COLUP0) >> 1
                        } else {
                            self.reg(COLUPF) >> 1
                        }
                    }),
                    _ => {
                        let missile = |m: usize| {
                            self.reg(ENAM0 + m as u8) & 2 != 0
                                && self.reg(RESMP0 + m as u8) & 2 == 0
                                && Self::block_at(&starts[M0 + m], self.missile_len(m), x)
                        };
                        let p0 = self.player_at(0, grp(0), &starts[P0], x) || missile(0);
                        let p1 = self.player_at(1, grp(1), &starts[P1], x) || missile(1);
                        let bl = ball_on && Self::block_at(&starts[BL], self.ball_len(), x);
                        let pf = self.pf_bit((x / 4) as u8);
                        Some(self.colour(u32::from(x), p0, p1, bl, pf))
                    }
                };
                if let Some(colour) = colour {
                    c.fill(u32::from(x), y0, 1, STRIP, colour);
                }
            }
        }
        let mut params: Vec<(&'static str, u32)> = PARAM_REGS
            .iter()
            .map(|&(name, r)| (name, u32::from(self.reg(r))))
            .collect();
        params.push(("grp0_old", u32::from(self.grp_old[0])));
        params.push(("grp1_old", u32::from(self.grp_old[1])));
        params.push(("enabl_old", u32::from(self.enabl_old)));
        for (i, name) in POS.iter().enumerate() {
            params.push((name, u32::from(self.object_position(i))));
        }
        params.push(("line", u32::from(self.line)));
        params.push(("clock", u32::from(self.cc)));
        let mut raw = self.regs.to_vec();
        raw.extend_from_slice(&self.grp_old);
        raw.push(u8::from(self.enabl_old));
        GfxView {
            id,
            kind,
            name,
            space: ctx.space,
            addr: 0,
            params,
            width: c.width,
            height: c.height,
            pixels: GfxPixels::Indexed {
                pixels: c.pixels,
                palette,
            },
            raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette;

    fn view(tia: &Tia, kind: GfxKind, opts: GfxOptions) -> GfxView {
        let pal = palette::tia(VideoStandard::Ntsc);
        let ctx = GfxContext {
            space: MemSpaceId(0),
            palette: &pal,
        };
        tia.gfx_view("objects", kind, "t".into(), &ctx, &opts)
    }

    fn colour_at(v: &GfxView, x: u32, y: u32) -> u8 {
        match &v.pixels {
            GfxPixels::Indexed { pixels, .. } => pixels[(y * v.width + x) as usize],
            GfxPixels::Rgb(_) => panic!("indexed expected"),
        }
    }

    fn run_to(tia: &mut Tia, cc: u8) {
        while tia.clock() != cc {
            tia.tick();
        }
    }

    /// The position the view reports is where the TIA draws the player.
    #[test]
    fn positions_match_the_picture() {
        let cases = [(100u8, 0u8), (150, 5), (20, 0), (227, 7), (180, 0x30)];
        for (reset_at, nusiz) in cases {
            // Once with the player, once with the missile.
            for missile in [false, true] {
                let mut tia = Tia::new(VideoStandard::Ntsc);
                tia.write(COLUP0, 0x1e);
                tia.write(COLUBK, 0x00);
                tia.write(NUSIZ0, nusiz);
                if missile {
                    tia.write(ENAM0, 2);
                } else {
                    tia.write(GRP0, 0x80);
                }
                run_to(&mut tia, reset_at);
                let line = tia.line();
                tia.write(if missile { RESM0 } else { RESP0 }, 0);
                // Draw two whole lines, stop in the blanking after them.
                while tia.line() < line + 3 {
                    tia.tick();
                }
                run_to(&mut tia, 40);
                let v = view(&tia, GfxKind::Sprites, GfxOptions::new());
                let y = u32::from(line) + 2;
                let row = &tia.pixels()[(y * WIDTH) as usize..][..WIDTH as usize];
                let first = row
                    .iter()
                    .position(|&p| p == 0x0f)
                    .unwrap_or_else(|| panic!("{reset_at} {missile}"))
                    as u32;
                let (param, strip) = if missile {
                    ("pos_m0", 4)
                } else {
                    ("pos_p0", 0)
                };
                assert_eq!(v.param(param), Some(first), "{param}, reset at {reset_at}");
                assert_eq!(colour_at(&v, first, strip * 9), 0x0f);
                assert_eq!(colour_at(&v, first, 8 * 9), 0x0f, "line strip");
            }
        }
    }

    #[test]
    fn strips_show_copies_playfield_and_vdel() {
        let mut tia = Tia::new(VideoStandard::Ntsc);
        tia.write(COLUP1, 0x44);
        tia.write(COLUPF, 0x86);
        tia.write(COLUBK, 0x02);
        tia.write(NUSIZ1, 0x03); // three copies close
        tia.write(GRP1, 0x01);
        tia.write(GRP0, 0x00); // copies GRP1 into its delayed copy
        tia.write(GRP1, 0xff);
        tia.write(PF0, 0x10); // playfield bit 0
        tia.write(CTRLPF, 0x01); // reflected: bit 39 too
        let v = view(&tia, GfxKind::Sprites, GfxOptions::new());
        assert_eq!((v.width, v.height), (160, 9 * 9 - 1));
        assert_eq!(v.param("grp1"), Some(0xff));
        assert_eq!(v.param("grp1_old"), Some(0x01));
        assert_eq!(v.raw.len(), 0x2d + 3);
        let p = v.param("pos_p1").unwrap();
        for copy in [0, 16, 32] {
            assert_eq!(colour_at(&v, (p + copy) % 160, 18), 0x22, "copy at +{copy}");
        }
        assert_eq!(colour_at(&v, (p + 48) % 160, 18), 0x01);
        // The delayed copy: only bit 0, the copy's last pixel.
        assert_eq!(colour_at(&v, (p + 7) % 160, 27), 0x22);
        assert_eq!(colour_at(&v, p, 27), 0x01);
        // Playfield bits 0 and 39.
        assert_eq!(colour_at(&v, 0, 63), 0x43);
        assert_eq!(colour_at(&v, 159, 63), 0x43);
        assert_eq!(colour_at(&v, 4, 63), 0x01);
        // Missile 1 is disabled: drawn in the gap colour.
        assert_eq!(colour_at(&v, v.param("pos_m1").unwrap(), 45), GAP);
        let one = view(&tia, GfxKind::Sprites, GfxOptions::new().with("index", 7));
        assert_eq!((one.width, one.height), (160, 8));
        assert_eq!(colour_at(&one, 0, 0), 0x43);
    }

    #[test]
    fn palette_has_128_colours() {
        let tia = Tia::new(VideoStandard::Ntsc);
        let v = view(&tia, GfxKind::Palette, GfxOptions::new());
        let pal = palette::tia(VideoStandard::Ntsc);
        assert_eq!(v.param("count"), Some(128));
        assert_eq!(v.raw[1], 2);
        assert_eq!(v.frame().rgb_at(9, 0), Some(pal[1]));
    }
}
