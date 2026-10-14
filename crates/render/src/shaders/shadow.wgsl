// Depth-only key-light shadow map pass.

@group(0) @binding(0) var<uniform> light_vp: mat4x4<f32>;

@vertex
fn vs_shadow(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return light_vp * vec4<f32>(pos, 1.0);
}
