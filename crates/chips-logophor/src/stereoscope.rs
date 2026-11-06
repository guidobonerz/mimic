//! Stereoscope, the 3D unit in Babbage (SPEC.md section 7): registers at
//! FFFF:0200, a display list in chip RAM.
//!
//! Geometry: 4 × 4 matrix (16.16), perspective, clipping at the near plane
//! (polygons are cut), backface culling, light per vertex. Rasteriser:
//! triangles, lines, points; flat or Gouraud; textures of 4, 8 or 16 bits
//! up to 256 × 256 with perspective correction every 16 pixels; a 16-bit Z
//! buffer; 8-bit targets light, fog and blend through tables, with
//! ordered dithering; 16-bit targets scale and average RGB 5:6:5 directly.
//!
//! **[S]** (decided here, recorded in SPEC.md 7): every list starts from a
//! reset state (identity matrix, state 0, no texture, viewport the whole
//! target); the camera looks along +z with y up, a triangle is front-
//! facing when it runs clockwise on the screen; the light direction is in
//! camera space; the list runs at once when started and the unit then
//! stays busy for the slots its work costs (100 a vertex, 6 a pixel
//! tested, 2 a pixel cleared with the Z buffer, 1 without, 2 a command),
//! taking free chip-bus slots as the Blitter does. Internally the unit
//! computes in double precision; the fixed-point formats are its
//! interface.

use emu_core::{StateError, StateReader, StateWriter};

use crate::blitter::ChipRamMut;

pub const S3DLSEG: u16 = 0x0200;
pub const S3DLOFS: u16 = 0x0202;
pub const S3DTSEG: u16 = 0x0204;
pub const S3DTOFS: u16 = 0x0206;
pub const S3DPITCH: u16 = 0x0208;
pub const S3DMODE: u16 = 0x020a;
pub const S3DZSEG: u16 = 0x020c;
pub const S3DZOFS: u16 = 0x020e;
pub const S3DSTART: u16 = 0x0210;
pub const S3DSTAT: u16 = 0x0212;

/// Display list opcodes (bits 31–24 of a command word).
pub const CMD_END: u8 = 0x00;
pub const CMD_MATRIX: u8 = 0x01;
pub const CMD_VERTICES: u8 = 0x02;
pub const CMD_TRIANGLES: u8 = 0x03;
pub const CMD_LINES: u8 = 0x04;
pub const CMD_POINTS: u8 = 0x05;
pub const CMD_TEXTURE: u8 = 0x06;
pub const CMD_STATE: u8 = 0x07;
pub const CMD_LIGHT: u8 = 0x08;
pub const CMD_TABLES: u8 = 0x09;
pub const CMD_CLEAR: u8 = 0x0a;
pub const CMD_VIEWPORT: u8 = 0x0b;
pub const CMD_JUMP: u8 = 0x0c;
pub const CMD_FOG: u8 = 0x0d;

/// STATE bits.
pub const ST_GOURAUD: u32 = 1 << 0;
pub const ST_TEXTURE: u32 = 1 << 1;
pub const ST_ZTEST: u32 = 1 << 2;
pub const ST_ZWRITE: u32 = 1 << 3;
pub const ST_CULL: u32 = 1 << 4;
pub const ST_LIGHT: u32 = 1 << 5;
pub const ST_DITHER: u32 = 1 << 6;
pub const ST_BLEND: u32 = 1 << 7;
pub const ST_FOG: u32 = 1 << 8;

const CHIP_END: u32 = 0x0040_0000;
/// Commands a list may run before it counts as runaway.
const MAX_COMMANDS: u32 = 1 << 20;
const COST_VERTEX: u64 = 100;
const COST_PIXEL: u64 = 6;
const COST_COMMAND: u64 = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stereoscope {
    list: (u16, u16),
    target: (u16, u16),
    pitch: u16,
    mode: u16,
    zbuf: (u16, u16),
    busy: bool,
    done: bool,
    error: bool,
    /// A START waits for the machine to run the list (it needs memory).
    start_pending: bool,
    /// Slots the work still costs.
    remaining: u64,
}

fn lin(p: (u16, u16)) -> u32 {
    u32::from(p.0) << 16 | u32::from(p.1)
}

/// A vertex in camera space with its attributes.
#[derive(Clone, Copy, Debug, Default)]
struct Cam {
    p: [f64; 3],
    u: f64,
    v: f64,
    /// Light intensity 0–255.
    i: f64,
    colour: u16,
}

/// A vertex on the screen.
#[derive(Clone, Copy, Debug, Default)]
struct Scr {
    x: f64,
    y: f64,
    iz: f64,
    uz: f64,
    vz: f64,
    i: f64,
}

#[derive(Clone, Copy, Debug)]
struct Texture {
    at: u32,
    w: u32,
    h: u32,
    bits: u32,
}

/// What a list sets up as it runs.
struct Run<'a> {
    mem: &'a mut dyn ChipRamMut,
    cost: u64,
    matrix: [f64; 16],
    verts: Vec<Cam>,
    texture: Option<Texture>,
    state: u32,
    light: ([f64; 3], f64, f64),
    blend: u32,
    shade: u32,
    viewport: (i32, i32, i32, i32),
    near: f64,
    scale: f64,
    fog: (f64, f64),
    target: u32,
    pitch: u32,
    sixteen: bool,
    zbuf: u32,
}

/// 4 × 4 ordered dither, −8..+7.
const BAYER: [[i32; 4]; 4] = [
    [-8, 0, -6, 2],
    [4, -4, 6, -2],
    [-5, 3, -7, 1],
    [7, -1, 5, -3],
];

impl Stereoscope {
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            S3DLSEG => self.list.0,
            S3DLOFS => self.list.1,
            S3DTSEG => self.target.0,
            S3DTOFS => self.target.1,
            S3DPITCH => self.pitch,
            S3DMODE => self.mode,
            S3DZSEG => self.zbuf.0,
            S3DZOFS => self.zbuf.1,
            S3DSTAT => {
                u16::from(self.busy) | u16::from(self.done) << 1 | u16::from(self.error) << 2
            }
            _ => 0,
        }
    }

    /// Register read: S3DSTAT's done and error bits clear.
    pub fn read(&mut self, off: u16) -> u16 {
        let v = self.peek(off);
        if off & !1 == S3DSTAT {
            self.done = false;
            self.error = false;
        }
        v
    }

    pub fn write(&mut self, off: u16, v: u16) {
        match off & !1 {
            S3DLSEG => self.list.0 = v,
            S3DLOFS => self.list.1 = v,
            S3DTSEG => self.target.0 = v,
            S3DTOFS => self.target.1 = v,
            S3DPITCH => self.pitch = v,
            S3DMODE => self.mode = v & 3,
            S3DZSEG => self.zbuf.0 = v,
            S3DZOFS => self.zbuf.1 = v,
            S3DSTART if !self.busy => self.start_pending = true,
            _ => {}
        }
    }

    /// A START was written: the machine calls [`Stereoscope::run`].
    pub fn take_start(&mut self) -> bool {
        std::mem::take(&mut self.start_pending)
    }

    /// One free slot for a busy unit; `true` when its work is done.
    pub fn slot(&mut self) -> bool {
        if !self.busy {
            return false;
        }
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining == 0 {
            self.busy = false;
            self.done = true;
            return true;
        }
        false
    }

    /// Run the display list; `false` on an error (S3DSTAT bit 2 set, the
    /// caller raises the DMA error). The work done stays; the unit is
    /// busy for its cost either way.
    pub fn run(&mut self, mem: &mut dyn ChipRamMut) -> bool {
        let (w, h) = if self.mode == 3 {
            (800, 600)
        } else {
            (320, 240)
        };
        let mut r = Run {
            mem,
            cost: 0,
            matrix: [
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
            verts: Vec::new(),
            texture: None,
            state: 0,
            light: ([0., 0., 1.], 0., 255.),
            blend: 0,
            shade: 0,
            viewport: (0, 0, w, h),
            near: 1.0,
            scale: f64::from(w) / 2.0,
            fog: (0., 0.),
            target: lin(self.target),
            pitch: u32::from(self.pitch),
            sixteen: self.mode == 1,
            zbuf: lin(self.zbuf),
        };
        let ok = lin(self.target) < CHIP_END && r.list(lin(self.list)).is_ok();
        self.error = !ok;
        self.busy = true;
        self.done = false;
        self.remaining = r.cost.max(1);
        ok
    }

    pub fn save(&self, w: &mut StateWriter) {
        for v in [
            self.list.0,
            self.list.1,
            self.target.0,
            self.target.1,
            self.pitch,
            self.mode,
            self.zbuf.0,
            self.zbuf.1,
        ] {
            w.u16(v);
        }
        w.bool(self.busy);
        w.bool(self.done);
        w.bool(self.error);
        w.bool(self.start_pending);
        w.u64(self.remaining);
    }

    pub fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut s = Stereoscope::default();
        let mut f = || c.u16();
        s.list = (f()?, f()?);
        s.target = (f()?, f()?);
        s.pitch = f()?;
        s.mode = f()? & 3;
        s.zbuf = (f()?, f()?);
        s.busy = c.bool()?;
        s.done = c.bool()?;
        s.error = c.bool()?;
        s.start_pending = c.bool()?;
        s.remaining = c.u64()?.min(1 << 32);
        *self = s;
        Ok(())
    }
}

fn fix(v: u32) -> f64 {
    f64::from(v as i32) / 65536.0
}

impl Run<'_> {
    fn byte(&self, a: u32) -> Result<u8, ()> {
        if a >= CHIP_END {
            return Err(());
        }
        Ok(self.mem.get(a))
    }

    fn word(&self, a: u32) -> Result<u16, ()> {
        Ok(u16::from_le_bytes([self.byte(a)?, self.byte(a + 1)?]))
    }

    fn dword(&self, a: u32) -> Result<u32, ()> {
        Ok(u32::from(self.word(a)?) | u32::from(self.word(a + 2)?) << 16)
    }

    /// Parameter `n` after the command at `pc`.
    fn par(&self, pc: u32, n: u32) -> Result<u32, ()> {
        self.dword(pc + 4 * n)
    }

    /// Run the list at `pc`.
    fn list(&mut self, mut pc: u32) -> Result<(), ()> {
        for _ in 0..MAX_COMMANDS {
            let cmd = self.dword(pc)?;
            pc += 4;
            self.cost += COST_COMMAND;
            let (op, arg) = ((cmd >> 24) as u8, cmd & 0x00ff_ffff);
            match op {
                CMD_END => return Ok(()),
                CMD_MATRIX => {
                    for k in 0..16 {
                        self.matrix[k] = fix(self.par(pc, k as u32)?);
                    }
                    pc += 64;
                }
                CMD_VERTICES => {
                    let at = self.par(pc, 0)?;
                    pc += 4;
                    self.vertices(at, arg.min(256))?;
                }
                CMD_TRIANGLES | CMD_LINES | CMD_POINTS => {
                    let at = self.par(pc, 0)?;
                    pc += 4;
                    let per = match op {
                        CMD_TRIANGLES => 3,
                        CMD_LINES => 2,
                        _ => 1,
                    };
                    for n in 0..arg {
                        let mut idx = [0usize; 3];
                        for (k, slot) in idx.iter_mut().take(per).enumerate() {
                            *slot = usize::from(self.word(at + 2 * (per as u32 * n + k as u32))?);
                        }
                        if idx[..per].iter().any(|i| *i >= self.verts.len()) {
                            continue;
                        }
                        match op {
                            CMD_TRIANGLES => self.triangle(idx),
                            CMD_LINES => self.line(idx[0], idx[1]),
                            _ => self.point(idx[0]),
                        }
                    }
                }
                CMD_TEXTURE => {
                    let at = self.par(pc, 0)?;
                    pc += 4;
                    if at >= CHIP_END {
                        return Err(());
                    }
                    self.texture = Some(Texture {
                        at,
                        w: 1 << (arg & 15).min(8),
                        h: 1 << (arg >> 4 & 15).min(8),
                        bits: match arg >> 8 & 3 {
                            0 => 4,
                            1 => 8,
                            _ => 16,
                        },
                    });
                }
                CMD_STATE => self.state = arg,
                CMD_LIGHT => {
                    let d = [
                        fix(self.par(pc, 0)?),
                        fix(self.par(pc, 1)?),
                        fix(self.par(pc, 2)?),
                    ];
                    pc += 12;
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
                    self.light = (
                        [d[0] / len, d[1] / len, d[2] / len],
                        f64::from(arg & 0xff),
                        f64::from(arg >> 8 & 0xff),
                    );
                }
                CMD_TABLES => {
                    self.blend = self.par(pc, 0)?;
                    self.shade = self.par(pc, 1)?;
                    pc += 8;
                }
                CMD_CLEAR => {
                    let z = self.par(pc, 0)? as u16;
                    pc += 4;
                    self.clear(arg as u16, z);
                }
                CMD_VIEWPORT => {
                    let (xy, wh) = (self.par(pc, 0)?, self.par(pc, 1)?);
                    let (near, scale) = (fix(self.par(pc, 2)?), fix(self.par(pc, 3)?));
                    pc += 16;
                    self.viewport = (
                        i32::from(xy as u16),
                        i32::from((xy >> 16) as u16),
                        i32::from(wh as u16),
                        i32::from((wh >> 16) as u16),
                    );
                    self.near = near.max(1.0 / 65536.0);
                    self.scale = scale;
                }
                CMD_JUMP => pc = self.par(pc, 0)?,
                CMD_FOG => {
                    self.fog = (fix(self.par(pc, 0)?), fix(self.par(pc, 1)?));
                    pc += 8;
                }
                _ => return Err(()),
            }
        }
        Err(())
    }

    /// Transform and light `n` vertices at `at` (24 bytes each).
    fn vertices(&mut self, at: u32, n: u32) -> Result<(), ()> {
        self.verts.clear();
        let m = self.matrix;
        for k in 0..n {
            let a = at + 24 * k;
            let p = [
                fix(self.dword(a)?),
                fix(self.dword(a + 4)?),
                fix(self.dword(a + 8)?),
            ];
            let nrm =
                [0, 2, 4].map(|o| f64::from(self.word(a + 12 + o).unwrap_or(0) as i16) / 32768.0);
            let u = f64::from(self.word(a + 18)? as i16) / 16.0;
            let v = f64::from(self.word(a + 20)? as i16) / 16.0;
            let colour = self.word(a + 22)?;
            let t = |r: usize| {
                m[4 * r] * p[0] + m[4 * r + 1] * p[1] + m[4 * r + 2] * p[2] + m[4 * r + 3]
            };
            let cam = [t(0), t(1), t(2)];
            let i = if self.state & ST_LIGHT != 0 {
                let rn =
                    |r: usize| m[4 * r] * nrm[0] + m[4 * r + 1] * nrm[1] + m[4 * r + 2] * nrm[2];
                let n2 = [rn(0), rn(1), rn(2)];
                // Normalised after the transform: 1.15 cannot hold 1.0,
                // and a scaling matrix would change the length.
                let len = (n2[0] * n2[0] + n2[1] * n2[1] + n2[2] * n2[2])
                    .sqrt()
                    .max(1e-9);
                let n2 = n2.map(|c| c / len);
                let (l, amb, dif) = self.light;
                let d = -(n2[0] * l[0] + n2[1] * l[1] + n2[2] * l[2]);
                (amb + dif * d.max(0.0)).min(255.0)
            } else {
                255.0
            };
            self.verts.push(Cam {
                p: cam,
                u,
                v,
                i,
                colour,
            });
            self.cost += COST_VERTEX;
        }
        Ok(())
    }

    fn project(&self, c: &Cam) -> Scr {
        let (vx, vy, vw, vh) = self.viewport;
        let iz = 1.0 / c.p[2];
        Scr {
            x: f64::from(vx) + f64::from(vw) / 2.0 + self.scale * c.p[0] * iz,
            y: f64::from(vy) + f64::from(vh) / 2.0 - self.scale * c.p[1] * iz,
            iz,
            uz: c.u * iz,
            vz: c.v * iz,
            i: c.i,
        }
    }

    /// A triangle: clipped at the near plane, culled, filled.
    fn triangle(&mut self, idx: [usize; 3]) {
        let near = self.near;
        let mut poly: Vec<Cam> = idx.iter().map(|i| self.verts[*i]).collect();
        // Sutherland–Hodgman against z = near.
        let mut out = Vec::with_capacity(4);
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            let (ina, inb) = (a.p[2] >= near, b.p[2] >= near);
            if ina {
                out.push(a);
            }
            if ina != inb {
                let t = (near - a.p[2]) / (b.p[2] - a.p[2]);
                let lerp = |x: f64, y: f64| x + (y - x) * t;
                out.push(Cam {
                    p: [lerp(a.p[0], b.p[0]), lerp(a.p[1], b.p[1]), near],
                    u: lerp(a.u, b.u),
                    v: lerp(a.v, b.v),
                    i: lerp(a.i, b.i),
                    colour: a.colour,
                });
            }
        }
        poly = out;
        if poly.len() < 3 {
            return;
        }
        let s: Vec<Scr> = poly.iter().map(|c| self.project(c)).collect();
        let flat = s[0].i;
        let colour = self.verts[idx[0]].colour;
        for k in 1..s.len() - 1 {
            self.fill(s[0], s[k], s[k + 1], flat, colour);
        }
    }

    fn fill(&mut self, a: Scr, b: Scr, c: Scr, flat: f64, colour: u16) {
        let edge =
            |p: &Scr, q: &Scr, x: f64, y: f64| (q.x - p.x) * (y - p.y) - (q.y - p.y) * (x - p.x);
        let mut area = edge(&a, &b, c.x, c.y);
        let (mut a, mut b) = (a, b);
        if area <= 0.0 {
            if self.state & ST_CULL != 0 || area == 0.0 {
                return;
            }
            std::mem::swap(&mut a, &mut b);
            area = -area;
        }
        let (vx, vy, vw, vh) = self.viewport;
        let x0 = (a.x.min(b.x).min(c.x).floor() as i32).max(vx);
        let x1 = (a.x.max(b.x).max(c.x).ceil() as i32).min(vx + vw - 1);
        let y0 = (a.y.min(b.y).min(c.y).floor() as i32).max(vy);
        let y1 = (a.y.max(b.y).max(c.y).ceil() as i32).min(vy + vh - 1);
        // Ties on an edge belong to one side only.
        let owns = |p: &Scr, q: &Scr| {
            let (dx, dy) = (q.x - p.x, q.y - p.y);
            dy > 0.0 || dy == 0.0 && dx > 0.0
        };
        let (oab, obc, oca) = (owns(&a, &b), owns(&b, &c), owns(&c, &a));
        let inside = |w: f64, own: bool| w > 0.0 || w == 0.0 && own;
        let bary = |x: f64, y: f64| {
            (
                edge(&b, &c, x, y) / area,
                edge(&c, &a, x, y) / area,
                edge(&a, &b, x, y) / area,
            )
        };
        let interp =
            |l: (f64, f64, f64), f: fn(&Scr) -> f64| l.0 * f(&a) + l.1 * f(&b) + l.2 * f(&c);
        for y in y0..=y1 {
            let py = f64::from(y) + 0.5;
            let mut group = i32::MIN;
            let mut uv = ((0.0, 0.0), (0.0, 0.0));
            for x in x0..=x1 {
                let px = f64::from(x) + 0.5;
                let (wa, wb, wc) = (
                    edge(&b, &c, px, py),
                    edge(&c, &a, px, py),
                    edge(&a, &b, px, py),
                );
                if !(inside(wa, obc) && inside(wb, oca) && inside(wc, oab)) {
                    continue;
                }
                self.cost += COST_PIXEL;
                let l = (wa / area, wb / area, wc / area);
                let iz = interp(l, |s| s.iz);
                let z = 1.0 / iz;
                let i = if self.state & ST_GOURAUD != 0 {
                    interp(l, |s| s.i)
                } else {
                    flat
                };
                let (u, v) = if self.state & ST_TEXTURE != 0 {
                    // Exact at every 16th pixel, straight in between.
                    let g = x & !15;
                    if g != group {
                        group = g;
                        let exact = |gx: i32| {
                            let l = bary(f64::from(gx) + 0.5, py);
                            let iz = interp(l, |s| s.iz);
                            (interp(l, |s| s.uz) / iz, interp(l, |s| s.vz) / iz)
                        };
                        uv = (exact(g), exact(g + 16));
                    }
                    let t = f64::from(x - group) / 16.0;
                    (
                        uv.0 .0 + (uv.1 .0 - uv.0 .0) * t,
                        uv.0 .1 + (uv.1 .1 - uv.0 .1) * t,
                    )
                } else {
                    (0.0, 0.0)
                };
                self.plot(x, y, z, i, colour, u, v);
            }
        }
    }

    /// One pixel: Z test, colour, light, fog, blend, write.
    #[allow(clippy::too_many_arguments)]
    fn plot(&mut self, x: i32, y: i32, z: f64, i: f64, colour: u16, u: f64, v: f64) {
        let depth = (z * 256.0).clamp(0.0, 65535.0) as u16;
        let zat = self
            .zbuf
            .wrapping_add(2 * (y as u32 * self.row_pixels() + x as u32));
        if self.state & ST_ZTEST != 0 && self.zbuf < CHIP_END {
            let old = u16::from_le_bytes([self.mem.get(zat), self.mem.get(zat + 1)]);
            if depth >= old {
                return;
            }
        }
        if self.state & ST_ZWRITE != 0 && self.zbuf < CHIP_END {
            let [lo, hi] = depth.to_le_bytes();
            self.mem.set(zat, lo);
            self.mem.set(zat + 1, hi);
        }
        let mut i = i;
        if self.state & ST_FOG != 0 && self.fog.1 > self.fog.0 {
            let t = ((z - self.fog.0) / (self.fog.1 - self.fog.0)).clamp(0.0, 1.0);
            i *= 1.0 - t;
        }
        let base = if self.state & ST_TEXTURE != 0 {
            self.texel(u, v)
        } else {
            colour
        };
        let at = self
            .target
            .wrapping_add((y as u32) * self.pitch + x as u32 * if self.sixteen { 2 } else { 1 });
        if at + 1 >= CHIP_END {
            return;
        }
        if self.sixteen {
            let k = i.clamp(0.0, 255.0) / 255.0;
            let ch = |c: u16, sh: u16, m: u16| ((f64::from(c >> sh & m) * k) as u16).min(m) << sh;
            let mut c = ch(base, 11, 31) | ch(base, 5, 63) | ch(base, 0, 31);
            if self.state & ST_BLEND != 0 {
                let d = u16::from_le_bytes([self.mem.get(at), self.mem.get(at + 1)]);
                let avg = |sh: u16, m: u16| (((c >> sh & m) + (d >> sh & m)) / 2) << sh;
                c = avg(11, 31) | avg(5, 63) | avg(0, 31);
            }
            let [lo, hi] = c.to_le_bytes();
            self.mem.set(at, lo);
            self.mem.set(at + 1, hi);
        } else {
            let mut idx = base as u8;
            if self.shade != 0 && (self.state & (ST_LIGHT | ST_FOG) != 0) {
                let mut li = i.round() as i32;
                if self.state & ST_DITHER != 0 {
                    li += BAYER[(y & 3) as usize][(x & 3) as usize];
                }
                let li = li.clamp(0, 255) as u32;
                idx = self
                    .mem
                    .get(self.shade.wrapping_add(li * 256 + u32::from(idx)));
            }
            if self.state & ST_BLEND != 0 && self.blend != 0 {
                let d = self.mem.get(at);
                idx = self
                    .mem
                    .get(self.blend.wrapping_add(u32::from(idx) * 256 + u32::from(d)));
            }
            self.mem.set(at, idx);
        }
    }

    /// Pixels a target row holds (the Z buffer's row in pixels).
    fn row_pixels(&self) -> u32 {
        if self.sixteen {
            self.pitch / 2
        } else {
            self.pitch
        }
    }

    fn texel(&self, u: f64, v: f64) -> u16 {
        let Some(t) = self.texture else {
            return 0;
        };
        let tx = (u.floor() as i64).rem_euclid(i64::from(t.w)) as u32;
        let ty = (v.floor() as i64).rem_euclid(i64::from(t.h)) as u32;
        let n = ty * t.w + tx;
        match t.bits {
            4 => {
                let b = self.mem.get(t.at + n / 2);
                u16::from(if n.is_multiple_of(2) { b >> 4 } else { b & 15 })
            }
            8 => u16::from(self.mem.get(t.at + n)),
            _ => u16::from_le_bytes([self.mem.get(t.at + 2 * n), self.mem.get(t.at + 2 * n + 1)]),
        }
    }

    fn line(&mut self, a: usize, b: usize) {
        let (ca, cb) = (self.verts[a], self.verts[b]);
        if ca.p[2] < self.near || cb.p[2] < self.near {
            return;
        }
        let (sa, sb) = (self.project(&ca), self.project(&cb));
        let steps = (sb.x - sa.x).abs().max((sb.y - sa.y).abs()).ceil().max(1.0) as i32;
        for k in 0..=steps {
            let t = f64::from(k) / f64::from(steps);
            let x = (sa.x + (sb.x - sa.x) * t).floor() as i32;
            let y = (sa.y + (sb.y - sa.y) * t).floor() as i32;
            let iz = sa.iz + (sb.iz - sa.iz) * t;
            self.pixel_in_view(x, y, 1.0 / iz, sa.i, ca.colour);
        }
    }

    fn point(&mut self, a: usize) {
        let c = self.verts[a];
        if c.p[2] < self.near {
            return;
        }
        let s = self.project(&c);
        self.pixel_in_view(
            s.x.floor() as i32,
            s.y.floor() as i32,
            c.p[2],
            s.i,
            c.colour,
        );
    }

    fn pixel_in_view(&mut self, x: i32, y: i32, z: f64, i: f64, colour: u16) {
        let (vx, vy, vw, vh) = self.viewport;
        if (vx..vx + vw).contains(&x) && (vy..vy + vh).contains(&y) {
            self.cost += COST_PIXEL;
            let state = self.state;
            self.state &= !ST_TEXTURE;
            self.plot(x, y, z, i, colour, 0.0, 0.0);
            self.state = state;
        }
    }

    /// CLEAR: the viewport in `colour`, the Z buffer to `z`.
    fn clear(&mut self, colour: u16, z: u16) {
        let (vx, vy, vw, vh) = self.viewport;
        let with_z = self.state & (ST_ZTEST | ST_ZWRITE) != 0;
        for y in vy..vy + vh {
            for x in vx..vx + vw {
                let (x, y) = (x as u32, y as u32);
                let at = self.target + y * self.pitch + x * if self.sixteen { 2 } else { 1 };
                if at + 1 < CHIP_END {
                    if self.sixteen {
                        let [lo, hi] = colour.to_le_bytes();
                        self.mem.set(at, lo);
                        self.mem.set(at + 1, hi);
                    } else {
                        self.mem.set(at, colour as u8);
                    }
                }
                if with_z {
                    let zat = self.zbuf + 2 * (y * self.row_pixels() + x);
                    if zat + 1 < CHIP_END {
                        let [lo, hi] = z.to_le_bytes();
                        self.mem.set(zat, lo);
                        self.mem.set(zat + 1, hi);
                    }
                }
                self.cost += if with_z { 2 } else { 1 };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRamMut for Ram {
        fn get(&self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn set(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
    }

    const LIST: u32 = 0x1000;
    const VERTS: u32 = 0x2000;
    const INDEX: u32 = 0x3000;
    const TARGET: u32 = 0x1_0000;
    const ZBUF: u32 = 0x4_0000;
    const TABLE: u32 = 0x8_0000;

    fn ram() -> Ram {
        Ram(vec![0; CHIP_END as usize])
    }

    fn fx(v: f64) -> u32 {
        (v * 65536.0).round() as i32 as u32
    }

    fn cmd(op: u8, arg: u32) -> u32 {
        u32::from(op) << 24 | arg
    }

    struct V {
        p: [f64; 3],
        n: [f64; 3],
        uv: [f64; 2],
        colour: u16,
    }

    fn v(x: f64, y: f64, z: f64, colour: u16) -> V {
        V {
            p: [x, y, z],
            n: [0.0, 0.0, -1.0],
            uv: [0.0, 0.0],
            colour,
        }
    }

    impl Ram {
        fn dwords(&mut self, at: u32, d: &[u32]) {
            for (k, w) in d.iter().enumerate() {
                let a = (at + 4 * k as u32) as usize;
                self.0[a..a + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
        fn words(&mut self, at: u32, d: &[u16]) {
            for (k, w) in d.iter().enumerate() {
                let a = (at + 2 * k as u32) as usize;
                self.0[a..a + 2].copy_from_slice(&w.to_le_bytes());
            }
        }
        fn verts(&mut self, vs: &[V]) {
            for (k, x) in vs.iter().enumerate() {
                let a = VERTS + 24 * k as u32;
                self.dwords(a, &x.p.map(fx));
                let n = x.n.map(|c| (c * 32767.0).round() as i16 as u16);
                let uv = x.uv.map(|c| (c * 16.0).round() as i16 as u16);
                self.words(a + 12, &[n[0], n[1], n[2], uv[0], uv[1], x.colour]);
            }
        }
        fn px(&self, x: u32, y: u32) -> u8 {
            self.0[(TARGET + y * 320 + x) as usize]
        }
        fn px16(&self, x: u32, y: u32) -> u16 {
            let a = (TARGET + y * 640 + 2 * x) as usize;
            u16::from_le_bytes([self.0[a], self.0[a + 1]])
        }
        fn count(&self, c: u8) -> usize {
            self.0[TARGET as usize..][..320 * 240]
                .iter()
                .filter(|p| **p == c)
                .count()
        }
    }

    /// `list` at LIST plus END, run in mode `mode` (0: 8 bit, pitch 320;
    /// 1: 16 bit, pitch 640); the slots it took.
    fn run(ram: &mut Ram, mode: u16, list: &[u32]) -> u64 {
        let mut l = list.to_vec();
        l.push(cmd(CMD_END, 0));
        ram.dwords(LIST, &l);
        let mut s = Stereoscope::default();
        for (r, x) in [
            (S3DLSEG, LIST >> 16),
            (S3DLOFS, LIST & 0xffff),
            (S3DTSEG, TARGET >> 16),
            (S3DTOFS, TARGET & 0xffff),
            (S3DPITCH, if mode == 1 { 640 } else { 320 }),
            (S3DMODE, u32::from(mode)),
            (S3DZSEG, ZBUF >> 16),
            (S3DZOFS, ZBUF & 0xffff),
        ] {
            s.write(r, x as u16);
        }
        s.write(S3DSTART, 1);
        assert!(s.take_start());
        assert!(s.run(ram), "list ran");
        assert_eq!(s.read(S3DSTAT), 1, "busy");
        let mut slots = 1;
        while !s.slot() {
            slots += 1;
            assert!(slots < 100_000_000);
        }
        assert_eq!(s.read(S3DSTAT), 2, "done");
        assert_eq!(s.read(S3DSTAT), 0, "cleared on read");
        slots
    }

    fn triangles(n: u32) -> [u32; 4] {
        [
            cmd(CMD_VERTICES, 3 * n),
            VERTS,
            cmd(CMD_TRIANGLES, n),
            INDEX,
        ]
    }

    #[test]
    fn a_triangle_is_projected_filled_and_culled() {
        // (0,0,2), (1,0,2), (0,-1,2) -> (160,120), (240,120), (160,200):
        // clockwise on the screen, so its front faces the camera.
        let mut r = ram();
        r.verts(&[v(0., 0., 2., 7), v(1., 0., 2., 7), v(0., -1., 2., 7)]);
        r.words(INDEX, &[0, 1, 2]);
        run(
            &mut r,
            0,
            &[&[cmd(CMD_STATE, ST_CULL)][..], &triangles(1)].concat(),
        );
        assert_eq!(r.px(170, 130), 7);
        assert_eq!(r.px(160, 120), 7, "top-left corner pixel");
        assert_eq!((r.px(230, 190), r.px(159, 130), r.px(170, 119)), (0, 0, 0));
        let n = r.count(7);
        assert!((3180..=3260).contains(&n), "half of 80 x 80: {n}");
        // The other way round it faces away: culled, or drawn without CULL.
        let mut r = ram();
        r.verts(&[v(0., 0., 2., 7), v(1., 0., 2., 7), v(0., -1., 2., 7)]);
        r.words(INDEX, &[0, 2, 1]);
        run(
            &mut r,
            0,
            &[&[cmd(CMD_STATE, ST_CULL)][..], &triangles(1)].concat(),
        );
        assert_eq!(r.count(7), 0);
        run(&mut r, 0, &triangles(1));
        assert_eq!(r.count(7), n);
    }

    #[test]
    fn shared_edges_draw_every_pixel_once() {
        // Two triangles of a square, blended over black: a pixel drawn
        // twice would come out at a quarter, not half.
        let mut r = ram();
        let c = 0xffff;
        r.verts(&[
            v(-1., 1., 2., c),
            v(1., 1., 2., c),
            v(1., -1., 2., c),
            v(-1., -1., 2., c),
        ]);
        r.words(INDEX, &[0, 1, 2, 0, 2, 3]);
        let list = [
            cmd(CMD_CLEAR, 0),
            0,
            cmd(CMD_STATE, ST_BLEND),
            cmd(CMD_VERTICES, 4),
            VERTS,
            cmd(CMD_TRIANGLES, 2),
            INDEX,
        ];
        run(&mut r, 1, &list);
        let half = (15 << 11) | (31 << 5) | 15;
        for (x, y) in [
            (81, 41),
            (160, 120),
            (120, 80),
            (121, 80),
            (200, 160),
            (239, 199),
        ] {
            assert_eq!(r.px16(x, y), half, "({x}, {y})");
        }
        assert_eq!((r.px16(79, 120), r.px16(240, 120)), (0, 0));
    }

    #[test]
    fn the_z_buffer_keeps_the_nearer_surface() {
        for near_first in [true, false] {
            let mut r = ram();
            // A square at z = 2 (colour 1) and a larger one at z = 4
            // (colour 2, screen x 40 to 280).
            let near = [
                v(-1., 1., 2., 1),
                v(1., 1., 2., 1),
                v(1., -1., 2., 1),
                v(-1., -1., 2., 1),
            ];
            let far = [
                v(-3., 3., 4., 2),
                v(3., 3., 4., 2),
                v(3., -3., 4., 2),
                v(-3., -3., 4., 2),
            ];
            let (a, b) = if near_first { (near, far) } else { (far, near) };
            r.verts(&a.into_iter().chain(b).collect::<Vec<_>>());
            r.words(INDEX, &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
            let list = [
                cmd(CMD_STATE, ST_ZTEST | ST_ZWRITE),
                cmd(CMD_CLEAR, 9),
                0xffff,
                cmd(CMD_VERTICES, 8),
                VERTS,
                cmd(CMD_TRIANGLES, 4),
                INDEX,
            ];
            run(&mut r, 0, &list);
            assert_eq!(r.px(160, 120), 1, "near_first {near_first}");
            assert_eq!(r.px(60, 120), 2);
            assert_eq!(r.px(2, 2), 9, "cleared");
            let z = |x: u32, y: u32| {
                let a = (ZBUF + 2 * (y * 320 + x)) as usize;
                u16::from_le_bytes([r.0[a], r.0[a + 1]])
            };
            assert_eq!((z(160, 120), z(60, 120), z(2, 2)), (512, 1024, 0xffff));
        }
    }

    #[test]
    fn the_near_plane_cuts_polygons() {
        // A floor from z = -2 to z = 6 below the camera: the part behind
        // the near plane (z = 1) is cut off, the rest drawn.
        let mut r = ram();
        r.verts(&[
            v(-1., -1., -2., 3),
            v(1., -1., -2., 3),
            v(1., -1., 6., 3),
            v(-1., -1., 6., 3),
        ]);
        r.words(INDEX, &[0, 3, 2, 0, 2, 1]);
        run(&mut r, 0, &triangles(2));
        // At z = 1 the floor is at screen y 280 (off the target), at z = 6
        // at y 146.7.
        assert_eq!((r.px(160, 150), r.px(160, 239), r.px(160, 140)), (3, 3, 0));
        // Wholly behind: nothing, and no fault.
        let mut r = ram();
        r.verts(&[v(0., 0., -2., 3), v(1., 0., -2., 3), v(0., -1., -2., 3)]);
        r.words(INDEX, &[0, 1, 2]);
        run(&mut r, 0, &triangles(1));
        assert_eq!(r.count(3), 0);
    }

    #[test]
    fn textures_are_perspective_correct() {
        // A 2 x 2 texture of 8 bits (1 2 / 3 4) on a square facing the
        // camera, then on a quad receding to the right.
        let quad = |zr: f64, r: &mut Ram| {
            let mut vs = [
                v(-1., 1., 2., 0),
                v(1., 1., zr, 0),
                v(1., -1., zr, 0),
                v(-1., -1., 2., 0),
            ];
            for (x, uv) in vs.iter_mut().zip([[0., 0.], [2., 0.], [2., 2.], [0., 2.]]) {
                x.uv = uv;
            }
            r.verts(&vs);
            r.words(INDEX, &[0, 1, 2, 0, 2, 3]);
            r.0[0x5000..0x5004].copy_from_slice(&[1, 2, 3, 4]);
        };
        let list = [
            cmd(CMD_TEXTURE, 1 | 1 << 4 | 1 << 8),
            0x5000,
            cmd(CMD_STATE, ST_TEXTURE),
            cmd(CMD_VERTICES, 4),
            VERTS,
            cmd(CMD_TRIANGLES, 2),
            INDEX,
        ];
        let mut r = ram();
        quad(2.0, &mut r);
        run(&mut r, 0, &list);
        assert_eq!(
            [r.px(100, 60), r.px(220, 60), r.px(100, 180), r.px(220, 180)],
            [1, 2, 3, 4]
        );
        // Receding to z = 6: u = 1 lies at x = 0, z = 4, screen x 160,
        // where an affine mapping would put it at 133.
        let mut r = ram();
        quad(6.0, &mut r);
        run(&mut r, 0, &list);
        assert_eq!((r.px(140, 110), r.px(157, 110), r.px(163, 110)), (1, 1, 2));
    }

    #[test]
    fn lines_points_and_the_viewport() {
        let mut r = ram();
        r.verts(&[v(0., 0., 2., 5), v(1., 0., 2., 5), v(-1., 1., 2., 6)]);
        r.words(INDEX, &[0, 1, 2]);
        let list = [
            cmd(CMD_VERTICES, 3),
            VERTS,
            cmd(CMD_LINES, 1),
            INDEX,
            cmd(CMD_POINTS, 1),
            INDEX + 4,
        ];
        run(&mut r, 0, &list);
        assert!((160..=240).all(|x| r.px(x, 120) == 5));
        assert_eq!((r.px(159, 120), r.px(241, 120), r.px(80, 40)), (0, 0, 6));
        // A viewport of 20 x 20 at (10, 10): centre (20, 20), scale 10.
        let mut r = ram();
        let view = [
            cmd(CMD_VIEWPORT, 0),
            10 | 10 << 16,
            20 | 20 << 16,
            fx(1.0),
            fx(10.0),
            cmd(CMD_CLEAR, 4),
            0,
        ];
        let slots = run(&mut r, 0, &view);
        assert_eq!(
            (r.px(10, 10), r.px(29, 29), r.px(9, 10), r.px(30, 29)),
            (4, 4, 0, 0)
        );
        assert_eq!(slots, 400 + 3 * 2, "a slot a pixel, two a command");
        r.verts(&[v(0., 0., 2., 5), v(1., 0., 2., 5)]);
        r.words(INDEX, &[0, 1]);
        let list = [
            &view[..5],
            &[cmd(CMD_VERTICES, 2), VERTS, cmd(CMD_LINES, 1), INDEX][..],
        ]
        .concat();
        run(&mut r, 0, &list);
        assert!((20..=25).all(|x| r.px(x, 20) == 5));
        assert_eq!(r.px(26, 20), 4);
    }

    #[test]
    fn light_and_the_shade_table() {
        // shade[I][c] = I / 16 + c: the intensity shows in the index.
        let mut r = ram();
        for i in 0..256u32 {
            for c in 0..256u32 {
                r.0[(TABLE + 256 * i + c) as usize] = (i / 16 + c) as u8;
            }
        }
        // The left triangle faces the light, the right one is turned 60
        // degrees away.
        let mut vs = vec![v(-2., 0., 2., 0), v(-1., 0., 2., 0), v(-2., -1., 2., 0)];
        vs.extend([v(1., 0., 2., 0), v(2., 0., 2., 0), v(1., -1., 2., 0)]);
        for x in &mut vs[3..] {
            x.n = [0.75f64.sqrt(), 0.0, -0.5];
        }
        r.verts(&vs);
        r.words(INDEX, &[0, 1, 2, 3, 4, 5]);
        let list = [
            cmd(CMD_TABLES, 0),
            0,
            TABLE,
            cmd(CMD_LIGHT, 16 | 160 << 8),
            0,
            0,
            fx(1.0),
            cmd(CMD_STATE, ST_LIGHT),
            cmd(CMD_VERTICES, 6),
            VERTS,
            cmd(CMD_TRIANGLES, 2),
            INDEX,
        ];
        run(&mut r, 0, &list);
        assert_eq!(r.px(5, 125), 176 / 16, "ambient 16 + diffuse 160");
        assert_eq!(r.px(255, 125), 96 / 16, "16 + 160 cos 60");
        // 16 bits: RGB scaled by I / 255.
        let mut r = ram();
        r.verts(&[
            v(-2., 0., 2., 0xffff),
            v(-1., 0., 2., 0xffff),
            v(-2., -1., 2., 0xffff),
        ]);
        r.words(INDEX, &[0, 1, 2]);
        let list = [
            cmd(CMD_LIGHT, 128 << 8),
            0,
            0,
            fx(1.0),
            cmd(CMD_STATE, ST_LIGHT),
            cmd(CMD_VERTICES, 3),
            VERTS,
            cmd(CMD_TRIANGLES, 1),
            INDEX,
        ];
        run(&mut r, 1, &list);
        assert_eq!(r.px16(5, 125), (15 << 11) | (31 << 5) | 15);
    }

    #[test]
    fn errors_and_the_savestate() {
        let mut r = ram();
        r.dwords(LIST, &[cmd(0x7f, 0)]);
        let mut s = Stereoscope::default();
        s.write(S3DLOFS, LIST as u16);
        s.write(S3DSTART, 1);
        assert!(s.take_start());
        assert!(!s.run(&mut r), "unknown command");
        assert_eq!(s.peek(S3DSTAT) & 5, 5, "busy and error");
        // A START while busy is ignored.
        s.write(S3DSTART, 1);
        assert!(!s.take_start());
        let mut w = StateWriter::new();
        s.save(&mut w);
        let bytes = w.into_bytes();
        let mut t = Stereoscope::default();
        t.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(s, t);
        // A list running past chip RAM, and one jumping in a circle.
        let mut s = Stereoscope::default();
        s.write(S3DLSEG, 0x3f);
        s.write(S3DLOFS, 0xfffe);
        s.write(S3DSTART, 1);
        s.take_start();
        assert!(!s.run(&mut r));
        r.dwords(LIST, &[cmd(CMD_JUMP, 0), LIST]);
        let mut s = Stereoscope::default();
        s.write(S3DLOFS, LIST as u16);
        s.write(S3DSTART, 1);
        s.take_start();
        assert!(!s.run(&mut r), "runaway list");
    }
}
