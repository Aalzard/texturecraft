// TextureCraft viewport shaders: metal/roughness (or specular/glossiness) PBR with image-based
// lighting and optional parallax occlusion mapping, the 2D UV view,
// the background, debug views and the 3D wireframe. Output is scene-linear HDR, premultiplied by
// alpha = scene coverage over the flat studio backdrop; exposure, tone mapping and sRGB encoding
// happen in the post pass (post.wgsl). WebGPU/WebGL2-safe (fragment passes only).

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>, // background rays: a perspective camera even when the view is orthographic
    eye: vec4<f32>,      // xyz eye position, w -
    env: vec4<f32>,      // x rotation (radians), y max lod, z AO intensity, w -
    sh: array<vec4<f32>, 9>,
    mode: vec4<f32>,     // x view mode (0 material, 1 channel, 2 mesh map, 3 UV checker), y channel index, z height strength, w normal Y sign
    bg: vec4<f32>,       // x environment opacity, y background blur lod, z UV view (1) or 3D (0), w wireframe opacity
    uv_xform: vec4<f32>, // UV view: scale xy, offset zw (clip space)
    light: vec4<f32>,    // xyz world direction to the key light (rotated with the environment), w intensity (0 = off)
    light_vp: mat4x4<f32>, // world -> shadow map clip space
    shadow: vec4<f32>,   // x shadows on (1), y shadow map texel (uv), z normal offset (world units), w -
    material: vec4<f32>, // x specular/glossiness shading (1), y parallax occlusion (1), z accumulation sample index, w -
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var env_tex: texture_2d<f32>;
@group(0) @binding(2) var env_samp: sampler;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_samp: sampler_comparison;

@group(1) @binding(0) var t_base: texture_2d<f32>;
@group(1) @binding(1) var t_orm: texture_2d<f32>;
@group(1) @binding(2) var t_nrm: texture_2d<f32>;
@group(1) @binding(3) var t_height: texture_2d<f32>;
@group(1) @binding(4) var t_emis: texture_2d<f32>;
@group(1) @binding(5) var t_aux: texture_2d<f32>;
@group(1) @binding(6) var samp: sampler;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) tan: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) tan: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_mesh(v: VIn) -> VOut {
    var o: VOut;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.wpos = v.pos;
    o.nrm = v.nrm;
    o.tan = v.tan;
    o.uv = v.uv;
    return o;
}

@vertex
fn vs_uv(v: VIn) -> VOut {
    var o: VOut;
    let p = vec2<f32>(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0) * g.uv_xform.xy + g.uv_xform.zw;
    o.clip = vec4<f32>(p, 0.5, 1.0);
    o.wpos = v.pos;
    o.nrm = v.nrm;
    o.tan = v.tan;
    o.uv = v.uv;
    return o;
}

const PI: f32 = 3.14159265;

fn env_uv(d: vec3<f32>) -> vec2<f32> {
    let c = cos(g.env.x);
    let s = sin(g.env.x);
    let r = vec3<f32>(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
    let phi = atan2(r.z, r.x);
    return vec2<f32>(fract(phi / (2.0 * PI) + 1.0), acos(clamp(r.y, -1.0, 1.0)) / PI);
}

fn env_rotate(d: vec3<f32>) -> vec3<f32> {
    let c = cos(g.env.x);
    let s = sin(g.env.x);
    return vec3<f32>(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
}

fn sh_irradiance(n0: vec3<f32>) -> vec3<f32> {
    let n = env_rotate(n0);
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var r = g.sh[0].xyz * 0.282095;
    r += g.sh[1].xyz * (0.488603 * y);
    r += g.sh[2].xyz * (0.488603 * z);
    r += g.sh[3].xyz * (0.488603 * x);
    r += g.sh[4].xyz * (1.092548 * x * y);
    r += g.sh[5].xyz * (1.092548 * y * z);
    r += g.sh[6].xyz * (0.315392 * (3.0 * z * z - 1.0));
    r += g.sh[7].xyz * (1.092548 * x * z);
    r += g.sh[8].xyz * (0.546274 * (x * x - y * y));
    return max(r, vec3<f32>(0.0));
}

// Analytic split-sum environment BRDF (Karis, "Physically Based Shading on Mobile").
fn env_brdf(f0: vec3<f32>, rough: f32, nov: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nov)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Data views bypass exposure and tone mapping: linear color, clamped.
fn data_out(c: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

// A value shown as-is on screen (normals, height, masks): undo the post pass's sRGB encoding.
fn raw_out(v: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(to_linear(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}

// Key-light visibility from the shadow map: 3x3 hardware-filtered PCF taps.
fn shadow_factor(wpos: vec3<f32>, n_geo: vec3<f32>) -> f32 {
    let p = g.light_vp * vec4<f32>(wpos + n_geo * g.shadow.z, 1.0);
    let ndc = p.xyz / p.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z >= 1.0) {
        return 1.0;
    }
    let depth = ndc.z - 0.0005;
    var s = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            s += textureSampleCompareLevel(shadow_map, shadow_samp, uv + vec2<f32>(f32(x), f32(y)) * g.shadow.y * 1.5, depth);
        }
    }
    return s / 9.0;
}

// Reoriented normal mapping of a detail normal onto a base normal (tangent space, unpacked).
fn rnm(base: vec3<f32>, detail: vec3<f32>) -> vec3<f32> {
    let t = base + vec3<f32>(0.0, 0.0, 1.0);
    let u = detail * vec3<f32>(-1.0, -1.0, 1.0);
    return normalize(t * dot(t, u) - u * t.z);
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

const PARALLAX_STEPS: i32 = 16;

// Parallax occlusion mapping: march the view ray down through the signed height field (the mesh
// surface at height 0; height 1 lies strength/16 UV units above it, the same scale as
// height-to-normal) and return the UV where it first goes below the heights. `v_ts`: tangent-space
// direction to the eye (+y = up the image, so v grows along -y). A fixed step count keeps the
// loop free of data-dependent exits (derivatives stay valid after it).
fn parallax_uv(uv: vec2<f32>, v_ts: vec3<f32>) -> vec2<f32> {
    let dx = dpdx(uv);
    let dy = dpdy(uv);
    // UV shift per unit of height along the ray (grazing angles capped).
    let shift = vec2<f32>(v_ts.x, -v_ts.y) / max(v_ts.z, 0.15) * (g.mode.z / 16.0);
    var prev_z = 1.0;
    var prev_d = 1.0 - textureSampleGrad(t_height, samp, uv + shift, dx, dy).r;
    // Heights at the top of the range already hide the ray at entry.
    var found = prev_d <= 0.0;
    var hit = select(-1.0, 1.0, found);
    for (var k = 1; k <= PARALLAX_STEPS; k++) {
        let z = 1.0 - 2.0 * f32(k) / f32(PARALLAX_STEPS);
        let d = z - textureSampleGrad(t_height, samp, uv + shift * z, dx, dy).r;
        if (!found && d <= 0.0) {
            // Linear refinement between the last step above and the first below the surface.
            hit = mix(prev_z, z, prev_d / max(prev_d - d, 1e-5));
            found = true;
        }
        prev_z = z;
        prev_d = d;
    }
    return uv + shift * hit;
}

@fragment
fn fs_mesh(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n_geo = normalize(i.nrm);
    if (!front) {
        n_geo = -n_geo;
    }
    let t = normalize(i.tan.xyz - n_geo * dot(n_geo, i.tan.xyz));
    let b = cross(n_geo, t) * i.tan.w;
    let v = normalize(g.eye.xyz - i.wpos);
    var uv = i.uv;
    if (g.material.y > 0.5) {
        uv = parallax_uv(uv, vec3<f32>(dot(v, t), dot(v, b), dot(v, n_geo)));
    }
    let base_s = textureSample(t_base, samp, uv);
    let orm = textureSample(t_orm, samp, uv);
    let nrm_s = textureSample(t_nrm, samp, uv).xyz;
    let emis = textureSample(t_emis, samp, uv).rgb;
    let aux = textureSample(t_aux, samp, uv);
    let dims = vec2<f32>(textureDimensions(t_height));
    let ts = 1.0 / dims;
    let hgt = textureSample(t_height, samp, uv);
    let hc = hgt.r;
    let hl = textureSample(t_height, samp, uv - vec2<f32>(ts.x, 0.0)).r;
    let hr = textureSample(t_height, samp, uv + vec2<f32>(ts.x, 0.0)).r;
    let hu = textureSample(t_height, samp, uv - vec2<f32>(0.0, ts.y)).r;
    let hd = textureSample(t_height, samp, uv + vec2<f32>(0.0, ts.y)).r;

    // Tangent-space normal: height gradient (+Y = up the image) combined with the normal channel.
    // Same slope rule as texture export: strength 1 tilts 45° for a height change of 1 over 1/16 of the width.
    let k = g.mode.z * dims.x / 16.0;
    let n_height = normalize(vec3<f32>(-(hr - hl) * 0.5 * k, (hd - hu) * 0.5 * k, 1.0));
    var n_detail = nrm_s * 2.0 - 1.0;
    n_detail.y *= g.mode.w;
    let n_ts = rnm(n_height, normalize(n_detail));
    let n = normalize(t * n_ts.x + b * n_ts.y + n_geo * n_ts.z);

    let mode = i32(g.mode.x + 0.5);
    if (mode == 1) {
        let ch = i32(g.mode.y + 0.5);
        switch ch {
            case 0: { return data_out(base_s.rgb); }
            case 1: { return data_out(vec3<f32>(orm.b)); }
            case 2: { return data_out(vec3<f32>(orm.g)); }
            case 3: { return raw_out(n_ts * 0.5 + 0.5); }
            case 4: { return raw_out(vec3<f32>(hc * 0.5 + 0.5)); }
            case 5: { return data_out(emis); }
            case 6: { return raw_out(vec3<f32>(base_s.a)); }
            case 7: { return raw_out(vec3<f32>(orm.r)); }
            case 9: { return data_out(hgt.gba); }
            case 10: { return data_out(vec3<f32>(1.0 - orm.g)); }
            default: { return raw_out(vec3<f32>(orm.a)); }
        }
    }
    if (mode == 2) {
        return raw_out(aux.rgb);
    }
    if (mode == 3) {
        let c = floor(uv * 32.0);
        let chk = (c.x + c.y) - 2.0 * floor((c.x + c.y) * 0.5);
        let fine = floor(uv * 256.0);
        let f2 = (fine.x + fine.y) - 2.0 * floor((fine.x + fine.y) * 0.5);
        let col = mix(vec3<f32>(0.85, 0.85, 0.85), vec3<f32>(0.18, 0.2, 0.24), chk) * (0.92 + 0.08 * f2);
        let tint = vec3<f32>(fract(uv.x), fract(uv.y), 0.5) * 0.25;
        let diff = max(dot(n, normalize(vec3<f32>(0.4, 0.8, 0.5))), 0.0) * 0.6 + 0.4;
        return data_out((col + tint) * diff);
    }

    // Opacity: dithered alpha test (a new pattern per accumulated frame, so it averages out).
    if (base_s.a < hash12(i.clip.xy + g.material.z * vec2<f32>(17.0, 59.0))) {
        discard;
    }

    let albedo = base_s.rgb;
    let ao = mix(1.0, orm.r, g.env.z);
    let rough = clamp(orm.g, 0.045, 1.0);
    let metal = orm.b;
    let nov = clamp(abs(dot(n, v)), 1e-4, 1.0);
    var f0 = mix(vec3<f32>(0.08 * orm.a), albedo, metal);
    var diffuse_col = albedo * (1.0 - metal);
    if (g.material.x > 0.5) {
        // Specular/glossiness: the Specular color is F0 and base color the diffuse color, with
        // the energy the specular takes removed (roughness already is 1 - glossiness).
        f0 = hgt.gba;
        diffuse_col = albedo * (1.0 - max(f0.r, max(f0.g, f0.b)));
    }

    let diffuse = sh_irradiance(n) * diffuse_col * ao;
    let r = reflect(-v, n);
    let pre = textureSampleLevel(env_tex, env_samp, env_uv(r), rough * g.env.y).rgb;
    // Horizon fading: reflections that point below the geometric surface are dimmed.
    let horizon = clamp(1.0 + 1.3 * dot(r, n_geo), 0.0, 1.0);
    let spec = pre * env_brdf(f0, rough, nov) * ao * horizon * horizon;

    var direct = vec3<f32>(0.0);
    if (g.light.w > 0.0) {
        let l = normalize(g.light.xyz);
        let h = normalize(l + v);
        let nol = max(dot(n, l), 0.0);
        let noh = max(dot(n, h), 0.0);
        let a = rough * rough;
        let a2 = a * a;
        let d = a2 / (PI * pow(noh * noh * (a2 - 1.0) + 1.0, 2.0));
        let vis = 0.5 / max(nol * sqrt(nov * nov * (1.0 - a2) + a2) + nov * sqrt(nol * nol * (1.0 - a2) + a2), 1e-5);
        let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - max(dot(v, h), 0.0), 5.0);
        direct = ((vec3<f32>(1.0) - f) * diffuse_col / PI + d * vis * f) * nol * g.light.w;
        if (g.shadow.x > 0.5 && nol > 0.0) {
            direct *= shadow_factor(i.wpos, n_geo);
        }
    }

    return vec4<f32>(diffuse + spec + direct + emis, 1.0);
}

// The 2D UV view: unlit material color (or the selected debug channel) over the UV layout.
@fragment
fn fs_uv(i: VOut) -> @location(0) vec4<f32> {
    let base_s = textureSample(t_base, samp, i.uv);
    let orm = textureSample(t_orm, samp, i.uv);
    let mode = i32(g.mode.x + 0.5);
    if (mode == 1) {
        let ch = i32(g.mode.y + 0.5);
        switch ch {
            case 1: { return data_out(vec3<f32>(orm.b)); }
            case 2: { return data_out(vec3<f32>(orm.g)); }
            case 3: { return raw_out(textureSample(t_nrm, samp, i.uv).rgb); }
            case 4: { return raw_out(vec3<f32>(textureSample(t_height, samp, i.uv).r * 0.5 + 0.5)); }
            case 5: { return data_out(textureSample(t_emis, samp, i.uv).rgb); }
            case 6: { return raw_out(vec3<f32>(base_s.a)); }
            case 7: { return raw_out(vec3<f32>(orm.r)); }
            case 8: { return raw_out(vec3<f32>(orm.a)); }
            case 9: { return data_out(textureSample(t_height, samp, i.uv).gba); }
            case 10: { return data_out(vec3<f32>(1.0 - orm.g)); }
            default: { return data_out(base_s.rgb); }
        }
    }
    if (mode == 2) {
        return raw_out(textureSample(t_aux, samp, i.uv).rgb);
    }
    if (mode == 3) {
        let c = floor(i.uv * 32.0);
        let chk = (c.x + c.y) - 2.0 * floor((c.x + c.y) * 0.5);
        return data_out(mix(vec3<f32>(0.85), vec3<f32>(0.18, 0.2, 0.24), chk));
    }
    // Lightly shade by the AO so the flat 2D material view still reads as a surface.
    return data_out(base_s.rgb * mix(1.0, orm.r, 0.5));
}

// Background: the blurred environment, premultiplied by its opacity (the post pass puts the flat
// studio backdrop behind it).
struct BgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_bg(@builtin(vertex_index) vi: u32) -> BgOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0));
    var o: BgOut;
    o.pos = vec4<f32>(p[vi], 1.0, 1.0);
    o.ndc = p[vi];
    return o;
}

@fragment
fn fs_bg(i: BgOut) -> @location(0) vec4<f32> {
    let o = g.bg.x;
    if (g.bg.z > 0.5 || o <= 0.001) {
        return vec4<f32>(0.0);
    }
    let a = g.inv_view_proj * vec4<f32>(i.ndc, 0.0, 1.0);
    let b = g.inv_view_proj * vec4<f32>(i.ndc, 1.0, 1.0);
    let d = normalize(b.xyz / b.w - a.xyz / a.w);
    let env = textureSampleLevel(env_tex, env_samp, env_uv(d), g.bg.y).rgb;
    return vec4<f32>(env * o, o);
}

// UV wireframe in the 2D view.
@fragment
fn fs_wire(i: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.6, 0.6, 0.6, 0.16);
}

// 3D wireframe over the shaded mesh: depth-tested, pulled slightly toward the camera so the
// lines win against their own triangles.
@vertex
fn vs_wire3d(v: VIn) -> VOut {
    var o: VOut;
    o.wpos = v.pos;
    o.nrm = v.nrm;
    o.tan = v.tan;
    o.uv = v.uv;
    o.clip = g.view_proj * vec4<f32>(v.pos + (g.eye.xyz - v.pos) * 0.002, 1.0);
    return o;
}

@fragment
fn fs_wire3d(i: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.35, 0.35, 0.35, 0.45);
}
