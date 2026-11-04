//! SubCraft smart materials (`.scsmart`, read only): a layer folder with its fills, masks,
//! generators and embedded images, imported as a TextureCraft smart material.
//!
//! `.scsmart` is the smart-material file of SubCraft, an open-source sibling app. It is a zip
//! container (the "fragment" format of SubCraft's public file specification):
//!
//! ```text
//! fragment.json         format "subcraft-fragment", schema version, kind, name, resolution,
//!                       layers (a tree, bottom to top) and the table of embedded images
//! rasters/NNNNN.tiles   sparse half-float tiles: paint channels, mask bases, mask paint effects
//! resources/<id>.<ext>  the original bytes of images used by fills and image masks
//! ```
//!
//! TextureCraft keeps its own reader (the apps share the format, not code). Importing never
//! fails on something it cannot map: an unsupported blend mode, filter, generator or fill source
//! is left out or approximated, and the [`ImportReport`] says which and why. Only a file that is
//! not a smart material at all (wrong format, a newer schema, damaged container or rasters,
//! sizes beyond the limits) is an error. Nothing is extracted to disk and every size is capped
//! before it is allocated, so a hostile file produces an error, never a panic.

pub mod fixture;
mod map;
mod raster;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use serde::Serialize;
use serde_json::Value;

use crate::{FormatError, ProjectFile, Result};

/// The smart-material file extension.
pub const SCSMART_EXT: &str = "scsmart";
/// `format` of a SubCraft fragment container (smart materials and smart masks).
pub const FRAGMENT_FORMAT: &str = "subcraft-fragment";
/// The newest SubCraft schema this reader understands. Older schemas load unchanged (what a
/// schema added is simply absent from older files); a newer one may change what existing fields
/// mean, so it is refused the way SubCraft itself refuses it.
pub const SUPPORTED_SCHEMA: u32 = 16;

/// Largest `.scsmart` file read (its images and rasters are decoded in memory).
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// Largest single inflated entry.
const MAX_ENTRY_BYTES: u64 = 512 << 20;
/// Entries in the container.
const MAX_ENTRIES: usize = 4096;
/// Layers (and folders) in one smart material.
pub const MAX_LAYERS: usize = 2048;
/// Embedded images.
const MAX_RESOURCES: usize = 256;
/// Issues kept in a report (the rest are counted).
const MAX_ISSUES: usize = 300;
/// Decoded pixel data kept in memory (rasters and embedded images), in bytes.
const MAX_DECODED_BYTES: u64 = if cfg!(target_arch = "wasm32") { 512 << 20 } else { 2 << 30 };

/// How much of the file's meaning made it across.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Imported as it is; worth knowing (a convention that differs, an editable source that
    /// became pixels).
    Note,
    /// Imported with the closest TextureCraft equivalent, which looks or behaves a little
    /// differently.
    Approximated,
    /// Left out: TextureCraft has nothing to map it to.
    Dropped,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Note => "note",
            Severity::Approximated => "approximated",
            Severity::Dropped => "not imported",
        }
    }
}

/// One thing that did not map exactly.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Issue {
    pub severity: Severity,
    /// The layer it is on (`Folder / Layer / mask`); empty for the whole file.
    pub layer: String,
    /// What it is about (`blend mode "foo"`, `filter median`).
    pub what: String,
    pub message: String,
    /// How many places had this same problem (the first is named in `layer`).
    pub count: u32,
}

/// What an import brought in and what it could not map.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ImportReport {
    /// The smart material's name.
    pub name: String,
    /// The SubCraft version that wrote the file.
    pub writer: String,
    pub schema_version: u32,
    /// Texture resolution the file was saved at (its pixels are resampled to the project's).
    pub resolution: u32,
    /// Layers imported (folders included).
    pub layers: u32,
    pub folders: u32,
    pub masks: u32,
    /// Mask and content effects imported.
    pub effects: u32,
    /// Embedded images imported.
    pub images: u32,
    pub issues: Vec<Issue>,
    /// Issues beyond the report's cap.
    pub more_issues: u32,
}

impl ImportReport {
    /// True when everything mapped exactly (notes aside).
    pub fn is_clean(&self) -> bool {
        self.issues.iter().all(|i| i.severity == Severity::Note) && self.more_issues == 0
    }

    pub fn count(&self, severity: Severity) -> u32 {
        self.issues.iter().filter(|i| i.severity == severity).map(|i| i.count).sum()
    }

    /// One line: `Imported "Rust": 5 layers, 2 masks; 3 approximated, 1 not imported`.
    pub fn summary(&self) -> String {
        let mut s = format!("Imported \"{}\": {} layers", self.name, self.layers);
        if self.masks > 0 {
            s.push_str(&format!(", {} masks", self.masks));
        }
        if self.images > 0 {
            s.push_str(&format!(", {} images", self.images));
        }
        let (approx, dropped) = (self.count(Severity::Approximated), self.count(Severity::Dropped));
        if approx > 0 {
            s.push_str(&format!("; {approx} approximated"));
        }
        if dropped > 0 {
            s.push_str(&format!("{} {dropped} not imported", if approx > 0 { "," } else { ";" }));
        }
        s
    }

    /// The report as lines of text, worst first (for logs and the CLI).
    pub fn lines(&self) -> Vec<String> {
        let mut issues: Vec<&Issue> = self.issues.iter().collect();
        issues.sort_by_key(|i| std::cmp::Reverse(i.severity));
        let mut out = vec![self.summary()];
        for i in issues {
            let at = if i.layer.is_empty() { String::new() } else { format!("{}: ", i.layer) };
            let times = if i.count > 1 { format!(" (x{})", i.count) } else { String::new() };
            out.push(format!("  [{}] {at}{}: {}{times}", i.severity.label(), i.what, i.message));
        }
        if self.more_issues > 0 {
            out.push(format!("  ... and {} more", self.more_issues));
        }
        out
    }

    pub(crate) fn issue(&mut self, severity: Severity, layer: &str, what: impl Into<String>, message: impl Into<String>) {
        let (what, message) = (what.into(), message.into());
        if let Some(found) = self.issues.iter_mut().find(|i| i.severity == severity && i.what == what && i.message == message) {
            found.count = found.count.saturating_add(1);
        } else if self.issues.len() < MAX_ISSUES {
            self.issues.push(Issue { severity, layer: layer.to_string(), what, message, count: 1 });
        } else {
            self.more_issues = self.more_issues.saturating_add(1);
        }
    }
}

/// A smart material read from a `.scsmart`: a one-texture-set project (the same shape a
/// `.tcmat` holds, so it is added to a document the same way) and the import report. The
/// smart material is the last layer of the set's stack.
#[derive(Clone, Debug)]
pub struct ScsmartImport {
    pub file: ProjectFile,
    pub report: ImportReport,
}

/// Counts decoded bytes against a limit so a hostile file cannot exhaust memory.
#[derive(Debug)]
pub(crate) struct Budget {
    left: u64,
}

impl Budget {
    pub(crate) fn new() -> Budget {
        Budget { left: MAX_DECODED_BYTES }
    }

    pub(crate) fn charge(&mut self, bytes: u64) -> std::result::Result<(), String> {
        if bytes > self.left {
            return Err(format!("the smart material holds more than {} MB of pixel data", MAX_DECODED_BYTES >> 20));
        }
        self.left -= bytes;
        Ok(())
    }
}

fn malformed(msg: impl Into<String>) -> FormatError {
    FormatError::Malformed { kind: "SubCraft smart material", msg: msg.into() }
}

/// Read one entry of the container, at most `cap` bytes.
pub(crate) fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str, cap: u64, budget: &mut Budget) -> Result<Vec<u8>> {
    let file = zip.by_name(name).map_err(|_| malformed(format!("{name} is missing")))?;
    // The declared size is attacker-controlled: cap the allocation and the bytes inflated.
    if file.size() > cap {
        return Err(FormatError::TooLarge(format!("{name} is {} MB (limit {} MB)", file.size() >> 20, cap >> 20)));
    }
    budget.charge(file.size()).map_err(FormatError::TooLarge)?;
    let mut out = Vec::with_capacity(file.size() as usize);
    file.take(cap + 1).read_to_end(&mut out).map_err(|e| malformed(format!("{name}: {e}")))?;
    if out.len() as u64 > cap {
        return Err(FormatError::TooLarge(format!("{name} is larger than {} MB", cap >> 20)));
    }
    Ok(out)
}

fn open(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(FormatError::TooLarge(format!("the file is {} MB (limit {} MB)", bytes.len() >> 20, MAX_FILE_BYTES >> 20)));
    }
    let zip = crate::guard("SubCraft smart material", || zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| malformed(e.to_string())))?;
    if zip.len() > MAX_ENTRIES {
        return Err(FormatError::TooLarge(format!("{} entries in the container (limit {MAX_ENTRIES})", zip.len())));
    }
    Ok(zip)
}

/// The `fragment.json` document of a container.
fn read_fragment_json<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, budget: &mut Budget) -> Result<Value> {
    let json = read_entry(zip, "fragment.json", 64 << 20, budget).map_err(|_| malformed("fragment.json is missing: this is not a SubCraft smart material"))?;
    serde_json::from_slice(&json).map_err(|e| malformed(format!("fragment.json: {e}")))
}

/// Whether `bytes` is a SubCraft fragment container (a smart material or a smart mask).
pub fn is_scsmart(bytes: &[u8]) -> bool {
    let Ok(mut zip) = open(bytes) else { return false };
    let mut budget = Budget::new();
    read_fragment_json(&mut zip, &mut budget).is_ok_and(|v| v.get("format").and_then(Value::as_str) == Some(FRAGMENT_FORMAT))
}

/// Read a `.scsmart` and map it to TextureCraft layers. See the module docs for what is an
/// error and what only lands in the report.
pub fn read_scsmart(bytes: &[u8]) -> Result<ScsmartImport> {
    let mut zip = open(bytes)?;
    let mut budget = Budget::new();
    let doc = read_fragment_json(&mut zip, &mut budget)?;
    if doc.get("format").and_then(Value::as_str) != Some(FRAGMENT_FORMAT) {
        return Err(malformed("fragment.json is not a SubCraft fragment: this is not a SubCraft smart material"));
    }
    let schema = doc.get("schema_version").and_then(Value::as_u64).ok_or_else(|| malformed("fragment.json has no schema_version"))?;
    if schema == 0 {
        return Err(malformed("schema_version 0 does not exist"));
    }
    if schema > u64::from(SUPPORTED_SCHEMA) {
        return Err(FormatError::Unsupported(format!(
            "this smart material was saved by a newer SubCraft (schema {schema}; this version of TextureCraft reads up to {SUPPORTED_SCHEMA}). Update TextureCraft or save it again from an older SubCraft"
        )));
    }
    match doc.get("kind").and_then(Value::as_str) {
        Some("smart_material") => {}
        Some("smart_mask") => return Err(FormatError::Unsupported("this is a SubCraft smart mask (.scmask), not a smart material".into())),
        other => return Err(malformed(format!("unknown fragment kind {other:?}"))),
    }
    let resolution = doc
        .get("resolution")
        .and_then(Value::as_u64)
        .and_then(|r| u32::try_from(r).ok())
        .filter(|r| map::RESOLUTIONS.contains(r))
        .ok_or_else(|| malformed("unsupported resolution (expected 128, 256, 512, 1024, 2048 or 4096)"))?;
    let name = doc.get("name").and_then(Value::as_str).map(clean_name).filter(|n| !n.is_empty()).unwrap_or_else(|| "SubCraft smart material".into());
    let report = ImportReport {
        name: name.clone(),
        writer: doc.get("writer").and_then(Value::as_str).map(|w| w.chars().take(80).collect()).unwrap_or_default(),
        schema_version: schema as u32,
        resolution,
        ..ImportReport::default()
    };
    let mut mapper = map::Mapper::new(&mut zip, &name, resolution, report, budget);
    mapper.run(&doc)?;
    Ok(mapper.finish())
}

/// A name with control characters removed and a sane length.
pub(crate) fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(120).collect::<String>().trim().to_string()
}

/// Resource name -> bytes, as `ProjectFile` holds them.
pub(crate) type Resources = BTreeMap<String, Vec<u8>>;
