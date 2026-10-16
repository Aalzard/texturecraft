# TextureCraft roadmap

TextureCraft aims for 1:1 parity with Adobe Substance 3D Painter's texturing workflow, with better
performance, a smaller footprint, every feature drivable by agents (MCP, CLI, control channel), and
builds for Windows, macOS, Linux, FreeBSD and the web.

## Honest parity estimate (2026-10-15)

Weighted by how much of a professional's daily work each area carries. "Done" counts what works
end to end today (engine + UI + automation + tests), not what is planned.

| Area | Weight | Done | What works | Main gaps |
|---|---:|---:|---|---|
| Project and files | 8 | 95% | New project from glTF/GLB, FBX (binary and ASCII, 6.x and 7.x), COLLADA, **USD (usda, usdc, usdz)**, **Alembic (Ogawa)**, OBJ, PLY, STL or built-in meshes (auto-unwrap when UVs are missing), **project templates** (built-in and saved from a project), `.tcraft` open/save/save as/save a copy, autosave, recent files, drag and drop, reimport mesh **with strokes preserved on the surface**, image resources, remove unused resources, OBJ, textured GLB and **USD** mesh export, browser open/save/download | HDF5 Alembic; Alembic animation (first sample only) |
| Texture sets | 6 | 95% | One set per material, **UDIM / UV tiles** (auto-detected or switched on later with resampling, **a resolution per tile**, one texture per tile at its own size, `$udim` export), list with visibility, sizes 128–8192 with resampling, add/remove channels (19 channel types incl. coat, sheen, anisotropy and scattering), height strength, normal format | Filter radii and height slopes of a tile smaller than its atlas block follow the block |
| Layer stack | 16 | 92% | Paint, fill, folder and **instance** layers; 32 per-channel blend modes and opacities; white/black/painted masks with **mask with height**; generator, fill, levels, invert/threshold, color selection, compare, blur, sharpen and HSL effects on masks and layer content; anchor points; smart masks; mask view (Alt+click); duplicate, group, drag to reorder, rename, color tags, copy/cut/paste (also as instance) across sets, flatten, instantiate across sets, save layers as smart materials | Layer effects beyond the filter library (Painter's full effect catalog) |
| Painting tools | 14 | 88% | Paint, eraser, projection with image stencils, polygon fill, **path** and **lasso fill**, smudge, clone, color picker, quick mask, Shift+click lines with preview, 2D view painting; size, flow, stroke opacity, spacing, hardness, angle, follow path, jitters, alignment, angle fade, backface culling, lazy mouse, X/Y/Z symmetry, pen pressure, procedural and image alphas, **particle brush** | Pen tilt (not exposed by the windowing layer) |
| Materials and assets | 8 | 80% | 48 procedural materials (incl. car paint, velvet, brushed steel, skin, wax), 17 smart materials, 10 smart masks, **67 procedural brush alphas**, 22 procedurals, 6 environments plus imported HDR/EXR panoramas, image resources, a user smart material library, a tabbed shelf with search and **drag and drop** onto layers and the viewport | More filters (Painter ships dozens) |
| Baking | 10 | 98% | AO, curvature (also from the normal map), thickness, world normal, position, ID, normal, height, **bent normals, opacity, color map and world-space direction**; **high-poly meshes**, cage, frontal/rear distances, average normals, **match by mesh name**; per-texture-set selection, common and per-map parameters, progress and cancel | High-poly textures in external files (glTF URIs, USD layers outside a usdz) |
| Viewport and display | 12 | 93% | PBR with image-based lighting, 6 procedural environments or imported HDR/EXR panoramas with exposure, rotation, blur, opacity, ACES/linear tone mapping, HDR pipeline with bloom, vignette, sharpen and grain, key-light shadows, 4× MSAA plus idle temporal accumulation, channel / mesh-map / mask / UV checker views, 3D and 2D wireframes, brush cursor, axis gizmos, **path-traced Rendering mode** (F10, on the GPU or the CPU) with environment importance sampling and an edge-aware **denoiser** | Subsurface scattering, depth of field, multiple cameras |
| Shader | 6 | 95% | Metallic/roughness and specular/glossiness PBR, **clear coat, sheen, anisotropy and subsurface scattering** (viewport and path tracer), height-to-normal, painted normals, parallax occlusion, emissive, opacity (dithered), AO intensity | Displacement in shadows and the path tracer (viewport only, **screen-space refraction** and **displacement** done) |
| Export | 8 | 88% | 8 presets plus **user output templates** (per-channel packing, converted maps, JSON import/export), PNG/JPEG/TGA/TIFF/BMP/WEBP/EXR/HDR, 8/16/32-bit, padding modes, size override, OpenGL/DirectX normals, `$udim` tiles, OBJ, textured GLB (with clearcoat/sheen) and USD mesh export, zip download in the browser | USD export of the new shader lobes |
| UI and UX | 8 | 95% | Painter-style layout with **dockable, tabbed, floating panels**, menus, tool shelf, tool bar, asset shelf, texture set list, layer stack with context menu, properties, display/shader/history/log panels, export dialog, new project dialog, bake mode, rendering mode, settings (interface scale, autosave, viewport texture resolution), saved UI layouts, **shortcut editor** with conflict detection, tooltips with shortcuts; all menu items live ([docs/parity.md](docs/parity.md)); **9 UI languages** with CJK fonts ([docs/localization-parity.md](docs/localization-parity.md)); closed panels reopen where they were; frame-budgeted compositing and uploads keep 8K sets responsive | Native-speaker review of the translations |
| Platforms | 4 | 75% | Windows, macOS (universal, signed and notarized), Linux (AppImage, deb, rpm, tar.gz, Flatpak), FreeBSD and web release workflows | First real release run; web build polish |
| **Total** | **100** | **≈ 91%** | | |

Beyond Painter: every command is available to agents through MCP, the CLI and the app's
control channel, and the whole engine runs headless.

## Remaining effort

Estimated in wall-clock hours of Claude Opus 5.5 agent work, with three or four agents working in
parallel on separate crates:

| Milestone | Scope | Estimate |
|---|---|---:|
| M1 Layer stack depth | Done | 0 h |
| M2 Tools | Pen tilt once the windowing layer reports it | 0.5 h |
| M3 Assets | Done | 0 h |
| M4 Baking | High-poly textures in external files | 0.5 h |
| M5 Viewport | Hardware ray tracing | 1.5 h |
| M6 Shader | Done | 0 h |
| M7 Formats | Done | 0 h |
| M8 UX | Native review of translations | 1 h |
| M9 Release | CI green on main (first full clippy and test run on Linux and Windows); org admin setup (`release` environment, ruleset, the twelve signing secrets, Default runner group); first signed release run on every platform; web deployment; Flathub submission | 4 h |
| **Total** | | **≈ 7.5 h** |

## Current focus

1. Release (M9): get CI green on `main` (formatting is fixed; clippy runs on a pinned toolchain,
   and the test suite has not yet run on the CI runners), then the one-time org admin setup in
   [docs/releasing.md](docs/releasing.md), then the first release run on every platform. Packaging
   is ready: Windows MSI and zip and the web build pass local dry runs, and file associations cover
   every mesh format TextureCraft opens.
2. Hardware ray tracing in Rendering mode (M5).
