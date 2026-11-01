// Depth-only key-light shadow map pass, displaced like the 3D view when it displaces.

struct ShadowPass {
    light_vp: mat4x4<f32>,
    disp: vec4<f32>, // x displacement on (1), y world units per unit of height, z vertex spacing in UV units, w -
};

@group(0) @binding(0) var<uniform> sp: ShadowPass;
// The set bind group of mesh.wgsl: only its height and sampler.
@group(1) @binding(3) var t_height: texture_2d<f32>;
@group(1) @binding(9) var samp: sampler;

@vertex
fn vs_shadow(@location(0) pos: vec3<f32>, @location(1) nrm: vec3<f32>, @location(3) uv: vec2<f32>) -> @builtin(position) vec4<f32> {
    var p = pos;
    if (sp.disp.x > 0.5) {
        p = displace_by_height(pos, nrm, uv, t_height, samp, sp.disp.y, sp.disp.z);
    }
    return sp.light_vp * vec4<f32>(p, 1.0);
}
