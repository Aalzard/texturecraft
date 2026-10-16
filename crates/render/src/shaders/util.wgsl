// Fullscreen helpers: mip generation and blitting the viewport into the UI pass.

struct FsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FsOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0));
    var o: FsOut;
    o.pos = vec4<f32>(p[vi], 0.0, 1.0);
    o.uv = vec2<f32>(p[vi].x * 0.5 + 0.5, 0.5 - p[vi].y * 0.5);
    return o;
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;

@fragment
fn fs_mip(i: FsOut) -> @location(0) vec4<f32> {
    return textureSample(src, src_samp, i.uv);
}

// A same-size copy: the texel under the pixel. Exact on every adapter, where a filtered read at
// the interpolated UV can blend neighbours when the UV lands a hair off the texel center.
@fragment
fn fs_copy(i: FsOut) -> @location(0) vec4<f32> {
    let d = vec2<i32>(textureDimensions(src));
    return textureLoad(src, clamp(vec2<i32>(floor(i.pos.xy)), vec2<i32>(0), d - 1), 0);
}

struct Blit {
    rect: vec4<f32>,   // destination x, y, w, h in target pixels
    dst: vec4<f32>,    // target w, h, decode sRGB (1 when the target format is sRGB), -
};

@group(0) @binding(2) var<uniform> blit: Blit;

struct BlitOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) vi: u32) -> BlitOut {
    var c = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let t = c[vi];
    let px = blit.rect.xy + t * blit.rect.zw;
    var o: BlitOut;
    o.pos = vec4<f32>(px.x / blit.dst.x * 2.0 - 1.0, 1.0 - px.y / blit.dst.y * 2.0, 0.0, 1.0);
    o.uv = t;
    return o;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_blit(i: BlitOut) -> @location(0) vec4<f32> {
    let c = textureSample(src, src_samp, i.uv);
    if (blit.dst.z > 0.5) {
        return vec4<f32>(to_linear(c.rgb), 1.0);
    }
    return vec4<f32>(c.rgb, 1.0);
}
