# texturecraft-bake

Mesh-map baker: `bake(&BakeRequest, &Progress) -> BakeOutput` turns one texture set's UV layout into float maps (usually 0..1, rows top → bottom), supersampled, padded and deterministic. The supersamples are processed in batches: their high-poly transfer rays, then the secondary rays of ambient occlusion, bent normals and thickness go to a `tracing::Tracer`, then all requested maps are evaluated per sample (rayon over rows).

The rays run on the CPU (`tracing::CpuTracer`, the oracle) or, through `bake_on(&BakeRequest, &Progress, &dyn Tracer)`, on a GPU tracer (the render crate's `gpu_bake::GpuBaker`, wgpu compute shaders on the path tracer's BVH traversal). The GPU receives the same rays and per-sample rotations and casts the same secondary-ray directions, so both agree up to float rounding; when the tracer fails (no adapter, a mesh too large for it, a lost device) `bake_on` bakes again on the CPU. `BakeOutput::tracer` says where the rays ran.

With a high poly (`BakeRequest::high`, every high-definition mesh appended into one), each low-poly sample casts one ray from the cage (`BakeRequest::cage`, same triangles as the low poly) or from `max_frontal` in front of the surface back through it to `max_rear` behind (both relative to the bounding-box diagonal), along normals averaged over hard edges when `average_normals` is on. Every map then reads the high-poly surface that ray found (AO, thickness and bent normals cast against the high poly).

Ambient occlusion, thickness and bent normals each have their own `RaySettings`: ray count, max distance (relative to the bounding-box diagonal), spread angle (the full cone around the normal; 180° = the hemisphere) and distribution (cosine-weighted or uniform over the cone). Ambient occlusion can ignore back faces and limit occluders to parts of the same mesh name (`SelfOcclusion::SameMeshName`); bent normals follow those occluder rules and share the AO rays when their settings match.

| Map | Meaning | Range |
| --- | --- | --- |
| Ambient occlusion | How open the surface is (rays over the cone, distance falloff) | 0 occluded … 1 open |
| Bent normals | Mean unoccluded direction of the bent-normal rays, world space | packed ·0.5+0.5 |
| Thickness | Mean distance through the mesh along inward rays, relative to the max distance | 0 thin … 1 thick |
| Curvature | Short-ray mesh curvature, or (`CurvatureMethod::NormalMap`) the divergence of the baked normal map over a texel radius derived from the output resolution | 0 concave … 0.5 flat … 1 convex |
| Normal | Tangent-space normal from the high poly (OpenGL, +Y up); flat without one | packed ·0.5+0.5 |
| Height | High-poly offset along the ray, scaled by frontal + rear distance | 0 rear … 0.5 surface … 1 front |
| World normal | Shading normal in world space | packed ·0.5+0.5 |
| Position | Bounding-box or bounding-sphere normalization; None preserves mesh coordinates | 0 … 1, or mesh units for None |
| ID | A distinct hue per mesh part (submesh) | RGB 0 … 1 |
| Opacity | 1 where the high poly was found (times its vertex and base color alpha), 0 where the ray missed | 0 … 1 |

Uncovered texels beyond the padding get `BakedMap::neutral` values. Benchmarks (2K, the 7 default maps at 64 rays): `cargo test -p texturecraft-bake --release -- --ignored --nocapture bench` (CPU, per-map breakdown) and `cargo test -p texturecraft-render --release --lib -- --ignored --nocapture gpu_bake_speed` (CPU vs GPU).

## Settings model

The clean-room `core::BakeSettings` model contains separate common, high-poly, cage, skew and per-map
settings containers. Each map uses `MapSettings<T>` to keep `is_enabled` beside its typed
parameters; bake selection and UI control visibility read that same flag. Curvature groups its method-specific settings;
AO groups its ground-plane settings. Each container owns its defaults. New projects use the reference defaults: 2048 square output, dilation 1, 64 AO /
thickness / bent-normal rays, curvature from the normal map, and bounding-sphere position
normalization. Old project representations are not preserved.

New model options include rectangular output (`common.resolution_height: None` locks the axes),
diffusion, high-poly backface filtering and suffix, cage distance units, skew edge protection,
ID source/generator, minimum occluder distance and ray units, AO attenuation/ground plane,
independent bent-normal and thickness occlusion, both curvature methods' detailed controls,
position axis/scope/no-normalization, and thickness/height normalization. AO backface handling
and position normalization each use one enum field. Curvature methods also store named
choices (`NormalMap`/`Mesh`). All dropdowns use enums,
including resolution and antialiasing. Map display labels belong to the localized UI; the
model contains identities and values. `BakedMap::id()` uses the shared MaterialCraft mesh-map
identifiers in commands, project entries and mesh-map exports; `BakedMap::parse()` is their
single parser.

Bake Mode exposes the model through Texture Sets, Settings and Bake Maps tabs on the left,
with collapsible sections, conditional controls, reference numeric ranges, section resets, and a lockable
output-size pair. Paint and Bake Mode keep separate panel layouts; texture sets use visibility
buttons in Paint Mode and bake-inclusion checkboxes in Bake Mode.
`bake.settings` accepts the complete nested model; edits persist with the project.
Algorithm integration for the added options remains a subsequent stage, including by-mesh-name
backface filtering. Paint Cage is a disabled placeholder. The current command path preserves
the full AO backface and position enums; position None returns mesh coordinates.
The algorithms described above still apply. Color map and World space direction have been
removed without legacy-map migration.

## Code organization

Types and their implementations live in individual files named after the full type. Module
folders are singular (`map`, `tracing`, `skew`). Shared sampling and ray-setting logic has one
implementation. Persisted settings live in `core`; Bake Mode controls and localized labels
live in `ui-egui`. The baker has no localization dependency.
