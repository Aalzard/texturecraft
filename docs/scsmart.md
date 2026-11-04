# SubCraft smart materials (`.scsmart`)

`.scsmart` is the smart-material file of [SubCraft](https://github.com/Aalzard/subcraft), an
open-source sibling of TextureCraft (MIT OR Apache-2.0, same author). TextureCraft **reads** it
(File › Import SubCraft smart material, File › Import material, a drop on the window, the import
button of *My smart materials*, `material.importScsmart`, the `material_load` tool) and maps it to
its own layers. It does not write it. The reader is original code written from SubCraft's public
file format (`crates/formats/src/scsmart/`); no SubCraft code or assets are copied, and the test
fixtures are generated in code from the format description (`scsmart::fixture`).

## The format

A zip container ("fragment" in SubCraft's terms):

| Entry | Content |
|---|---|
| `fragment.json` | `format` = `subcraft-fragment`, `schema_version`, `writer`, `kind` (`smart_material` or `smart_mask`), `name`, `resolution` (128–4096), `layers` (a tree, bottom to top), `resources` (table of embedded images) |
| `rasters/NNNNN.tiles` | sparse 64×64 half-float tiles (`SCTILE` blob): paint channels (values then alpha, straight), mask bases (one value), mask paint effects (value, alpha) |
| `resources/<id>.<ext>` | the original bytes of images used by fills and image masks |

**Versions.** `schema_version` counts up with every incompatible change (SubCraft 16 today).
Older schemas load unchanged (what a later schema added is simply absent). A **newer** schema
than this build reads (16) is refused with a message naming both versions, as SubCraft itself
does; schema 0 or a missing/non-numeric version is a damaged file. A `smart_mask` (`.scmask`) is
refused: TextureCraft's smart masks are presets, not files.

## What maps

| SubCraft | TextureCraft |
|---|---|
| Folder | Group (expanded state kept) |
| Paint layer | Paint layer; pixels converted from straight to premultiplied, resampled to the texture set on add. Editable pen paths and text become plain paint (note) |
| Fill layer | Fill layer; per channel: uniform → uniform, image → image resource (OpenGL normals), procedural → pattern, `enabled: false` → channel not written |
| Layer blend (per channel mode and opacity), visibility, name | the same (the 32 blend modes are the same set) |
| Procedural kinds | clouds, cells, grunge, scratches, brushed, checker, gradient U, streaks, pores map directly; wood rings → wood, gradient V → gradient, dirt → grunge, cracks → cells, speckles → fine noise, scratch generator → scratches (approximated). *Balance/contrast* follow SubCraft's remap exactly; patterns are TextureCraft's own, so similar, not pixel-identical (note) |
| Fill strength | folded into uniform and procedural values; not applied to images or normal tilt (approximated) |
| Projection | UV → UV; triplanar / planar → TextureCraft's (one scale, no rotation: approximated when they differ) |
| Mask (white/black base, painted base) | Mask with that background; painted base tiles become a Paint effect at the bottom |
| Mask effects | Paint, Fill (uniform or procedural), Levels, Invert, Image (fill from the image; its `invert` becomes an Invert effect above), ID color (selection; softness dropped) |
| Generators | edges → edge wear, cavity → dirt, directional → light/direction, position gradient → position (axis kept), curvature / AO / thickness mesh maps → curvature / AO / thin areas, concave → curvature inverted (approximated). Level, softness, invert, breakup amount/scale/seed kept; edge spread and organic breakup dropped (approximated) |
| Filters | blur, directional blur, threshold, clamp, invert, grayscale, HSL, posterize, color balance, replace color, histogram shift, mosaic, gradient map, normal Y flip map; sharpen, highpass, bevel, edge detect, glow, warp, directional warp, contrast, histogram scan/range map with some parameters dropped (approximated). Radii are in texels of the file's resolution and are rescaled to TextureCraft's 2K reference |
| Content effects (paint, fill) | content effects; one blend mode and opacity per effect (first channel's; approximated when channels differ) |
| Content filters and levels | only on a layer that writes just that channel (TextureCraft applies content effects to every channel); otherwise reported as not imported |
| Embedded images | image resources named `<name>-<hash>.<ext>`, so different images never collide and the same image is shared |

## What does not map (reported, never fatal)

| SubCraft | Why |
|---|---|
| `user0`–`user3` channels | no TextureCraft counterpart |
| Mesh-map fill sources (`mesh_map`) | TextureCraft fills cannot read baked maps |
| Median, Mask Outline filters; X/Z normal flips | no equivalent |
| Generators on normal, world normal, ID, position, height, bent normal maps | TextureCraft generators read curvature, AO and thickness |
| Layer mesh-part restriction (`parts`) | indices, where TextureCraft limits by part name; the layer shows on all parts |
| Unknown blend modes, layer/effect/filter/generator/source kinds | reported (a blend mode falls back to Normal) |
| Folders nested more than 12 deep | cut |

## Limits (hostile files)

Nothing is extracted to disk. A file must be ≤ 1 GB, hold ≤ 4096 entries, ≤ 2048 layers, ≤ 256
images and ≤ 256 effects per mask or layer; one entry inflates to ≤ 512 MB, and all inflated
entries and decoded pixels together stay ≤ 2 GB (512 MB in the browser). Raster blobs are bounds-checked before
allocation (size must equal the file's resolution, tile indices must be unique and inside the
image, non-finite values read as 0), and images are checked from their headers. The report keeps
300 issues (equal ones merge). These cases are errors: an unreadable container, a missing or
damaged raster (the error names the entry), a limit exceeded.
