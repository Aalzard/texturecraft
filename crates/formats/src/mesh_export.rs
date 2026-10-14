//! Mesh export of the working mesh: Wavefront OBJ (positions, UVs, normals, one group per
//! submesh) and binary glTF (one primitive per submesh, a metallic-roughness material per
//! material with its textures embedded).

use std::fmt::Write as _;

use serde_json::{Map, Value, json};
use texturecraft_core::NormalFormat;
use texturecraft_geom::glam::Vec3;
use texturecraft_geom::{Mesh, SubMesh};

use crate::presets::{GLTF_BASE_COLOR, GLTF_EMISSIVE, GLTF_NORMAL, GLTF_ORM, gltf_pbr_maps};
use crate::{ExportOptions, ExportPreset, ExportedFile, FileFormat, FormatError, Result};

/// The mesh as OBJ text (UV v flipped back to OBJ's bottom-up convention).
pub fn obj_bytes(mesh: &Mesh, name: &str) -> Vec<u8> {
    let mut s = String::with_capacity(mesh.positions.len() * 96 + mesh.indices.len() * 12);
    let _ = writeln!(s, "# {name}, exported by TextureCraft\no {}", token(name));
    for p in &mesh.positions {
        let _ = writeln!(s, "v {} {} {}", p.x, p.y, p.z);
    }
    let uvs = mesh.uvs.len() == mesh.positions.len();
    let normals = mesh.normals.len() == mesh.positions.len();
    if uvs {
        for t in &mesh.uvs {
            let _ = writeln!(s, "vt {} {}", t.x, 1.0 - t.y);
        }
    }
    if normals {
        for n in &mesh.normals {
            let _ = writeln!(s, "vn {} {} {}", n.x, n.y, n.z);
        }
    }
    for sm in &parts(mesh, name) {
        let material = mesh.materials.get(sm.material).map_or("default", String::as_str);
        let _ = writeln!(s, "g {}\nusemtl {}", token(&sm.name), token(material));
        let (a, b) = (sm.start as usize, sm.start as usize + sm.count as usize);
        for tri in mesh.indices.get(a..b).unwrap_or_default().chunks_exact(3) {
            s.push('f');
            for i in tri {
                let i = i + 1;
                let _ = match (uvs, normals) {
                    (true, true) => write!(s, " {i}/{i}/{i}"),
                    (true, false) => write!(s, " {i}/{i}"),
                    (false, true) => write!(s, " {i}//{i}"),
                    (false, false) => write!(s, " {i}"),
                };
            }
            s.push('\n');
        }
    }
    s.into_bytes()
}

/// The submeshes, or one covering the whole mesh when it has none.
fn parts(mesh: &Mesh, name: &str) -> Vec<SubMesh> {
    if mesh.submeshes.is_empty() {
        vec![SubMesh { name: name.into(), material: 0, start: 0, count: mesh.indices.len() as u32 }]
    } else {
        mesh.submeshes.clone()
    }
}

/// Textures of one glTF material (PNG bytes), from the files of [`glb_export_options`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlbTextures {
    pub base_color: Option<Vec<u8>>,
    /// Occlusion (R), roughness (G), metallic (B).
    pub orm: Option<Vec<u8>>,
    /// OpenGL convention (+Y up the image), as glTF expects.
    pub normal: Option<Vec<u8>>,
    pub emissive: Option<Vec<u8>>,
}

impl GlbTextures {
    /// Sort rendered files into slots; an all-black emissive map is dropped.
    pub fn from_files(files: Vec<ExportedFile>) -> GlbTextures {
        let mut t = GlbTextures::default();
        for f in files {
            let slot = match f.name.strip_suffix(".png").unwrap_or_default() {
                GLTF_BASE_COLOR => &mut t.base_color,
                GLTF_ORM => &mut t.orm,
                GLTF_NORMAL => &mut t.normal,
                GLTF_EMISSIVE if !is_black(&f.bytes) => &mut t.emissive,
                _ => continue,
            };
            *slot = Some(f.bytes);
        }
        t
    }
}

/// Texture export options for [`GlbTextures`]: the glTF maps as 8-bit PNG named by slot.
pub fn glb_export_options() -> ExportOptions {
    let mut o = ExportOptions::new(ExportPreset { name: "glTF binary".into(), maps: gltf_pbr_maps(str::to_string) });
    o.format_override = Some(FileFormat::Png);
    o.bit_depth_override = Some(8);
    o.normal_format = Some(NormalFormat::OpenGl);
    o
}

fn is_black(png: &[u8]) -> bool {
    image::load_from_memory_with_format(png, image::ImageFormat::Png).map_or(true, |img| img.to_rgb8().iter().all(|v| *v == 0))
}

/// PNG color type 4 (gray + alpha) or 6 (RGBA), from the IHDR chunk.
fn png_has_alpha(png: &[u8]) -> bool {
    matches!(png.get(25), Some(4 | 6))
}

/// The binary chunk and the JSON arrays that index into it.
#[derive(Default)]
struct GlbBuilder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    images: Vec<Value>,
    textures: Vec<Value>,
}

impl GlbBuilder {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = t.into();
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(&mut self, values: impl Iterator<Item = [u8; 4]>, target: u32, component: u32, kind: &str, count: usize) -> Value {
        let bytes: Vec<u8> = values.flatten().collect();
        let view = self.view(&bytes, Some(target));
        self.accessors.push(json!({ "bufferView": view, "componentType": component, "count": count, "type": kind }));
        Value::from(self.accessors.len() - 1)
    }

    /// A float vertex attribute of `kind` (VEC2 / VEC3) over `n` vertices.
    fn attribute(&mut self, values: impl Iterator<Item = f32>, kind: &str, n: usize) -> Value {
        self.accessor(values.map(f32::to_le_bytes), 34962, 5126, kind, n)
    }

    /// `{"index": n}` of a new texture holding `png`.
    fn texture(&mut self, png: &[u8]) -> Value {
        let view = self.view(png, None);
        self.images.push(json!({ "bufferView": view, "mimeType": "image/png" }));
        self.textures.push(json!({ "source": self.images.len() - 1, "sampler": 0 }));
        json!({ "index": self.textures.len() - 1 })
    }

    fn material(&mut self, name: &str, t: &GlbTextures) -> Value {
        let (mut m, mut pbr) = (Map::new(), Map::new());
        m.insert("name".into(), name.into());
        if let Some(png) = &t.base_color {
            pbr.insert("baseColorTexture".into(), self.texture(png));
            if png_has_alpha(png) {
                m.insert("alphaMode".into(), "BLEND".into());
            }
        }
        match &t.orm {
            Some(png) => {
                let tex = self.texture(png);
                pbr.insert("metallicRoughnessTexture".into(), tex.clone());
                m.insert("occlusionTexture".into(), tex);
            }
            None => {
                pbr.insert("metallicFactor".into(), 0.0.into());
            }
        }
        if let Some(png) = &t.normal {
            m.insert("normalTexture".into(), self.texture(png));
        }
        if let Some(png) = &t.emissive {
            m.insert("emissiveTexture".into(), self.texture(png));
            m.insert("emissiveFactor".into(), json!([1.0, 1.0, 1.0]));
        }
        m.insert("pbrMetallicRoughness".into(), pbr.into());
        m.into()
    }
}

/// The mesh as binary glTF (GLB) named `name`, with `textures[i]` on material `i` (materials
/// without an entry get a plain one). Vertex data is written as is: the mesh already follows
/// glTF's conventions.
pub fn glb_bytes(mesh: &Mesh, name: &str, textures: &[GlbTextures]) -> Result<Vec<u8>> {
    let n = mesh.positions.len();
    if n == 0 || mesh.indices.len() < 3 {
        return Err(FormatError::Invalid("the mesh is empty".into()));
    }
    let mut g = GlbBuilder::default();
    let vec3s = |v: &[Vec3]| v.iter().flat_map(|p| p.to_array()).collect::<Vec<f32>>();
    let mut attributes = Map::new();
    let position = g.attribute(vec3s(&mesh.positions).into_iter(), "VEC3", n);
    let (lo, hi) = mesh.bounds();
    if let Some(a) = g.accessors.last_mut() {
        a["min"] = json!(lo.to_array());
        a["max"] = json!(hi.to_array());
    }
    attributes.insert("POSITION".into(), position);
    if mesh.normals.len() == n {
        attributes.insert("NORMAL".into(), g.attribute(vec3s(&mesh.normals).into_iter(), "VEC3", n));
    }
    if mesh.uvs.len() == n {
        attributes.insert("TEXCOORD_0".into(), g.attribute(mesh.uvs.iter().flat_map(|t| t.to_array()), "VEC2", n));
    }
    let mut primitives = Vec::new();
    for sm in parts(mesh, name) {
        let tris = mesh.indices.get(sm.start as usize..(sm.start as usize).saturating_add(sm.count as usize)).unwrap_or_default();
        let tris = tris.get(..tris.len() / 3 * 3).unwrap_or_default();
        if tris.is_empty() {
            continue;
        }
        let indices = g.accessor(tris.iter().map(|i| i.to_le_bytes()), 34963, 5125, "SCALAR", tris.len());
        let mut p = json!({ "attributes": attributes, "indices": indices });
        if sm.material < mesh.materials.len() {
            p["material"] = sm.material.into();
        }
        primitives.push(p);
    }
    let none = GlbTextures::default();
    let materials: Vec<Value> = mesh.materials.iter().enumerate().map(|(i, m)| g.material(m, textures.get(i).unwrap_or(&none))).collect();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "TextureCraft" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0, "name": name }],
        "meshes": [{ "name": name, "primitives": primitives }],
        "buffers": [{ "byteLength": g.bin.len() }],
        "bufferViews": g.views,
        "accessors": g.accessors,
    });
    if !g.textures.is_empty() {
        doc["samplers"] = json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
    }
    // glTF forbids empty arrays.
    for (key, list) in [("materials", materials), ("images", g.images), ("textures", g.textures)] {
        if !list.is_empty() {
            doc[key] = list.into();
        }
    }
    let mut js = serde_json::to_vec(&doc).map_err(|e| FormatError::Invalid(format!("glTF JSON: {e}")))?;
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let mut bin = g.bin;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + js.len() + 8 + bin.len();
    let len32 = |v: usize| u32::try_from(v).map(u32::to_le_bytes).map_err(|_| FormatError::TooLarge(format!("the GLB would be {total} bytes (limit 4 GiB)")));
    let mut out = Vec::with_capacity(total);
    for chunk in [&b"glTF"[..], &2u32.to_le_bytes(), &len32(total)?, &len32(js.len())?, b"JSON", &js, &len32(bin.len())?, b"BIN\0", &bin] {
        out.extend_from_slice(chunk);
    }
    Ok(out)
}

/// OBJ names end at whitespace.
fn token(name: &str) -> String {
    let t: String = name.trim().chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
    if t.is_empty() { "unnamed".into() } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_round_trips_through_the_importer() {
        let mesh = texturecraft_geom::primitives::rounded_box(texturecraft_geom::glam::Vec3::splat(1.0), 0.1);
        let bytes = obj_bytes(&mesh, "My box");
        let back = crate::import_mesh_bytes("box.obj", &bytes).unwrap().mesh;
        assert_eq!(back.triangle_count(), mesh.triangle_count());
        let (a, b) = (mesh.bounds(), back.bounds());
        assert!((a.0 - b.0).length() < 1e-4 && (a.1 - b.1).length() < 1e-4);
        let uv_sum = |m: &Mesh| m.indices.iter().map(|&i| m.uvs[i as usize].y as f64).sum::<f64>();
        assert!((uv_sum(&mesh) - uv_sum(&back)).abs() < 1e-3 * mesh.indices.len() as f64);
        assert!(String::from_utf8(bytes).unwrap().contains("o My_box"));
    }
}
