//! Auto-rebake: one mesh map that bakes itself again when what it was baked from changes.
//!
//! The map is chosen with `bake.autoRebake`. From then on, [`Session::auto_rebake_due`] names the
//! texture sets whose bake inputs (the set's bake settings except the list of maps, the low poly,
//! the high polys and the cage) differ from those of their last bake of that map.
//! The apps poll it each frame and bake the due sets in the background; `bake.autoRebake.run`
//! does the same thing once, blocking, for agents and scripts.
//!
//! A set only takes part once it has the map baked at its current output size: Auto-rebake
//! keeps a bake up to date, it doesn't start one, and it never leaves a set's maps at two sizes.
//! The choice isn't part of the project.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};

use serde_json::{Value, json};
use texturecraft_bake::Progress;
use texturecraft_core::BakedMap;

use crate::bake_cmds::BakeTask;
use crate::commands::{CommandSpec, always, bad, boolp};
use crate::{Result, Session, cmd};

/// Which map rebakes itself, and what each texture set was last baked from.
#[derive(Clone, Debug, Default)]
pub struct AutoRebake {
    pub map: Option<BakedMap>,
    /// Fingerprint of each set's bake inputs when it last baked [`AutoRebake::map`].
    seen: BTreeMap<usize, u64>,
    /// Fingerprint taken when a set's bake started, kept when that bake finishes.
    pending: BTreeMap<usize, u64>,
    /// What each map image in use was baked from. Undo and redo bring back an older image; its own
    /// entry says its inputs no longer match. Weak, so a freed image doesn't linger and its
    /// address can't be taken by a new one.
    origin: Vec<(Weak<Vec<f32>>, u64)>,
}

impl AutoRebake {
    fn remember(&mut self, image: &Arc<Vec<f32>>, key: u64) {
        self.origin.retain(|(w, _)| w.strong_count() > 0);
        self.origin.retain(|(w, _)| !std::ptr::eq(w.as_ptr(), Arc::as_ptr(image)));
        self.origin.push((Arc::downgrade(image), key));
    }

    fn origin_of(&self, image: &Arc<Vec<f32>>) -> Option<u64> {
        self.origin.iter().find(|(w, _)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(image))).map(|(_, k)| *k)
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "bake.autoRebake",
            "Auto-rebake",
            [],
            None,
            "{map?: map id (e.g. \"normal_base\", \"height\") to keep up to date, or null / off: true to turn it off; no params: just report} → {map, due: [texture sets that need a rebake]}",
            always,
            auto_rebake
        ),
        cmd!(
            "bake.autoRebake.run",
            "Run Auto-rebake",
            [],
            None,
            "{cpu?: true (skip the GPU)} → {rebaked: [texture sets]}: bakes the Auto-rebake map of every set whose bake inputs changed since it was last baked, now",
            always,
            auto_rebake_run
        ),
    ]
}

impl Session {
    /// A fingerprint of everything a bake of texture set `ts` is made from, except which maps it
    /// makes.
    pub fn bake_inputs_key(&self, ts: usize) -> Option<u64> {
        let set = self.doc.project.texture_sets.get(ts)?;
        let mut settings = set.bake.clone();
        settings.maps.clear();
        settings.enabled = true;
        let mut h = DefaultHasher::new();
        serde_json::to_string(&settings).ok()?.hash(&mut h);
        (Arc::as_ptr(&self.mesh) as usize).hash(&mut h);
        for m in &self.bake_meshes.high {
            (Arc::as_ptr(&m.mesh) as usize).hash(&mut h);
        }
        self.bake_meshes.cage.as_ref().map(|c| Arc::as_ptr(&c.mesh) as usize).hash(&mut h);
        Some(h.finish())
    }

    /// Texture sets whose Auto-rebake map is out of date. A set seen for the first time (a project
    /// just opened, a bake from before Auto-rebake was on) is taken as up to date.
    pub fn auto_rebake_due(&mut self) -> Vec<usize> {
        let Some(map) = self.auto_rebake.map else { return Vec::new() };
        let mut due = Vec::new();
        for ts in 0..self.doc.project.texture_sets.len() {
            let Some(set) = self.doc.project.texture_sets.get(ts).filter(|t| t.bake.enabled) else { continue };
            let columns = set.tile_grid().map_or(1, |g| g.cols);
            let baked = self.doc.baked.get(&ts).is_some_and(|b| b.maps.contains_key(&map) && b.resolution == set.bake.resolution.saturating_mul(columns));
            let Some(key) = self.bake_inputs_key(ts).filter(|_| baked) else { continue };
            let image = self.doc.baked.get(&ts).and_then(|b| b.maps.get(&map)).cloned();
            // An image whose inputs are known is judged by them (this is what undo is caught by);
            // one not seen before is taken as up to date, like a project just opened.
            match (image.as_ref().and_then(|i| self.auto_rebake.origin_of(i)), self.auto_rebake.seen.get(&ts)) {
                (Some(made_from), _) if made_from != key => due.push(ts),
                (Some(_), _) => {}
                (None, Some(seen)) if *seen != key => due.push(ts),
                (None, Some(_)) => {}
                (None, None) => {
                    self.auto_rebake.seen.insert(ts, key);
                    if let Some(image) = &image {
                        self.auto_rebake.remember(image, key);
                    }
                }
            }
        }
        due
    }

    /// Call right before building the bake task of `ts`: remembers what it is baked from, so a
    /// finished bake of the Auto-rebake map marks the set up to date (and a change made while it
    /// ran still triggers another).
    pub fn begin_bake(&mut self, ts: usize) {
        if let Some(key) = self.bake_inputs_key(ts) {
            self.auto_rebake.pending.insert(ts, key);
        }
    }

    /// The bake task of `ts` restricted to one map (Auto-rebake's: the other maps stay as they are).
    pub fn bake_task_for(&self, ts: usize, only: Option<BakedMap>) -> Result<BakeTask> {
        let mut task = self.bake_task(ts)?;
        if let Some(m) = only {
            task.settings.maps = std::iter::once(m).collect();
        }
        Ok(task)
    }

    /// Records a finished bake: when it made the Auto-rebake map, the set is up to date.
    pub(crate) fn note_baked(&mut self, ts: usize, mut made: impl Iterator<Item = BakedMap>) {
        let key = self.auto_rebake.pending.remove(&ts);
        if let (Some(key), Some(map)) = (key, self.auto_rebake.map)
            && made.any(|m| m == map)
        {
            self.auto_rebake.seen.insert(ts, key);
            if let Some(image) = self.doc.baked.get(&ts).and_then(|b| b.maps.get(&map)).cloned() {
                self.auto_rebake.remember(&image, key);
            }
        }
    }

    /// Records bakes of `sets` that were cancelled, failed or never started: Auto-rebake leaves
    /// those sets alone until their bake inputs change again, instead of retrying every frame.
    pub fn bake_abandoned(&mut self, sets: impl IntoIterator<Item = usize>) {
        for ts in sets {
            if let Some(key) = self.auto_rebake.pending.remove(&ts).or_else(|| self.bake_inputs_key(ts)) {
                self.auto_rebake.seen.insert(ts, key);
                // The map as it stands is left alone for these inputs.
                let image = self.auto_rebake.map.and_then(|m| self.doc.baked.get(&ts)?.maps.get(&m).cloned());
                if let Some(image) = image {
                    self.auto_rebake.remember(&image, key);
                }
            }
        }
    }

    /// Bakes every due set's Auto-rebake map now (blocking); returns the sets.
    pub fn auto_rebake_now(&mut self, cpu: bool) -> Result<Vec<usize>> {
        let due = self.auto_rebake_due();
        let map = self.auto_rebake.map;
        for &ts in &due {
            self.begin_bake(ts);
            let mut task = self.bake_task_for(ts, map)?;
            if cpu {
                task.tracer = None;
            }
            let out = task.run(&Progress::new())?;
            self.apply_bake(ts, out)?;
        }
        Ok(due)
    }
}

fn state(s: &mut Session) -> Value {
    json!({ "map": s.auto_rebake.map.map(BakedMap::id), "due": s.auto_rebake_due() })
}

fn auto_rebake(s: &mut Session, p: &Value) -> Result<Value> {
    if boolp(p, "off") == Some(true) || p.get("map").is_some_and(Value::is_null) {
        s.auto_rebake = AutoRebake::default();
    } else if let Some(id) = p.get("map") {
        let map = id.as_str().and_then(BakedMap::parse).ok_or_else(|| bad("bake.autoRebake", format!("unknown mesh map {id}")))?;
        if s.auto_rebake.map != Some(map) {
            s.auto_rebake = AutoRebake { map: Some(map), ..AutoRebake::default() };
        }
    }
    Ok(state(s))
}

fn auto_rebake_run(s: &mut Session, p: &Value) -> Result<Value> {
    if s.auto_rebake.map.is_none() {
        return Err(bad("bake.autoRebake.run", "no Auto-rebake map is set; run bake.autoRebake {map} first"));
    }
    let rebaked = s.auto_rebake_now(boolp(p, "cpu") == Some(true))?;
    s.update();
    Ok(json!({ "rebaked": rebaked }))
}
