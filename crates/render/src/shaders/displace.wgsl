// Displacement shared by the 3D view (mesh.wgsl) and the key-light shadow pass (shadow.wgsl).

// `p` moved along normal `n` by `scale` times the height at `uv`, read at the mip whose texels
// match the vertex spacing `spacing` (UV units).
fn displace_by_height(p: vec3<f32>, n: vec3<f32>, uv: vec2<f32>, height: texture_2d<f32>, smp: sampler, scale: f32, spacing: f32) -> vec3<f32> {
    let dims = vec2<f32>(textureDimensions(height));
    let h = textureSampleLevel(height, smp, uv, max(log2(dims.x * spacing), 0.0)).r;
    return p + n * inverseSqrt(max(dot(n, n), 1e-12)) * (h * scale);
}
