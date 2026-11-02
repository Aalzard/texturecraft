// Scene effects between the scene pass and the post pass, appended to util.wgsl and post.wgsl
// (uses `src`, `src_samp`, `post`, `fx_a`): the screen-space subsurface blur and depth of field.
// Both read the G-buffer of `fs_gbuf` (mesh.wgsl): rgb the scattering part of the diffuse light,
// a the view depth (0: background). Colors are linear HDR premultiplied by scene coverage.

@group(0) @binding(5) var fx_b: texture_2d<f32>;
@group(0) @binding(6) var fx_c: texture_2d<f32>;

// The texel of `t` under `uv` (no filtering: depths must not blend across silhouettes).
fn texel(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(vec2<i32>(uv * vec2<f32>(d)), vec2<i32>(0), d - 1), 0);
}

// The texel at pixel `p` of a target the same size as the one being drawn (`p` is the fragment
// position): exact on every adapter, where a filtered read at the interpolated UV can blend
// neighbours wherever the UV lands a hair off the texel center.
fn texel_at(t: texture_2d<f32>, p: vec2<f32>) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(vec2<i32>(floor(p)), vec2<i32>(0), d - 1), 0);
}

// ---- Subsurface scattering: a separable bilateral blur (Jimenez et al., "Separable Subsurface
// Scattering", 2015) of the scattering diffuse light, a Gaussian per color channel as wide as
// its mean free path. A tap across a depth discontinuity (or off the mesh) takes the centre's
// light instead, so no tap drops out of the sum as it crosses a silhouette.

const SSS_TAPS: i32 = 8;

fn px_per_world(z: f32) -> f32 {
    return select(post.d.w / max(z, 1e-4), post.d.w, post.e.x > 0.5);
}

// One direction of the blur over `src` (rgb light, a depth) at pixel `p` (same-size target).
fn sss_blur(p: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    let c = texel_at(src, p);
    let sigma = post.d.xyz * px_per_world(c.a);
    let smax = max(sigma.r, max(sigma.g, sigma.b));
    if (c.a <= 0.0 || smax < 0.5) {
        return c;
    }
    let stride = dir * (smax * 3.0 / f32(SSS_TAPS));
    let depth_range = 3.0 * max(post.d.x, max(post.d.y, post.d.z));
    let inv = 1.0 / (2.0 * sigma * sigma + vec3<f32>(1e-4));
    // Wide blurs space their taps pixels apart: shifting them per pixel (interleaved gradient
    // noise) and per accumulated frame turns what would show as copies of edges into fine grain.
    let ign = fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
    let jitter = fract(ign + post.e.y * 0.618034) - 0.5;
    var sum = c.rgb;
    var wsum = vec3<f32>(1.0);
    for (var k = -SSS_TAPS; k <= SSS_TAPS; k++) {
        if (k == 0) {
            continue;
        }
        let o = f32(k) + jitter;
        let t = texel_at(src, p + stride * o);
        let x = o * smax * 3.0 / f32(SSS_TAPS);
        let wd = select(0.0, max(1.0 - abs(t.a - c.a) / depth_range, 0.0), t.a > 0.0);
        let w = exp(-x * x * inv);
        sum += mix(c.rgb, t.rgb, wd) * w;
        wsum += w;
    }
    return vec4<f32>(sum / wsum, c.a);
}

// src: the G-buffer.
@fragment
fn fs_sss_h(i: FsOut) -> @location(0) vec4<f32> {
    return sss_blur(i.pos.xy, vec2<f32>(1.0, 0.0));
}

// src: the horizontal pass, fx_a: the scene (without its scattering light, see `fs_mesh`). Adds
// the blurred scattering light.
@fragment
fn fs_sss_v(i: FsOut) -> @location(0) vec4<f32> {
    let blurred = sss_blur(i.pos.xy, vec2<f32>(0.0, 1.0)).rgb;
    let scene = texel_at(fx_a, i.pos.xy);
    return scene + vec4<f32>(blurred * clamp(scene.a, 0.0, 1.0), 0.0);
}

// ---- Depth of field: a thin lens. Scatter-as-gather over a golden-angle spiral at half
// resolution (after D. Gustafsson, "Bokeh depth of field in a single pass", 2018): a tap counts
// where its own circle of confusion reaches the pixel; taps behind the pixel spread at most twice
// its own blur (no background bleeding over a sharp foreground), taps in front spread fully (a
// blurred foreground covers the sharp background). The full-resolution composite keeps the
// sharp scene where the pixel is in focus and nothing blurred covers it.

const DOF_TAPS: i32 = 48;
const GOLDEN_ANGLE: f32 = 2.39996323;

// Signed circle of confusion radius in full-resolution pixels (negative in front of the focus).
fn coc(z: f32) -> f32 {
    var c = post.c.x;
    if (z > 0.0) {
        c = post.c.x * (z - post.c.y) / z;
    }
    return clamp(c, -post.c.z, post.c.z);
}

struct DofOut {
    @location(0) color: vec4<f32>,
    // How much blurred foreground covers the pixel (r).
    @location(1) near: vec4<f32>,
};

// src: the scene (full resolution), fx_a: the G-buffer.
@fragment
fn fs_dof_gather(i: FsOut) -> DofOut {
    let px = 1.0 / vec2<f32>(textureDimensions(src));
    let zc = texel(fx_a, i.uv).a;
    let cc = abs(coc(zc));
    let rmax = post.c.z;
    // Half the spacing of the taps: the soft edge of each tap's reach.
    let h = max(rmax * sqrt(PI_FX / f32(DOF_TAPS)) * 0.5, 0.5);
    var acc = textureSampleLevel(src, src_samp, i.uv, 0.0);
    var tot = 1.0;
    var near = 0.0;
    // The spiral turns per pixel (interleaved gradient noise) and per accumulated frame, so the
    // taps' pattern averages into smooth discs instead of showing as dots in bright highlights.
    let ign = fract(52.9829189 * fract(dot(i.pos.xy, vec2<f32>(0.06711056, 0.00583715))));
    let turn = (ign + post.c.w * 0.618034) * 6.2831853;
    for (var k = 0; k < DOF_TAPS; k++) {
        let r = rmax * sqrt((f32(k) + 0.5) / f32(DOF_TAPS));
        let a = f32(k) * GOLDEN_ANGLE + turn;
        let uv = i.uv + vec2<f32>(cos(a), sin(a)) * (r * px);
        let z = texel(fx_a, uv).a;
        let nearer = z > 0.0 && (zc <= 0.0 || z < zc);
        var size = abs(coc(z));
        if (!nearer) {
            size = min(size, cc * 2.0);
        }
        let m = smoothstep(r - h, r + h, size);
        acc += mix(acc / tot, textureSampleLevel(src, src_samp, uv, 0.0), m);
        tot += 1.0;
        if (nearer) {
            near = max(near, m * smoothstep(0.5, 1.5, size));
        }
    }
    return DofOut(acc / tot, vec4<f32>(near));
}

const PI_FX: f32 = 3.14159265;

// src: the sharp scene, fx_a: the gathered blur, fx_b: the G-buffer, fx_c: the near coverage.
@fragment
fn fs_dof(i: FsOut) -> @location(0) vec4<f32> {
    let sharp = texel_at(src, i.pos.xy);
    let f = max(smoothstep(0.5, 1.5, abs(coc(texel_at(fx_b, i.pos.xy).a))), textureSampleLevel(fx_c, src_samp, i.uv, 0.0).r);
    // In focus with nothing blurred in front: the sharp pixel itself (no blend through the
    // half-resolution blur at all).
    if (f <= 0.0) {
        return sharp;
    }
    return mix(sharp, textureSampleLevel(fx_a, src_samp, i.uv, 0.0), f);
}
