//! Mesh-map baking commands (Painter's "Bake Mesh Maps" window): bake settings, the
//! high-definition mesh list, the cage, and running a bake.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};
use texturecraft_bake::{BakeOutput, BakeRequest, HighPoly, MAX_RAYS, Progress, bake};
use texturecraft_core::{BakeMatch, BakeSettings, BakedMap};
use texturecraft_geom::{Bvh, Mesh};

use crate::commands::{CommandSpec, always, bad, boolp, f32p, strp, u64p};
use crate::{EngineError, Result, Session, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("bake.run", "Bake mesh maps", ["Mode"], None, "{ts?: index|[indices]|\"all\"|\"selected\", ...any bake.settings parameter}", always, bake_run),
        cmd!(
            "bake.settings",
            "Bake settings",
            [],
            None,
            "{ts?: index|[indices]|\"all\"|\"selected\" (default: the active set), resolution?, antialiasing?: 1|2|4|8, dilation?, rays?, maxDistance?, curvatureStrength?, curvatureFromNormal?, direction?: [x,y,z], maps?: [ids], enabled?, useLowAsHigh?, maxFrontal?, maxRear?, averageNormals?, matching?: always|byMeshName, lowSuffix?, highSuffix?}",
            always,
            settings
        ),
        cmd!("bake.highPoly.add", "Add high definition mesh", [], None, "{path} — a mesh file (glb, gltf, fbx, dae, obj, ply, stl)", always, high_add),
        cmd!("bake.highPoly.remove", "Remove high definition mesh", [], None, "{index? | name? | all?: true}", always, high_remove),
        cmd!("bake.highPoly.list", "List high definition meshes", [], None, "{}", always, |s, _| Ok(meshes_json(s))),
        cmd!("bake.cage.set", "Load cage mesh", [], None, "{path} — a mesh with the low poly's triangles", always, cage_set),
        cmd!("bake.cage.clear", "Remove cage mesh", [], None, "{}", always, |s, _| {
            s.bake_meshes.cage = None;
            s.modified = true;
            Ok(meshes_json(s))
        }),
    ]
}

/// A mesh file as saved in a project: (file name, bytes).
type MeshFile = (String, Vec<u8>);

/// A mesh file the baker reads (high poly or cage): its name, original bytes (saved in the
/// project) and parsed mesh.
#[derive(Clone, Debug)]
pub struct BakeMesh {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub mesh: Arc<Mesh>,
}

/// The high-definition meshes and the cage. Like the low-poly mesh they are not part of the
/// undo history.
#[derive(Clone, Debug, Default)]
pub struct BakeMeshes {
    pub high: Vec<BakeMesh>,
    pub cage: Option<BakeMesh>,
    /// Every high poly appended into one mesh, with its BVH (rebuilt when the list changes).
    merged: Option<(Arc<Mesh>, Arc<Bvh>)>,
}

impl BakeMeshes {
    // shortcut: the merged copy doubles high-poly memory; build the BVH over the parts in place if
    // multi-million-triangle sets run out of memory.
    fn rebuild(&mut self) {
        self.merged = (!self.high.is_empty()).then(|| {
            let mut all = Mesh::default();
            for h in &self.high {
                all.append(&h.mesh);
            }
            let bvh = Bvh::new(&all);
            (Arc::new(all), Arc::new(bvh))
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
    pub high: Option<(Arc<Mesh>, Arc<Bvh>)>,
    pub cage: Option<Arc<Mesh>>,
    /// UV tile grid (columns, rows) of the set; (1, 1) without UV tiles.
    pub tiles: (u32, u32),
}

impl BakeTask {
    pub fn run(&self, progress: &Progress) -> Result<BakeOutput> {
        let high = self.high.as_ref().map(|(mesh, bvh)| HighPoly { mesh, bvh });
        let req = BakeRequest {
            mesh: &self.mesh,
            bvh: &self.bvh,
            material: self.material,
            settings: &self.settings,
            high,
            cage: self.cage.as_deref(),
            tiles: self.tiles,
        };
        bake(&req, progress).map_err(|e| EngineError::Other(format!("bake failed: {e}")))
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

fn parse_mesh(file_name: &str, bytes: Vec<u8>) -> Result<(Arc<Vec<u8>>, Arc<Mesh>)> {
    let mesh = texturecraft_formats::import_high_poly_bytes(file_name, &bytes).map_err(|e| EngineError::Other(format!("{file_name}: {e}")))?;
    Ok((Arc::new(bytes), Arc::new(mesh)))
}

impl Session {
    pub fn bake_task(&self, ts: usize) -> Result<BakeTask> {
        let set = self.doc.project.texture_set(ts)?;
        let high = if set.bake.use_low_as_high { None } else { self.bake_meshes.merged.clone() };
        let cage = high.as_ref().and(self.bake_meshes.cage.as_ref()).map(|c| c.mesh.clone());
        let tiles = set.tile_grid().map_or((1, 1), |g| (g.cols, g.rows));
        Ok(BakeTask { ts, mesh: self.mesh.clone(), bvh: self.bvh.clone(), material: set.material, settings: set.bake.clone(), high, cage, tiles })
    }

    /// Store a finished bake (one undo step).
    pub fn apply_bake(&mut self, ts: usize, out: BakeOutput) -> Result<()> {
        self.edit("Bake mesh maps", |s| {
            let maps: BTreeSet<BakedMap> = out.maps.keys().copied().collect();
            let entry = s.doc.baked.entry(ts).or_default();
            entry.resolution = out.resolution;
            for (m, data) in out.maps {
                entry.maps.insert(m, Arc::new(data));
            }
            s.doc.project.texture_set_mut(ts)?.baked.extend(maps);
            Ok(())
        })
    }

    /// Bake texture set `ts` now (blocking).
    pub fn bake_now(&mut self, ts: usize) -> Result<()> {
        let task = self.bake_task(ts)?;
        let out = task.run(&Progress::new())?;
        self.apply_bake(ts, out)
    }

    /// Add a high-definition mesh from file bytes (the web picker, the desktop dialog and
    /// `bake.highPoly.add`); every texture set then bakes from the high polys. Returns its name.
    pub fn add_high_poly_bytes(&mut self, file_name: &str, bytes: Vec<u8>) -> Result<String> {
        let (bytes, mesh) = parse_mesh(file_name, bytes)?;
        let taken: Vec<&str> = self.bake_meshes.high.iter().map(|h| h.name.as_str()).collect();
        let name = unique_name(file_name, &taken);
        self.log.push(format!("High poly {name}: {} triangles", mesh.triangle_count()));
        self.bake_meshes.high.push(BakeMesh { name: name.clone(), bytes, mesh });
        self.bake_meshes.rebuild();
        for set in &mut self.doc.project.texture_sets {
            set.bake.use_low_as_high = false;
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
        let (bytes, mesh) = parse_mesh(file_name, bytes)?;
        if mesh.triangle_count() != self.mesh.triangle_count() {
            return Err(EngineError::Other(format!(
                "the cage has {} triangles but the low poly has {}; export the cage from the low poly",
                mesh.triangle_count(),
                self.mesh.triangle_count()
            )));
        }
        let name = unique_name(file_name, &[]);
        self.bake_meshes.cage = Some(BakeMesh { name: name.clone(), bytes, mesh });
        self.modified = true;
        Ok(name)
    }

    /// Restore the bake meshes saved in a project file; unreadable ones are logged and skipped.
    pub(crate) fn load_bake_meshes(&mut self, high: Vec<MeshFile>, cage: Option<MeshFile>) {
        for (name, bytes) in high {
            match parse_mesh(&name, bytes) {
                Ok((bytes, mesh)) => self.bake_meshes.high.push(BakeMesh { name, bytes, mesh }),
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

    /// The bake meshes for a project file: (high polys, cage).
    pub(crate) fn saved_bake_meshes(&self) -> (Vec<MeshFile>, Option<MeshFile>) {
        let entry = |m: &BakeMesh| (m.name.clone(), m.bytes.as_ref().clone());
        (self.bake_meshes.high.iter().map(entry).collect(), self.bake_meshes.cage.as_ref().map(entry))
    }
}

/// `bake.highPoly.list`: the high polys and the cage (also part of `project.inspect`).
pub fn meshes_json(s: &Session) -> Value {
    let one = |m: &BakeMesh| json!({ "name": m.name, "triangles": m.mesh.triangle_count(), "parts": m.mesh.submeshes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>() });
    json!({ "highPoly": s.bake_meshes.high.iter().map(one).collect::<Vec<_>>(), "cage": s.bake_meshes.cage.as_ref().map(one) })
}

#[cfg(not(target_arch = "wasm32"))]
fn read_mesh_file(cmd: &str, p: &Value) -> Result<(String, Vec<u8>)> {
    let path = strp(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
    let meta = std::fs::metadata(path).map_err(|e| bad(cmd, format!("cannot read {path}: {e}")))?;
    if meta.len() > texturecraft_formats::mesh_import::MAX_MESH_FILE_BYTES {
        return Err(bad(cmd, format!("{path} is too large")));
    }
    let bytes = std::fs::read(path).map_err(|e| bad(cmd, format!("cannot read {path}: {e}")))?;
    Ok((path.to_string(), bytes))
}

#[cfg(target_arch = "wasm32")]
fn read_mesh_file(_: &str, _: &Value) -> Result<(String, Vec<u8>)> {
    Err(EngineError::Other("file paths are not available in the browser; use the bake panel's file picker".into()))
}

fn high_add(s: &mut Session, p: &Value) -> Result<Value> {
    let (path, bytes) = read_mesh_file("bake.highPoly.add", p)?;
    let name = s.add_high_poly_bytes(&path, bytes)?;
    Ok(json!({ "name": name, "meshes": meshes_json(s) }))
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

fn cage_set(s: &mut Session, p: &Value) -> Result<Value> {
    let (path, bytes) = read_mesh_file("bake.cage.set", p)?;
    let name = s.set_cage_bytes(&path, bytes)?;
    Ok(json!({ "name": name, "meshes": meshes_json(s) }))
}

/// Texture sets named by `ts` (default: the active one).
fn sets_param(s: &Session, cmd: &str, p: &Value) -> Result<Vec<usize>> {
    let n = s.set_count();
    let one = |v: &Value| v.as_u64().map(|t| t as usize).filter(|t| *t < n).ok_or_else(|| bad(cmd, format!("invalid texture set {v}")));
    match p.get("ts") {
        Some(Value::String(a)) if a == "all" => Ok((0..n).collect()),
        Some(Value::String(a)) if a == "selected" => Ok((0..n).filter(|t| s.doc.project.texture_sets.get(*t).is_some_and(|set| set.bake.enabled)).collect()),
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(v) => Ok(vec![one(v)?]),
        None => Ok(vec![s.ts()]),
    }
}

/// `b` as `bake.settings` parameters (what the command returns and the UI sends back).
pub fn settings_params(b: &BakeSettings) -> Value {
    json!({
        "resolution": b.resolution,
        "antialiasing": b.antialiasing,
        "dilation": b.dilation,
        "rays": b.rays,
        "maxDistance": b.max_distance,
        "curvatureStrength": b.curvature_strength,
        "curvatureFromNormal": b.curvature_from_normal,
        "direction": b.direction,
        "maps": b.maps.iter().map(|m| m.id()).collect::<Vec<_>>(),
        "enabled": b.enabled,
        "useLowAsHigh": b.use_low_as_high,
        "maxFrontal": b.max_frontal,
        "maxRear": b.max_rear,
        "averageNormals": b.average_normals,
        "matching": match b.matching { BakeMatch::Always => "always", BakeMatch::ByMeshName => "byMeshName" },
        "lowSuffix": b.low_suffix,
        "highSuffix": b.high_suffix,
    })
}

/// Apply the `bake.settings` parameters present in `p` to `b`.
fn apply_settings(b: &mut BakeSettings, p: &Value, cmd: &str) -> Result<()> {
    if let Some(r) = u64p(p, "resolution") {
        b.resolution = crate::session::check_resolution(u32::try_from(r).unwrap_or(0))?;
    }
    if let Some(a) = u64p(p, "antialiasing") {
        if ![1, 2, 4, 8].contains(&a) {
            return Err(bad(cmd, "antialiasing must be 1, 2, 4 or 8"));
        }
        b.antialiasing = a as u32;
    }
    if let Some(d) = u64p(p, "dilation") {
        b.dilation = d.min(256) as u32;
    }
    if let Some(r) = u64p(p, "rays") {
        b.rays = r.clamp(1, u64::from(MAX_RAYS)) as u32;
    }
    for (k, slot, lo, hi) in [
        ("maxDistance", &mut b.max_distance, 0.001, 10.0),
        ("curvatureStrength", &mut b.curvature_strength, 0.0, 16.0),
        ("maxFrontal", &mut b.max_frontal, 0.0, 1.0),
        ("maxRear", &mut b.max_rear, 0.0, 1.0),
    ] {
        if let Some(v) = f32p(p, k).filter(|v| v.is_finite()) {
            *slot = v.clamp(lo, hi);
        }
    }
    for (k, slot) in [
        ("curvatureFromNormal", &mut b.curvature_from_normal),
        ("enabled", &mut b.enabled),
        ("useLowAsHigh", &mut b.use_low_as_high),
        ("averageNormals", &mut b.average_normals),
    ] {
        if let Some(v) = boolp(p, k) {
            *slot = v;
        }
    }
    if let Some(d) = p.get("direction") {
        let v: Vec<f32> = d.as_array().into_iter().flatten().filter_map(Value::as_f64).map(|x| x as f32).collect();
        match v[..] {
            [x, y, z] if [x, y, z].iter().all(|c| c.is_finite()) && [x, y, z] != [0.0; 3] => b.direction = [x, y, z],
            _ => return Err(bad(cmd, "`direction` must be three finite numbers, not all zero")),
        }
    }
    if let Some(a) = p.get("maps").and_then(Value::as_array) {
        b.maps = a.iter().map(|v| v.as_str().and_then(BakedMap::parse).ok_or_else(|| bad(cmd, format!("unknown mesh map {v}")))).collect::<Result<_>>()?;
    }
    match strp(p, "matching") {
        Some("always") => b.matching = BakeMatch::Always,
        Some("byMeshName") | Some("byName") => b.matching = BakeMatch::ByMeshName,
        Some(o) => return Err(bad(cmd, format!("unknown matching `{o}` (always|byMeshName)"))),
        None => {}
    }
    for (k, slot) in [("lowSuffix", &mut b.low_suffix), ("highSuffix", &mut b.high_suffix)] {
        if let Some(v) = strp(p, k) {
            *slot = v.chars().take(64).collect();
        }
    }
    Ok(())
}

/// Validate and apply the settings parameters to every set first, so a bad parameter changes
/// nothing.
fn set_settings(s: &mut Session, p: &Value, cmd: &str) -> Result<Vec<usize>> {
    let sets = sets_param(s, cmd, p)?;
    let mut next = Vec::with_capacity(sets.len());
    for &ts in &sets {
        let mut b = s.doc.project.texture_set(ts)?.bake.clone();
        apply_settings(&mut b, p, cmd)?;
        next.push((ts, b));
    }
    for (ts, b) in next {
        let set = s.doc.project.texture_set_mut(ts)?;
        if set.bake != b {
            set.bake = b;
            s.modified = true;
        }
    }
    Ok(sets)
}

fn settings(s: &mut Session, p: &Value) -> Result<Value> {
    let sets = set_settings(s, p, "bake.settings")?;
    let out: Vec<Value> =
        sets.iter().filter_map(|t| s.doc.project.texture_sets.get(*t).map(|set| json!({ "ts": t, "settings": settings_params(&set.bake) }))).collect();
    Ok(json!(out))
}

fn bake_run(s: &mut Session, p: &Value) -> Result<Value> {
    let sets = set_settings(s, p, "bake.run")?;
    for &ts in &sets {
        s.bake_now(ts)?;
    }
    s.update();
    Ok(json!({ "baked": sets }))
}
