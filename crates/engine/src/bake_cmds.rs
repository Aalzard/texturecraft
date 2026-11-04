//! Mesh-map baking commands (the "Bake Mesh Maps" window): bake settings, the
//! high-definition mesh list, the cage, and running a bake. Bakes cast their rays on the GPU
//! tracer the app installs ([`install_gpu_tracer`]) and on the CPU otherwise.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use texturecraft_bake::tracing::Tracer;
use texturecraft_bake::{BakeOutput, BakeRequest, HighPoly, MAX_RAYS, Progress, SkewMap, bake, bake_on};
use texturecraft_core::{
    BackfaceMode, BakeAntialiasing, BakeResolution, BakeSettings, BakedMap, CurvatureMethod, PositionMode, RayDistribution, RaySettings, SelfOcclusion,
};
use texturecraft_geom::{BaseColor, Bvh, Mesh};

use crate::commands::{CommandSpec, always, bad, boolp, f32p, strp, u64p};
use crate::{EngineError, Result, Session, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "bake.run",
            "Bake mesh maps",
            ["Mode"],
            None,
            "{ts?: index|[indices]|\"all\"|\"selected\", cpu?: true (skip the GPU), ...any bake.settings parameter} → {baked, tracer: [where each set's rays ran]}",
            always,
            bake_run
        ),
        cmd!(
            "bake.settings",
            "Bake settings",
            [],
            None,
            "{ts?: index|[indices]|\"all\"|\"selected\" (default: the active set), model?: complete nested BakeSettings (named enum values), preserveEnabled?: keep each texture set enabled flag when replacing the model, resolution?, antialiasing?: 1|2|4|8, dilation?, ao?: {rays?, maxDistance?, spread?: degrees (180 = hemisphere), distribution?: cosine|uniform, ignoreBackface?: never|always|byMeshName, selfOcclusion?: always|sameMeshName}, thickness?: {rays?, maxDistance?, spread?, distribution?}, bentNormal?: {rays?, maxDistance?, spread?, distribution?}, rays?, maxDistance? (all three ray maps), curvatureMethod?: normalMap|mesh, positionMode?: boundingBox|boundingSphere|none, maps?: [ids], enabled?, useLowAsHigh?, maxFrontal?, maxRear?, averageNormals?, lowSuffix?, highSuffix?}",
            always,
            settings
        ),
        cmd!(
            "bake.highPoly.add",
            "Add high definition mesh",
            [],
            None,
            "{path} — a mesh file (glb, gltf, fbx, dae, usd, usda, usdc, usdz, abc, obj, ply, stl; base color textures from glTF, embedded FBX media, usdz packages and image files next to a .gltf or USD layer supply opacity)",
            always,
            high_add
        ),
        cmd!("bake.highPoly.remove", "Remove high definition mesh", [], None, "{index? | name? | all?: true}", always, high_remove),
        cmd!("bake.highPoly.list", "List high definition meshes", [], None, "{}", always, |s, _| Ok(meshes_json(s))),
        cmd!("bake.cage.set", "Load cage mesh", [], None, "{path} — a mesh with the low poly's triangles", always, cage_set),
        cmd!("bake.cage.clear", "Remove cage mesh", [], None, "{}", always, |s, _| {
            s.bake_meshes.cage = None;
            s.modified = true;
            Ok(meshes_json(s))
        }),
        cmd!(
            "bake.skew.set",
            "Set skew correction",
            [],
            None,
            "{value?: 0..1 (a constant map, default 1) | path: a greyscale image over the UV atlas (white turns the rays onto the base normal, black keeps the cage's averaged direction), edgeDistance?: fraction of the mesh diagonal over which hard edges hold the correction back (default 0), edgeContrast?: falloff exponent (default 1)} → {skew}. Applies to the transfer rays of a bake from the built-in distance cage; a cage mesh ignores it",
            always,
            skew_set
        ),
        cmd!(
            "bake.skew.paint",
            "Paint skew correction",
            [],
            None,
            "{points: [[u, v], ...] (atlas uv, a stroke), radius?: uv units (default 0.03), value?: 0..1 (1 turns rays onto the base normal, 0 erases; default 1), hardness?: 0..1 (default 0.5), strength?: 0..1 per dab (default 1), size?: px of a new or constant map (default 1024, 16..4096)} → {skew}. Paints into the skew map; a missing or constant map is first widened to `size`",
            always,
            skew_paint
        ),
        cmd!("bake.skew.clear", "Remove skew correction", [], None, "{}", always, |s, _| {
            s.bake_meshes.skew = None;
            s.bake_meshes.skew_rev += 1;
            s.modified = true;
            Ok(meshes_json(s))
        }),
    ]
}

/// A mesh file as saved in a project: (file name, bytes).
type MeshFile = (String, Vec<u8>);

/// A mesh file the baker reads (high poly or cage): its name, original bytes (saved in the
/// project), parsed mesh and its materials' base colors (high polys).
#[derive(Clone, Debug)]
pub struct BakeMesh {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub mesh: Arc<Mesh>,
    pub base_color: Vec<Option<Arc<BaseColor>>>,
}

/// Every high poly appended into one mesh, its BVH and per-material base colors.
#[derive(Clone, Debug)]
pub struct MergedHigh {
    pub mesh: Arc<Mesh>,
    pub bvh: Arc<Bvh>,
    pub base_color: Arc<Vec<Option<Arc<BaseColor>>>>,
}

static GPU_TRACER: OnceLock<Arc<dyn Tracer>> = OnceLock::new();

/// Cast bake rays with `tracer` (the apps install the render crate's GPU baker at start-up);
/// the first call wins.
pub fn install_gpu_tracer(tracer: Arc<dyn Tracer>) {
    let _ = GPU_TRACER.set(tracer);
}

/// The high-definition meshes and the cage. Like the low-poly mesh they are not part of the
/// undo history.
#[derive(Clone, Debug, Default)]
pub struct BakeMeshes {
    pub high: Vec<BakeMesh>,
    pub cage: Option<BakeMesh>,
    /// Skew correction of the transfer rays (saved in the project as `skew.bin`).
    pub skew: Option<Arc<SkewMap>>,
    /// Counts changes to `skew`, so a view knows when to redraw it.
    pub skew_rev: u64,
    /// Every high poly appended into one mesh (rebuilt when the list changes).
    merged: Option<MergedHigh>,
}

impl BakeMeshes {
    // shortcut: the merged copy doubles high-poly memory; build the BVH over the parts in place if
    // multi-million-triangle sets run out of memory.
    fn rebuild(&mut self) {
        self.merged = (!self.high.is_empty()).then(|| {
            let (mut all, mut base_color) = (Mesh::default(), Vec::new());
            for h in &self.high {
                all.append(&h.mesh);
                base_color.extend(h.base_color.iter().cloned().chain(std::iter::repeat(None)).take(h.mesh.materials.len()));
            }
            let bvh = Bvh::new(&all);
            MergedHigh { mesh: Arc::new(all), bvh: Arc::new(bvh), base_color: Arc::new(base_color) }
        });
    }
}

/// A self-contained bake task that can run on another thread.
pub struct BakeTask {
    pub ts: usize,
    pub mesh: Arc<Mesh>,
    pub bvh: Arc<Bvh>,
    pub material: usize,
    pub settings: BakeSettings,
    /// The merged high polys, unless the set bakes from the low poly.
    pub high: Option<MergedHigh>,
    pub cage: Option<Arc<Mesh>>,
    pub skew: Option<Arc<SkewMap>>,
    /// UV tile grid (columns, rows) of the set; (1, 1) without UV tiles.
    pub tiles: (u32, u32),
    /// The GPU ray tracer, if installed and not turned off (`bake.run {cpu: true}`).
    pub tracer: Option<Arc<dyn Tracer>>,
}

impl BakeTask {
    pub fn run(&self, progress: &Progress) -> Result<BakeOutput> {
        let high = self.high.as_ref().map(|h| HighPoly { mesh: &h.mesh, bvh: &h.bvh, base_color: &h.base_color });
        let req = BakeRequest {
            mesh: &self.mesh,
            bvh: &self.bvh,
            material: self.material,
            settings: &self.settings,
            high,
            cage: self.cage.as_deref(),
            skew: self.skew.as_deref(),
            tiles: self.tiles,
        };
        match &self.tracer {
            Some(t) => bake_on(&req, progress, t.as_ref()),
            None => bake(&req, progress),
        }
        .map_err(|e| EngineError::Other(format!("bake failed: {e}")))
    }
}

/// A mesh file name made unique among `taken` (and safe as a project file entry).
fn unique_name(file_name: &str, taken: &[&str]) -> String {
    let leaf = std::path::Path::new(file_name).file_name().and_then(|n| n.to_str()).unwrap_or("mesh");
    let base = texturecraft_formats::sanitize_resource_name(leaf);
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name.as_str()) {
        name = format!("{n}_{base}");
        n += 1;
    }
    name
}

fn parse_mesh(file_name: &str, bytes: Vec<u8>) -> Result<BakeMesh> {
    let h = texturecraft_formats::import_high_poly_bytes(file_name, &bytes).map_err(|e| EngineError::Other(format!("{file_name}: {e}")))?;
    let base_color = h.base_color.into_iter().map(|c| c.map(Arc::new)).collect();
    Ok(BakeMesh { name: file_name.to_string(), bytes: Arc::new(bytes), mesh: Arc::new(h.mesh), base_color })
}

impl Session {
    pub fn bake_task(&self, ts: usize) -> Result<BakeTask> {
        let set = self.doc.project.texture_set(ts)?;
        let high = if set.bake.high_poly.use_low_as_high { None } else { self.bake_meshes.merged.clone() };
        let cage = high.as_ref().and(self.bake_meshes.cage.as_ref()).map(|c| c.mesh.clone());
        let skew = high.as_ref().and(self.bake_meshes.skew.clone());
        let tiles = set.tile_grid().map_or((1, 1), |g| (g.cols, g.rows));
        let tracer = GPU_TRACER.get().cloned();
        Ok(BakeTask { ts, mesh: self.mesh.clone(), bvh: self.bvh.clone(), material: set.material, settings: set.bake.clone(), high, cage, skew, tiles, tracer })
    }

    /// Store a finished bake (one undo step).
    pub fn apply_bake(&mut self, ts: usize, out: BakeOutput) -> Result<()> {
        self.log.push(format!("Baked texture set {ts} ({})", out.tracer));
        self.edit("Bake mesh maps", |s| {
            let maps: BTreeSet<BakedMap> = out.maps.keys().copied().collect();
            let entry = s.doc.baked.entry(ts).or_default();
            entry.resolution = out.resolution;
            for (m, data) in out.maps {
                entry.maps.insert(m, Arc::new(data));
            }
            s.doc.project.texture_set_mut(ts)?.baked.extend(maps);
            Ok(())
        })?;
        Ok(())
    }

    /// Bake texture set `ts` now (blocking), on the CPU when `cpu`; returns where the rays ran.
    pub fn bake_now(&mut self, ts: usize, cpu: bool) -> Result<String> {
        let mut task = self.bake_task(ts)?;
        if cpu {
            task.tracer = None;
        }
        let out = task.run(&Progress::new())?;
        let tracer = out.tracer.clone();
        self.apply_bake(ts, out)?;
        Ok(tracer)
    }

    /// Add a high-definition mesh from file bytes (the web picker, the desktop dialog and
    /// `bake.highPoly.add`); every texture set then bakes from the high polys. Returns its name.
    pub fn add_high_poly_bytes(&mut self, file_name: &str, bytes: Vec<u8>) -> Result<String> {
        let mut m = parse_mesh(file_name, bytes)?;
        let taken: Vec<&str> = self.bake_meshes.high.iter().map(|h| h.name.as_str()).collect();
        m.name = unique_name(file_name, &taken);
        let textures = m.base_color.iter().flatten().filter(|c| !c.texels.is_empty()).count();
        self.log.push(format!("High poly {}: {} triangles, {textures} base color textures", m.name, m.mesh.triangle_count()));
        let name = m.name.clone();
        self.bake_meshes.high.push(m);
        self.bake_meshes.rebuild();
        for set in &mut self.doc.project.texture_sets {
            set.bake.high_poly.use_low_as_high = false;
        }
        self.modified = true;
        Ok(name)
    }

    /// Remove high-definition mesh `index`.
    pub fn remove_high_poly(&mut self, index: usize) -> Result<BakeMesh> {
        if index >= self.bake_meshes.high.len() {
            return Err(EngineError::Other(format!("no high poly mesh {index}")));
        }
        let removed = self.bake_meshes.high.remove(index);
        self.bake_meshes.rebuild();
        self.modified = true;
        Ok(removed)
    }

    /// Use a cage mesh (same triangles as the low poly) from file bytes.
    pub fn set_cage_bytes(&mut self, file_name: &str, bytes: Vec<u8>) -> Result<String> {
        let mut m = parse_mesh(file_name, bytes)?;
        if m.mesh.triangle_count() != self.mesh.triangle_count() {
            return Err(EngineError::Other(format!(
                "the cage has {} triangles but the low poly has {}; export the cage from the low poly",
                m.mesh.triangle_count(),
                self.mesh.triangle_count()
            )));
        }
        m.name = unique_name(file_name, &[]);
        m.base_color.clear();
        let name = m.name.clone();
        self.bake_meshes.cage = Some(m);
        self.modified = true;
        Ok(name)
    }

    /// Add mesh file `f` as a high-definition mesh, or set it as the cage, logging what it could
    /// not fold in. Replies like `bake.highPoly.add` / `bake.cage.set`: `{name, meshes}`.
    pub fn add_bake_mesh(&mut self, f: texturecraft_formats::MeshFile, cage: bool) -> Result<Value> {
        let name = if cage { self.set_cage_bytes(&f.name, f.bytes)? } else { self.add_high_poly_bytes(&f.name, f.bytes)? };
        self.log.extend(f.warnings);
        Ok(json!({ "name": name, "meshes": meshes_json(self) }))
    }

    /// Restore the bake meshes saved in a project file; unreadable ones are logged and skipped.
    pub(crate) fn load_bake_meshes(&mut self, high: Vec<MeshFile>, cage: Option<MeshFile>) {
        for (name, bytes) in high {
            match parse_mesh(&name, bytes) {
                Ok(m) => self.bake_meshes.high.push(m),
                Err(e) => self.log.push(format!("skipped high poly mesh: {e}")),
            }
        }
        self.bake_meshes.rebuild();
        if let Some((name, bytes)) = cage
            && let Err(e) = self.set_cage_bytes(&name, bytes)
        {
            self.log.push(format!("skipped cage mesh: {e}"));
        }
    }

    /// The skew map of a project file, if it passes the baker's checks.
    pub(crate) fn load_skew(&mut self, skew: Option<texturecraft_formats::SkewData>) {
        let Some(k) = skew else { return };
        let map = SkewMap { width: k.width, height: k.height, values: k.values, edge_distance: k.edge_distance, edge_contrast: k.edge_contrast };
        match map.validate() {
            Ok(()) => {
                self.bake_meshes.skew = Some(Arc::new(map));
                self.bake_meshes.skew_rev += 1;
            }
            Err(e) => self.log.push(format!("skipped skew map: {e}")),
        }
    }

    /// The skew map for a project file.
    pub(crate) fn saved_skew(&self) -> Option<texturecraft_formats::SkewData> {
        let k = self.bake_meshes.skew.as_ref()?;
        Some(texturecraft_formats::SkewData {
            width: k.width,
            height: k.height,
            values: k.values.clone(),
            edge_distance: k.edge_distance,
            edge_contrast: k.edge_contrast,
        })
    }

    /// The bake meshes for a project file: (high polys, cage).
    pub(crate) fn saved_bake_meshes(&self) -> (Vec<MeshFile>, Option<MeshFile>) {
        let entry = |m: &BakeMesh| (m.name.clone(), m.bytes.as_ref().clone());
        (self.bake_meshes.high.iter().map(entry).collect(), self.bake_meshes.cage.as_ref().map(entry))
    }
}

/// `bake.highPoly.list`: the high polys and the cage (also part of `project.inspect`).
pub fn meshes_json(s: &Session) -> Value {
    let one = |m: &BakeMesh| json!({ "name": m.name, "triangles": m.mesh.triangle_count(), "parts": m.mesh.submeshes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>() });
    let skew =
        s.bake_meshes.skew.as_ref().map(|k| json!({ "width": k.width, "height": k.height, "edgeDistance": k.edge_distance, "edgeContrast": k.edge_contrast }));
    json!({ "highPoly": s.bake_meshes.high.iter().map(one).collect::<Vec<_>>(), "cage": s.bake_meshes.cage.as_ref().map(one), "skew": skew })
}

#[cfg(not(target_arch = "wasm32"))]
fn read_mesh_file(cmd: &str, p: &Value) -> Result<texturecraft_formats::MeshFile> {
    let path = strp(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
    crate::file_cmds::read_mesh(cmd, path)
}

#[cfg(target_arch = "wasm32")]
fn read_mesh_file(_: &str, _: &Value) -> Result<texturecraft_formats::MeshFile> {
    Err(EngineError::Other("file paths are not available in the browser; use the bake panel's file picker".into()))
}

fn high_add(s: &mut Session, p: &Value) -> Result<Value> {
    s.add_bake_mesh(read_mesh_file("bake.highPoly.add", p)?, false)
}

fn high_remove(s: &mut Session, p: &Value) -> Result<Value> {
    if boolp(p, "all") == Some(true) {
        s.bake_meshes.high.clear();
        s.bake_meshes.rebuild();
        s.modified = true;
        return Ok(meshes_json(s));
    }
    let index = match (u64p(p, "index"), strp(p, "name")) {
        (Some(i), _) => usize::try_from(i).unwrap_or(usize::MAX),
        (None, Some(n)) => s.bake_meshes.high.iter().position(|h| h.name == n).ok_or_else(|| bad("bake.highPoly.remove", format!("no high poly `{n}`")))?,
        (None, None) => return Err(bad("bake.highPoly.remove", "`index`, `name` or `all` is required")),
    };
    s.remove_high_poly(index)?;
    Ok(meshes_json(s))
}

/// The greyscale skew map of an image file (the mean of its colour channels, as stored).
#[cfg(not(target_arch = "wasm32"))]
fn skew_image(path: &str) -> Result<(u32, u32, Vec<f32>)> {
    const MAX_BYTES: u64 = 256 << 20;
    let cmd = "bake.skew.set";
    let len = std::fs::metadata(path).map_err(|e| bad(cmd, format!("cannot read {path}: {e}")))?.len();
    if len > MAX_BYTES {
        return Err(bad(cmd, format!("{path} is over 256 MiB")));
    }
    let bytes = std::fs::read(path).map_err(|e| bad(cmd, format!("cannot read {path}: {e}")))?;
    // The size is checked from the header, before the pixels are decoded.
    let img = texturecraft_formats::decode_image_within(&bytes, texturecraft_bake::MAX_TEXELS).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    Ok((img.width, img.height, img.rgba.iter().map(|p| (p[0] + p[1] + p[2]) / 3.0).collect()))
}

#[cfg(target_arch = "wasm32")]
fn skew_image(_: &str) -> Result<(u32, u32, Vec<f32>)> {
    Err(EngineError::Other("file paths are not available in the browser".into()))
}

fn skew_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "bake.skew.set";
    let mut map = match strp(p, "path") {
        Some(path) => {
            let (width, height, values) = skew_image(path)?;
            SkewMap { width, height, values: values.into_iter().map(|v| v.clamp(0.0, 1.0)).collect(), ..SkewMap::constant(0.0) }
        }
        None => SkewMap::constant(f32p(p, "value").unwrap_or(1.0).clamp(0.0, 1.0)),
    };
    map.edge_distance = f32p(p, "edgeDistance").unwrap_or(0.0);
    map.edge_contrast = f32p(p, "edgeContrast").unwrap_or(1.0);
    map.validate().map_err(|e| bad(cmd, e.to_string()))?;
    s.bake_meshes.skew = Some(Arc::new(map));
    s.bake_meshes.skew_rev += 1;
    s.modified = true;
    Ok(meshes_json(s))
}

fn cage_set(s: &mut Session, p: &Value) -> Result<Value> {
    s.add_bake_mesh(read_mesh_file("bake.cage.set", p)?, true)
}

/// Texture sets named by `ts` (default: the active one).
fn sets_param(s: &Session, cmd: &str, p: &Value) -> Result<Vec<usize>> {
    let n = s.set_count();
    let one = |v: &Value| v.as_u64().map(|t| t as usize).filter(|t| *t < n).ok_or_else(|| bad(cmd, format!("invalid texture set {v}")));
    match p.get("ts") {
        Some(Value::String(a)) if a == "all" => Ok((0..n).collect()),
        Some(Value::String(a)) if a == "selected" => {
            Ok((0..n).filter(|t| s.doc.project.texture_sets.get(*t).is_some_and(|set| set.bake.common.is_enabled)).collect())
        }
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(v) => Ok(vec![one(v)?]),
        None => Ok(vec![s.ts()]),
    }
}

/// Parameters shared by the current bake command and UI
pub fn settings_params(settings: &BakeSettings) -> Value {
    let mut ao = rays_params(&settings.maps.ao.parameters.rays);
    ao["ignoreBackface"] = match settings.maps.ao.parameters.ignore_backface {
        BackfaceMode::Never => "never",
        BackfaceMode::Always => "always",
        BackfaceMode::ByMeshName => "byMeshName",
    }
    .into();
    ao["selfOcclusion"] = match settings.maps.ao.parameters.self_occlusion {
        SelfOcclusion::Always => "always",
        SelfOcclusion::SameMeshName => "sameMeshName",
    }
    .into();

    json!({
        "resolution": settings.common.resolution.pixels(),
        "antialiasing": settings.common.antialiasing.samples_per_axis(),
        "dilation": settings.common.dilation,
        "ao": ao,
        "thickness": rays_params(&settings.maps.thickness.parameters.rays),
        "bentNormal": rays_params(&settings.maps.bent_normal.parameters.rays),
        "curvatureMethod": match settings.maps.curvature.parameters.method { CurvatureMethod::NormalMap => "normalMap", CurvatureMethod::Mesh => "mesh" },
        "positionMode": match settings.maps.position.parameters.normalization { PositionMode::BoundingBox => "boundingBox", PositionMode::BoundingSphere => "boundingSphere", PositionMode::None => "none" },
        "maps": settings.maps.enabled().map(|m| m.id()).collect::<Vec<_>>(),
        "enabled": settings.common.is_enabled,
        "useLowAsHigh": settings.high_poly.use_low_as_high,
        "maxFrontal": settings.cage.max_frontal,
        "maxRear": settings.cage.max_rear,
        "averageNormals": settings.cage.average_normals,
        "lowSuffix": settings.high_poly.low_suffix,
        "highSuffix": settings.high_poly.high_suffix,
    })
}

/// Apply only the bake parameters supplied by the caller
fn apply_settings(settings: &mut BakeSettings, params: &Value, command: &str) -> Result<()> {
    // Keep the current command limits until the reference controls are integrated
    const MAX_DILATION_PIXELS: u32 = 256;
    const MAX_SUFFIX_CHARS: usize = 64;

    if let Some(model) = params.get("model") {
        let replacement: BakeSettings = serde_json::from_value(model.clone()).map_err(|error| bad(command, error.to_string()))?;
        validate_model_settings(&replacement, command)?;
        let is_enabled = settings.common.is_enabled;
        *settings = replacement;
        if boolp(params, "preserveEnabled") == Some(true) {
            settings.common.is_enabled = is_enabled;
        }
    }

    if let Some(resolution) = u64p(params, "resolution") {
        settings.common.resolution = u32::try_from(resolution)
            .ok()
            .and_then(BakeResolution::from_pixels)
            .ok_or_else(|| bad(command, "resolution must be a power of two from 16 to 8192"))?;
    }

    if let Some(antialiasing) = u64p(params, "antialiasing") {
        settings.common.antialiasing = u32::try_from(antialiasing)
            .ok()
            .and_then(BakeAntialiasing::from_samples_per_axis)
            .ok_or_else(|| bad(command, "antialiasing must be 1, 2, 4 or 8"))?;
    }

    if let Some(dilation) = u64p(params, "dilation") {
        settings.common.dilation = dilation.min(u64::from(MAX_DILATION_PIXELS)) as u32;
    }

    // Per-map values override the shared ray settings
    for ray_settings in [&mut settings.maps.ao.parameters.rays, &mut settings.maps.thickness.parameters.rays, &mut settings.maps.bent_normal.parameters.rays] {
        apply_rays(ray_settings, params, command)?;
    }

    for (key, ray_settings) in [
        ("ao", &mut settings.maps.ao.parameters.rays),
        ("thickness", &mut settings.maps.thickness.parameters.rays),
        ("bentNormal", &mut settings.maps.bent_normal.parameters.rays),
    ] {
        if let Some(map_params) = params.get(key) {
            apply_rays(ray_settings, map_params, command)?;
        }
    }

    if let Some(ao) = params.get("ao") {
        if let Some(value) = ao.get("ignoreBackface") {
            let mode = value.as_str().ok_or_else(|| bad(command, "ignoreBackface must be never, always or byMeshName"))?;
            settings.maps.ao.parameters.ignore_backface = match mode {
                "never" => BackfaceMode::Never,
                "always" => BackfaceMode::Always,
                "byMeshName" => BackfaceMode::ByMeshName,
                _ => return Err(bad(command, "ignoreBackface must be never, always or byMeshName")),
            };
        }

        if let Some(self_occlusion) = strp(ao, "selfOcclusion") {
            settings.maps.ao.parameters.self_occlusion = match self_occlusion {
                "always" => SelfOcclusion::Always,
                "sameMeshName" => SelfOcclusion::SameMeshName,
                _ => return Err(bad(command, "selfOcclusion must be always or sameMeshName")),
            };
        }
    }

    if let Some(position_mode) = strp(params, "positionMode") {
        settings.maps.position.parameters.normalization = match position_mode {
            "boundingBox" => PositionMode::BoundingBox,
            "boundingSphere" => PositionMode::BoundingSphere,
            "none" => PositionMode::None,
            _ => return Err(bad(command, "positionMode must be boundingBox, boundingSphere or none")),
        };
    }

    if let Some(value) = params.get("curvatureMethod") {
        let method = value.as_str().ok_or_else(|| bad(command, "curvatureMethod must be normalMap or mesh"))?;
        settings.maps.curvature.parameters.method = match method {
            "normalMap" => CurvatureMethod::NormalMap,
            "mesh" => CurvatureMethod::Mesh,
            _ => return Err(bad(command, "curvatureMethod must be normalMap or mesh")),
        };
    }

    for (key, setting, minimum, maximum) in [("maxFrontal", &mut settings.cage.max_frontal, 0.0, 1.0), ("maxRear", &mut settings.cage.max_rear, 0.0, 1.0)] {
        if let Some(value) = f32p(params, key).filter(|value| value.is_finite()) {
            *setting = value.clamp(minimum, maximum);
        }
    }

    for (key, setting) in [
        ("enabled", &mut settings.common.is_enabled),
        ("useLowAsHigh", &mut settings.high_poly.use_low_as_high),
        ("averageNormals", &mut settings.cage.average_normals),
    ] {
        if let Some(value) = boolp(params, key) {
            *setting = value;
        }
    }

    if let Some(maps) = params.get("maps").and_then(Value::as_array) {
        let selected = maps
            .iter()
            .map(|map| map.as_str().and_then(BakedMap::parse).ok_or_else(|| bad(command, format!("unknown mesh map {map}"))))
            .collect::<Result<Vec<_>>>()?;
        settings.maps.select(selected);
    }

    for (key, setting) in [("lowSuffix", &mut settings.high_poly.low_suffix), ("highSuffix", &mut settings.high_poly.high_suffix)] {
        if let Some(suffix) = strp(params, key) {
            *setting = suffix.chars().take(MAX_SUFFIX_CHARS).collect();
        }
    }

    Ok(())
}

fn validate_model_settings(settings: &BakeSettings, command: &str) -> Result<()> {
    let range = |name: &str, value: f32, minimum: f32, maximum: f32| {
        if value.is_finite() && (minimum..=maximum).contains(&value) { Ok(()) } else { Err(bad(command, format!("{name} must be in {minimum}..={maximum}"))) }
    };
    if settings.common.dilation > 128 {
        return Err(bad(command, "dilation must be at most 128"));
    }
    for rays in [&settings.maps.ao.parameters.rays, &settings.maps.thickness.parameters.rays, &settings.maps.bent_normal.parameters.rays] {
        if !(1..=256).contains(&rays.rays) {
            return Err(bad(command, "secondary rays must be in 1..=256"));
        }
        range("minimum occluder distance", rays.min_distance, 0.00001, 1.0)?;
        range("maximum occluder distance", rays.max_distance, 0.00001, if rays.relative_to_bounding_box { 1.0 } else { 10.0 })?;
        range("spread angle", rays.spread, 0.0, 180.0)?;
    }
    let curvature = &settings.maps.curvature.parameters;
    if !(1..=256).contains(&curvature.mesh.rays) {
        return Err(bad(command, "curvature rays must be in 1..=256"));
    }
    range("sampling radius", curvature.mesh.sampling_radius, 0.00001, if curvature.mesh.relative_to_bounding_box { 10.0 } else { 1.0 })?;
    for (name, value, minimum, maximum) in [
        ("frontal distance", settings.cage.max_frontal, 0.0, 0.05),
        ("rear distance", settings.cage.max_rear, 0.0, 0.05),
        ("edge distance", settings.skew.edge_distance, 0.0, 1.0),
        ("edge contrast", settings.skew.edge_contrast, 0.0, 1.0),
        ("ground plane offset", settings.maps.ao.parameters.ground_plane.offset, 0.0, 100.0),
        ("curvature details", curvature.normal_map.details, 0.0, 1.0),
        ("seams intensity", curvature.normal_map.seams_intensity, 0.0, 1.0),
        ("value bounds minimum", curvature.mesh.value_min, -10.0, 10.0),
        ("value bounds maximum", curvature.mesh.value_max, -10.0, 10.0),
        ("height scaling divisor", settings.maps.height.parameters.scaling_divisor, 0.00001, 100.0),
    ] {
        range(name, value, minimum, maximum)?;
    }
    Ok(())
}

/// Ray settings supported by the current bake command
fn rays_params(settings: &RaySettings) -> Value {
    json!({
        "rays": settings.rays,
        "maxDistance": settings.max_distance,
        "spread": settings.spread,
        "distribution": match settings.distribution {
            RayDistribution::Cosine => "cosine",
            RayDistribution::Uniform => "uniform",
        },
    })
}

/// Apply only the ray parameters supplied by the caller
fn apply_rays(settings: &mut RaySettings, params: &Value, command: &str) -> Result<()> {
    if let Some(count) = u64p(params, "rays") {
        settings.rays = count.clamp(1, u64::from(MAX_RAYS)) as u32;
    }

    for (key, setting, minimum, maximum) in [("maxDistance", &mut settings.max_distance, 0.001, 10.0), ("spread", &mut settings.spread, 0.0, 180.0)] {
        if let Some(value) = f32p(params, key).filter(|value| value.is_finite()) {
            *setting = value.clamp(minimum, maximum);
        }
    }

    if let Some(distribution) = strp(params, "distribution") {
        settings.distribution = match distribution {
            "cosine" => RayDistribution::Cosine,
            "uniform" => RayDistribution::Uniform,
            _ => return Err(bad(command, "distribution must be cosine or uniform")),
        };
    }

    Ok(())
}

/// Validate every target before applying settings
fn set_settings(session: &mut Session, params: &Value, command: &str) -> Result<Vec<usize>> {
    let sets = sets_param(session, command, params)?;
    let mut next = Vec::with_capacity(sets.len());
    for &set_index in &sets {
        let mut settings = session.doc.project.texture_set(set_index)?.bake.clone();
        apply_settings(&mut settings, params, command)?;
        next.push((set_index, settings));
    }

    for (set_index, settings) in next {
        let texture_set = session.doc.project.texture_set_mut(set_index)?;
        if texture_set.bake != settings {
            texture_set.bake = settings;
            session.modified = true;
        }
    }

    Ok(sets)
}

fn settings(session: &mut Session, params: &Value) -> Result<Value> {
    let sets = set_settings(session, params, "bake.settings")?;
    let result: Vec<Value> = sets
        .iter()
        .filter_map(|set_index| {
            session
                .doc
                .project
                .texture_sets
                .get(*set_index)
                .map(|texture_set| json!({ "ts": set_index, "settings": settings_params(&texture_set.bake), "model": &texture_set.bake }))
        })
        .collect();

    Ok(json!(result))
}

fn bake_run(s: &mut Session, p: &Value) -> Result<Value> {
    let sets = set_settings(s, p, "bake.run")?;
    let cpu = boolp(p, "cpu") == Some(true);
    let tracer = sets.iter().map(|&ts| s.bake_now(ts, cpu)).collect::<Result<Vec<_>>>()?;
    s.update();
    Ok(json!({ "baked": sets, "tracer": tracer }))
}

fn skew_paint(s: &mut Session, p: &Value) -> Result<Value> {
    use texturecraft_geom::glam::Vec2;
    let cmd = "bake.skew.paint";
    const MAX_POINTS: usize = 100_000;
    let points: Vec<Vec2> = p
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, "`points` is required: [[u, v], ...]"))?
        .iter()
        .map(|q| match (q.get(0).and_then(Value::as_f64), q.get(1).and_then(Value::as_f64)) {
            (Some(u), Some(v)) => Ok(Vec2::new(u as f32, v as f32)),
            _ => Err(bad(cmd, "each point is [u, v]")),
        })
        .collect::<Result<_>>()?;
    if points.is_empty() || points.len() > MAX_POINTS {
        return Err(bad(cmd, format!("give 1 to {MAX_POINTS} points")));
    }
    let size = p.get("size").and_then(Value::as_u64).unwrap_or(1024);
    if !(16..=4096).contains(&size) {
        return Err(bad(cmd, "`size` is 16 to 4096"));
    }
    let (value, radius) = (f32p(p, "value").unwrap_or(1.0), f32p(p, "radius").unwrap_or(0.03));
    if !(radius.is_finite() && radius > 0.0 && radius <= 1.0) {
        return Err(bad(cmd, "`radius` is above 0 and at most 1 (uv units)"));
    }
    let (hardness, strength) = (f32p(p, "hardness").unwrap_or(0.5), f32p(p, "strength").unwrap_or(1.0));
    let stroke = |m: &mut SkewMap| m.paint(&points, radius, value, hardness, strength).map_err(|e| bad(cmd, e.to_string()));
    match s.bake_meshes.skew.as_mut() {
        // In place: the map is copied only while a running bake still holds it, and `paint`
        // refuses a stroke before it changes a texel.
        Some(k) if k.values.len() > 1 => stroke(Arc::make_mut(k))?,
        // A constant map has no resolution to paint at: widen it, keeping its value everywhere.
        k => {
            let mut map = match k {
                Some(k) => k.resized(size as u32, size as u32),
                None => SkewMap::constant(0.0).resized(size as u32, size as u32),
            }
            .map_err(|e| bad(cmd, e.to_string()))?;
            stroke(&mut map)?;
            s.bake_meshes.skew = Some(Arc::new(map));
        }
    }
    s.bake_meshes.skew_rev += 1;
    s.modified = true;
    Ok(meshes_json(s))
}
