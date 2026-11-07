//! Import mesh maps baked in another app: many files at once, each to the set and slot its file name names

use std::collections::BTreeMap;

use serde_json::{Value, json};
use texturecraft_core::BakedMap;

use crate::commands::{CommandSpec, always, bad, strp};
use crate::meshmap_cmds::use_image;
use crate::meshmap_file_name::{baked_map_from_file_name, texture_set_from_file_name};
use crate::resource_cmds::read_image_file;
use crate::{Result, Session, cmd};

const CMD: &str = "textureSet.meshMap.import";

/// Most files one import takes
const MAX_FILES: usize = 1024;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "textureSet.meshMap.import",
        "Import baked maps",
        [],
        None,
        "{files: [{path, map?: mesh map id}]} → {assigned: [{path, ts, map}], unmatched: [path]} — each image file becomes the mesh map its name names (`Body_ao.png`, `Body_world_normal.png`; or `map`) of the texture set its name starts with (any name when there is one set), read as by textureSet.meshMap.set: a UV-tiled set takes one file per tile (`Body_ao.1001.png`, `Body_ao.1002.png`). Files matching no set or map, and any file after the first for one slot, are left out and listed in `unmatched`. One undo step; desktop only",
        always,
        import
    )]
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let files = p.get("files").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`files` (a list of {path, map?}) is required"))?;
    if files.len() > MAX_FILES {
        return Err(bad(CMD, format!("at most {MAX_FILES} files at once")));
    }

    let set_names: Vec<String> = s.doc.project.texture_sets.iter().map(|set| set.name.clone()).collect();
    let set_names: Vec<&str> = set_names.iter().map(String::as_str).collect();
    let mut slots: BTreeMap<(usize, BakedMap), Vec<&str>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    for file in files {
        let path = strp(file, "path").ok_or_else(|| bad(CMD, "every file needs a `path`"))?;
        let map = match strp(file, "map") {
            Some(id) => Some(BakedMap::parse(id).ok_or_else(|| bad(CMD, format!("unknown mesh map `{id}`")))?),
            None => baked_map_from_file_name(path),
        };
        match (texture_set_from_file_name(path, &set_names), map) {
            (Some(ts), Some(map)) => slots.entry((ts, map)).or_default().push(path),
            _ => unmatched.push(path),
        }
    }

    let result = s.edit("Import baked maps", |s| {
        let mut assigned = Vec::new();
        for ((ts, map), paths) in &slots {
            // A UV-tiled set reads every tile's file; any other set takes the first file for a slot
            let files_used = if s.doc.project.texture_set(*ts)?.tile_grid().is_some() { paths.len() } else { 1 };
            let (taken, extra) = paths.split_at(files_used);
            let mut image_name = None;
            for path in taken {
                let name = s.add_image_resource(path, read_image_file(CMD, path)?)?;
                image_name.get_or_insert(name);
            }
            if let Some(name) = image_name {
                use_image(s, CMD, *ts, *map, &name)?;
            }

            assigned.extend(taken.iter().map(|path| json!({"path": path, "ts": ts, "map": map.id()})));
            unmatched.extend(extra);
        }
        Ok(assigned)
    });
    // A failed edit leaves the document as it was; forget the images it decoded
    if result.is_err() {
        s.sync_images();
    }

    Ok(json!({ "assigned": result?, "unmatched": unmatched }))
}
