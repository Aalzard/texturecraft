<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>

<h1 align="center">TextureCraft</h1>

<p align="center">
  <b>3D texture painting; an open-source, clean-room reimplementation of Adobe Substance 3D Painter, rebuilt in pure Rust.</b><br>
  Paint PBR materials straight onto your models in layers with masks, bake mesh maps,<br>
  drive smart materials from them, and export textures for any engine. Open source, offline, and yours.
</p>

<p align="center">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="macOS · Windows · Linux · FreeBSD · Web" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20FreeBSD%20%C2%B7%20Web-native-c2672b?style=flat-square">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square">
  <img alt="Status: early alpha" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Join the ArtCraft community on Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="https://getartcraft.com/apps/texturecraft"><b>TextureCraft on getartcraft.com</b></a> ·
  <a href="https://getartcraft.com/">ArtCraft</a> ·
  <a href="https://getartcraft.com/apps">All Crafting Apps</a>
</p>

<br>

<p align="center">
  <img src="docs/images/texturecraft-workspace.png" alt="The TextureCraft workspace: the asset shelf on the left, the 3D viewport with a material ball wearing a painted-steel smart material on a rusted iron base, the 2D UV view, and the texture set list, layer stack and properties on the right" width="100%">
  <br>
  <sub>The workspace: asset shelf, 3D viewport, 2D UV view, texture sets, layer stack and properties.</sub>
</p>

> [!NOTE]
> **ArtCraft is a community of artists from all walks of life.** Painters, sculptors, game
> artists, photographers, filmmakers, illustrators, designers, hobbyists, and people who picked up
> a stylus last week. If you make things, you belong here. **[Come say hi on Discord](https://discord.gg/artcraft).**

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#paint-in-3d">Painting</a> ·
  <a href="#layers-masks-and-smart-materials">Layers</a> ·
  <a href="#bake-and-export">Bake and export</a> ·
  <a href="#built-for-agents">Agents</a> ·
  <a href="#get-started">Get started</a> ·
  <a href="#downloads">Downloads</a> ·
  <a href="#the-crafting-apps">Crafting Apps</a> ·
  <a href="https://discord.gg/artcraft">Discord</a>
</p>

<br>

<table>
  <tr>
    <td width="25%" valign="top">
      <h3>🎨 Familiar by design</h3>
      Texture sets, a layer stack with per-channel blending, masks and generators, the tool shelf, the 3D and 2D views: everything sits where a Painter user expects it.
    </td>
    <td width="25%" valign="top">
      <h3>⚡ Native and fast</h3>
      A tiled, copy-on-write texel engine on every CPU core and a wgpu viewport (Vulkan, Metal, DX12, WebGPU). No Electron, no web view, no waiting.
    </td>
    <td width="25%" valign="top">
      <h3>🧵 Seamless by construction</h3>
      Brush stamps live on the 3D surface, so strokes flow across UV seams, and the camera's visibility keeps paint off the back of your model.
    </td>
    <td width="25%" valign="top">
      <h3>🤖 Agent-ready</h3>
      Every action is a command, so you can drive the same engine from the UI, the CLI, a JSON control channel or an MCP server.
    </td>
  </tr>
</table>

## Features

### Paint in 3D

- **Brushes** with size, flow, stroke opacity, spacing, hardness, angle and follow path; size,
  angle, flow and position jitter; tangent or camera alignment, angle fade, backface culling, lazy
  mouse, X/Y/Z mirror symmetry and pen pressure. Seven procedural brush alphas (round, square,
  spray, grunge, cloud, star, dry streaks), or any imported image as the alpha.
- **Tools:** paint, eraser, **projection** with image stencils (hold S to move, turn and scale the
  stencil), polygon fill (triangle, UV island or the whole mesh), **smudge**, **clone** (V + click
  sets the source), color picker, Shift+click straight lines, and painting directly in the
  **2D UV view**.
- **Quick mask** (Y): paint a temporary mask (U to edit, Ctrl+Shift+I to invert) that protects
  everything outside it while you paint.
- **Every channel at once:** base color, metallic, roughness, height, normal, emissive, opacity,
  ambient occlusion and specular level, each toggled per stroke, the way Painter's material
  properties work.

### Layers, masks and smart materials

- **Paint, fill and folder layers** with **32 blend modes per channel** (Normal through Normal map
  detail), per-channel opacity, visibility, color tags, drag-and-drop reordering, duplicate, group,
  flatten, cut, copy and paste across texture sets.
- **Content effects** on any layer: fill, paint, levels, blur, sharpen, invert, threshold and
  **HSL**, applied to the layer's channels before it blends. **Anchor points** record the stack at a
  layer so masks and fills higher up can react to it (for example, edge detail from painted height).
- **Masks** with a white or black background and a stack of effects: painted strokes, fills,
  **mesh-map generators** (edge wear, dirt, thin areas, curvature, AO, position and direction
  gradients, all with world-space noise), levels, blur, sharpen, invert, threshold, ID color
  selection and compare. Alt+click a mask to view it alone.
- **Fill layers** take uniform values, **procedurals** (clouds, Perlin, noise, cells, grunge,
  scratches, checker, stripes, gradient, wood grain, bricks, tiles, fabric weave, leather grain,
  hammered, brushed lines, marble veins, Voronoi, streaks, fingerprints, pores, paint chips) or your
  own **images**, through UV, tri-planar, planar, spherical or cylindrical projection.
- **43 materials** (metals from measured reflectance, woods, stone and masonry, plastics, fabric,
  leather, ceramic, glass), **17 smart materials** (rusted iron, worn and rust-streaked paint,
  military paint, oxidized copper, bronze statue, weathered wood, chipped paint, dirty concrete,
  mossy brick…) and **10 smart masks** (worn and chipped edges, dirt, dust, ground dirt, rust
  leaks, scratches, grunge, paint chips, fingerprints), all procedural, so they work at any
  resolution. The shelf groups them in tabs, with search across every tab.
- **Your own library:** right-click a layer or folder, choose *Create smart material*, and it
  appears under *My smart materials*, pixels and images included.

### Bake and export

- **Bake mesh maps** on every CPU core: ambient occlusion, curvature, thickness, world-space
  normal, position, ID, and normal and height maps from a high poly. A 1024² bake of seven maps on
  the material ball takes about 1.5 seconds.
- **Export presets** for glTF PBR Metal Roughness, Unreal Engine (packed), Unity HDRP (mask map),
  Unity Standard, Blender (Principled BSDF), separate maps, mesh maps and document channels, in
  PNG, JPEG, TGA, TIFF, BMP, WEBP, EXR or HDR at 8, 16 or 32 bits, with UV padding, size
  override and OpenGL or DirectX normals.
- **Projects** are a single `.tcraft` file: the mesh, every layer (sparse and lossless), baked maps
  and imported images.

### The viewport

- Physically based shading (metallic/roughness or specular/glossiness) with image-based lighting
  from six procedural studio and outdoor environments or **your own HDR/EXR panoramas**, exposure,
  rotation, blur, ACES or linear tone mapping, key-light shadows, bloom, vignette, sharpen and
  grain, parallax occlusion, and temporal anti-aliasing that refines the image while you look.
- **Rendering mode** (F10): a progressive, path-traced view of your textures with multi-bounce
  light, environment importance sampling and one-click PNG save.
- View any channel, any baked mesh map, or a UV checker; Alt+drag to orbit, middle-drag to pan,
  wheel to zoom, F to frame, M / C / B to cycle views.

## Built for agents

Everything you can do in the window, an agent can do too:

- **CLI:** `texturecraft-cli new builtin:material_ball out.tcraft`, `bake`, `export`, `render`,
  `run <command> <params>`, `commands`.
- **MCP server:** `texturecraft-cli mcp` gives Claude and other agents tools to create projects,
  add layers and masks, paint strokes, bake, export, render previews and inspect everything they
  changed.
- **Control channel:** launch the app with `--control <port>` to drive the running UI: run any
  command, invoke menu items, paint, and take screenshots.

See [`docs/control-protocol.md`](docs/control-protocol.md) for the full protocol and examples.

## Get started

You need a recent stable Rust toolchain (1.95 or newer).

```sh
git clone https://github.com/storytold/texturecraft
cd texturecraft
cargo run --release -p texturecraft
```

The app opens with a baked sample project. **File ▸ New project** starts from one of the built-in
meshes or your own glTF, GLB, OBJ, PLY or STL file (UVs are generated if the mesh has none).

Run the tests with `cargo test --workspace --release`. The architecture is described in
[`docs/architecture.md`](docs/architecture.md), the plan and parity estimate in
[`ROADMAP.md`](ROADMAP.md), and contributor rules in [`AGENTS.md`](AGENTS.md).

## Downloads

The easiest way to install TextureCraft is from its page on the ArtCraft website:
**[getartcraft.com/apps/texturecraft](https://getartcraft.com/apps/texturecraft)**.

Technical users can grab builds from GitHub: the
[latest release](https://github.com/storytold/texturecraft/releases/latest) or
[all releases](https://github.com/storytold/texturecraft/releases).

| Windows | File |
|---|---|
| Installer (x64, x86, arm64) | `texturecraft-<version>-windows-<arch>.msi` |
| Portable (x64, x86, arm64) | `texturecraft-<version>-windows-<arch>-portable.zip` |

| macOS | File |
|---|---|
| Universal app (Apple silicon and Intel), signed and notarized | `texturecraft-<version>-macos-universal.dmg` |
| Command-line tools | `texturecraft-cli-<version>-macos-universal.zip` |

| Linux (x86_64, aarch64) | File |
|---|---|
| AppImage (any distribution) | `texturecraft-<version>-linux-<arch>.AppImage` |
| Debian, Ubuntu, Mint, Pop!_OS | `texturecraft-<version>-linux-<arch>.deb` |
| Fedora, openSUSE, RHEL | `texturecraft-<version>-linux-<arch>.rpm` |
| Flatpak bundle | `texturecraft-<version>-linux-<arch>.flatpak` |
| Plain archive | `texturecraft-<version>-linux-<arch>.tar.gz` |

| FreeBSD | File |
|---|---|
| x86_64 | `texturecraft-<version>-freebsd-x86_64.tar.gz` |

| Web (WebAssembly) | File |
|---|---|
| Static site to host or embed | `texturecraft-web-<version>.zip` |

## The Crafting Apps

TextureCraft is one of the **Crafting Apps**: free, open-source creative tools from the
[ArtCraft](https://getartcraft.com/) team, each written from scratch in Rust and each able to
stand on its own.

| | App | What it's for | Code | Learn more |
|:-:|---|---|---|---|
| <img src="https://raw.githubusercontent.com/storytold/texturecraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.texturecraft.png" alt="" width="32" height="32"> | **TextureCraft** | **3D texture painting · you are here** | [GitHub](https://github.com/storytold/texturecraft) | [Website](https://getartcraft.com/apps/texturecraft) |
| <img src="https://raw.githubusercontent.com/storytold/photocraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.photocraft.png" alt="" width="32" height="32"> | **PhotoCraft** | Image editing: layers, masks, type and real PSD files | [GitHub](https://github.com/storytold/photocraft) | [Website](https://getartcraft.com/apps/photocraft) |
| <img src="https://raw.githubusercontent.com/storytold/vectorcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.vectorcraft.png" alt="" width="32" height="32"> | **VectorCraft** | Vector illustration | [GitHub](https://github.com/storytold/vectorcraft) | [Website](https://getartcraft.com/apps/vectorcraft) |
| <img src="https://raw.githubusercontent.com/storytold/filmcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.filmcraft.png" alt="" width="32" height="32"> | **FilmCraft** | Video editing, color and sound | [GitHub](https://github.com/storytold/filmcraft) | [Website](https://getartcraft.com/apps/filmcraft) |
| <img src="https://raw.githubusercontent.com/storytold/lightcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.lightcraft.png" alt="" width="32" height="32"> | **LightCraft** | Photo library and raw development | [GitHub](https://github.com/storytold/lightcraft) | [Website](https://getartcraft.com/apps/lightcraft) |
| <img src="https://raw.githubusercontent.com/storytold/pdfcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.pdfcraft.png" alt="" width="32" height="32"> | **PdfCraft** | Reading, organizing and protecting PDFs | [GitHub](https://github.com/storytold/pdfcraft) | [Website](https://getartcraft.com/apps/pdfcraft) |
| <img src="https://raw.githubusercontent.com/storytold/effectcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.effectcraft.png" alt="" width="32" height="32"> | **EffectCraft** | Motion graphics and visual effects | [GitHub](https://github.com/storytold/effectcraft) | [Website](https://getartcraft.com/apps/effectcraft) |
| <img src="https://raw.githubusercontent.com/storytold/designcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.designcraft.png" alt="" width="32" height="32"> | **DesignCraft** | Page layout and publishing | [GitHub](https://github.com/storytold/designcraft) | [Website](https://getartcraft.com/apps/designcraft) |
| <img src="https://raw.githubusercontent.com/storytold/gridcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.gridcraft.png" alt="" width="32" height="32"> | **GridCraft** | Spreadsheets and data | [GitHub](https://github.com/storytold/gridcraft) | [Website](https://getartcraft.com/apps/gridcraft) |

And [**ArtCraft**](https://getartcraft.com/) itself, our AI image and video studio for artists who want real control.

<br>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Join the ArtCraft community on Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<h3 align="center">Come make things with us</h3>

<p align="center">
  Our Discord is where artists of every kind hang out: people who paint, sculpt, model, shoot, draw,
  cut film, set type, and people still figuring out what they like to make. Share what you're
  working on, ask for help, tell us what's broken, or tell us what you wish these tools could do.
  Whatever your medium and however long you've been at it, you're welcome here.
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><b>discord.gg/artcraft</b></a> ·
  <a href="https://getartcraft.com/">getartcraft.com</a> ·
  <a href="https://getartcraft.com/apps">The Crafting Apps</a> ·
  <a href="https://getartcraft.com/apps/texturecraft">TextureCraft</a>
</p>

## License and credits

TextureCraft is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Copyright (c) 2026 ArtCraft Team and the TextureCraft contributors. Required notices are in [NOTICE](NOTICE).

Bundled fonts, icons, images and other assets keep their own open licenses; each one is listed
with its author, source and license in [ATTRIBUTION.md](ATTRIBUTION.md).

Every material, environment, brush alpha and mesh shown is generated procedurally by TextureCraft
itself; no third-party textures or models appear in the screenshots.

The ArtCraft name, wordmark and logos in [`docs/brand/`](docs/brand/) are trademarks of the
ArtCraft Team and are not covered by this license. They may be used only unmodified, and only as
part of this repository and TextureCraft, under [`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt).
Forks and modified versions must remove them.

<sub>Adobe, Substance 3D Painter, Photoshop, Illustrator, Premiere Pro, Lightroom, Acrobat, After Effects and InDesign are trademarks or registered trademarks of Adobe Inc. in the United States and/or other countries. TextureCraft is an independent, open-source project and is not affiliated with, sponsored by or endorsed by Adobe Inc.; these names are used only to describe the workflows it is compatible with.</sub>

<p align="center">
  <a href="https://getartcraft.com/"><img alt="ArtCraft" src="docs/brand/artcraft-mark.svg" width="28"></a><br>
  <sub>Made by the <a href="https://getartcraft.com/">ArtCraft</a> team and community.</sub>
</p>
