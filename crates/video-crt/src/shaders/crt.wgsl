// CRT stage, GPU path. Same model as the CPU path (src/cpu.rs): linear
// horizontal resampling, scanlines whose beam width follows brightness,
// output gamma and tint. Drawn as one full-screen triangle into a target
// sized from the signal metadata (not the window).

struct Params {
    // Source frame size in pixels.
    src_size: vec2<f32>,
    // Output size in pixels.
    out_size: vec2<f32>,
    tint: vec3<f32>,
    scanlines: f32,
    gamma: f32,
    _pad: f32,
    // Top-left corner of the output rectangle in the render target.
    origin: vec2<f32>,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;

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
    let max_x = i32(params.src_size.x) - 1;
    return textureLoad(source, vec2<i32>(clamp(x, 0, max_x), y), 0).rgb;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let out_px = in.position.xy - params.origin;
    let scale_y = params.out_size.y / params.src_size.y;
    let sy = i32(floor(out_px.y / scale_y));

    // Centre-aligned linear resampling along the line.
    let pos = max(out_px.x * params.src_size.x / params.out_size.x - 0.5, 0.0);
    let x0 = i32(floor(pos));
    let t = pos - floor(pos);
    let c = mix(fetch(x0, sy), fetch(x0 + 1, sy), t);

    // 0 at the beam centre, 1 in the gap between lines.
    var offset = 0.0;
    if (scale_y > 1.0) {
        offset = abs(fract(out_px.y / scale_y) * 2.0 - 1.0);
    }
    let luma = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    let beam = 0.5 + 0.5 * luma;
    let falloff = min(offset / max(beam, 0.01), 1.0);
    let k = 1.0 - params.scanlines * falloff * falloff;
    let lit = clamp(c * k * params.tint, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(pow(lit, vec3<f32>(1.0 / max(params.gamma, 0.01))), 1.0);
}
