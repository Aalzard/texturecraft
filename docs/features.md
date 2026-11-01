# TextureCraft features

Everything TextureCraft does today, by area. What's still missing is in [`ROADMAP.md`](../ROADMAP.md).

## Paint in 3D

- **Brushes** with size, flow, stroke opacity, spacing, hardness, angle and follow path; size,
  angle, flow and position jitter; tangent or camera alignment, angle fade, backface culling, lazy
  mouse, X/Y/Z mirror symmetry and pen pressure. **67 procedural brush alphas** in their own shelf
  tab, or any imported image as the alpha.
- **Tools:** paint, eraser, **projection** with image stencils (hold S to move, turn and scale the
  stencil), polygon fill (triangle, UV island or the whole mesh), **path** and **lasso fill**, a
  **particle brush**, **smudge**, **clone** (V + click sets the source), color picker, Shift+click
  straight lines, and painting directly in the **2D UV view**.
- **Geometry decals:** drop an image on the surface, then move, scale and turn its frame; it stays
  an editable fill layer.
- **Quick mask** (Y): paint a temporary mask (U to edit, Ctrl+Shift+I to invert) that protects
  everything outside it while you paint.
- **Every channel at once:** base color, metallic, roughness, height, normal, emissive, opacity,
  ambient occlusion and specular level, plus clear coat, sheen, anisotropy, subsurface scattering
  and transmission, each toggled per stroke in the brush's material properties.

## Layers, masks and smart materials

- **Paint, fill, folder and instance layers** with **32 blend modes per channel** (Normal through
  Normal map detail), per-channel opacity, visibility, color tags, drag-and-drop reordering,
  duplicate, group, flatten, cut, copy and paste across texture sets.
- **Filters and content effects** on any layer or mask: **27 filters** (HSL, levels,
  contrast/luminosity, color balance, gradient map, posterize, clamp, invert, threshold,
  grayscale, edge detect, normal invert, baked lighting, blur, directional and slope blur, sharpen, high pass,
  bevel, glow, warp, directional warp, mosaic and four material finishes), fills and paint.
  **Anchor points** record the stack at a layer so masks and fills higher up can react to it.
- **Masks** with a white or black background and a stack of effects: painted strokes, fills,
  **mesh-map generators** (edge wear, dirt, thin areas, curvature, AO, position and direction
  gradients, all with world-space noise), filters, ID color selection and compare. Alt+click a
  mask to view it alone.
- **Fill layers** take uniform values, **22 procedurals** (clouds, cells, grunge, scratches, wood
  grain, bricks, fabric weave, leather grain, marble veins, Voronoi, paint chips…) or your own
  **images**, through UV, tri-planar, planar, spherical, cylindrical or decal projection.
- **48 materials** (metals from measured reflectance, car paint, brushed anisotropic steel,
  velvet, skin, wax, glass, woods, stone, plastics, fabric, leather, ceramic), **17 smart
  materials** (rusted iron, worn paint, oxidized copper, bronze statue, weathered wood, mossy
  brick…) and **10 smart masks**, all procedural, so they work at any resolution. Drag them from
  the tabbed, searchable shelf onto the layer stack or straight onto the model.
- **Your own library:** right-click a layer or folder, choose *Create smart material*, and it
  appears under *My smart materials*, pixels and images included.

## Files and texture sets

- **Open** glTF/GLB, FBX (binary and ASCII), COLLADA, **USD** (usda, usdc, usdz), **Alembic**,
  OBJ, PLY and STL, or a built-in mesh; UVs are generated when a mesh has none. A `.gltf`'s buffers
  and images and a loose USD layer's textures are read from its folder and kept in the project.
  Reimport a changed mesh and your painted strokes stay on the surface.
- **UDIM / UV tiles:** detected on import or switched on later, with a resolution per tile and one
  texture per tile on export (`$udim`).
- **Texture sets** from 128 to 8192 px per material, resampled when you change them, with 21
  channel types to add or remove.
- **Projects** are a single `.tcraft` file: the mesh, every layer (sparse and lossless), baked maps
  and imported images. **Project templates** start new work from a saved setup.

## Bake and export

- **Bake mesh maps** on the GPU (ray traced, about 12× the CPU path) or on every CPU core: ambient
  occlusion, curvature, thickness, world-space normal, position, ID, bent normals, opacity, color
  and direction maps, and normal and height maps from high-poly meshes, with a cage, ray distances
  and matching by mesh name.
- **Export presets** for glTF PBR Metal Roughness, Unreal Engine (packed), Unity HDRP (mask map),
  Unity Standard, Blender (Principled BSDF), separate maps, mesh maps and document channels, in
  PNG, JPEG, TGA, TIFF, BMP, WEBP, EXR or HDR at 8, 16 or 32 bits, with UV padding, size
  override and OpenGL or DirectX normals.
- **Output template editor:** build your own presets with per-channel packing and converted maps,
  and share them as JSON.
- **Mesh export** to OBJ, textured GLB (with clear coat and sheen) and USD (with clear coat).

## The viewport

- Physically based shading (metallic/roughness or specular/glossiness) with clear coat, sheen,
  anisotropy, subsurface scattering, transmission, parallax occlusion and **displacement** (with its own shadows) from the
  height channel; image-based lighting from
  six procedural studio and outdoor environments or **your own HDR/EXR panoramas**; ACES or linear
  tone mapping, key-light shadows, bloom, vignette, sharpen, grain, **depth of field** and
  **saved cameras** (also imported from glTF, FBX, COLLADA and USD files).
- **Rendering mode** (F10): a progressive, **path-traced** view of your textures with multi-bounce
  light, environment importance sampling, displaced geometry, a denoiser and one-click PNG save,
  traced on the GPU
  (compute shaders, about 25× the CPU tracer, desktop and WebGPU) or on the CPU.
- View any channel, any baked mesh map, or a UV checker; Alt+drag to orbit, middle-drag to pan,
  wheel to zoom, F to frame, M / C / B to cycle views.

## Interface

- A modern look in **four brightness levels** (dark, medium dark, medium light and light) with
  **seven accent colors**, set in Settings › Interface.
- **Dockable, tabbed and floating panels**, saved layouts, and panels that reopen where you closed
  them.
- A **shortcut editor** with conflict detection, and every shortcut shown in its tooltip.
- **9 languages:** English, French, German, Spanish, Portuguese, Italian, Japanese, Chinese and
  Korean, switchable live.
- **In the browser:** the same app compiled to WebAssembly, on WebGPU or WebGL2, with touch
  navigation and project open, save and download.

## Built for agents

Everything you can do in the window, an agent can do too:

- **CLI:** `texturecraft-cli new builtin:material_ball out.tcraft`, `bake`, `export`, `render`,
  `run <command> <params>`, `batch`, `commands`, `info`.
- **MCP server:** `texturecraft-cli mcp` gives Claude and other agents tools to create projects,
  add layers and masks, paint strokes, add high polys and a cage, bake, export textures and the
  mesh, reimport a changed mesh, render previews and inspect everything they changed, with file
  access limited to the folders you give it.
- **Control channel:** launch the app with `--control <port>` to drive the running UI: run any
  command, invoke menu items, paint, and take screenshots.

See [`control-protocol.md`](control-protocol.md) for the full protocol and examples.
