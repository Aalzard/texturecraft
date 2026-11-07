//! User smart materials: a layer (or folder) saved with its pixels and images as a `.tcmat` file
//! (the `.tcraft` container holding a one-texture-set project), and added back above the
//! selection in any project.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;
use texturecraft_compose::PixelStore;
use texturecraft_core::{EffectKind, LayerId, LayerKind, Project, Source, TextureSet};

use crate::commands::{CommandSpec, always, has_layer};
use crate::materials::Place;
use crate::session::{Document, StackTarget};
use crate::{EngineError, Result, Session, cmd};

/// Smart material file extension.
pub const SMART_MATERIAL_EXT: &str = "tcmat";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("material.save", "Create smart material", ["Layers"], None, "{layer?, path: .tcmat file, name?}", has_layer, save),
        cmd!(
            "material.load",
            "Add smart material from file",
            [],
            None,
            "{path: .tcmat smart material, .scsmart SubCraft smart material, .mcmat MaterialCraft material or .mcraft package, above?: layer id, inside?: bool, index?: n} — above the selection by default, or placed like material.apply; a .scsmart also returns its `import` report",
            always,
            load
        ),
        cmd!(
            "material.importScsmart",
            "Import SubCraft smart material",
            [],
            None,
            "{path: .scsmart file, above?: layer id, inside?: bool, index?: n} — maps the SubCraft smart material (folder, fills, masks, generators, images) to layers; returns {layer, name, import: {name, schema_version, layers, masks, effects, images, issues: [{severity: note|approximated|dropped, layer, what, message, count}]}}. What cannot be mapped is reported, never an error",
            always,
            import_scsmart
        ),
    ]
}

fn fe(e: texturecraft_formats::FormatError) -> EngineError {
    EngineError::Other(e.to_string())
}

/// Image resources a layer subtree uses (fill sources and fill effects).
pub(crate) fn images_used(p: &Project, id: LayerId, out: &mut Vec<String>, depth: usize) {
    if depth > texturecraft_core::project::MAX_DEPTH {
        return;
    }
    let Some(l) = p.layers.get(&id) else { return };
    if let LayerKind::Fill(f) = &l.kind {
        for s in f.sources.values() {
            if let Source::Image { name, .. } = s {
                out.push(name.clone());
            }
        }
        out.extend(f.link.as_ref().map(|link| link.package.clone()));
    }
    for e in l.mask.iter().flat_map(|m| m.effects.iter()) {
        if let EffectKind::Fill { source: Source::Image { name, .. }, .. } = &e.kind {
            out.push(name.clone());
        }
    }
    for c in l.children() {
        images_used(p, *c, out, depth + 1);
    }
}

impl Session {
    /// Serialize layer `id` (with children, pixels and images) as a smart material file.
    pub fn smart_material_bytes(&self, id: LayerId, name: &str) -> Result<Vec<u8>> {
        let (ts, _, _) = self.doc.project.locate(id).ok_or_else(|| EngineError::Other("layer not in a stack".into()))?;
        let src_set = self.doc.project.texture_set(ts)?;
        let mut project = Project::new(name, "builtin:none");
        let mut set = TextureSet::new(name, 0, src_set.resolution);
        set.uv_tiles = src_set.uv_tiles.clone();
        set.tile_resolutions = src_set.tile_resolutions.clone();
        set.channels = src_set.channels.clone();
        project.texture_sets.push(set);
        let mut mini = Document { bake_paint: BTreeMap::new(), project, pixels: PixelStore::default(), baked: BTreeMap::new(), resources: BTreeMap::new() };
        let new = crate::layer_ops::copy_tree(&self.doc, id, &mut mini, 0, None, false, 0)?;
        if let Ok(l) = mini.project.layer_mut(new) {
            l.name = name.to_string();
        }
        let mut used = Vec::new();
        images_used(&mini.project, new, &mut used, 0);
        let resources = used.iter().filter_map(|n| self.doc.resources.get(n).map(|b| (n.clone(), b.as_ref().clone()))).collect();
        let pf = texturecraft_formats::ProjectFile {
            project: mini.project,
            pixels: mini.pixels,
            baked: BTreeMap::new(),
            mesh: None,
            high_poly: Vec::new(),
            cage: None,
            bake_paint: BTreeMap::new(),
            thumbnail: None,
            resources,
            warnings: Vec::new(),
        };
        texturecraft_formats::save_project_bytes(&pf).map_err(fe)
    }

    /// Add a smart material file above the selection (one undo step). Returns the new layer.
    pub fn add_smart_material_bytes(&mut self, bytes: &[u8]) -> Result<LayerId> {
        self.add_smart_material_bytes_at(bytes, Place::Selection)
    }

    /// [`Session::add_smart_material_bytes`] at `place` (shelf drag and drop). MaterialCraft
    /// portable materials and packages become linked fill layers, SubCraft smart materials
    /// (`.scsmart`) are mapped to layers (their report goes to the log).
    pub fn add_smart_material_bytes_at(&mut self, bytes: &[u8], place: Place) -> Result<LayerId> {
        if texturecraft_formats::mcmat::is_mcmat(bytes) || texturecraft_formats::mcmat::is_mcraft(bytes) {
            return self.add_linked_material_bytes_at(bytes, place);
        }
        if texturecraft_formats::scsmart::is_scsmart(bytes) {
            return self.import_scsmart_bytes_at(bytes, place).map(|(id, _)| id);
        }
        let pf = texturecraft_formats::load_project_bytes(bytes).map_err(fe)?;
        self.place_smart_material(pf, place, "Add smart material")
    }

    /// [`Session::import_scsmart_bytes_at`] above the selection.
    pub fn import_scsmart_bytes(&mut self, bytes: &[u8]) -> Result<(LayerId, texturecraft_formats::scsmart::ImportReport)> {
        self.import_scsmart_bytes_at(bytes, Place::Selection)
    }

    /// Map a SubCraft smart material (`.scsmart`) to layers and add it above the selection (or at
    /// `place`) in one undo step. Returns the new layer and the import report: what could not be
    /// mapped is in the report, never an error. The report is also written to the log.
    pub fn import_scsmart_bytes_at(&mut self, bytes: &[u8], place: Place) -> Result<(LayerId, texturecraft_formats::scsmart::ImportReport)> {
        let imported = texturecraft_formats::scsmart::read_scsmart(bytes).map_err(fe)?;
        let id = self.place_smart_material(imported.file, place, "Import SubCraft smart material")?;
        for line in imported.report.lines() {
            self.log.push(format!("SubCraft: {line}"));
        }
        Ok((id, imported.report))
    }

    /// Add the smart material held by a one-texture-set project file (its last root layer).
    fn place_smart_material(&mut self, pf: texturecraft_formats::ProjectFile, place: Place, label: &str) -> Result<LayerId> {
        let root = pf
            .project
            .texture_sets
            .first()
            .and_then(|t| t.stack.last().copied())
            .ok_or_else(|| EngineError::Other("the smart material file has no layer".into()))?;
        let resources: BTreeMap<String, Arc<Vec<u8>>> = pf.resources.into_iter().map(|(k, v)| (k, Arc::new(v))).collect();
        let src = Document { bake_paint: BTreeMap::new(), project: pf.project, pixels: pf.pixels, baked: BTreeMap::new(), resources: resources.clone() };
        let id = self.edit(label, |s| {
            for (k, v) in &resources {
                s.doc.resources.entry(k.clone()).or_insert_with(|| v.clone());
            }
            let ts = s.ts();
            let sel = s.selected_layer();
            let inside = sel.and_then(|l| s.doc.project.layer(l).ok()).is_some_and(|l| l.is_group());
            let id = crate::layer_ops::copy_tree(&src, root, &mut s.doc, ts, sel, inside, 0)?;
            place.apply(s, id)?;
            s.selected.insert(ts, id);
            s.target = StackTarget::Content;
            Ok(id)
        })?;
        self.sync_images();
        Ok(id)
    }
}

#[cfg(not(target_arch = "wasm32"))]
use crate::commands::{bad, layer_param, strp};
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

#[cfg(not(target_arch = "wasm32"))]
fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_param(s, "material.save", p)?;
    let path = strp(p, "path").ok_or_else(|| bad("material.save", "`path` is required"))?;
    let name = strp(p, "name").map(str::to_string).unwrap_or_else(|| s.doc.project.layer(id).map(|l| l.name.clone()).unwrap_or_default());
    let bytes = s.smart_material_bytes(id, &name)?;
    let path = if path.ends_with(&format!(".{SMART_MATERIAL_EXT}")) { path.to_string() } else { format!("{path}.{SMART_MATERIAL_EXT}") };
    if let Some(dir) = std::path::Path::new(&path).parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| bad("material.save", format!("cannot create {}: {e}", dir.display())))?;
    }
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| bad("material.save", format!("cannot write {path}: {e}")))?;
    Ok(json!({ "path": path }))
}

#[cfg(not(target_arch = "wasm32"))]
fn load(s: &mut Session, p: &Value) -> Result<Value> {
    let path = strp(p, "path").ok_or_else(|| bad("material.load", "`path` is required"))?;
    let bytes = std::fs::read(path).map_err(|e| bad("material.load", format!("cannot read {path}: {e}")))?;
    if texturecraft_formats::scsmart::is_scsmart(&bytes) {
        return scsmart_result(s, &bytes, p);
    }
    let id = s.add_smart_material_bytes_at(&bytes, Place::from_params(p))?;
    Ok(json!({ "layer": id.0 }))
}

#[cfg(not(target_arch = "wasm32"))]
fn import_scsmart(s: &mut Session, p: &Value) -> Result<Value> {
    let path = strp(p, "path").ok_or_else(|| bad("material.importScsmart", "`path` is required"))?;
    let bytes = read_scsmart_file(path)?;
    scsmart_result(s, &bytes, p)
}

/// A `.scsmart` file's bytes, refusing files over the importer's size limit before reading them.
#[cfg(not(target_arch = "wasm32"))]
fn read_scsmart_file(path: &str) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|e| bad("material.importScsmart", format!("cannot read {path}: {e}")))?;
    if !meta.is_file() || meta.len() > texturecraft_formats::scsmart::MAX_FILE_BYTES {
        return Err(bad("material.importScsmart", format!("{path} is not a regular file under {} MB", texturecraft_formats::scsmart::MAX_FILE_BYTES >> 20)));
    }
    std::fs::read(path).map_err(|e| bad("material.importScsmart", format!("cannot read {path}: {e}")))
}

/// Import `bytes` as a SubCraft smart material and describe the result.
#[cfg(not(target_arch = "wasm32"))]
fn scsmart_result(s: &mut Session, bytes: &[u8], p: &Value) -> Result<Value> {
    let (id, report) = s.import_scsmart_bytes_at(bytes, Place::from_params(p))?;
    let name = s.doc.project.layer(id).map(|l| l.name.clone()).unwrap_or_default();
    Ok(json!({ "layer": id.0, "name": name, "import": report }))
}

#[cfg(target_arch = "wasm32")]
fn save(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("file paths are not available in the browser".into()))
}

#[cfg(target_arch = "wasm32")]
fn load(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("file paths are not available in the browser".into()))
}

#[cfg(target_arch = "wasm32")]
fn import_scsmart(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("file paths are not available in the browser".into()))
}
