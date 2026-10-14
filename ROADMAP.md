# TextureCraft roadmap

TextureCraft aims for 1:1 parity with Adobe Substance 3D Painter's texturing workflow, with better
performance, a smaller footprint, every feature drivable by agents (MCP, CLI, control channel), and
builds for Windows, macOS, Linux, FreeBSD and the web.

## Honest parity estimate (2026-10-14)

Weighted by how much of a professional's daily work each area carries. "Done" counts what works
end to end today (engine + UI + automation + tests), not what is planned.

| Area | Weight | Done | What works | Main gaps |
|---|---:|---:|---|---|
| Project and files | 8 | 75% | New project from glTF/GLB, FBX (binary and ASCII 7.x), COLLADA, OBJ, PLY, STL or built-in meshes (auto-unwrap when UVs are missing), `.tcraft` open/save/save as/save a copy, autosave, recent files, drag and drop (projects, meshes, images), reimport mesh, image resources, remove unused resources, OBJ and textured GLB mesh export, browser open/save/download | USD, Alembic import; FBX 6; project templates; preserving strokes on reimport |
| Texture sets | 6 | 62% | One set per material, list with visibility, sizes 128–8192 with resampling, add/remove channels (11 channel types incl. specular and glossiness), height strength, normal format | UDIM / UV tiles, per-channel bit depth, normal and AO mixing options, UV padding modes |
| Layer stack | 16 | 75% | Paint, fill and folder layers; 32 per-channel blend modes and opacities; white/black/painted masks; generator, fill, levels, invert/threshold, color selection, compare, blur, sharpen and HSL effects on masks **and layer content**; anchor points read by masks and fills above; smart masks; mask view (Alt+click); duplicate, group, drag to reorder, rename, color tags, copy/cut/paste across sets, flatten, instantiate across sets, save layers as smart materials and add them back | Instance layers, mask with height combination, anchor position inside a content stack |
| Painting tools | 14 | 68% | Paint, eraser, projection with image stencils, polygon fill (triangle, UV island, mesh), smudge, clone, color picker, quick mask, Shift+click lines, 2D view painting; size, flow, stroke opacity, spacing, hardness, angle, follow path, jitters, alignment, angle fade, backface culling, lazy mouse, X/Y/Z symmetry, pen pressure, 7 procedural alphas and image alphas, seam-free 3D stamps with camera visibility | Pen tilt, path / filled path / ribbon tools, particle brushes, geometry decals, clone from the current layer only |
| Materials and assets | 8 | 55% | 43 built-in procedural materials, 17 smart materials, 10 smart masks (`mask.smart.apply`), 22 procedurals, 6 environments plus imported HDR/EXR panoramas, image resources, a user smart material library, a tabbed shelf with search across tabs | Filter and alpha libraries; drag from shelf onto layers (Substance `.sbsar` support is a non-goal: the engine is proprietary) |
| Baking | 10 | 60% | AO, curvature, thickness, world normal, position, ID, normal and height from high poly (engine), resolution, antialiasing, dilation, rays, progress and cancel, bake mode UI | High-poly / cage UI, match by name, bent normals, opacity and color transfer, curvature from normal map, GPU ray-traced baking |
| Viewport and display | 12 | 72% | PBR with image-based lighting, 6 procedural environments or imported HDR/EXR panoramas with exposure, rotation, blur, opacity, ACES/linear tone mapping, HDR pipeline with bloom, vignette, sharpen and grain, key-light shadows, 4× MSAA plus idle temporal accumulation, channel / mesh-map / mask / UV checker views, 3D and 2D wireframes, brush cursor, axis gizmos, **path-traced Rendering mode** (F10) with environment importance sampling | Subsurface scattering, depth of field, multiple cameras, GPU path tracing |
| Shader | 6 | 45% | Metallic/roughness and specular/glossiness PBR, height-to-normal, painted normals, parallax occlusion, emissive, opacity (dithered), AO intensity | OpenPBR features (coat, sheen, subsurface, anisotropy, thin film), tessellated displacement |
| Export | 8 | 70% | 8 presets (glTF, Unreal, Unity HDRP/Standard, Blender, separate maps, mesh maps, document channels), PNG/JPEG/TGA/TIFF/BMP/WEBP/EXR/HDR, 8/16/32-bit, padding modes, size override, OpenGL/DirectX normals, template file names, OBJ and textured GLB mesh export, zip download in the browser | Output template editor, USD export |
| UI and UX | 8 | 65% | Painter-style layout: menus, tool shelf, tool bar, asset shelf, texture set list, layer stack with context menu, properties, display/shader/history windows, export dialog, new project dialog, bake mode, rendering mode, settings (interface scale, autosave), saved UI layouts; all 51 menu items live ([docs/parity.md](docs/parity.md)) | Docking, shortcut editor, localization, tooltips everywhere |
| Platforms | 4 | 60% | Windows, macOS (universal, signed and notarized), Linux (AppImage, deb, rpm, tar.gz, Flatpak), FreeBSD and web release workflows | First real release run; web build polish |
| **Total** | **100** | **≈ 65%** | | |

Beyond Painter: every command is available to agents through MCP, the CLI and the app's
control channel, and the whole engine runs headless.

## Remaining effort

Estimated in wall-clock hours of Claude Opus 5.5 agent work, with three or four agents working in
parallel on separate crates:

| Milestone | Scope | Estimate |
|---|---|---:|
| M1 Layer stack depth | Instance layers, height-combined masks | 3 h |
| M2 Tools | Path, filled-path and ribbon tools, particle brushes, decals, pen tilt | 6 h |
| M3 Assets | Filter and alpha libraries, shelf drag and drop | 5 h |
| M4 Baking | High-poly and cage UI, match by name, bent normals, transfers, GPU ray-traced baking | 8 h |
| M5 Viewport | SSS, depth of field, cameras, GPU path tracing | 6 h |
| M6 Shader | OpenPBR lobes (coat, sheen, subsurface, anisotropy), displacement | 4 h |
| M7 Formats | USD, Alembic import; USD export; output template editor; UDIM | 8 h |
| M8 UX | Docking, shortcut editor, localization, performance passes at 4K/8K | 6 h |
| M9 Release | First signed release on every platform, web deployment, README screenshots refresh | 4 h |
| **Total** | | **≈ 50 h** |

## Current focus

1. Baking (M4): high-poly and cage UI, match by name, bent normals, transfers.
2. Tools (M2): path, filled-path and ribbon tools; particle brushes.
3. UX (M8): docking and a shortcut editor; release (M9) once the repository exists.
