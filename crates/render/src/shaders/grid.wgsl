// The ground grid of the 3D view, appended to mesh.wgsl (uses `g`). A large quad on the ground
// plane, centred under the eye; every line is computed per pixel, so the grid is endless to the
// eye, anti-aliased by screen-space derivatives and free of moire: a layer fades out as its
// cells approach the size of a pixel, and the whole grid fades with distance from the eye and
// at grazing angles. Zooming moves through decades: three layers (cells of s, 10 s, 100 s) trade
// weight, so minor lines turn into major lines and new minor lines appear without popping.
// Output is premultiplied scene-linear light, blended over the scene (see `GridShade`).

struct GridOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
};

@vertex
fn vs_grid(@builtin(vertex_index) vi: u32) -> GridOut {
    var corner = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corner[vi];
    let half = g.grid2.z;
    let p = vec3<f32>(g.eye.x + c.x * half, g.grid2.y, g.eye.z + c.y * half);
    var o: GridOut;
    o.wpos = p;
    o.clip = g.view_proj * vec4<f32>(p, 1.0);
    return o;
}

// One layer of lines every integer of `q`: 1 on a line, 0 between, with a `width` pixel falloff,
// and gone once the cells are only a few pixels wide.
fn grid_layer(q: vec2<f32>, width: f32) -> f32 {
    let w = max(fwidth(q), vec2<f32>(1e-7));
    let d = abs(fract(q - 0.5) - 0.5) / w;
    let line = vec2<f32>(1.0) - clamp(d / width, vec2<f32>(0.0), vec2<f32>(1.0));
    let cells = clamp((vec2<f32>(1.0) / w - 3.0) / 8.0, vec2<f32>(0.0), vec2<f32>(1.0));
    return max(line.x * cells.x, line.y * cells.y);
}

@fragment
fn fs_grid(i: GridOut) -> @location(0) vec4<f32> {
    // Everything with derivatives first (uniform control flow).
    let s = g.grid.x;
    let blend = g.grid.y;
    let p = i.wpos.xz;
    let fine = grid_layer(p / s, 1.0);
    let mid = grid_layer(p / (s * 10.0), 1.15);
    let coarse = grid_layer(p / (s * 100.0), 1.3);
    let axis_px = abs(p) / max(fwidth(p), vec2<f32>(1e-7));
    let axis = vec2<f32>(1.0) - clamp(axis_px / 1.6, vec2<f32>(0.0), vec2<f32>(1.0));

    // Minor lines every `s`, major every 10 `s`; across a decade the fine layer fades out, the
    // mid layer goes from major to minor weight and the coarse layer comes in as the majors.
    let minor_a = 0.20;
    let major_a = 0.50;
    let lines = max(max(fine * minor_a * (1.0 - blend), mid * mix(major_a, minor_a, blend)), coarse * major_a * blend);

    // Fade with distance from the eye and at grazing angles (an orthographic view looks along
    // one direction; a perspective one from the eye).
    let to_eye = g.eye.xyz - i.wpos;
    let dist = length(to_eye);
    let view_dir = select(to_eye / max(dist, 1e-6), -g.fwd.xyz, g.fwd.w > 0.5);
    let angle = smoothstep(0.01, 0.15, abs(view_dir.y));
    let near = smoothstep(g.grid3.x, g.grid3.y, dot(i.wpos - g.eye.xyz, g.fwd.xyz));
    let range = (1.0 - smoothstep(g.grid.z, g.grid.w, dist)) * near;
    let fade = angle * range * g.grid2.x;

    let line_color = vec3<f32>(0.62, 0.65, 0.70);
    let x_axis = vec3<f32>(0.90, 0.22, 0.22); // the line z = 0 runs along X
    let z_axis = vec3<f32>(0.25, 0.45, 1.00); // the line x = 0 runs along Z
    // The axes stay visible over the finer lines (and a bit farther out).
    let axis_fade = sqrt(angle * range) * g.grid2.x;
    let ax = axis.y * 0.85 * axis_fade;
    let az = axis.x * 0.85 * axis_fade;
    let a_axis = max(ax, az);
    let axis_color = select(z_axis, x_axis, ax >= az);

    let a_line = lines * fade;
    let a = a_axis + a_line * (1.0 - a_axis);
    let rgb = axis_color * a_axis + line_color * a_line * (1.0 - a_axis);
    return vec4<f32>(rgb * g.grid2.w, a);
}
