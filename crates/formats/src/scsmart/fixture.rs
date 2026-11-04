//! Builds `.scsmart` files from the public SubCraft file format description, without any SubCraft
//! code: the fixtures of the importer's tests (here and in the engine), and the sample the docs
//! describe. Not a writer for real files: it emits exactly what it is given.

use std::io::{Cursor, Write};

use serde_json::{Value, json};

/// Texel function of a fixture tile: the texel index inside the tile (0..4096) to the component values.
pub type TexelFn<'a> = &'a dyn Fn(usize) -> Vec<f32>;

/// A sparse raster blob (`SCTILE`): `tiles` are `(tile index, texel function)`; the function gets
/// the texel index inside the tile (0..4096) and returns the `comps` component values.
pub fn raster(width: u32, comps: usize, fill: [f32; 4], tiles: &[(u32, TexelFn)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"SCTILE");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.push(comps as u8);
    for f in fill {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out.extend_from_slice(&(tiles.len() as u32).to_le_bytes());
    for (index, texel) in tiles {
        out.extend_from_slice(&index.to_le_bytes());
        for t in 0..64 * 64 {
            let v = texel(t);
            for c in 0..comps {
                out.extend_from_slice(&half::f16::from_f32(v.get(c).copied().unwrap_or(0.0)).to_bits().to_le_bytes());
            }
        }
    }
    out
}

/// A raster with nothing painted: every texel is `fill`.
pub fn blank(width: u32, comps: usize, fill: [f32; 4]) -> Vec<u8> {
    raster(width, comps, fill, &[])
}

/// A small PNG (`w`×`h`, a gradient) for fills and image masks.
pub fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 128, 255]));
    let mut out = Cursor::new(Vec::new());
    if img.write_to(&mut out, image::ImageFormat::Png).is_err() {
        return Vec::new();
    }
    out.into_inner()
}

/// A `.scsmart` container under construction.
pub struct Builder {
    /// The `fragment.json` document; edit it freely.
    pub json: Value,
    entries: Vec<(String, Vec<u8>)>,
}

impl Builder {
    /// A smart material at `resolution` with the current schema and no layers.
    pub fn new(name: &str, resolution: u32) -> Builder {
        Builder {
            json: json!({
                "format": "subcraft-fragment", "schema_version": 16, "writer": "SubCraft 0.0.0-fixture",
                "kind": "smart_material", "name": name, "resolution": resolution, "layers": [], "mask_effects": [], "resources": []
            }),
            entries: Vec::new(),
        }
    }

    pub fn layers(mut self, layers: Value) -> Builder {
        self.json["layers"] = layers;
        self
    }

    /// Add a container entry (a raster blob or an image).
    pub fn entry(mut self, name: &str, bytes: Vec<u8>) -> Builder {
        self.entries.push((name.to_string(), bytes));
        self
    }

    /// Add an image to the resource table and the container.
    pub fn image(mut self, id: &str, name: &str, bytes: Vec<u8>) -> Builder {
        let file = format!("resources/{id}.png");
        if let Some(list) = self.json["resources"].as_array_mut() {
            list.push(json!({"id": id, "name": name, "kind": "texture", "file": file}));
        }
        self.entries.push((file, bytes));
        self
    }

    /// The container's bytes.
    pub fn build(&self) -> Vec<u8> {
        self.build_with(&self.json.to_string())
    }

    /// The container with `fragment.json` replaced by raw text (for malformed documents).
    pub fn build_with(&self, fragment_json: &str) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut put = |name: &str, bytes: &[u8]| {
            if z.start_file(name, opts).is_ok() {
                let _ = z.write_all(bytes);
            }
        };
        put("fragment.json", fragment_json.as_bytes());
        for (name, bytes) in &self.entries {
            put(name, bytes);
        }
        z.finish().map(Cursor::into_inner).unwrap_or_default()
    }
}

fn blends(channels: &[(&str, &str, f64)]) -> Value {
    Value::Array(channels.iter().map(|(c, m, o)| json!({"channel": c, "mode": m, "opacity": o})).collect())
}

/// The sample smart material "Painted Metal" (128×128): a folder with a steel fill (uniform,
/// procedural and image sources, a mesh-map source TextureCraft cannot map, a channel it has no
/// counterpart for), a scratches paint layer with a painted roughness tile and a mask (painted
/// base, edge generator, image, median filter, blur, levels), and a rust fill with a blend mode
/// no TextureCraft mode matches and a content filter.
pub fn sample() -> Vec<u8> {
    let tile = |_: usize| vec![0.9, 1.0]; // roughness 0.9, fully covered
    let hole = |_: usize| vec![0.0]; // a hole in the mask base
    let uv = json!({"mode": "uv", "scale": [1.0, 1.0], "rotation_deg": 0.0, "offset": [0.0, 0.0], "triplanar_sharpness": 8.0, "planar_axis": 2});
    let steel = json!({
        "id": "00000000-0000-0000-0000-000000000002", "name": "Steel", "visible": true,
        "blend": blends(&[("base_color", "normal", 1.0), ("metallic", "normal", 1.0), ("roughness", "normal", 1.0), ("normal", "normal", 1.0), ("height", "normal", 0.5), ("user0", "normal", 1.0)]),
        "content": {"type": "fill",
            "projection": {"mode": "triplanar", "scale": [2.0, 2.0], "rotation_deg": 0.0, "offset": [0.0, 0.0], "triplanar_sharpness": 8.0, "planar_axis": 2},
            "channels": [
                {"channel": "base_color", "enabled": true, "source": {"type": "uniform", "value": [0.6, 0.6, 0.65]}, "strength": 1.0},
                {"channel": "metallic", "enabled": true, "source": {"type": "uniform", "value": [1.0, 1.0, 1.0]}},
                {"channel": "roughness", "enabled": true, "source": {"type": "procedural",
                    "params": {"kind": "clouds", "scale": 6.0, "seed": 3, "balance": 0.5, "contrast": 0.0, "invert": false},
                    "low": [0.3, 0.3, 0.3], "high": [0.6, 0.6, 0.6]}},
                {"channel": "normal", "enabled": true, "source": {"type": "bitmap", "resource": "img-1"}},
                {"channel": "height", "enabled": true, "source": {"type": "mesh_map", "map": "curvature"}}
            ]}
    });
    let effect = |id: &str, name: &str, blend: &str, opacity: f64, kind: Value| json!({"id": id, "name": name, "visible": true, "blend": blend, "opacity": opacity, "kind": kind});
    let noise = json!({"kind": "clouds", "scale": 8.0, "seed": 5, "balance": 0.5, "contrast": 0.0, "invert": false});
    let mask_effects = json!([
        effect(
            "e1",
            "Edges",
            "normal",
            1.0,
            json!({"type": "generator", "generator": {"kind": "edges", "level": 0.4, "softness": 0.2, "invert": false, "noise_amount": 0.3, "noise": noise, "direction": [0.0, 1.0, 0.0]}})
        ),
        effect("e2", "Stencil", "multiply", 0.5, json!({"type": "image", "resource": "img-2", "projection": uv, "invert": true})),
        effect("e3", "Median", "normal", 1.0, json!({"type": "filter", "filter": {"kind": "median", "radius": 2.0}})),
        effect("e4", "Soften", "normal", 1.0, json!({"type": "filter", "filter": {"kind": "blur", "radius": 4.0}})),
        effect(
            "e5",
            "Levels",
            "normal",
            1.0,
            json!({"type": "levels", "levels": {"in_low": 0.1, "in_high": 0.9, "gamma": 1.2, "out_low": 0.0, "out_high": 1.0}})
        ),
    ]);
    let scratches = json!({
        "id": "00000000-0000-0000-0000-000000000003", "name": "Scratches", "visible": true,
        "blend": blends(&[("roughness", "multiply", 0.8)]),
        "mask": {"enabled": true, "base": "rasters/00001.tiles", "effects": mask_effects},
        "content": {"type": "paint", "channels": [{"channel": "roughness", "raster": "rasters/00000.tiles"}]}
    });
    let rust = json!({
        "id": "00000000-0000-0000-0000-000000000004", "name": "Rust", "visible": false,
        "blend": blends(&[("base_color", "wormhole", 0.7)]),
        "content": {"type": "fill", "projection": {"mode": "uv", "scale": [3.0, 3.0], "rotation_deg": 15.0, "offset": [0.1, 0.2], "triplanar_sharpness": 8.0, "planar_axis": 2},
            "channels": [{"channel": "base_color", "enabled": true, "source": {"type": "uniform", "value": [0.45, 0.2, 0.1]}, "strength": 0.5}]},
        "content_effects": [{"id": "00000000-0000-0000-0000-000000000020", "name": "Warm", "visible": true, "blend": [],
            "kind": {"type": "filter", "channel": "base_color", "filter": {"kind": "hsl", "hue": 18.0, "saturation": 1.5, "lightness": 0.1}}}]
    });
    let folder = json!({
        "id": "00000000-0000-0000-0000-000000000001", "name": "Painted Metal", "visible": true,
        "blend": blends(&[("base_color", "passthrough", 1.0), ("roughness", "passthrough", 1.0), ("metallic", "passthrough", 1.0)]),
        "content": {"type": "folder", "expanded": true, "children": [steel, scratches, rust]}
    });
    Builder::new("Painted Metal", 128)
        .image("img-1", "grain.png", png(4, 4))
        .image("img-2", "stencil.png", png(8, 8))
        .entry("rasters/00000.tiles", raster(128, 2, [0.0; 4], &[(1, &tile)]))
        .entry("rasters/00001.tiles", raster(128, 1, [1.0; 4], &[(3, &hole)]))
        .layers(json!([folder]))
        .build()
}
