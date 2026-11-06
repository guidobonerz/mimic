//! The brass zeppelin of Foundry's 3D scene (step 7), generated at build
//! time: a Stereoscope display list with its mesh (hull of revolution,
//! four tail fins, a gondola; 255 vertices, 436 triangles) and a 64 × 64
//! RGB 5:6:5 brass texture, laid out as one block for chip RAM, and 256
//! matrices for one turn. Formats: SPEC.md 7.
//!
//! Two assembler includes come out: `zeppelin.inc` (constants, included
//! at the top) and `zeppelin_data.inc` (the labels `zep_blob`, the block
//! copied to `ZEP_SEG:0000`, and `zep_mats`, 64 bytes a frame).

use std::f64::consts::PI;
use std::fmt::Write;

/// Chip RAM segment of the block.
pub const SEG: u32 = 0x000e;
const VERTS: u32 = 0x0100;
const INDEX: u32 = 0x1900;
const TEX: u32 = 0x2400;
const END: usize = 0x4400;
/// Frames of one turn.
const FRAMES: usize = 256;
/// Sky colour, RGB 5:6:5 (dark teal).
const SKY: u32 = 0x1146;

struct Vert {
    p: [f64; 3],
    n: [f64; 3],
    uv: [f64; 2],
}

fn fx(v: f64) -> u32 {
    (v * 65536.0).round() as i32 as u32
}

fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    v.map(|c| c / l)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Hull radius at s (-1 tail … 1 nose): an ellipsoid drawn in at the tail.
fn radius(s: f64) -> f64 {
    0.62 * (1.0 - s * s).max(0.0).sqrt() * (1.0 + 0.25 * s.min(0.0))
}

struct Mesh {
    verts: Vec<Vert>,
    tris: Vec<[u16; 3]>,
}

impl Mesh {
    fn add(&mut self, p: [f64; 3], n: [f64; 3], uv: [f64; 2]) -> u16 {
        self.verts.push(Vert { p, n: norm(n), uv });
        (self.verts.len() - 1) as u16
    }

    /// A triangle facing `out`: its edges' cross product points outwards,
    /// which the Stereoscope sees as clockwise on the screen (front).
    fn tri(&mut self, a: u16, b: u16, c: u16, out: [f64; 3]) {
        let p = |i: u16| self.verts[usize::from(i)].p;
        let n = cross(sub(p(b), p(a)), sub(p(c), p(a)));
        self.tris.push(if dot(n, out) >= 0.0 {
            [a, b, c]
        } else {
            [a, c, b]
        });
    }
}

fn mesh() -> Mesh {
    let mut m = Mesh {
        verts: Vec::new(),
        tris: Vec::new(),
    };
    const RINGS: usize = 13;
    const SEGS: usize = 16;
    // Hull: rings with a duplicated seam column for the texture.
    let mut ring = Vec::new();
    for i in 0..RINGS {
        let s = -1.0 + 2.0 * (i + 1) as f64 / (RINGS + 1) as f64;
        let (r, h) = (radius(s), 1e-4);
        let slope = (radius(s + h) - radius(s - h)) / (2.0 * h) / 2.0; // dr/dx, x = 2s
        let mut row = Vec::new();
        for j in 0..=SEGS {
            let a = 2.0 * PI * j as f64 / SEGS as f64;
            let (c, sn) = (a.cos(), a.sin());
            row.push(m.add(
                [2.0 * s, r * c, r * sn],
                [-slope, c, sn],
                [(s + 1.0) / 2.0 * 128.0, 4.0 * j as f64],
            ));
        }
        ring.push(row);
    }
    let tail = m.add([-2.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 32.0]);
    let nose = m.add([2.0, 0.0, 0.0], [1.0, 0.0, 0.0], [128.0, 32.0]);
    let centre = |m: &Mesh, ids: [u16; 3]| {
        let mut c = [0.0; 3];
        for i in ids {
            for (s, p) in c.iter_mut().zip(m.verts[usize::from(i)].p) {
                *s += p / 3.0;
            }
        }
        c
    };
    for i in 0..RINGS - 1 {
        for j in 0..SEGS {
            let (a, b, c, d) = (
                ring[i][j],
                ring[i + 1][j],
                ring[i + 1][j + 1],
                ring[i][j + 1],
            );
            for t in [[a, b, c], [a, c, d]] {
                let o = centre(&m, t);
                m.tri(t[0], t[1], t[2], [0.0, o[1], o[2]]);
            }
        }
    }
    for j in 0..SEGS {
        let o = centre(&m, [tail, ring[0][j], ring[0][j + 1]]);
        m.tri(tail, ring[0][j], ring[0][j + 1], [-1.0, o[1], o[2]]);
        let l = RINGS - 1;
        let o = centre(&m, [nose, ring[l][j], ring[l][j + 1]]);
        m.tri(nose, ring[l][j], ring[l][j + 1], [1.0, o[1], o[2]]);
    }
    // Four fins at the tail, each with a face to either side.
    for f in 0..4 {
        let a = PI / 4.0 + PI / 2.0 * f as f64;
        let (e, side) = ([0.0, a.cos(), a.sin()], [0.0, -a.sin(), a.cos()]);
        let at = |x: f64, r: f64| [x, r * e[1], r * e[2]];
        let pts = [(-1.15, 0.40), (-1.85, 0.14), (-2.1, 0.82)];
        for sgn in [1.0, -1.0] {
            let n = side.map(|c| c * sgn);
            let ids: Vec<u16> = pts
                .iter()
                .map(|&(x, r)| m.add(at(x, r), n, [(x + 2.2) * 24.0, 40.0 + r * 20.0]))
                .collect();
            m.tri(ids[0], ids[1], ids[2], n);
        }
    }
    // The gondola: a box under the hull, normals through its corners.
    let (lo, hi) = ([-0.35, -0.80, -0.14], [0.45, -0.50, 0.14]);
    let mid = [0.05, -0.65, 0.0];
    let mut corner = [0u16; 8];
    for (k, c) in corner.iter_mut().enumerate() {
        let p = [0, 1, 2].map(|i| if k >> i & 1 != 0 { hi[i] } else { lo[i] });
        let n = [0, 1, 2].map(|i| (p[i] - mid[i]) / (hi[i] - lo[i]));
        *c = m.add(p, n, [(p[0] + 0.4) * 40.0, (p[1] + 0.8) * 40.0 + 20.0]);
    }
    for axis in 0..3 {
        for side in 0..2 {
            let mut out = [0.0; 3];
            out[axis] = if side == 1 { 1.0 } else { -1.0 };
            // The four corners with bit `axis` = side, in a cycle.
            let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
            let id = |bu: usize, bw: usize| corner[side << axis | bu << u | bw << w];
            let q = [id(0, 0), id(1, 0), id(1, 1), id(0, 1)];
            m.tri(q[0], q[1], q[2], out);
            m.tri(q[0], q[2], q[3], out);
        }
    }
    m
}

/// 64 × 64 brass plates: seams, rivets, a jacquard band, plates of
/// slightly different shades.
fn texture() -> Vec<u16> {
    let hash = |a: u32, b: u32| {
        let mut h = a.wrapping_mul(0x9e37_79b9) ^ b.wrapping_mul(0x85eb_ca6b);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        f64::from(h & 0xffff) / 65535.0
    };
    let mut t = Vec::with_capacity(64 * 64);
    for v in 0..64u32 {
        for u in 0..64u32 {
            let mut c = if (40..44).contains(&u) {
                [184.0, 96.0, 52.0]
            } else {
                [196.0, 156.0, 72.0]
            };
            let mut k = 0.86 + 0.22 * hash(u / 16, v / 8);
            k *= 0.95 + 0.1 * hash(u, v);
            if u % 16 == 0 || v % 8 == 0 {
                k *= 0.55;
            } else if u % 4 == 2 && (v % 8 == 1 || v % 8 == 7) {
                k *= 1.3;
            }
            for x in &mut c {
                *x = (*x * k).min(255.0);
            }
            let [r, g, b] = c.map(|x| x as u16);
            t.push((r >> 3) << 11 | (g >> 2) << 5 | b >> 3);
        }
    }
    t
}

/// Frame k: a turn about y, tilted towards the camera, bobbing.
fn matrix(k: usize) -> [u32; 16] {
    let yaw = 2.0 * PI * k as f64 / FRAMES as f64;
    let tilt: f64 = -0.32;
    let (cy, sy, ct, st) = (yaw.cos(), yaw.sin(), tilt.cos(), tilt.sin());
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rx = [[1.0, 0.0, 0.0], [0.0, ct, -st], [0.0, st, ct]];
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| rx[i][k] * ry[k][j]).sum();
        }
    }
    let t = [0.0, 0.1 + 0.1 * (2.0 * yaw).sin(), 4.6];
    let mut m = [0u32; 16];
    for i in 0..3 {
        for j in 0..3 {
            m[4 * i + j] = fx(r[i][j]);
        }
        m[4 * i + 3] = fx(t[i]);
    }
    m[15] = fx(1.0);
    m
}

fn cmd(op: u32, arg: u32) -> u32 {
    op << 24 | arg
}

/// The block for chip RAM and the offset of the list's matrix.
fn block(m: &Mesh) -> (Vec<u8>, usize) {
    let base = SEG << 16;
    let mut list = vec![
        cmd(0x07, 0x0c), // STATE: Z test and write
        cmd(0x0b, 0),    // VIEWPORT
        0,
        320 | 240 << 16,
        fx(1.0),
        fx(280.0),
        cmd(0x0a, SKY), // CLEAR
        0xffff,
        cmd(0x08, 70 | 210 << 8), // LIGHT: from the upper left
        fx(0.5),
        fx(-0.6),
        fx(0.6),
        cmd(0x06, 6 | 6 << 4 | 2 << 8), // TEXTURE 64 x 64, 16 bits
        base + TEX,
        cmd(0x07, 0x3f), // STATE: Gouraud, texture, Z, cull, light
        cmd(0x01, 0),    // MATRIX
    ];
    let mat = 4 * list.len();
    list.extend(matrix(0));
    list.extend([
        cmd(0x02, m.verts.len() as u32),
        base + VERTS,
        cmd(0x03, m.tris.len() as u32),
        base + INDEX,
        cmd(0x00, 0),
    ]);
    let mut b = vec![0u8; END];
    for (k, d) in list.iter().enumerate() {
        b[4 * k..4 * k + 4].copy_from_slice(&d.to_le_bytes());
    }
    assert!(4 * list.len() <= VERTS as usize);
    for (k, v) in m.verts.iter().enumerate() {
        let mut at = VERTS as usize + 24 * k;
        for c in v.p {
            b[at..at + 4].copy_from_slice(&fx(c).to_le_bytes());
            at += 4;
        }
        let words = [
            v.n[0] * 32767.0,
            v.n[1] * 32767.0,
            v.n[2] * 32767.0,
            v.uv[0] * 16.0,
            v.uv[1] * 16.0,
        ]
        .map(|x| x.round() as i16 as u16);
        for w in words.into_iter().chain([0xffff]) {
            b[at..at + 2].copy_from_slice(&w.to_le_bytes());
            at += 2;
        }
    }
    assert!(VERTS as usize + 24 * m.verts.len() <= INDEX as usize);
    for (k, i) in m.tris.iter().flatten().enumerate() {
        let at = INDEX as usize + 2 * k;
        b[at..at + 2].copy_from_slice(&i.to_le_bytes());
    }
    assert!(INDEX as usize + 6 * m.tris.len() <= TEX as usize);
    for (k, t) in texture().iter().enumerate() {
        let at = TEX as usize + 2 * k;
        b[at..at + 2].copy_from_slice(&t.to_le_bytes());
    }
    (b, mat)
}

fn words(out: &mut String, bytes: &[u8]) {
    for line in bytes.chunks(32) {
        let w: Vec<String> = line
            .chunks(2)
            .map(|p| format!("${:04x}", u16::from_le_bytes([p[0], p[1]])))
            .collect();
        writeln!(out, "        .word   {}", w.join(",")).unwrap();
    }
}

/// (`zeppelin.inc`, `zeppelin_data.inc`).
pub fn includes() -> (String, String) {
    let m = mesh();
    assert!(m.verts.len() <= 256, "{} vertices", m.verts.len());
    let (b, mat) = block(&m);
    let consts = format!(
        "; generated by build.rs (zeppelin.rs): {} vertices, {} triangles\n\
         ZEP_SEG  = ${SEG:04x}\nZEP_WORDS = {}\nZEP_MAT  = ${mat:04x}\nZEP_FRAMES = {FRAMES}\n",
        m.verts.len(),
        m.tris.len(),
        END / 2
    );
    let mut data =
        String::from("; generated by build.rs (zeppelin.rs)\n        .align  2\nzep_blob:\n");
    words(&mut data, &b);
    data.push_str("zep_mats:\n");
    for k in 0..FRAMES {
        let bytes: Vec<u8> = matrix(k).iter().flat_map(|d| d.to_le_bytes()).collect();
        words(&mut data, &bytes);
    }
    (consts, data)
}
