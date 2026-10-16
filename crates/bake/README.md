# texturecraft-bake

Mesh-map baker: `bake(&BakeRequest, &Progress) -> BakeOutput` turns one texture set's UV layout into float maps (0..1, rows top → bottom), supersampled, padded and deterministic. The supersamples are processed in batches: their high-poly transfer rays, then the secondary rays of ambient occlusion, bent normals and thickness go to a `rays::Tracer`, then all requested maps are evaluated per sample (rayon over rows).

The rays run on the CPU (`rays::CpuTracer`, the oracle) or, through `bake_on(&BakeRequest, &Progress, &dyn Tracer)`, on a GPU tracer (the render crate's `gpu_bake::GpuBaker`, wgpu compute shaders on the path tracer's BVH traversal). The GPU receives the same rays and per-sample rotations and casts the same secondary-ray directions, so both agree up to float rounding; when the tracer fails (no adapter, a mesh too large for it, a lost device) `bake_on` bakes again on the CPU. `BakeOutput::tracer` says where the rays ran.

With a high poly (`BakeRequest::high`, every high-definition mesh appended into one), each low-poly sample casts one ray from the cage (`BakeRequest::cage`, same triangles as the low poly) or from `max_frontal` in front of the surface back through it to `max_rear` behind (both relative to the bounding-box diagonal), along normals averaged over hard edges when `average_normals` is on. With `BakeMatch::ByMeshName`, a low-poly part only sees high-poly parts of the same name once `low_suffix` / `high_suffix` are removed (`wheel_low` ↔ `wheel_high`, case-insensitive). Every map but position and world-space direction then reads the high-poly surface that ray found (AO, thickness and bent normals cast against the high poly).

Ambient occlusion, thickness and bent normals each have their own `RaySettings`: ray count, max distance (relative to the bounding-box diagonal), spread angle (the full cone around the normal; 180° = the hemisphere) and distribution (cosine-weighted or uniform over the cone). Ambient occlusion can ignore back faces and limit occluders to parts of the same mesh name (`SelfOcclusion::SameMeshName`); bent normals follow those occluder rules and share the AO rays when their settings match.

| Map | Meaning | Range |
| --- | --- | --- |
| Ambient occlusion | How open the surface is (rays over the cone, distance falloff) | 0 occluded … 1 open |
| Bent normals | Mean unoccluded direction of the bent-normal rays, world space | packed ·0.5+0.5 |
| Thickness | Mean distance through the mesh along inward rays, relative to the max distance | 0 thin … 1 thick |
| Curvature | Mean curvature of the position-welded mesh, or (`curvature_from_normal`) the divergence of the baked normal map over `curvature_radius` texels (0: by resolution) | 0 concave … 0.5 flat … 1 convex |
| Normal | Tangent-space normal from the high poly (OpenGL, +Y up); flat without one | packed ·0.5+0.5 |
| Height | High-poly offset along the ray, scaled by frontal + rear distance | 0 rear … 0.5 surface … 1 front |
| World normal | Shading normal in world space | packed ·0.5+0.5 |
| Position | Position in the low poly's bounding box (per axis) or bounding sphere (`position_mode`) | 0 … 1 |
| ID | A distinct hue per mesh part (submesh) | RGB 0 … 1 |
| Opacity | 1 where the high poly was found (times its vertex and base color alpha), 0 where the ray missed | 0 … 1 |
| Color map | The high poly's vertex colors times its materials' base color (`HighPoly::base_color`: factor × texture at the hit UV, bilinear in linear light), sRGB-encoded like a base color texture | RGB 0 … 1 |
| World space direction | Facing ratio towards `direction`: signed (0.5 perpendicular) or positive (`direction_mode`) | 0 away … 1 facing |

Uncovered texels beyond the padding get `BakedMap::neutral` values. Benchmarks (2K, the 7 default maps at 64 rays): `cargo test -p texturecraft-bake --release -- --ignored --nocapture bench` (CPU, per-map breakdown) and `cargo test -p texturecraft-render --release --lib -- --ignored --nocapture gpu_bake_speed` (CPU vs GPU).
