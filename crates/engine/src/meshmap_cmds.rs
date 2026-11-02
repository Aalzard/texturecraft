//! Mesh map slots of the Texture Set Settings: use an image resource as a mesh map (baked in
//! another app, or edited by hand) instead of baking it, or clear a slot.

use std::sync::Arc;

use rayon::prelude::*;
use serde_json::{Value, json};
use texturecraft_compose::ImageRes;
use texturecraft_core::{BakedMap, MAX_RESOLUTION, NormalFormat};

use crate::commands::{CommandSpec, always, bad, strp};
use crate::set_cmds::ts_param;
use crate::{Result, Session, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "textureSet.meshMap.set",
            "Use image as mesh map",
            [],
            None,
            "{ts?, map: mesh map id (normal_base, ambient_occlusion, curvature, id, height…), image: resource name} → {map, resolution} — resampled to the size of the set's other mesh maps (or its bake size); a normal map is read in the project's normal format",
            always,
            set
        ),
        cmd!("textureSet.meshMap.clear", "Clear mesh map", [], None, "{ts?, map}", always, clear),
    ]
}

fn map_param(p: &Value, cmd: &str) -> Result<BakedMap> {
    let id = strp(p, "map").ok_or_else(|| bad(cmd, "`map` is required"))?;
    BakedMap::parse(id).ok_or_else(|| bad(cmd, format!("unknown mesh map `{id}`")))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "textureSet.meshMap.set";
    let ts = ts_param(s, p, CMD)?;
    let map = map_param(p, CMD)?;
    let name = strp(p, "image").ok_or_else(|| bad(CMD, "`image` (an image resource name) is required"))?;
    let img = s.images.get(name).cloned().ok_or_else(|| bad(CMD, format!("no image resource `{name}`")))?;
    let set = s.doc.project.texture_set(ts)?;
    // shortcut: one image spans the whole set; UV-tiled sets would need one image per tile.
    if !set.uv_tiles.is_empty() {
        return Err(bad(CMD, format!("\"{}\" has UV tiles: mesh maps from images need a set without them", set.name)));
    }
    // Every mesh map of a set shares one size: the other maps', or what a bake would make.
    let res = s.doc.baked.get(&ts).filter(|b| !b.maps.is_empty()).map_or(set.bake.resolution, |b| b.resolution);
    if !(1..=MAX_RESOLUTION).contains(&res) {
        return Err(bad(CMD, format!("mesh map size {res} is not in 1..={MAX_RESOLUTION}")));
    }
    let directx = map == BakedMap::Normal && s.doc.project.normal_format == NormalFormat::DirectX;
    let data = resample(&img, map.components(), res, directx);
    s.edit("Use image as mesh map", |s| {
        let entry = s.doc.baked.entry(ts).or_default();
        entry.resolution = res;
        entry.maps.insert(map, Arc::new(data));
        s.doc.project.texture_set_mut(ts)?.baked.insert(map);
        Ok(())
    })?;
    Ok(json!({ "map": map.id(), "resolution": res }))
}

/// `img` as `res` × `res` texels of `comps` values each, as stored in the file (mesh maps hold
/// data, not colors, so nothing is linearized); `flip_green` reads a DirectX normal map.
fn resample(img: &ImageRes, comps: usize, res: u32, flip_green: bool) -> Vec<f32> {
    let n = res as usize;
    let mut out = vec![0.0; n * n * comps];
    out.par_chunks_mut(n * comps).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / res as f32;
        for (x, texel) in row.chunks_exact_mut(comps).enumerate() {
            let mut c = img.sample((x as f32 + 0.5) / res as f32, v);
            if flip_green {
                c[1] = 1.0 - c[1];
            }
            for (o, c) in texel.iter_mut().zip(c) {
                *o = c;
            }
        }
    });
    out
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "textureSet.meshMap.clear";
    let ts = ts_param(s, p, CMD)?;
    let map = map_param(p, CMD)?;
    if !s.doc.baked.get(&ts).is_some_and(|b| b.maps.contains_key(&map)) {
        return Err(bad(CMD, format!("texture set {ts} has no {} map", map.label())));
    }
    s.edit("Clear mesh map", |s| {
        if let Some(b) = s.doc.baked.get_mut(&ts) {
            b.maps.remove(&map);
        }
        s.doc.project.texture_set_mut(ts)?.baked.remove(&map);
        Ok(())
    })?;
    Ok(Value::Null)
}
