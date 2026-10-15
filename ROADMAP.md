# TextureCraft roadmap

TextureCraft aims for 1:1 parity with Adobe Substance 3D Painter's texturing workflow, with better
performance, a smaller footprint, every feature drivable by agents (MCP, CLI, control channel), and
builds for Windows, macOS, Linux, FreeBSD and the web.

## Honest parity estimate (2026-10-15)

Weighted by how much of a professional's daily work each area carries. "Done" counts what works
end to end today (engine + UI + automation + tests), not what is planned.

| Area | Weight | Done | What works | Main gaps |
|---|---:|---:|---|---|
| Project and files | 8 | 85% | New project from glTF/GLB, FBX (binary and ASCII 7.x), COLLADA, **USD (usda, usdc, usdz)**, OBJ, PLY, STL or built-in meshes (auto-unwrap when UVs are missing), `.tcraft` open/save/save as/save a copy, autosave, recent files, drag and drop, reimport mesh, image resources, remove unused resources, OBJ, textured GLB and **USD** mesh export, browser open/save/download | Alembic import; FBX 6; project templates; preserving strokes on reimport |
| Texture sets | 6 | 80% | One set per material, **UDIM / UV tiles** (auto-detected, one texture per tile, `$udim` export), list with visibility, sizes 128–8192 with resampling, add/remove channels (19 channel types incl. coat, sheen, anisotropy and scattering), height strength, normal format | Per-tile resolution, converting a set to UV tiles after creation |
| Layer stack | 16 | 82% | Paint, fill, folder and **instance** layers; 32 per-channel blend modes and opacities; white/black/painted masks with **mask with height**; generator, fill, levels, invert/threshold, color selection, compare, blur, sharpen and HSL effects on masks and layer content; anchor points; smart masks; mask view (Alt+click); duplicate, group, drag to reorder, rename, color tags, copy/cut/paste (also as instance) across sets, flatten, instantiate across sets, save layers as smart materials | Anchor position inside a content stack; instances across texture sets of different resolutions |
| Painting tools | 14 | 78% | Paint, eraser, projection with image stencils, polygon fill, **path** and **lasso fill**, smudge, clone, color picker, quick mask, Shift+click lines with preview, 2D view painting; size, flow, stroke opacity, spacing, hardness, angle, follow path, jitters, alignment, angle fade, backface culling, lazy mouse, X/Y/Z symmetry, pen pressure, procedural and image alphas, **particle brush** | Pen tilt (not exposed by the windowing layer), geometry decals, clone from the current layer only |
| Materials and assets | 8 | 70% | 48 procedural materials (incl. car paint, velvet, brushed steel, skin, wax), 17 smart materials, 10 smart masks, **67 procedural brush alphas**, 22 procedurals, 6 environments plus imported HDR/EXR panoramas, image resources, a user smart material library, a tabbed shelf with search and **drag and drop** onto layers and the viewport | Filter library; dragging saved smart materials |
| Baking | 10 | 82% | AO, curvature (also from the normal map), thickness, world normal, position, ID, normal, height, **bent normals, opacity, color map and world-space direction**; **high-poly meshes**, cage, frontal/rear distances, average normals, **match by mesh name**; per-texture-set selection, common and per-map parameters, progress and cancel | Per-map ray settings, GPU ray-traced baking, high-poly material textures for the color map |
| Viewport and display | 12 | 75% | PBR with image-based lighting, 6 procedural environments or imported HDR/EXR panoramas with exposure, rotation, blur, opacity, ACES/linear tone mapping, HDR pipeline with bloom, vignette, sharpen and grain, key-light shadows, 4× MSAA plus idle temporal accumulation, channel / mesh-map / mask / UV checker views, 3D and 2D wireframes, brush cursor, axis gizmos, **path-traced Rendering mode** (F10) with environment importance sampling and an edge-aware **denoiser** | Subsurface scattering, depth of field, multiple cameras, GPU path tracing |
| Shader | 6 | 75% | Metallic/roughness and specular/glossiness PBR, **clear coat, sheen, anisotropy and subsurface scattering** (viewport and path tracer), height-to-normal, painted normals, parallax occlusion, emissive, opacity (dithered), AO intensity | Transmission/IOR, coat normal, screen-space subsurface blur |
| Export | 8 | 85% | 8 presets plus **user output templates** (per-channel packing, converted maps, JSON import/export), PNG/JPEG/TGA/TIFF/BMP/WEBP/EXR/HDR, 8/16/32-bit, padding modes, size override, OpenGL/DirectX normals, `$udim` tiles, OBJ, textured GLB (with clearcoat/sheen) and USD mesh export, zip download in the browser | USD export of the new shader lobes |
| UI and UX | 8 | 78% | Painter-style layout with **dockable, tabbed, floating panels**, menus, tool shelf, tool bar, asset shelf, texture set list, layer stack with context menu, properties, display/shader/history/log panels, export dialog, new project dialog, bake mode, rendering mode, settings (interface scale, autosave), saved UI layouts, **shortcut editor** with conflict detection, tooltips with shortcuts; all menu items live ([docs/parity.md](docs/parity.md)); progressive compositing keeps 4K and 8K sets responsive | Translations (the lookup is in place), remembering where a closed panel was |
| Platforms | 4 | 60% | Windows, macOS (universal, signed and notarized), Linux (AppImage, deb, rpm, tar.gz, Flatpak), FreeBSD and web release workflows | First real release run; web build polish |
| **Total** | **100** | **≈ 78%** | | |

Beyond Painter: every command is available to agents through MCP, the CLI and the app's
control channel, and the whole engine runs headless.

## Remaining effort

Estimated in wall-clock hours of Claude Opus 5.5 agent work, with three or four agents working in
parallel on separate crates:

| Milestone | Scope | Estimate |
|---|---|---:|
| M1 Layer stack depth | Anchor position in content stacks, cross-resolution instances | 1 h |
| M2 Tools | Geometry decals, clone from layer, pen tilt when the platform exposes it | 3 h |
| M3 Assets | Filter library, dragging saved smart materials | 2 h |
| M4 Baking | Per-map ray settings, GPU ray-traced baking, high-poly material transfer | 3 h |
| M5 Viewport | SSS, depth of field, cameras, GPU path tracing | 6 h |
| M6 Shader | Transmission/IOR, coat normal, tessellated displacement | 2 h |
| M7 Formats | Alembic import, per-tile UDIM resolution | 3 h |
| M8 UX | Translations, panel placement memory, more 8K tuning | 3 h |
| M9 Release | First signed release on every platform, web deployment, README screenshots refresh | 4 h |
| **Total** | | **≈ 27 h** |

## Current focus

1. Viewport (M5): depth of field, multiple cameras, GPU path tracing.
2. Baking (M4): per-map ray settings, GPU ray-traced baking.
3. Release (M9): signing secrets on the repository, then the first release run.
