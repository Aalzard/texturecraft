# texturecraft-bake

CPU mesh-map baker: `bake(&BakeRequest, &Progress) -> BakeOutput` turns one texture set's UV layout into float maps (0..1, rows top → bottom), supersampled, padded and deterministic. All requested maps are evaluated in one pass per texel (rayon over rows, no per-texel allocation).

With a high poly (`BakeRequest::high`, every high-definition mesh appended into one), each low-poly sample casts one ray from the cage (`BakeRequest::cage`, same triangles as the low poly) or from `max_frontal` in front of the surface back through it to `max_rear` behind (both relative to the bounding-box diagonal), along normals averaged over hard edges when `average_normals` is on. With `BakeMatch::ByMeshName`, a low-poly part only sees high-poly parts of the same name once `low_suffix` / `high_suffix` are removed (`wheel_low` ↔ `wheel_high`, case-insensitive). Every map but position and world-space direction then reads the high-poly surface that ray found (AO, thickness and bent normals cast against the high poly).

| Map | Meaning | Range |
| --- | --- | --- |
| Ambient occlusion | How open the surface is to the sky (cosine-weighted rays, distance falloff) | 0 occluded … 1 open |
| Bent normals | Mean unoccluded direction of the AO rays, world space | packed ·0.5+0.5 |
| Thickness | Mean distance through the mesh along inward rays, relative to the max distance | 0 thin … 1 thick |
| Curvature | Mean curvature of the position-welded mesh, or (`curvature_from_normal`) the divergence of the baked normal map | 0 concave … 0.5 flat … 1 convex |
| Normal | Tangent-space normal from the high poly (OpenGL, +Y up); flat without one | packed ·0.5+0.5 |
| Height | High-poly offset along the ray, scaled by frontal + rear distance | 0 rear … 0.5 surface … 1 front |
| World normal | Shading normal in world space | packed ·0.5+0.5 |
| Position | Position inside the low-poly bounding box, per axis | 0 … 1 |
| ID | A distinct hue per mesh part (submesh) | RGB 0 … 1 |
| Opacity | 1 where the high poly was found (times its vertex alpha), 0 where the ray missed | 0 … 1 |
| Color map | The high poly's vertex colors (white without) | RGB 0 … 1 |
| World space direction | Facing ratio towards `direction` | 0 away … 0.5 perpendicular … 1 facing |

Uncovered texels beyond the padding get `BakedMap::neutral` values. Benchmark (2K, the 7 default maps, per-map breakdown): `cargo test -p texturecraft-bake --release -- --ignored --nocapture bench`.
