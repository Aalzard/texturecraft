//! Files: projects (`.tcraft`), mesh import, texture export. Byte-based functions work on the
//! web; path-based commands are native only.

use serde_json::{Value, json};
use texturecraft_core::NormalFormat;
#[cfg(not(target_arch = "wasm32"))]
use texturecraft_core::PROJECT_EXT;
use texturecraft_formats::{ExportInput, ExportOptions, ExportedFile, FileFormat, Padding, ProjectFile};
use texturecraft_geom::{Mesh, primitives};

use crate::commands::{CommandSpec, always, bad, strp, u64p};
use crate::session::{Document, Session};
use crate::{EngineError, Result, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.new",
            "New project from a mesh file",
            [],
            None,
            "{mesh: path to .glb/.gltf/.fbx/.dae/.obj/.ply/.stl, resolution?: 2048, name?}",
            always,
            new_from_mesh
        ),
        cmd!("file.open", "Open...", ["File"], Some("Ctrl+O"), "{path: .tcraft project or mesh file}", always, open),
        cmd!("file.save", "Save", ["File"], Some("Ctrl+S"), "{path?: .tcraft (default: the project's own file)}", always, save),
        cmd!("file.saveAs", "Save as...", ["File"], Some("Ctrl+Shift+S"), "{path}", always, save_as),
        cmd!("file.saveCopy", "Save a copy...", ["File"], None, "{path}", always, save_copy),
        cmd!("file.close", "Close", ["File"], Some("Ctrl+F4"), "{}", always, close),
        cmd!("file.reimportMesh", "Reimport mesh", ["Edit"], Some("Ctrl+Shift+R"), "{path?}", always, reimport),
        cmd!(
            "file.exportTextures",
            "Export textures",
            ["File"],
            None,
            "{dir, preset?: built-in or user preset name, or a preset object {name, maps}, ts?: index|\"all\", format?: png|jpg|tga|tif|exr|..., bitDepth?, padding?: none|dilation|dilationDefault|infinite, dilation?: px, size?: px, normalFormat?: openGl|directX}",
            always,
            export_textures
        ),
        cmd!(
            "file.exportMesh",
            "Export Mesh...",
            ["File"],
            None,
            "{path: .obj or .glb file, format?: obj|glb (default: from the extension, else obj)}",
            always,
            export_mesh
        ),
        cmd!(
            "export.plan",
            "List export outputs",
            [],
            None,
            "{preset?: name or preset object, ts?, format?, bitDepth?, size?} → [{ts, map, file, format, bitDepth, warnings}]",
            always,
            plan
        ),
    ]
}

fn fe(e: texturecraft_formats::FormatError) -> EngineError {
    EngineError::Other(e.to_string())
}

fn ext_of(name: &str) -> String {
    std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn stem_of(name: &str) -> String {
    std::path::Path::new(name).file_stem().and_then(|e| e.to_str()).unwrap_or("Untitled").to_string()
}

impl Session {
    /// The working mesh as Wavefront OBJ.
    pub fn mesh_obj_bytes(&self) -> Vec<u8> {
        texturecraft_formats::obj_bytes(&self.mesh, &self.doc.project.name)
    }

    /// The working mesh as binary glTF with each material's textures (from its texture set's
    /// composite, through the texture export pipeline) embedded.
    pub fn mesh_glb_bytes(&mut self) -> Result<Vec<u8>> {
        let opts = texturecraft_formats::glb_export_options();
        let mut textures = Vec::with_capacity(self.mesh.materials.len());
        for m in 0..self.mesh.materials.len() {
            let ts = self.doc.project.texture_sets.iter().position(|t| t.material == m);
            let files = match ts {
                Some(ts) => self.export_files(&[ts], &opts)?,
                None => Vec::new(),
            };
            textures.push(texturecraft_formats::GlbTextures::from_files(files));
        }
        texturecraft_formats::glb_bytes(&self.mesh, &self.doc.project.name, &textures).map_err(fe)
    }

    /// A new project from mesh file bytes (one texture set per material).
    pub fn from_mesh_bytes(file_name: &str, bytes: &[u8], resolution: u32, name: &str) -> Result<Session> {
        let imported = texturecraft_formats::import_mesh_bytes(file_name, bytes).map_err(fe)?;
        let ext = ext_of(file_name);
        let mut s = Session::new_project(name, imported.mesh, &format!("mesh.{ext}"), Some((ext, bytes.to_vec())), resolution)?;
        s.log.extend(imported.report.warnings.iter().cloned());
        s.log.push(format!(
            "Imported {} ({} triangles, {} materials{})",
            file_name,
            imported.report.triangles,
            imported.report.materials,
            if imported.report.uvs_generated { ", UVs generated" } else { "" }
        ));
        Ok(s)
    }

    /// Open a `.tcraft` project from bytes.
    pub fn from_project_bytes(bytes: &[u8]) -> Result<Session> {
        let pf = texturecraft_formats::load_project_bytes(bytes).map_err(fe)?;
        Session::from_project_file(pf)
    }

    pub fn from_project_file(pf: ProjectFile) -> Result<Session> {
        let mesh = match (&pf.mesh, pf.project.mesh.strip_prefix("builtin:")) {
            (_, Some(id)) => primitives::builtin(id).ok_or_else(|| EngineError::Other(format!("unknown built-in mesh `{id}`")))?,
            (Some((ext, bytes)), None) => texturecraft_formats::import_mesh_bytes(&format!("mesh.{ext}"), bytes).map_err(fe)?.mesh,
            (None, None) => return Err(EngineError::Other("the project has no mesh".into())),
        };
        check_materials(&pf.project, &mesh)?;
        let source = pf.mesh.clone();
        let resources = pf.resources.into_iter().map(|(k, v)| (k, std::sync::Arc::new(v))).collect();
        let mut s = Session::new(Document { project: pf.project, pixels: pf.pixels, baked: pf.baked, resources }, mesh, source)?;
        s.sync_images();
        s.load_bake_meshes(pf.high_poly, pf.cage);
        Ok(s)
    }

    /// The project as a `ProjectFile` (for saving).
    pub fn project_file(&self) -> ProjectFile {
        let mut pixels = self.doc.pixels.clone();
        // Drop pixel images whose size no longer matches their texture set (never unloadable).
        let res_of = |id: texturecraft_core::LayerId| {
            self.doc.project.locate(id).and_then(|(ts, _, _)| self.doc.project.texture_sets.get(ts)).map(|t| t.resolution).or_else(|| {
                self.doc
                    .project
                    .effect(id)
                    .and_then(|(owner, _)| self.doc.project.locate(owner))
                    .and_then(|(ts, _, _)| self.doc.project.texture_sets.get(ts))
                    .map(|t| t.resolution)
            })
        };
        pixels.layers.retain(|(id, _), img| res_of(*id) == Some(img.width()));
        pixels.masks.retain(|id, img| res_of(*id) == Some(img.width()));
        let (high_poly, cage) = self.saved_bake_meshes();
        ProjectFile {
            project: self.doc.project.clone(),
            pixels,
            baked: self.doc.baked.clone(),
            mesh: self.mesh_source.clone(),
            high_poly,
            cage,
            thumbnail: None,
            resources: self.doc.resources.iter().map(|(k, v)| (k.clone(), v.as_ref().clone())).collect(),
        }
    }

    pub fn to_project_bytes(&self) -> Result<Vec<u8>> {
        texturecraft_formats::save_project_bytes(&self.project_file()).map_err(fe)
    }

    /// Render every export file for texture sets `sets` (in memory; works on the web).
    pub fn export_files(&mut self, sets: &[usize], opts: &ExportOptions) -> Result<Vec<ExportedFile>> {
        self.update();
        let mesh_name = stem_of(self.doc.project.mesh.trim_start_matches("builtin:"));
        let mut out = Vec::new();
        let empty = Default::default();
        for &ts in sets {
            let cache = self.caches.get(ts).ok_or_else(|| EngineError::Other(format!("no texture set {ts}")))?;
            let coverage = cache.gbuf.coverage_mask();
            let input = ExportInput {
                project: &self.doc.project,
                ts,
                composite: &cache.composite,
                baked: self.doc.baked.get(&ts).unwrap_or(&empty),
                coverage: &coverage,
                mesh_name: &mesh_name,
            };
            out.extend(texturecraft_formats::render_maps(&input, opts).map_err(fe)?);
        }
        Ok(out)
    }
}

fn check_materials(p: &texturecraft_core::Project, mesh: &Mesh) -> Result<()> {
    for t in &p.texture_sets {
        if t.material >= mesh.materials.len().max(1) {
            return Err(EngineError::Other(format!("texture set \"{}\" refers to a material the mesh doesn't have", t.name)));
        }
    }
    Ok(())
}

/// Export options from command params; `preset` is a built-in or user preset name, or an inline
/// preset object (validated).
pub fn export_options(s: &Session, p: &Value, cmd: &str) -> Result<ExportOptions> {
    let preset = match p.get("preset") {
        None | Some(Value::Null) => s.export_preset("Document channels + Normal + AO"),
        Some(Value::String(name)) => s.export_preset(name),
        Some(v @ Value::Object(_)) => Some(crate::preset_cmds::preset_param(v, cmd)?),
        Some(_) => return Err(bad(cmd, "`preset` must be a preset name or a preset object")),
    };
    let preset = preset.ok_or_else(|| bad(cmd, format!("unknown export preset {}", p.get("preset").unwrap_or(&Value::Null))))?;
    let mut o = ExportOptions::new(preset);
    if let Some(f) = strp(p, "format") {
        o.format_override = Some(FileFormat::from_extension(f).ok_or_else(|| bad(cmd, format!("unknown format `{f}`")))?);
    }
    if let Some(b) = u64p(p, "bitDepth") {
        o.bit_depth_override = Some(b.min(32) as u8);
    }
    let px = u64p(p, "dilation").map(|v| v.min(256) as u32).unwrap_or(16);
    if let Some(pad) = strp(p, "padding") {
        o.padding = match pad {
            "none" => Padding::None,
            "dilation" => Padding::Dilation(px),
            "dilationDefault" => Padding::DilationDefault(px),
            "infinite" => Padding::Infinite,
            other => return Err(bad(cmd, format!("unknown padding `{other}`"))),
        };
    }
    if let Some(sz) = u64p(p, "size") {
        o.size = Some(crate::session::check_resolution(u32::try_from(sz).unwrap_or(0))?);
    }
    o.normal_format = match strp(p, "normalFormat") {
        Some("openGl") | Some("opengl") => Some(NormalFormat::OpenGl),
        Some("directX") | Some("directx") => Some(NormalFormat::DirectX),
        Some(other) => return Err(bad(cmd, format!("unknown normal format `{other}`"))),
        None => None,
    };
    Ok(o)
}

fn sets_param(s: &Session, p: &Value, cmd: &str) -> Result<Vec<usize>> {
    match p.get("ts") {
        None => Ok((0..s.set_count()).collect()),
        Some(Value::String(a)) if a == "all" => Ok((0..s.set_count()).collect()),
        Some(v) => Ok(vec![v.as_u64().map(|t| t as usize).filter(|t| *t < s.set_count()).ok_or_else(|| bad(cmd, "invalid `ts`"))?]),
    }
}

fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    let opts = export_options(s, p, "export.plan")?;
    let mesh_name = stem_of(s.doc.project.mesh.trim_start_matches("builtin:"));
    let mut names = Vec::new();
    for ts in sets_param(s, p, "export.plan")? {
        for f in texturecraft_formats::list_outputs(&s.doc.project, ts, &mesh_name, &opts).map_err(fe)? {
            names.push(json!({"ts": ts, "map": f.map, "file": f.name, "format": f.format.extension(), "bitDepth": f.bit_depth, "warnings": f.warnings}));
        }
    }
    Ok(json!(names))
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::path::Path;

    pub fn path_param<'a>(p: &'a Value, key: &str, cmd: &str) -> Result<&'a str> {
        strp(p, key).filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(cmd, format!("`{key}` is required")))
    }

    /// Open a project or a mesh file.
    pub fn open_path(path: &str) -> Result<Session> {
        let p = Path::new(path);
        if ext_of(path) == PROJECT_EXT {
            let pf = texturecraft_formats::load_project(p).map_err(fe)?;
            let mut s = Session::from_project_file(pf)?;
            s.path = Some(path.to_string());
            s.modified = false;
            Ok(s)
        } else {
            let bytes = std::fs::read(p).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
            Session::from_mesh_bytes(path, &bytes, 2048, &stem_of(path))
        }
    }

    pub fn save_to(s: &mut Session, path: &str, adopt: bool) -> Result<Value> {
        let path = if ext_of(path) == PROJECT_EXT { path.to_string() } else { format!("{path}.{PROJECT_EXT}") };
        texturecraft_formats::save_project(Path::new(&path), &s.project_file()).map_err(fe)?;
        if adopt {
            s.path = Some(path.clone());
            s.modified = false;
        }
        s.log.push(format!("Saved {path}"));
        Ok(json!({ "path": path }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::open_path;

#[cfg(not(target_arch = "wasm32"))]
fn new_from_mesh(s: &mut Session, p: &Value) -> Result<Value> {
    let path = native::path_param(p, "mesh", "file.new")?;
    let bytes = std::fs::read(path).map_err(|e| bad("file.new", format!("cannot read {path}: {e}")))?;
    let res = u32::try_from(u64p(p, "resolution").unwrap_or(2048)).unwrap_or(0);
    let name = strp(p, "name").map(str::to_string).unwrap_or_else(|| stem_of(path));
    let next = Session::from_mesh_bytes(path, &bytes, res, &name)?;
    s.switch_to(next);
    Ok(crate::inspect::session(s))
}

#[cfg(not(target_arch = "wasm32"))]
fn open(s: &mut Session, p: &Value) -> Result<Value> {
    let path = native::path_param(p, "path", "file.open")?;
    let next = native::open_path(path)?;
    s.switch_to(next);
    Ok(crate::inspect::session(s))
}

#[cfg(not(target_arch = "wasm32"))]
fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match strp(p, "path") {
        Some(x) => x.to_string(),
        None => s.path.clone().ok_or_else(|| bad("file.save", "the project has no file yet: give `path` (Save as)"))?,
    };
    native::save_to(s, &path, true)
}

#[cfg(not(target_arch = "wasm32"))]
fn save_as(s: &mut Session, p: &Value) -> Result<Value> {
    let path = native::path_param(p, "path", "file.saveAs")?.to_string();
    native::save_to(s, &path, true)
}

#[cfg(not(target_arch = "wasm32"))]
fn save_copy(s: &mut Session, p: &Value) -> Result<Value> {
    let path = native::path_param(p, "path", "file.saveCopy")?.to_string();
    native::save_to(s, &path, false)
}

#[cfg(not(target_arch = "wasm32"))]
fn reimport(s: &mut Session, p: &Value) -> Result<Value> {
    // Re-read the mesh and keep the project (texture sets map by material index).
    let mesh = match strp(p, "path") {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| bad("file.reimportMesh", format!("cannot read {path}: {e}")))?;
            let m = texturecraft_formats::import_mesh_bytes(path, &bytes).map_err(fe)?.mesh;
            s.mesh_source = Some((ext_of(path), bytes));
            s.doc.project.mesh = format!("mesh.{}", ext_of(path));
            m
        }
        None => match &s.mesh_source {
            Some((ext, bytes)) => texturecraft_formats::import_mesh_bytes(&format!("mesh.{ext}"), bytes).map_err(fe)?.mesh,
            None => return Err(bad("file.reimportMesh", "built-in meshes can't be reimported; give `path`")),
        },
    };
    check_materials(&s.doc.project, &mesh)?;
    let doc = s.doc.clone();
    let source = s.mesh_source.clone();
    let mut next = Session::new(doc, mesh, source)?;
    next.path = s.path.clone();
    s.switch_to(next);
    Ok(Value::Null)
}

#[cfg(not(target_arch = "wasm32"))]
fn export_textures(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = native::path_param(p, "dir", "file.exportTextures")?.to_string();
    let opts = export_options(s, p, "file.exportTextures")?;
    let sets = sets_param(s, p, "file.exportTextures")?;
    std::fs::create_dir_all(&dir).map_err(|e| bad("file.exportTextures", format!("cannot create {dir}: {e}")))?;
    let files = s.export_files(&sets, &opts)?;
    let warnings: Vec<String> = files.iter().flat_map(|f| f.warnings.iter().cloned()).collect();
    let written = texturecraft_formats::write_files(&files, std::path::Path::new(&dir)).map_err(fe)?;
    s.log.push(format!("Exported {} textures to {dir}", written.len()));
    Ok(json!({ "files": written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(), "warnings": warnings }))
}

#[cfg(not(target_arch = "wasm32"))]
fn export_mesh(s: &mut Session, p: &Value) -> Result<Value> {
    let path = native::path_param(p, "path", "file.exportMesh")?;
    let ext = match strp(p, "format") {
        Some(f @ ("obj" | "glb")) => f.to_string(),
        Some(other) => return Err(bad("file.exportMesh", format!("unknown mesh format `{other}` (obj or glb)"))),
        None if ext_of(path) == "glb" => "glb".into(),
        None => "obj".into(),
    };
    let path = if ext_of(path) == ext { path.to_string() } else { format!("{path}.{ext}") };
    let bytes = if ext == "glb" { s.mesh_glb_bytes()? } else { s.mesh_obj_bytes() };
    std::fs::write(&path, bytes).map_err(|e| bad("file.exportMesh", format!("cannot write {path}: {e}")))?;
    s.log.push(format!("Exported mesh to {path}"));
    Ok(json!({ "path": path }))
}

#[cfg(target_arch = "wasm32")]
fn unsupported(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("file paths are not available in the browser; use the web file buttons".into()))
}
#[cfg(target_arch = "wasm32")]
use unsupported as new_from_mesh;
#[cfg(target_arch = "wasm32")]
use unsupported as open;
#[cfg(target_arch = "wasm32")]
use unsupported as save;
#[cfg(target_arch = "wasm32")]
use unsupported as save_as;
#[cfg(target_arch = "wasm32")]
use unsupported as save_copy;
#[cfg(target_arch = "wasm32")]
use unsupported as reimport;
#[cfg(target_arch = "wasm32")]
use unsupported as export_textures;
#[cfg(target_arch = "wasm32")]
use unsupported as export_mesh;

fn close(s: &mut Session, _: &Value) -> Result<Value> {
    let next = Session::new_project("Untitled", primitives::material_ball(), "builtin:material_ball", None, 2048)?;
    s.switch_to(next);
    Ok(Value::Null)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn export_mesh_writes_obj_by_default_and_textured_glb() {
        let mut s = Session::new_project("t", primitives::material_ball(), "builtin:material_ball", None, 128).unwrap();
        let dir = std::env::temp_dir().join(format!("tc_export_mesh_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("ball").display().to_string();
        let obj = crate::execute(&mut s, "file.exportMesh", &json!({ "path": base })).unwrap();
        assert!(obj["path"].as_str().unwrap().ends_with("ball.obj"));
        let glb = crate::execute(&mut s, "file.exportMesh", &json!({ "path": base, "format": "glb" })).unwrap();
        let bytes = std::fs::read(glb["path"].as_str().unwrap()).unwrap();
        let back = texturecraft_formats::import_mesh_bytes("ball.glb", &bytes).unwrap();
        assert_eq!(back.mesh.triangle_count(), s.mesh.triangle_count());
        assert_eq!(back.mesh.materials, s.mesh.materials);
        // Base color, ORM and normal per texture set.
        assert_eq!(bytes.windows(9).filter(|w| *w == b"image/png").count(), 3 * s.mesh.materials.len());
        assert!(crate::execute(&mut s, "file.exportMesh", &json!({ "path": base, "format": "fbx" })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
