// CRT stage, GPU path. Same model as the CPU path (src/cpu.rs, step by
// step): curvature, linear horizontal resampling, scanlines whose beam
// width follows brightness, bloom from a fixed 7-tap glow, tint, phosphor
// mask in screen pixels, vignette, output gamma, and persistence against
// the previous output. Drawn as one full-screen triangle into a target
// sized from the signal metadata (not the window).
//
// Cost per pixel: 2 texel loads for the beam, 14 for the glow (only with
// bloom), 1 for the previous output (only with persistence); no loops.

struct Params {
    // Source frame size in pixels.
    src_size: vec2<f32>,
    // Output size in pixels.
    out_size: vec2<f32>,
    tint: vec3<f32>,
    scanlines: f32,
    gamma: f32,
    // Fraction of the previous output kept (0 = `previous` unused).
    persistence: f32,
    // Top-left corner of the output rectangle in the render target.
    origin: vec2<f32>,
    // 0 none, 1 shadow, 2 aperture, 3 slot.
    mask_kind: f32,
    mask_strength: f32,
    mask_pitch: f32,
    bloom: f32,
    curvature: f32,
    vignette: f32,
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
// The previous output (same size as the output rectangle, at 0,0).
@group(0) @binding(2) var previous: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    // Full-screen triangle.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VertexOut;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn fetch(x: i32, y: i32) -> vec3<f32> {
    let m = vec2<i32>(params.src_size) - vec2<i32>(1, 1);
    return textureLoad(source, clamp(vec2<i32>(x, y), vec2<i32>(0, 0), m), 0).rgb;
}

// Glow around a source pixel: centre 0.2, ±1 0.15, ±2 0.1, lines above
// and below 0.15 (GLOW in cpu.rs).
fn glow(x: i32, y: i32) -> vec3<f32> {
    return 0.2 * fetch(x, y)
        + 0.15 * (fetch(x - 1, y) + fetch(x + 1, y))
        + 0.1 * (fetch(x - 2, y) + fetch(x + 2, y))
        + 0.15 * (fetch(x, y - 1) + fetch(x, y + 1));
}

// Per-channel mask factors (gain included) of the screen pixel centred at p.
fn mask(p: vec2<f32>) -> vec3<f32> {
    let s = clamp(params.mask_strength, 0.0, 1.0);
    let kind = i32(params.mask_kind + 0.5);
    if (kind == 0 || s == 0.0) {
        return vec3<f32>(1.0);
    }
    let pitch = max(params.mask_pitch, 1.0);
    var shift = 0.0;
    var gap = 1.0;
    if (kind == 1) {
        let row = floor(p.y / (pitch * 0.5));
        if (row - 2.0 * floor(row * 0.5) >= 1.0) {
            shift = 0.5;
        }
    } else if (kind == 3) {
        let h = pitch * 2.0;
        let row = floor(p.y / h);
        if (row - 2.0 * floor(row * 0.5) >= 1.0) {
            shift = 0.5;
        }
        if (p.y / h - row > 0.875) {
            gap = 1.0 - s;
        }
    }
    let u = p.x / pitch + shift;
    let t = u - floor(u);
    let centre = (vec3<f32>(0.0, 1.0, 2.0) + 0.5) / 3.0;
    let d0 = abs(vec3<f32>(t) - centre);
    let d = min(d0, vec3<f32>(1.0) - d0) * 3.0;
    let w = clamp(vec3<f32>(1.0) - d, vec3<f32>(0.0), vec3<f32>(1.0));
    let m = vec3<f32>(1.0) - s * (vec3<f32>(1.0) - w);
    return m * (gap / (1.0 - s / 3.0));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let screen = in.position.xy - params.origin;
    var p = screen;
    var c = p * 2.0 / params.out_size - 1.0;
    var lit = vec3<f32>(0.0);
    var visible = true;
    let k = clamp(params.curvature, 0.0, 0.25);
    if (k > 0.0) {
        c = c * ((1.0 + k * dot(c, c)) / (1.0 + k));
        if (abs(c.x) > 1.0 || abs(c.y) > 1.0) {
            visible = false;
        }
        p = (c + 1.0) * 0.5 * params.out_size;
    }
    if (visible) {
        let scale_y = params.out_size.y / params.src_size.y;
        let ly = p.y / scale_y;
        let sy = i32(floor(ly));

        // Centre-aligned linear resampling along the line.
        let pos = max(p.x * params.src_size.x / params.out_size.x - 0.5, 0.0);
        let x0 = i32(floor(pos));
        let t = pos - floor(pos);
        let col = mix(fetch(x0, sy), fetch(x0 + 1, sy), t);

        // 0 at the beam centre, 1 in the gap between lines.
        var offset = 0.0;
        if (scale_y > 1.0) {
            offset = abs(fract(ly) * 2.0 - 1.0);
        }
        let luma = dot(col, vec3<f32>(0.299, 0.587, 0.114));
        let beam = 0.5 + 0.5 * luma;
        let falloff = min(offset / max(beam, 0.01), 1.0);
        lit = col * (1.0 - params.scanlines * falloff * falloff);
        let bloom = clamp(params.bloom, 0.0, 1.0);
        if (bloom > 0.0) {
            let g = mix(glow(x0, sy), glow(x0 + 1, sy), t);
            lit = lit + bloom * g * g;
        }
        lit = lit * params.tint * mask(screen);
        let vignette = clamp(params.vignette, 0.0, 1.0);
        if (vignette > 0.0) {
            lit = lit * max(1.0 - vignette * 0.5 * dot(c, c), 0.0);
        }
    }
    var out = pow(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / max(params.gamma, 0.01)));
    let keep = clamp(params.persistence, 0.0, 1.0);
    if (keep > 0.0) {
        let prev = textureLoad(previous, vec2<i32>(floor(screen)), 0).rgb;
        out = max(out, prev * keep);
    }
    return vec4<f32>(out, 1.0);
}
