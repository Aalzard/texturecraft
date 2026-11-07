//! Maps a SubCraft fragment document to TextureCraft layers, masks, effects and pixels.
//!
//! The polymorphic parts of `fragment.json` (layer content, fill sources, mask effects, filters)
//! are read leniently from [`serde_json::Value`]: a kind or parameter this reader has never heard
//! of becomes an [`Issue`](super::Issue) instead of failing the whole file.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};
use std::sync::Arc;

use serde_json::Value;
use texturecraft_compose::PixelStore;
use texturecraft_core::project::MAX_DEPTH;
use texturecraft_core::{
    BlendMode, Channel, ChannelBlend, ChannelFormat, ChannelKind, Effect, EffectKind, Fill, Filter, Generator, GeneratorKind, Layer, LayerId, LayerKind,
    Levels, Mask, MaskBackground, NormalFormat, Procedural, ProceduralKind, Project, Projection, Source, TextureSet,
};
use texturecraft_texel::{TileImage, to_u16, to_u16_signed};

use super::raster::{self, Raster, TILE_AREA, half_to_f32};
use super::{Budget, ImportReport, MAX_ENTRY_BYTES, MAX_LAYERS, MAX_RESOURCES, Resources, ScsmartImport, Severity, clean_name, malformed, read_entry};
use crate::{FormatError, ProjectFile, Result};

/// Texture set resolutions SubCraft writes.
pub(super) const RESOLUTIONS: [u32; 6] = [128, 256, 512, 1024, 2048, 4096];
/// Folder nesting kept (TextureCraft allows [`MAX_DEPTH`] in all, and the import lands inside
/// whatever folder the user has selected).
const MAX_NEST: usize = MAX_DEPTH - 4;
/// Effects on one layer or mask.
const MAX_EFFECTS: usize = 256;
/// Texels TextureCraft's filter radii are measured at.
const REF_RES: f32 = 2048.0;

// ---------------------------------------------------------------------------------------------
// Lenient JSON access.

fn num(v: &Value, key: &str) -> Option<f32> {
    v.get(key)?.as_f64().filter(|x| x.is_finite()).map(|x| x as f32)
}
fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str()
}
fn flag(v: &Value, key: &str) -> Option<bool> {
    v.get(key)?.as_bool()
}
/// A JSON array of at least `N` finite numbers.
fn array<const N: usize>(v: &Value) -> Option<[f32; N]> {
    let a = v.as_array()?;
    let mut out = [0.0f32; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = a.get(i)?.as_f64().filter(|x| x.is_finite())? as f32;
    }
    Some(out)
}
fn floats<const N: usize>(v: &Value, key: &str) -> Option<[f32; N]> {
    array(v.get(key)?)
}
fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}
fn unit(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}
/// A fraction from the file: finite, in 0..1, `default` otherwise.
fn opacity(v: &Value, key: &str) -> f32 {
    num(v, key).map_or(1.0, unit)
}

// ---------------------------------------------------------------------------------------------
// Vocabulary tables.

/// SubCraft channel id to TextureCraft channel. `user0`..`user3` have no counterpart.
fn channel(name: &str) -> Option<Channel> {
    Some(match name {
        "base_color" => Channel::BaseColor,
        "height" => Channel::Height,
        "roughness" => Channel::Roughness,
        "metallic" => Channel::Metallic,
        "normal" => Channel::Normal,
        "ambient_occlusion" => Channel::AmbientOcclusion,
        "emissive" => Channel::Emissive,
        "opacity" => Channel::Opacity,
        "specular" => Channel::SpecularLevel,
        _ => return None,
    })
}

/// SubCraft blend mode id to TextureCraft blend mode.
fn blend_mode(name: &str) -> Option<BlendMode> {
    Some(match name {
        "normal" => BlendMode::Normal,
        "passthrough" => BlendMode::Passthrough,
        "disable" => BlendMode::Disable,
        "replace" => BlendMode::Replace,
        "multiply" => BlendMode::Multiply,
        "divide" => BlendMode::Divide,
        "inverse_divide" => BlendMode::InverseDivide,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "linear_dodge" => BlendMode::LinearDodge,
        "subtract" => BlendMode::Subtract,
        "inverse_subtract" => BlendMode::InverseSubtract,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "signed_addition" => BlendMode::SignedAddition,
        "overlay" => BlendMode::Overlay,
        "screen" => BlendMode::Screen,
        "linear_burn" => BlendMode::LinearBurn,
        "color_burn" => BlendMode::ColorBurn,
        "color_dodge" => BlendMode::ColorDodge,
        "soft_light" => BlendMode::SoftLight,
        "hard_light" => BlendMode::HardLight,
        "vivid_light" => BlendMode::VividLight,
        "linear_light" => BlendMode::LinearLight,
        "pin_light" => BlendMode::PinLight,
        "tint" => BlendMode::Tint,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "value" => BlendMode::Value,
        "normal_map_combine" => BlendMode::NormalMapCombine,
        "normal_map_detail" => BlendMode::NormalMapDetail,
        "normal_map_inverse_detail" => BlendMode::NormalMapInverseDetail,
        _ => return None,
    })
}

/// The closest TextureCraft pattern for a SubCraft procedural kind; the flag says the pattern
/// differs in character (not just in noise) from the original.
fn pattern_kind(name: &str) -> Option<(ProceduralKind, bool)> {
    Some(match name {
        "clouds" => (ProceduralKind::Clouds, false),
        "cells" => (ProceduralKind::Cells, false),
        "grunge" => (ProceduralKind::Grunge, false),
        "scratches" => (ProceduralKind::Scratches, false),
        "wood_rings" => (ProceduralKind::Wood, true),
        "brushed" => (ProceduralKind::Brushed, false),
        "checker" => (ProceduralKind::Checker, false),
        "gradient_u" => (ProceduralKind::Gradient, false),
        "gradient_v" => (ProceduralKind::Gradient, true),
        "dirt" => (ProceduralKind::Grunge, true),
        "cracks" => (ProceduralKind::Cells, true),
        "streaks" => (ProceduralKind::Streaks, false),
        "speckles" => (ProceduralKind::Noise, true),
        "pores" => (ProceduralKind::Pores, false),
        "scratch" => (ProceduralKind::Scratches, true),
        _ => return None,
    })
}

/// A unique, file-safe name for an embedded image: `<stem>-<hash>.<ext>`, so two different
/// images never share a name (a project may already hold a resource of the same name) and the
/// same image imported twice shares one.
fn resource_name(name: &str, file: &str, bytes: &[u8]) -> String {
    let safe = crate::sanitize_resource_name(name);
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s.to_string(), e.to_ascii_lowercase()),
        _ => (safe.clone(), file.rsplit_once('.').map_or("png", |(_, e)| e).to_ascii_lowercase()),
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash = (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    let stem: String = stem.chars().take(60).collect();
    format!("{stem}-{:08x}.{}", (hash ^ (hash >> 32)) as u32, crate::sanitize_resource_name(&ext))
}

// ---------------------------------------------------------------------------------------------
// Pixel conversion.

/// A SubCraft paint raster (straight values then alpha, half floats) as a TextureCraft paint
/// image (premultiplied values then coverage, u16). `kind` says how values are encoded.
fn paint_image(r: &Raster, kind: ChannelKind) -> TileImage {
    let comps = r.comps;
    let mut img = TileImage::new(r.width, r.height, comps);
    let tiles_x = r.tiles_x();
    let value_comps = comps.saturating_sub(1);
    let encode = |v: f32| if kind == ChannelKind::Signed { to_u16_signed(v.clamp(-1.0, 1.0)) } else { to_u16(v) };
    let convert = |src: &dyn Fn(usize) -> f32| -> Vec<u16> {
        let mut out = vec![0u16; TILE_AREA * comps];
        for texel in 0..TILE_AREA {
            let alpha = unit(src(texel * comps + value_comps));
            if alpha <= 0.0 {
                continue;
            }
            for k in 0..value_comps {
                if let Some(slot) = out.get_mut(texel * comps + k) {
                    *slot = encode(src(texel * comps + k) * alpha);
                }
            }
            if let Some(slot) = out.get_mut(texel * comps + value_comps) {
                *slot = to_u16(alpha);
            }
        }
        out
    };
    let mut allocated = std::collections::BTreeSet::new();
    for (index, bits) in &r.tiles {
        allocated.insert(*index);
        let data = convert(&|i| bits.get(i).copied().map_or(0.0, half_to_f32));
        if data.iter().any(|v| *v != 0) {
            put_tile(&mut img, *index, tiles_x, data);
        }
    }
    // A raster whose fill is not transparent paints every tile it does not store.
    if r.fill.get(value_comps).copied().unwrap_or(0.0) > 0.0 {
        let tile_count = (tiles_x * r.height.div_ceil(raster::TILE)) as usize;
        let data = convert(&|i| r.fill.get(i % comps).copied().unwrap_or(0.0));
        for index in (0..tile_count).filter(|i| !allocated.contains(i)) {
            put_tile(&mut img, index, tiles_x, data.clone());
        }
    }
    img
}

/// A SubCraft mask base (one straight value per texel) as a TextureCraft mask paint image
/// (value, coverage 1) holding only the tiles that differ from the base's fill.
fn mask_base_image(r: &Raster) -> TileImage {
    let mut img = TileImage::new(r.width, r.height, 2);
    let tiles_x = r.tiles_x();
    let fill = r.fill.first().copied().unwrap_or(1.0);
    for (index, bits) in &r.tiles {
        let values: Vec<f32> = bits.iter().map(|b| half_to_f32(*b)).collect();
        if values.iter().all(|v| (v - fill).abs() < 1e-3) {
            continue;
        }
        let mut data = vec![0u16; TILE_AREA * 2];
        for (texel, v) in values.iter().enumerate() {
            if let (Some(a), Some(b)) = (texel.checked_mul(2), texel.checked_mul(2).and_then(|i| i.checked_add(1))) {
                if let Some(slot) = data.get_mut(a) {
                    *slot = to_u16(*v);
                }
                if let Some(slot) = data.get_mut(b) {
                    *slot = u16::MAX;
                }
            }
        }
        put_tile(&mut img, *index, tiles_x, data);
    }
    img
}

fn put_tile(img: &mut TileImage, index: usize, tiles_x: u32, data: Vec<u16>) {
    let tiles_x = tiles_x.max(1) as usize;
    img.set_tile((index % tiles_x) as u32, (index / tiles_x) as u32, Some(Arc::new(data)));
}

// ---------------------------------------------------------------------------------------------
// The mapper.

/// One embedded image of the fragment's table.
#[derive(Clone, Debug)]
struct ResourceEntry {
    name: String,
    file: String,
}

pub(super) struct Mapper<'z, R: Read + Seek> {
    zip: &'z mut zip::ZipArchive<R>,
    resolution: u32,
    project: Project,
    pixels: PixelStore,
    report: ImportReport,
    budget: Budget,
    /// Embedded images by their id in the file.
    table: HashMap<String, ResourceEntry>,
    /// Images already brought in: id -> TextureCraft resource name (None = unusable).
    used: HashMap<String, Option<String>>,
    resources: Resources,
    /// Names of the layers above the one being mapped, for the report.
    path: Vec<String>,
    layer_count: usize,
    /// Channels any layer writes (the mini texture set offers them all).
    channels: Vec<Channel>,
}

impl<'z, R: Read + Seek> Mapper<'z, R> {
    pub(super) fn new(zip: &'z mut zip::ZipArchive<R>, name: &str, resolution: u32, report: ImportReport, budget: Budget) -> Self {
        Mapper {
            zip,
            resolution,
            project: Project::new(name, "builtin:none"),
            pixels: PixelStore::default(),
            report,
            budget,
            table: HashMap::new(),
            used: HashMap::new(),
            resources: Resources::new(),
            path: Vec::new(),
            layer_count: 0,
            channels: Vec::new(),
        }
    }

    pub(super) fn finish(self) -> ScsmartImport {
        ScsmartImport {
            file: ProjectFile {
                project: self.project,
                pixels: self.pixels,
                baked: BTreeMap::new(),
                mesh: None,
                high_poly: Vec::new(),
                cage: None,
                bake_paint: BTreeMap::new(),
                warnings: Vec::new(),
                thumbnail: None,
                resources: self.resources,
            },
            report: self.report,
        }
    }

    fn issue(&mut self, severity: Severity, what: impl Into<String>, message: impl Into<String>) {
        let at = self.path.join(" / ");
        self.report.issue(severity, &at, what, message);
    }

    /// Texels of a SubCraft pixel radius at TextureCraft's 2K reference.
    fn px(&self, v: f32) -> f32 {
        v * REF_RES / self.resolution as f32
    }

    pub(super) fn run(&mut self, doc: &Value) -> Result<()> {
        let entries = list(doc, "resources");
        if entries.len() > MAX_RESOURCES {
            return Err(FormatError::TooLarge(format!("{} embedded images (limit {MAX_RESOURCES})", entries.len())));
        }
        for e in entries {
            if let (Some(id), Some(file)) = (text(e, "id"), text(e, "file")) {
                let name = text(e, "name").map(clean_name).filter(|n| !n.is_empty()).unwrap_or_else(|| "image".into());
                self.table.insert(id.to_string(), ResourceEntry { name, file: file.to_string() });
            }
        }
        let mut set = TextureSet::new(self.project.name.clone(), 0, self.resolution);
        set.channels.clear();
        let roots = list(doc, "layers");
        let mut ids = Vec::new();
        for l in roots {
            if let Some(id) = self.layer(l, 0)? {
                ids.push(id);
            }
        }
        let root = match ids.as_slice() {
            [] => return Err(FormatError::Invalid("the smart material has no layers".into())),
            [one] => *one,
            many => {
                self.issue(Severity::Note, "several top-level layers", "wrapped in one folder named after the smart material");
                let id = self.project.alloc_id();
                let mut wrapper = Layer::new(id, self.project.name.clone(), LayerKind::Group { children: many.to_vec(), collapsed: false }, &[]);
                wrapper.channels = self.channels.iter().map(|c| (*c, ChannelBlend { mode: BlendMode::Passthrough, opacity: 1.0 })).collect();
                self.project.layers.insert(id, wrapper);
                self.report.layers += 1;
                self.report.folders += 1;
                id
            }
        };
        for c in &self.channels {
            set.channels.push((*c, ChannelFormat::default_for(*c)));
        }
        if set.channels.is_empty() {
            set.channels = Channel::DEFAULT_SET.iter().map(|c| (*c, ChannelFormat::default_for(*c))).collect();
        }
        set.stack.push(root);
        self.project.texture_sets.push(set);
        self.project.validate().map_err(|e| malformed(e.to_string()))
    }

    // ---- layers -----------------------------------------------------------------------------

    fn layer(&mut self, v: &Value, depth: usize) -> Result<Option<LayerId>> {
        if depth > MAX_NEST {
            let name = text(v, "name").unwrap_or("layer").to_string();
            self.issue(Severity::Dropped, format!("layer \"{name}\""), format!("folders nested more than {MAX_NEST} deep are not imported"));
            return Ok(None);
        }
        self.layer_count += 1;
        if self.layer_count > MAX_LAYERS {
            return Err(FormatError::TooLarge(format!("more than {MAX_LAYERS} layers")));
        }
        let name = text(v, "name").map(clean_name).filter(|n| !n.is_empty()).unwrap_or_else(|| "Layer".into());
        self.path.push(name.clone());
        let out = self.layer_inner(v, &name, depth);
        self.path.pop();
        out
    }

    fn layer_inner(&mut self, v: &Value, name: &str, depth: usize) -> Result<Option<LayerId>> {
        let content = v.get("content").cloned().unwrap_or(Value::Null);
        let ty = text(&content, "type").unwrap_or("").to_string();
        let id = self.project.alloc_id();
        let mut channels = self.blends(v.get("blend"));
        let kind = match ty.as_str() {
            "paint" => {
                if content.get("path").is_some_and(|p| !p.is_null()) {
                    self.issue(Severity::Note, "pen path", "the editable path became plain paint (its pixels are kept)");
                }
                if content.get("text").is_some_and(|p| !p.is_null()) {
                    self.issue(Severity::Note, "text layer", "the editable text became plain paint (its pixels are kept)");
                }
                for pc in list(&content, "channels") {
                    self.paint_channel(id, pc)?;
                }
                LayerKind::Paint
            }
            "fill" => {
                let projection = self.projection(content.get("projection"));
                let (fill, disabled) = self.fill_sources(list(&content, "channels"), projection);
                for c in &disabled {
                    channels.remove(c);
                }
                LayerKind::Fill(fill)
            }
            "folder" => {
                self.report.folders += 1;
                let mut children = Vec::new();
                for c in list(&content, "children") {
                    if let Some(child) = self.layer(c, depth + 1)? {
                        children.push(child);
                    }
                }
                LayerKind::Group { children, collapsed: !flag(&content, "expanded").unwrap_or(true) }
            }
            other => {
                self.issue(Severity::Dropped, "layer content", format!("unknown layer type \"{other}\" is not imported"));
                return Ok(None);
            }
        };
        let mut layer = Layer::new(id, name, kind, &[]);
        layer.visible = flag(v, "visible").unwrap_or(true);
        for c in channels.keys() {
            if !self.channels.contains(c) {
                self.channels.push(*c);
            }
        }
        layer.channels = channels;
        if let Some(parts) = v.get("parts").filter(|p| flag(p, "enabled").unwrap_or(false) && !list(p, "parts").is_empty()) {
            let n = list(parts, "parts").len();
            self.issue(
                Severity::Dropped,
                "mesh parts",
                format!("the layer was limited to {n} mesh part(s) by index; TextureCraft limits by part name, so it shows on all parts"),
            );
        }
        if let Some(m) = v.get("mask").filter(|m| !m.is_null()) {
            layer.mask = self.mask(id, m)?;
        }
        let only: Vec<Channel> = layer.channels.iter().filter(|(_, b)| b.mode != BlendMode::Disable && b.opacity > 0.0).map(|(c, _)| *c).collect();
        let fx = list(v, "content_effects");
        if fx.len() > MAX_EFFECTS {
            return Err(FormatError::TooLarge(format!("{} effects on one layer (limit {MAX_EFFECTS})", fx.len())));
        }
        for e in fx {
            if let Some(effect) = self.content_effect(e, &only)? {
                layer.content_effects.push(effect);
            }
        }
        self.report.layers += 1;
        self.project.layers.insert(id, layer);
        Ok(Some(id))
    }

    /// The per-channel blend list of a layer or content effect.
    fn blends(&mut self, v: Option<&Value>) -> BTreeMap<Channel, ChannelBlend> {
        let mut out = BTreeMap::new();
        for b in v.and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
            let Some(cname) = text(b, "channel") else { continue };
            let Some(c) = channel(cname) else {
                self.issue(
                    Severity::Dropped,
                    format!("channel \"{cname}\""),
                    "this channel has no TextureCraft counterpart; the layer's contribution to it is not imported",
                );
                continue;
            };
            let mode = self.mode(text(b, "mode").unwrap_or("normal"));
            out.insert(c, ChannelBlend { mode, opacity: opacity(b, "opacity") });
        }
        out
    }

    fn mode(&mut self, name: &str) -> BlendMode {
        blend_mode(name).unwrap_or_else(|| {
            self.issue(Severity::Approximated, format!("blend mode \"{name}\""), "no TextureCraft blend mode of that name; Normal is used");
            BlendMode::Normal
        })
    }

    /// Read a raster entry of the container.
    fn raster(&mut self, name: &str, comps: usize) -> Result<Raster> {
        let bytes = read_entry(self.zip, name, MAX_ENTRY_BYTES, &mut self.budget)?;
        raster::decode(&bytes, self.resolution, comps, &mut self.budget).map_err(|e| malformed(format!("{name}: {e}")))
    }

    /// One painted channel of a paint layer or paint effect: its pixels go into the store under `owner`.
    fn paint_channel(&mut self, owner: LayerId, pc: &Value) -> Result<Option<Channel>> {
        let (Some(cname), Some(file)) = (text(pc, "channel"), text(pc, "raster")) else { return Ok(None) };
        let Some(c) = channel(cname) else {
            self.issue(Severity::Dropped, format!("channel \"{cname}\""), "this channel has no TextureCraft counterpart; its paint is not imported");
            return Ok(None);
        };
        let r = self.raster(file, c.components() + 1)?;
        let img = paint_image(&r, c.kind());
        self.pixels.layers.insert((owner, c), img);
        Ok(Some(c))
    }

    // ---- fills --------------------------------------------------------------------------------

    fn projection(&mut self, p: Option<&Value>) -> Projection {
        let Some(p) = p.filter(|p| p.is_object()) else { return Projection::default() };
        let fix = |s: f32| if s.abs() < 1e-4 { 1.0 } else { s.clamp(-1000.0, 1000.0) };
        let scale = floats::<2>(p, "scale").unwrap_or([1.0, 1.0]).map(fix);
        let rotation = num(p, "rotation_deg").unwrap_or(0.0);
        let offset = floats::<2>(p, "offset").unwrap_or([0.0, 0.0]);
        match text(p, "mode").unwrap_or("uv") {
            "uv" => Projection::Uv { scale, rotation, offset },
            mode @ ("triplanar" | "planar") => {
                if (scale[0] - scale[1]).abs() > 1e-4 || rotation.abs() > 1e-4 {
                    self.issue(
                        Severity::Approximated,
                        format!("{mode} projection"),
                        "TextureCraft's world projections have one scale and no rotation; the first scale is used",
                    );
                }
                let offset = [offset[0], offset[1], 0.0];
                if mode == "triplanar" {
                    Projection::Triplanar { scale: scale[0], sharpness: num(p, "triplanar_sharpness").unwrap_or(8.0).clamp(1.0, 64.0), offset }
                } else {
                    let axis = p.get("planar_axis").and_then(Value::as_u64).map_or(2, |a| a.min(2)) as u8;
                    Projection::Planar { scale: scale[0], axis, offset }
                }
            }
            other => {
                self.issue(Severity::Approximated, format!("projection \"{other}\""), "unknown projection mode; UV projection is used");
                Projection::Uv { scale, rotation, offset }
            }
        }
    }

    /// A fill's channel sources. Returns the fill and the channels it does not produce (disabled
    /// in the file or without a mappable source), which the layer must not write.
    fn fill_sources(&mut self, chans: &[Value], projection: Projection) -> (Fill, Vec<Channel>) {
        let mut fill = Fill { sources: BTreeMap::new(), projection, link: None };
        let mut disabled = Vec::new();
        for fc in chans {
            let Some(cname) = text(fc, "channel") else { continue };
            let Some(c) = channel(cname) else {
                self.issue(Severity::Dropped, format!("channel \"{cname}\""), "this channel has no TextureCraft counterpart; the fill does not write it");
                continue;
            };
            if !flag(fc, "enabled").unwrap_or(true) {
                disabled.push(c);
                continue;
            }
            let strength = num(fc, "strength").unwrap_or(1.0).clamp(0.0, 16.0);
            match fc.get("source").and_then(|s| self.source(c, s, strength)) {
                Some(s) => {
                    fill.sources.insert(c, s);
                }
                None => disabled.push(c),
            }
        }
        (fill, disabled)
    }

    /// A value scaled by a SubCraft channel strength (brightness, value multiplier, bump scale).
    /// Normal tilt cannot be folded into a constant.
    fn strengthen(&mut self, c: Channel, v: [f32; 3], strength: f32) -> [f32; 3] {
        match c.kind() {
            ChannelKind::NormalMap => {
                if (strength - 1.0).abs() > 1e-3 {
                    self.issue(
                        Severity::Approximated,
                        "normal strength",
                        "the tilt scale of a normal fill has no TextureCraft equivalent; full strength is used",
                    );
                }
                v
            }
            ChannelKind::Signed => v.map(|x| (x * strength).clamp(-1.0, 1.0)),
            _ => v.map(|x| unit(x * strength)),
        }
    }

    fn source(&mut self, c: Channel, s: &Value, strength: f32) -> Option<Source> {
        match text(s, "type").unwrap_or("") {
            "uniform" => {
                let v = floats::<3>(s, "value").unwrap_or_else(|| c.default_value());
                Some(Source::Uniform(self.strengthen(c, v, strength)))
            }
            "bitmap" => {
                let id = text(s, "resource")?;
                let name = self.resource(id)?;
                if (strength - 1.0).abs() > 1e-3 && c.kind() != ChannelKind::NormalMap {
                    self.issue(Severity::Approximated, "image strength", "a strength other than 1 on an image fill is not applied");
                }
                Some(Source::Image { name, normal_format: NormalFormat::OpenGl })
            }
            "procedural" => {
                let params = s.get("params").unwrap_or(&Value::Null);
                let pattern = self.pattern(params)?;
                let low = floats::<3>(s, "low").unwrap_or([0.0; 3]);
                let high = floats::<3>(s, "high").unwrap_or([1.0; 3]);
                let (low, high) = (self.strengthen(c, low, strength), self.strengthen(c, high, strength));
                Some(Source::Procedural { pattern, low, high })
            }
            "mesh_map" => {
                let map = text(s, "map").unwrap_or("?");
                self.issue(
                    Severity::Dropped,
                    format!("mesh-map fill \"{map}\""),
                    "TextureCraft fills cannot read baked maps; the channel is not written (use a mask generator for curvature or occlusion)",
                );
                None
            }
            other => {
                self.issue(Severity::Dropped, format!("fill source \"{other}\""), "unknown fill source; the channel is not written");
                None
            }
        }
    }

    /// SubCraft procedural parameters as a TextureCraft pattern.
    ///
    /// SubCraft remaps a raw pattern `v` to `(v - balance) * k + 0.5` with
    /// `k = 1 / ((1 - contrast) + 0.002)`; TextureCraft remaps to `(v - 0.5) * contrast' + 0.5 +
    /// balance'`, so `contrast' = k` and `balance' = k * (0.5 - balance)`.
    fn pattern(&mut self, p: &Value) -> Option<Procedural> {
        let kname = text(p, "kind").unwrap_or("");
        let Some((kind, differs)) = pattern_kind(kname) else {
            self.issue(Severity::Dropped, format!("pattern \"{kname}\""), "unknown procedural pattern; the channel is not written");
            return None;
        };
        self.issue(Severity::Note, "procedural patterns", "TextureCraft draws patterns with its own generators: the same families, but not pixel-identical");
        if differs {
            self.issue(Severity::Approximated, format!("pattern \"{kname}\""), format!("mapped to the closest TextureCraft pattern ({})", kind.label()));
        }
        if flag(p, "colored").unwrap_or(false) {
            self.issue(Severity::Approximated, "colored noise", "per-component patterns are not available; one pattern drives all components");
        }
        let k = 1.0 / ((1.0 - num(p, "contrast").unwrap_or(0.0).clamp(0.0, 1.0)) + 0.002);
        let balance = num(p, "balance").unwrap_or(0.5);
        Some(Procedural {
            kind,
            scale: num(p, "scale").unwrap_or(4.0).clamp(0.01, 1000.0),
            seed: p.get("seed").and_then(Value::as_u64).map_or(1, |s| s.min(u64::from(u32::MAX)) as u32),
            contrast: k.clamp(0.0, 500.0),
            balance: (k * (0.5 - balance)).clamp(-500.0, 500.0),
            invert: flag(p, "invert").unwrap_or(false),
        })
    }

    /// An embedded image, brought into the resources under a unique name.
    fn resource(&mut self, id: &str) -> Option<String> {
        if let Some(done) = self.used.get(id) {
            return done.clone();
        }
        let result = self.load_resource(id);
        self.used.insert(id.to_string(), result.clone());
        result
    }

    fn load_resource(&mut self, id: &str) -> Option<String> {
        let Some(entry) = self.table.get(id).cloned() else {
            self.issue(Severity::Dropped, "image", format!("the file refers to an image ({id}) it does not contain"));
            return None;
        };
        let bytes = match read_entry(self.zip, &entry.file, MAX_ENTRY_BYTES, &mut self.budget) {
            Ok(b) => b,
            Err(e) => {
                self.issue(Severity::Dropped, format!("image \"{}\"", entry.name), e.to_string());
                return None;
            }
        };
        let dims = crate::guard("image", || {
            image::ImageReader::new(std::io::Cursor::new(&bytes[..]))
                .with_guessed_format()
                .map_err(|e| malformed(e.to_string()))?
                .into_dimensions()
                .map_err(|e| malformed(e.to_string()))
        });
        match dims {
            Ok((w, h)) if w > 0 && h > 0 && w <= crate::MAX_IMAGE_SIZE && h <= crate::MAX_IMAGE_SIZE => {}
            Ok((w, h)) => {
                self.issue(Severity::Dropped, format!("image \"{}\"", entry.name), format!("{w}x{h} is outside the supported image size"));
                return None;
            }
            Err(e) => {
                self.issue(Severity::Dropped, format!("image \"{}\"", entry.name), format!("not a readable image ({e})"));
                return None;
            }
        }
        let name = resource_name(&entry.name, &entry.file, &bytes);
        if self.resources.insert(name.clone(), bytes).is_none() {
            self.report.images += 1;
        }
        Some(name)
    }

    // ---- masks --------------------------------------------------------------------------------

    fn mask(&mut self, owner: LayerId, m: &Value) -> Result<Option<Mask>> {
        self.path.push("mask".into());
        let out = self.mask_inner(owner, m);
        self.path.pop();
        out
    }

    fn mask_inner(&mut self, _owner: LayerId, m: &Value) -> Result<Option<Mask>> {
        let Some(base_file) = text(m, "base") else { return Ok(None) };
        let base = self.raster(base_file, 1)?;
        let fill = base.fill.first().copied().unwrap_or(1.0);
        let background = if fill >= 0.5 { MaskBackground::White } else { MaskBackground::Black };
        if fill > 1e-3 && fill < 1.0 - 1e-3 {
            self.issue(
                Severity::Approximated,
                "mask background",
                format!("a mask whose unpainted value is {fill:.2} starts {} here", if fill >= 0.5 { "white" } else { "black" }),
            );
        }
        let mut mask = Mask::new(background);
        mask.enabled = flag(m, "enabled").unwrap_or(true);
        // Painted mask pixels become a paint effect at the bottom of the stack.
        let painted = mask_base_image(&base);
        if painted.allocated_count() > 0 {
            let id = self.project.alloc_id();
            self.pixels.masks.insert(id, painted);
            mask.effects.push(Effect::new(id, EffectKind::Paint));
            self.report.effects += 1;
        }
        let effects = list(m, "effects");
        if effects.len() > MAX_EFFECTS {
            return Err(FormatError::TooLarge(format!("{} mask effects (limit {MAX_EFFECTS})", effects.len())));
        }
        for e in effects {
            self.mask_effect(e, &mut mask.effects)?;
        }
        self.report.masks += 1;
        Ok(Some(mask))
    }

    /// Map one mask effect and push the TextureCraft effect(s) it becomes.
    fn mask_effect(&mut self, e: &Value, out: &mut Vec<Effect>) -> Result<()> {
        let kind = e.get("kind").unwrap_or(&Value::Null);
        let ty = text(kind, "type").unwrap_or("").to_string();
        let blend = self.mode(text(e, "blend").unwrap_or("normal"));
        let name = text(e, "name").map(clean_name).filter(|n| !n.is_empty());
        let mut extra_invert = false;
        let mapped: Option<EffectKind> = match ty.as_str() {
            "paint" => match text(kind, "raster") {
                Some(file) => {
                    let r = self.raster(file, 2)?;
                    let id = self.project.alloc_id();
                    self.pixels.masks.insert(id, paint_image(&r, ChannelKind::Scalar));
                    return self.push_effect(out, id, name, EffectKind::Paint, e, blend);
                }
                None => None,
            },
            "fill" => {
                let projection = self.projection(kind.get("projection"));
                let source = match kind.get("procedural").filter(|p| !p.is_null()).map(|p| self.pattern(p)) {
                    Some(Some(pattern)) => Some(Source::Procedural { pattern, low: [0.0; 3], high: [1.0; 3] }),
                    Some(None) => None,
                    None => Some(Source::Uniform([unit(num(kind, "value").unwrap_or(1.0)); 3])),
                };
                source.map(|source| EffectKind::Fill { source, projection })
            }
            "generator" => self.generator(kind.get("generator").unwrap_or(&Value::Null)).map(EffectKind::Generator),
            "levels" => {
                let l = kind.get("levels").unwrap_or(&Value::Null);
                Some(EffectKind::Levels(Levels {
                    in_low: num(l, "in_low").unwrap_or(0.0),
                    in_high: num(l, "in_high").unwrap_or(1.0),
                    gamma: num(l, "gamma").unwrap_or(1.0).clamp(0.01, 10.0),
                    out_low: num(l, "out_low").unwrap_or(0.0),
                    out_high: num(l, "out_high").unwrap_or(1.0),
                }))
            }
            "filter" => self.filter(kind.get("filter").unwrap_or(&Value::Null)).map(EffectKind::Filter),
            "invert" => Some(EffectKind::Filter(Filter::Invert)),
            "image" => {
                let projection = self.projection(kind.get("projection"));
                extra_invert = flag(kind, "invert").unwrap_or(false);
                text(kind, "resource").and_then(|id| self.resource(id)).map(|name| EffectKind::Fill { source: Source::image(name), projection })
            }
            "id_color" => {
                let ic = kind.get("id_color").unwrap_or(&Value::Null);
                let colors: Vec<[f32; 3]> = list(ic, "colors")
                    .iter()
                    .take(64)
                    .filter_map(|c| {
                        let a = c.as_array()?;
                        Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32])
                    })
                    .filter(|c| c.iter().all(|x| x.is_finite()))
                    .collect();
                if num(ic, "softness").unwrap_or(0.0) > 0.0 {
                    self.issue(Severity::Approximated, "ID color mask", "the soft falloff is not available; the selection has a hard edge");
                }
                extra_invert = flag(ic, "invert").unwrap_or(false);
                Some(EffectKind::ColorSelection { colors, tolerance: num(ic, "tolerance").unwrap_or(0.05).clamp(0.0, 1.0) })
            }
            other => {
                self.issue(Severity::Dropped, format!("mask effect \"{other}\""), "unknown mask effect; it is not imported");
                None
            }
        };
        let Some(kind) = mapped else { return Ok(()) };
        let id = self.project.alloc_id();
        self.push_effect(out, id, name, kind, e, blend)?;
        if extra_invert {
            let id = self.project.alloc_id();
            let mut inv = Effect::new(id, EffectKind::Filter(Filter::Invert));
            inv.name = "Invert".into();
            out.push(inv);
            self.report.effects += 1;
            self.issue(Severity::Note, "inverted source", "the inversion is a separate Invert effect above it");
        }
        Ok(())
    }

    fn push_effect(&mut self, out: &mut Vec<Effect>, id: LayerId, name: Option<String>, kind: EffectKind, e: &Value, blend: BlendMode) -> Result<()> {
        let mut effect = Effect::new(id, kind);
        if let Some(n) = name {
            effect.name = n;
        }
        effect.visible = flag(e, "visible").unwrap_or(true);
        effect.blend = blend;
        effect.opacity = opacity(e, "opacity");
        out.push(effect);
        self.report.effects += 1;
        Ok(())
    }

    fn generator(&mut self, g: &Value) -> Option<Generator> {
        let (kind, invert_flip) = match g.get("kind") {
            Some(Value::String(k)) => match k.as_str() {
                "edges" => (GeneratorKind::EdgeWear, false),
                "cavity" => (GeneratorKind::Dirt, false),
                "directional" => (GeneratorKind::Direction, false),
                "position_gradient" => (GeneratorKind::Position, false),
                "concave" => {
                    self.issue(Severity::Approximated, "generator \"concave\"", "mapped to the curvature generator, inverted, so creases are selected");
                    (GeneratorKind::Curvature, true)
                }
                other => {
                    self.issue(Severity::Dropped, format!("generator \"{other}\""), "unknown generator; it is not imported");
                    return None;
                }
            },
            Some(Value::Object(o)) => match o.get("mesh_map").and_then(Value::as_str) {
                Some("ambient_occlusion") => (GeneratorKind::AmbientOcclusion, false),
                Some("curvature") => (GeneratorKind::Curvature, false),
                Some("thickness") => {
                    self.issue(Severity::Approximated, "generator \"thickness map\"", "mapped to the thin-areas generator");
                    (GeneratorKind::ThinAreas, false)
                }
                other => {
                    self.issue(
                        Severity::Dropped,
                        format!("generator \"mesh map {}\"", other.unwrap_or("?")),
                        "TextureCraft generators read curvature, ambient occlusion and thickness only; the effect is not imported",
                    );
                    return None;
                }
            },
            _ => {
                self.issue(Severity::Dropped, "generator", "the generator kind is missing; it is not imported");
                return None;
            }
        };
        if g.get("edge_spread").is_some_and(|e| !e.is_null()) || flag(g, "organic_breakup").unwrap_or(false) {
            self.issue(Severity::Approximated, "generator wear options", "edge spread and organic breakup are not available; the plain threshold is used");
        }
        let mut gen_ = Generator::new(kind);
        gen_.level = unit(num(g, "level").unwrap_or(gen_.level));
        gen_.softness = unit(num(g, "softness").unwrap_or(gen_.softness));
        gen_.invert = flag(g, "invert").unwrap_or(false) ^ invert_flip;
        gen_.noise_amount = unit(num(g, "noise_amount").unwrap_or(0.0));
        if let Some(n) = g.get("noise") {
            gen_.noise_scale = num(n, "scale").unwrap_or(gen_.noise_scale).clamp(0.01, 1000.0);
            gen_.seed = n.get("seed").and_then(Value::as_u64).map_or(gen_.seed, |s| s.min(u64::from(u32::MAX)) as u32);
        }
        gen_.direction = match kind {
            GeneratorKind::Position => match text(g, "axis").unwrap_or("y") {
                "x" => [1.0, 0.0, 0.0],
                "z" => [0.0, 0.0, 1.0],
                _ => [0.0, 1.0, 0.0],
            },
            _ => floats::<3>(g, "direction").filter(|d| d.iter().any(|x| x.abs() > 1e-6)).unwrap_or([0.0, 1.0, 0.0]),
        };
        Some(gen_)
    }

    // ---- content effects ------------------------------------------------------------------------

    /// `only` lists the channels the layer writes: TextureCraft's content filters and levels act on
    /// every channel of the layer, so a per-channel one maps only when the layer writes just that channel.
    fn content_effect(&mut self, e: &Value, only: &[Channel]) -> Result<Option<Effect>> {
        let kind = e.get("kind").unwrap_or(&Value::Null);
        let ty = text(kind, "type").unwrap_or("").to_string();
        let label = text(e, "name").map(clean_name).filter(|n| !n.is_empty()).unwrap_or_else(|| ty.clone());
        self.path.push(label);
        let out = self.content_effect_inner(e, kind, &ty, only);
        self.path.pop();
        out
    }

    fn content_effect_inner(&mut self, e: &Value, kind: &Value, ty: &str, only: &[Channel]) -> Result<Option<Effect>> {
        let per_channel = |m: &mut Self, what: &str, cname: Option<&str>| -> bool {
            let target = cname.and_then(channel);
            if target.is_some() && only == [target.unwrap_or(Channel::BaseColor)] {
                return true;
            }
            m.issue(
                Severity::Dropped,
                format!("{what} on {}", cname.unwrap_or("a channel")),
                "TextureCraft applies content effects to every channel of a layer, so a single-channel one cannot be imported on a layer that writes several",
            );
            false
        };
        let id = self.project.alloc_id();
        let (mapped, blended) = match ty {
            "paint" => {
                for pc in list(kind, "channels") {
                    self.paint_channel(id, pc)?;
                }
                (EffectKind::Paint, true)
            }
            "fill" => {
                let projection = self.projection(kind.get("projection"));
                let (fill, _) = self.fill_sources(list(kind, "channels"), projection);
                (EffectKind::Material(fill), true)
            }
            "levels" => {
                if !per_channel(self, "levels", text(kind, "channel")) {
                    return Ok(None);
                }
                let l = kind.get("levels").unwrap_or(&Value::Null);
                (
                    EffectKind::Levels(Levels {
                        in_low: num(l, "in_low").unwrap_or(0.0),
                        in_high: num(l, "in_high").unwrap_or(1.0),
                        gamma: num(l, "gamma").unwrap_or(1.0).clamp(0.01, 10.0),
                        out_low: num(l, "out_low").unwrap_or(0.0),
                        out_high: num(l, "out_high").unwrap_or(1.0),
                    }),
                    false,
                )
            }
            "filter" => {
                if !per_channel(self, "filter", text(kind, "channel")) {
                    return Ok(None);
                }
                match self.filter(kind.get("filter").unwrap_or(&Value::Null)) {
                    Some(f) => (EffectKind::Filter(f), false),
                    None => return Ok(None),
                }
            }
            other => {
                self.issue(Severity::Dropped, format!("content effect \"{other}\""), "unknown content effect; it is not imported");
                return Ok(None);
            }
        };
        let mut effect = Effect::new(id, mapped);
        if let Some(n) = text(e, "name").map(clean_name).filter(|n| !n.is_empty()) {
            effect.name = n;
        }
        effect.visible = flag(e, "visible").unwrap_or(true);
        if blended {
            // One blend mode and opacity for the effect: the first channel's.
            let list = self.blends(e.get("blend"));
            let mut it = list.values();
            if let Some(first) = it.next() {
                effect.blend = first.mode;
                effect.opacity = first.opacity;
                if it.any(|b| b.mode != first.mode || (b.opacity - first.opacity).abs() > 1e-3) {
                    self.issue(
                        Severity::Approximated,
                        "per-channel blending",
                        "TextureCraft blends a content effect with one mode and opacity; the first channel's is used for all",
                    );
                }
            }
        }
        self.report.effects += 1;
        Ok(Some(effect))
    }

    // ---- filters --------------------------------------------------------------------------------

    fn filter(&mut self, f: &Value) -> Option<Filter> {
        let kind = text(f, "kind").unwrap_or("").to_string();
        let n = |k: &str, d: f32| num(f, k).unwrap_or(d);
        let radius = |m: &Self, k: &str, hi: f32| m.px(n(k, 0.0)).clamp(0.0, hi);
        let exact = |f: Filter| Some((f, None::<&str>));
        let result: Option<(Filter, Option<&str>)> = match kind.as_str() {
            "blur" => exact(Filter::Blur { radius: radius(self, "radius", 256.0) }),
            "sharpen" => Some((Filter::Sharpen { amount: n("amount", 1.0).clamp(0.0, 10.0) }, Some("the radius is fixed in TextureCraft"))),
            "directional_blur" => exact(Filter::DirectionalBlur { radius: radius(self, "radius", 256.0), angle: n("angle_deg", 0.0).rem_euclid(360.0) }),
            "highpass" => Some((
                Filter::HighPass { radius: radius(self, "radius", 256.0) },
                (n("amount", 1.0) - 1.0).abs().gt(&1e-3).then_some("the amount is not available"),
            )),
            "bevel" => Some((Filter::Bevel { width: radius(self, "radius", 256.0) }, Some("amount and threshold are not available"))),
            "edge_detect" => Some((Filter::EdgeDetect { width: radius(self, "radius", 256.0) }, Some("the strength is not available"))),
            "glow" => Some((
                Filter::Glow { radius: radius(self, "radius", 256.0), amount: n("amount", 1.0).clamp(0.0, 10.0) },
                (n("threshold", 0.0) > 0.0).then_some("the threshold is not available"),
            )),
            "warp" => Some((
                Filter::Warp {
                    intensity: radius(self, "intensity", 256.0),
                    scale: n("scale", 4.0).clamp(0.01, 1000.0),
                    seed: f.get("seed").and_then(Value::as_u64).map_or(1, |s| s.min(999) as u32),
                },
                Some("TextureCraft's noise field differs"),
            )),
            "directional_warp" => Some((
                Filter::DirectionalWarp { intensity: radius(self, "intensity", 256.0), angle: n("angle_deg", 0.0).rem_euclid(360.0), scale: 4.0, seed: 1 },
                Some("TextureCraft's noise field differs and its scale and seed are fixed"),
            )),
            "mosaic" => exact(Filter::Mosaic { size: self.px(n("size", 1.0)).clamp(1.0, 512.0) }),
            "contrast" => {
                let gain = n("gain", 1.0);
                Some((
                    Filter::ContrastLuminosity { contrast: (gain - 1.0).clamp(-1.0, 1.0), luminosity: 0.0 },
                    ((gain - 1.0).abs() > 1e-3).then_some("the contrast curve and its pivot differ"),
                ))
            }
            "hsl" => exact(Filter::Hsl {
                hue: (n("hue", 0.0) / 360.0).clamp(-1.0, 1.0),
                saturation: (n("saturation", 1.0) - 1.0).clamp(-1.0, 1.0),
                lightness: n("lightness", 0.0).clamp(-1.0, 1.0),
            }),
            "threshold" => exact(Filter::Threshold { level: unit(n("cutoff", 0.5)) }),
            "clamp" => exact(Filter::Clamp { min: unit(n("low", 0.0)), max: unit(n("high", 1.0)) }),
            "grayscale" => exact(Filter::Grayscale),
            "invert" => exact(Filter::Invert),
            "posterize" => exact(Filter::Posterize { levels: n("levels", 8.0).clamp(2.0, 64.0) }),
            "histogram_shift" => exact(Filter::HistogramShift { offset: n("shift", 0.0).clamp(-1.0, 1.0) }),
            "histogram_scan" => Some((
                Filter::HistogramScan { position: unit(n("position", 0.5)), contrast: unit(n("contrast", 0.5)) },
                Some("the ramp is computed differently, and an inverted scan is not available"),
            )),
            "histogram_range" => Some((
                Filter::HistogramRange { range: unit(n("range", 1.0)), position: unit(n("position", 0.5)) },
                Some("the softness is not available and the range is computed differently"),
            )),
            "color_balance" => {
                let band = |k: &str| floats::<3>(f, k).unwrap_or([0.0; 3]).map(|x| x.clamp(-1.0, 1.0));
                exact(Filter::ColorBalance { shadows: band("shadows"), midtones: band("midtones"), highlights: band("highlights") })
            }
            "replace_color" => exact(Filter::ReplaceColor {
                from: floats::<3>(f, "from").unwrap_or([0.0; 3]).map(unit),
                to: floats::<3>(f, "to").unwrap_or([0.0; 3]).map(unit),
                tolerance: unit(n("tolerance", 0.1)),
                softness: unit(n("softness", 0.1)),
            }),
            "normal_invert" => {
                let (x, y, z) = (flag(f, "flip_x").unwrap_or(false), flag(f, "flip_y").unwrap_or(false), flag(f, "flip_z").unwrap_or(false));
                if y { Some((Filter::NormalInvert, (x || z).then_some("only the green (Y) flip is available"))) } else { None }
            }
            "gradient_map" => self.gradient_map(f),
            "median" => None,
            "mask_outline" => None,
            _ => None,
        };
        match result {
            Some((filter, note)) => {
                if let Some(note) = note {
                    self.issue(Severity::Approximated, format!("filter \"{kind}\""), note);
                }
                Some(filter)
            }
            None => {
                let why = match kind.as_str() {
                    "median" | "mask_outline" | "normal_invert" => "TextureCraft has no equivalent; the effect is not imported",
                    _ => "unknown filter; the effect is not imported",
                };
                self.issue(Severity::Dropped, format!("filter \"{kind}\""), why);
                None
            }
        }
    }

    /// SubCraft gradient stops (anywhere in 0..1) as TextureCraft's evenly spaced colors.
    fn gradient_map(&mut self, f: &Value) -> Option<(Filter, Option<&'static str>)> {
        let mut stops: Vec<(f32, [f32; 3])> = f
            .get("gradient")?
            .as_array()?
            .iter()
            .take(32)
            .filter_map(|s| {
                let c = floats::<3>(s, "color")?;
                Some((num(s, "position")?.clamp(0.0, 1.0), c.map(|x| x.max(0.0))))
            })
            .collect();
        if stops.is_empty() {
            return None;
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let n = stops.len();
        let even = stops.iter().enumerate().all(|(i, s)| (s.0 - if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 }).abs() < 1e-3);
        if even && n <= 16 {
            return Some((Filter::GradientMap { colors: stops.iter().map(|s| s.1.map(unit)).collect() }, None));
        }
        let at = |t: f32| -> [f32; 3] {
            let Some(i) = stops.iter().position(|s| s.0 > t) else { return stops.last().map_or([0.0; 3], |s| s.1) };
            let (Some(b), Some(a)) = (stops.get(i), i.checked_sub(1).and_then(|p| stops.get(p))) else { return stops.first().map_or([0.0; 3], |s| s.1) };
            let k = (t - a.0) / (b.0 - a.0).max(1e-6);
            [0, 1, 2].map(|c| a.1.get(c).copied().unwrap_or(0.0) + (b.1.get(c).copied().unwrap_or(0.0) - a.1.get(c).copied().unwrap_or(0.0)) * k)
        };
        let colors = (0..16).map(|i| at(i as f32 / 15.0).map(unit)).collect();
        Some((Filter::GradientMap { colors }, Some("the stops are resampled to 16 evenly spaced colors")))
    }
}
