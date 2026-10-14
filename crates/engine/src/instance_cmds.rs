//! Instance layers (linked copies that draw another layer, possibly in another texture set) and
//! per-layer properties set through `layer.set`.

use serde_json::{Value, json};
use texturecraft_core::{Layer, LayerId, LayerKind};

use crate::commands::{CommandSpec, bad, boolp, content_param, f32p, has_layer, layer_param, u64p};
use crate::session::{Document, StackTarget};
use crate::{EngineError, Result, Session, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.instance",
            "Create instance",
            ["Layers"],
            None,
            "{layer?, ts?: texture set (default active)} — a linked instance above that set's selection; edits to the source show in every instance",
            has_layer,
            instance
        ),
        cmd!(
            "layer.pasteInstance",
            "Paste as instance",
            ["Layers"],
            None,
            "{} — an instance of the copied layer above the selection",
            can_paste_instance,
            paste_instance
        ),
        cmd!(
            "layer.set",
            "Set layer properties",
            [],
            None,
            "{layer?, heightMask?: {enabled?, contrast?: 0..64, offset?: -1..1} | bool} — mask with height: m' = saturate(m + contrast·4m(1−m)·(height + offset))",
            has_layer,
            set
        ),
    ]
}

/// Insert an instance of `source` (an instance's own source) into texture set `ts` above that
/// set's selection, and select it.
pub(crate) fn make_instance(s: &mut Session, source: LayerId, ts: usize) -> Result<LayerId> {
    let src = s.doc.project.content_layer(source).ok_or_else(|| EngineError::Other(format!("layer {} has no source to instance", source.0)))?;
    let (src_id, name) = (src.id, src.name.clone());
    let id = s.doc.project.alloc_id();
    let sel = s.selected.get(&ts).copied().filter(|l| s.doc.project.locate(*l).is_some_and(|(t, _, _)| t == ts));
    let inside = sel.and_then(|l| s.doc.project.layer(l).ok()).is_some_and(Layer::is_group);
    s.doc.project.insert_layer(ts, Layer::new(id, name, LayerKind::Instance { source: src_id }, &[]), sel, inside)?;
    s.selected.insert(ts, id);
    if ts == s.ts() {
        s.target = StackTarget::Content;
    }
    Ok(id)
}

fn instance(s: &mut Session, p: &Value) -> Result<Value> {
    let source = layer_param(s, "layer.instance", p)?;
    let ts = match u64p(p, "ts") {
        Some(t) => usize::try_from(t).ok().filter(|t| *t < s.set_count()).ok_or_else(|| bad("layer.instance", format!("no texture set {t}")))?,
        None => s.ts(),
    };
    let id = s.edit("Create instance", |s| make_instance(s, source, ts))?;
    Ok(json!({ "layer": id.0, "textureSet": ts }))
}

fn can_paste_instance(s: &Session) -> std::result::Result<(), String> {
    match &s.clipboard {
        Some((_, id)) if s.doc.project.locate(*id).is_some() => Ok(()),
        Some(_) => Err("the copied layer is no longer in the project (paste it instead)".into()),
        None => Err("the layer clipboard is empty".into()),
    }
}

fn paste_instance(s: &mut Session, _: &Value) -> Result<Value> {
    let id = s.clipboard.as_ref().map(|(_, id)| *id).ok_or_else(|| EngineError::Other("the layer clipboard is empty".into()))?;
    let ts = s.ts();
    let new = s.edit("Paste as instance", |s| make_instance(s, id, ts))?;
    Ok(json!({ "layer": new.0 }))
}

/// Replace instance `inst` with an independent copy of its source taken from `src` (its name,
/// visibility, color tag and own mask are kept). Returns the copy.
pub(crate) fn detach(s: &mut Session, src: &Document, inst: LayerId) -> Result<LayerId> {
    let layer = s.doc.project.layer(inst)?.clone();
    let source = layer.instance_source().ok_or_else(|| EngineError::Other(format!("\"{}\" is not an instance", layer.name)))?;
    let (ts, _, _) = s.doc.project.locate(inst).ok_or_else(|| EngineError::Other("layer not in a stack".into()))?;
    let copy = crate::layer_ops::copy_tree(src, source, &mut s.doc, ts, Some(inst), false, 0)?;
    let l = s.doc.project.layer_mut(copy)?;
    l.name = layer.name;
    l.visible = layer.visible;
    l.color_tag = layer.color_tag;
    if layer.mask.is_some() {
        l.mask = layer.mask;
    }
    s.doc.project.remove_layer(inst)?;
    if s.selected.get(&ts) == Some(&inst) {
        s.selected.insert(ts, copy);
    }
    Ok(copy)
}

/// After layers were removed: instances of a removed layer become independent copies (taken
/// from `before`, the document before the removal), then pixels of removed layers are dropped.
pub(crate) fn detach_orphans(s: &mut Session, before: &Document) -> Result<()> {
    // Copies can hold instances of removed layers in turn; the depth bounds the rounds.
    for _ in 0..=texturecraft_core::project::MAX_DEPTH {
        let orphans: Vec<LayerId> = s
            .doc
            .project
            .layers
            .values()
            .filter(|l| l.instance_source().is_some_and(|src| !s.doc.project.layers.contains_key(&src) && before.project.layers.contains_key(&src)))
            .map(|l| l.id)
            .collect();
        if orphans.is_empty() {
            break;
        }
        for inst in orphans {
            detach(s, before, inst)?;
        }
    }
    prune_pixels(&mut s.doc);
    Ok(())
}

/// Drop pixels of layers and effects that no longer exist.
pub(crate) fn prune_pixels(doc: &mut Document) {
    let p = &doc.project;
    let alive: std::collections::BTreeSet<LayerId> = p
        .layers
        .values()
        .flat_map(|l| std::iter::once(l.id).chain(l.mask.iter().flat_map(|m| m.effects.iter()).chain(l.content_effects.iter()).map(|e| e.id)))
        .collect();
    doc.pixels.retain(|x| alive.contains(&x));
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    // Linked properties live on an instance's source.
    let id = content_param(s, "layer.set", p)?;
    let hm = match p.get("heightMask") {
        Some(Value::Bool(on)) => json!({ "enabled": on }),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(bad("layer.set", "`heightMask` must be an object {enabled?, contrast?, offset?} or a bool")),
        None => return Err(bad("layer.set", "give `heightMask`")),
    };
    let out = s.edit("Layer properties", |s| {
        let h = &mut s.doc.project.layer_mut(id)?.height_mask;
        if let Some(on) = boolp(&hm, "enabled") {
            h.enabled = on;
        }
        if let Some(c) = f32p(&hm, "contrast") {
            h.contrast = c.clamp(0.0, 64.0);
        }
        if let Some(o) = f32p(&hm, "offset") {
            h.offset = o.clamp(-1.0, 1.0);
        }
        Ok(json!({ "layer": id.0, "heightMask": { "enabled": h.enabled, "contrast": h.contrast, "offset": h.offset } }))
    })?;
    Ok(out)
}
