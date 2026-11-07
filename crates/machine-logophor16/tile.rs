//! Panorama's skin (owner's request 2026-11-06): the owner's artwork
//! `system/panorama/tile.png` (an aged leather panel in a brass frame
//! with a screw in every corner, 1254 × 1254) made at build time into
//!
//! - 19 colours (palette entries 232–250, which replace the greys there;
//!   251–252 are the title bars' British racing green, 253–255 the
//!   desk's shadow greys) by median cut over the pictures below;
//! - the dock tile: the whole panel at 48 × 48;
//! - the window skin: the panel at 160 × 160, from which Panorama cuts
//!   the corners, the rails and the leather (SPEC.md 11.2);
//! - the wallpaper (owner's artwork 2026-11-06,
//!   `system/panorama/wallpaper.png`: the top left quarter of a leather
//!   picture at 1024 × 768, 512 × 384; Panorama mirrors it into the
//!   other three);
//! - the window buttons (owner's artwork 2026-11-06,
//!   `system/panorama/buttons.png`: minimise, maximise, close at
//!   18 × 18 with alpha; transparent pixels become 0, the colour key);
//! - the mouse pointer (owner's artwork 2026-11-06,
//!   `system/panorama/cursor.png`: a copper arrow, 32 × 32 with alpha;
//!   transparent pixels become 0, sprite 0's transparent colour);
//! - the dock's application tiles from the owner's artwork in
//!   `system/panorama/icons/` (2026-11-06; kept at 192 × 192), each at
//!   48 × 48 under its application's name;
//!
//! every pixel the nearest of Panorama's 256 colours (`gfx.brass`'s
//! palette with the 24 new entries). The file `PANORAMA/TILE.RAW` on the
//! system card: 19 RGB 5:6:5 words (little-endian), the 19 entries'
//! darker and lighter neighbours (a palette index each, for `gfx`'s
//! shade tables), 2 304 tile bytes, 25 600 skin bytes, 196 608
//! wallpaper bytes, 972 button bytes (54 × 18), 1 024 pointer bytes,
//! then per
//! application tile its name (16 bytes, zero-padded) and 2 304 bytes.

use crate::logo::{median_cut, rgb565};

pub const SOURCE: usize = 1254;
pub const TILE: usize = 48;
/// The bytes of a tile's name.
pub const NAME: usize = 16;
pub const SKIN: usize = 160;
/// The wallpaper's quarter.
pub const WALL_W: usize = 512;
pub const WALL_H: usize = 384;
const NEW_COLOURS: usize = 19;
/// British racing green for active title bars, a darker one for the
/// others (palette 251, 252; gfx.brass).
pub const RACING_GREEN: [u8; 3] = [0, 66, 37];
pub const RACING_GREEN_DARK: [u8; 3] = [0, 44, 26];
/// The window buttons (minimise, maximise, close), side by side.
pub const BUTTON: usize = 18;
/// The mouse pointer (sprite 0, 32 x 32, the tip at the top left).
pub const CURSOR: usize = 32;
const FIRST_NEW: usize = 232;

/// A w x h RGBA picture's pixels, `None` where it is transparent.
fn rgba(png_bytes: &[u8], what: &str, w: usize, h: usize) -> Vec<Option<[u8; 3]>> {
    let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut r = dec.read_info().expect(what);
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).expect(what);
    assert_eq!(
        (info.width as usize, info.height as usize, info.color_type),
        (w, h, png::ColorType::Rgba),
        "{what}: {w} x {h} RGBA"
    );
    buf.chunks(4)
        .map(|p| (p[3] >= 128).then_some([p[0], p[1], p[2]]))
        .collect()
}

/// A square picture's pixels and its side.
fn decode(png_bytes: &[u8], what: &str) -> (Vec<[u8; 3]>, usize) {
    let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut r = dec.read_info().expect(what);
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).expect(what);
    let side = info.width as usize;
    assert_eq!(info.height as usize, side, "{what}: square");
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        t => panic!("{what}: colour type {t:?}"),
    };
    let px = (0..side * side)
        .map(|i| [buf[i * ch], buf[i * ch + 1], buf[i * ch + 2]])
        .collect();
    (px, side)
}

/// The picture (`side` × `side`) scaled to `d` × `d` by averaging each
/// destination pixel's source rectangle.
fn scale(src: &[[u8; 3]], side: usize, d: usize) -> Vec<[u8; 3]> {
    let source = side;
    let mut out = Vec::with_capacity(d * d);
    for dy in 0..d {
        for dx in 0..d {
            let (x0, x1) = (
                dx * source / d,
                ((dx + 1) * source / d).max(dx * source / d + 1),
            );
            let (y0, y1) = (
                dy * source / d,
                ((dy + 1) * source / d).max(dy * source / d + 1),
            );
            let mut s = [0u64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    for c in 0..3 {
                        s[c] += u64::from(src[y * source + x][c]);
                    }
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as u64;
            out.push(s.map(|v| ((v + n / 2) / n) as u8));
        }
    }
    out
}

/// Panorama's palette as `gfx.brass` builds it (RGB 5:6:5 → 8 bits a
/// channel), with the 24 new colours at the end.
fn palette(new: &[[u8; 3]]) -> Vec<[u8; 3]> {
    let expand = |c: u16| -> [u8; 3] {
        let (r, g, b) = ((c >> 11) as u8, (c >> 5 & 63) as u8, (c & 31) as u8);
        [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2]
    };
    let mut p = Vec::with_capacity(256);
    for i in 0..10u16 {
        let v = (i * 255 / 9) as u8;
        p.push(expand(rgb565([v, v, v])));
    }
    for b in [
        [70, 50, 20],
        [110, 80, 35],
        [150, 112, 50],
        [185, 142, 66],
        [212, 172, 92],
        [236, 206, 140],
    ] {
        p.push(expand(rgb565(b)));
    }
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                p.push(expand(rgb565([r * 51, g * 51, b * 51])));
            }
        }
    }
    assert_eq!(p.len(), FIRST_NEW);
    for c in new {
        p.push(expand(rgb565(*c)));
    }
    p.push(expand(rgb565(RACING_GREEN)));
    p.push(expand(rgb565(RACING_GREEN_DARK)));
    // the desk's shadow greys (gfx.brass)
    for v in [78u8, 71, 64] {
        p.push(expand(rgb565([v, v, v])));
    }
    p
}

fn nearest(p: [u8; 3], pal: &[[u8; 3]]) -> u8 {
    let dist = |q: &[u8; 3]| -> i64 {
        (0..3)
            .map(|c| {
                let d = i64::from(p[c]) - i64::from(q[c]);
                // green weighs most, as the eye does
                d * d * [3, 4, 2][c]
            })
            .sum()
    };
    pal.iter()
        .enumerate()
        .min_by_key(|(_, q)| dist(q))
        .map(|(i, _)| i as u8)
        .unwrap_or(0)
}

/// A picture for an application (RGB or RGBA, any size) in Panorama's
/// palette `pal` (from [`tile_raw`]): width and height (16 bits each,
/// little-endian), then a palette index a pixel (transparent ones 0, the
/// colour key).
pub fn picture(png_bytes: &[u8], pal: &[[u8; 3]]) -> Vec<u8> {
    let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut r = dec.read_info().expect("picture");
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).expect("picture");
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        t => panic!("picture: colour type {t:?}"),
    };
    let (w, h) = (info.width as usize, info.height as usize);
    let mut out = Vec::with_capacity(4 + w * h);
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    for p in buf[..w * h * ch].chunks(ch) {
        // opaque pixels never 0, the colour key
        out.push(if ch == 4 && p[3] < 128 {
            0
        } else {
            1 + nearest([p[0], p[1], p[2]], &pal[1..])
        });
    }
    out
}

/// The skin file's bytes and the palette it uses: `png_bytes` is the
/// panel, `icons` the applications' tiles by name.
pub fn tile_raw(
    png_bytes: &[u8],
    wallpaper: &[u8],
    buttons: &[u8],
    cursor: &[u8],
    icons: &[(String, Vec<u8>)],
) -> (Vec<u8>, Vec<[u8; 3]>) {
    let cursor = rgba(cursor, "cursor.png", CURSOR, CURSOR);
    let buttons = {
        let dec = png::Decoder::new(std::io::Cursor::new(buttons));
        let mut r = dec.read_info().expect("buttons.png");
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).expect("buttons.png");
        assert_eq!(
            (info.width as usize, info.height as usize, info.color_type),
            (3 * BUTTON, BUTTON, png::ColorType::Rgba),
            "buttons.png: 54 x 18 RGBA"
        );
        buf.chunks(4)
            .map(|p| (p[3] >= 128).then_some([p[0], p[1], p[2]]))
            .collect::<Vec<Option<[u8; 3]>>>()
    };
    let wall = {
        let dec = png::Decoder::new(std::io::Cursor::new(wallpaper));
        let mut r = dec.read_info().expect("wallpaper.png");
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).expect("wallpaper.png");
        assert_eq!(
            (info.width as usize, info.height as usize),
            (WALL_W, WALL_H),
            "wallpaper.png: {WALL_W} x {WALL_H}"
        );
        let ch = if info.color_type == png::ColorType::Rgba {
            4
        } else {
            3
        };
        (0..WALL_W * WALL_H)
            .map(|i| [buf[i * ch], buf[i * ch + 1], buf[i * ch + 2]])
            .collect::<Vec<[u8; 3]>>()
    };
    let (src, side) = decode(png_bytes, "tile.png");
    assert_eq!(side, SOURCE, "tile.png: {SOURCE} x {SOURCE}");
    let tile = scale(&src, side, TILE);
    let skin = scale(&src, side, SKIN);
    let icons: Vec<(&String, Vec<[u8; 3]>)> = icons
        .iter()
        .map(|(name, png)| {
            assert!(name.len() <= NAME, "{name}: at most {NAME} bytes");
            let (px, side) = decode(png, name);
            (name, scale(&px, side, TILE))
        })
        .collect();
    let all: Vec<[u8; 3]> = tile
        .iter()
        .chain(&skin)
        .chain(icons.iter().flat_map(|(_, px)| px))
        // the wallpaper by every eighth pixel, so its browns do not
        // crowd out the brass
        .chain(wall.iter().step_by(8))
        .copied()
        .collect();
    let mut new = median_cut(&all, NEW_COLOURS);
    new.resize(NEW_COLOURS, [0, 0, 0]);
    let pal = palette(&new);
    let mut out = Vec::with_capacity(4 * NEW_COLOURS + TILE * TILE + SKIN * SKIN);
    for c in &new {
        out.extend_from_slice(&rgb565(*c).to_le_bytes());
    }
    // darker (× 0.8) and lighter (× 1.2) as the palette has them
    let scaled = |c: &[u8; 3], num: u32| c.map(|v| (u32::from(v) * num / 10).min(255) as u8);
    out.extend(new.iter().map(|c| nearest(scaled(c, 8), &pal)));
    out.extend(new.iter().map(|c| nearest(scaled(c, 12), &pal)));
    out.extend(tile.iter().map(|p| nearest(*p, &pal)));
    out.extend(skin.iter().map(|p| nearest(*p, &pal)));
    out.extend(wall.iter().map(|p| nearest(*p, &pal)));
    // a button's pixel never 0 (the colour key): the nearest of the rest
    out.extend(
        buttons
            .iter()
            .map(|p| p.map_or(0, |p| 1 + nearest(p, &pal[1..]))),
    );
    out.extend(
        cursor
            .iter()
            .map(|p| p.map_or(0, |p| 1 + nearest(p, &pal[1..]))),
    );
    for (name, px) in icons {
        let mut n = [0u8; NAME];
        n[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&n);
        out.extend(px.iter().map(|p| nearest(*p, &pal)));
    }
    (out, pal)
}
