//! Texture set commands (Texture Set List / Texture Set Settings).

use serde_json::{Value, json};
use texturecraft_core::{Channel, ChannelBlend, ChannelFormat, NormalFormat};

use crate::commands::{CommandSpec, always, bad, boolp, channel_param, f32p, strp, u64p};
use crate::session::{StackTarget, check_resolution};
use crate::{Result, Session, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("textureSet.select", "Select texture set", [], None, "{ts}", always, select),
        cmd!("textureSet.list", "List texture sets", [], None, "{}", always, |s, _| Ok(crate::inspect::texture_sets(s))),
        cmd!(
            "textureSet.inspect",
            "Inspect texture set",
            [],
            None,
            "{ts?} → {index, name, resolution, size: [w, h], uvTiles: [UDIM numbers] (empty: no UV tiles), tiles: [{udim, block: [x, y]}], channels, layers, baked, visible, active}",
            always,
            |s, p| Ok(crate::inspect::texture_set(s, ts_param(s, p, "textureSet.inspect")?))
        ),
        cmd!("textureSet.resolution", "Texture set size", [], None, "{ts?, resolution: 128..8192 (power of two)}", always, resolution),
        cmd!("textureSet.channel.add", "Add channel", [], None, "{ts?, channel}", always, |s, p| channel(s, p, true)),
        cmd!("textureSet.channel.remove", "Remove channel", [], None, "{ts?, channel}", always, |s, p| channel(s, p, false)),
        cmd!("textureSet.visible", "Toggle texture set visibility", [], None, "{ts?, visible?}", always, visible),
        cmd!("textureSet.rename", "Rename texture set", [], None, "{ts?, name}", always, rename),
        cmd!("textureSet.settings", "Texture set settings", [], None, "{ts?, description?, heightStrength?, normalFormat?: openGl|directX}", always, settings),
    ]
}

fn ts_param(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    match u64p(p, "ts") {
        Some(t) if (t as usize) < s.set_count() => Ok(t as usize),
        Some(t) => Err(bad(cmd, format!("no texture set {t}"))),
        None => Ok(s.ts()),
    }
}

fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let ts = u64p(p, "ts").map(|t| t as usize).filter(|t| *t < s.set_count()).ok_or_else(|| bad("textureSet.select", "valid `ts` index required"))?;
    s.cancel_stroke();
    s.active_set = ts;
    s.target = StackTarget::Content;
    s.revision = s.revision.wrapping_add(1);
    Ok(json!({ "ts": ts }))
}

fn resolution(s: &mut Session, p: &Value) -> Result<Value> {
    let ts = ts_param(s, p, "textureSet.resolution")?;
    let r = u64p(p, "resolution").ok_or_else(|| bad("textureSet.resolution", "`resolution` is required"))?;
    let r = check_resolution(u32::try_from(r).unwrap_or(0))?;
    s.edit("Texture set size", |s| {
        let set = s.doc.project.texture_set_mut(ts)?;
        set.resolution = r;
        crate::session::check_size(set)?;
        let size = set.size();
        // Painted pixels are resampled (bilinear); strokes are not replayed.
        crate::layer_ops::resample_set(s, ts, size);
        Ok(())
    })?;
    Ok(json!({ "ts": ts, "resolution": r }))
}

fn channel(s: &mut Session, p: &Value, add: bool) -> Result<Value> {
    let ts = ts_param(s, p, "textureSet.channel")?;
    let c = channel_param("textureSet.channel", p, "channel")?.ok_or_else(|| bad("textureSet.channel", "`channel` is required"))?;
    s.edit(if add { "Add channel" } else { "Remove channel" }, |s| {
        if add {
            return add_channel(s, ts, c);
        }
        if c == Channel::BaseColor {
            return Err(bad("textureSet.channel.remove", "base color can't be removed"));
        }
        s.doc.project.texture_set_mut(ts)?.channels.retain(|(x, _)| *x != c);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Add channel `c` to texture set `ts` (no-op when present) and enable it on the set's layers.
/// Call inside [`Session::edit`].
pub(crate) fn add_channel(s: &mut Session, ts: usize, c: Channel) -> Result<()> {
    let set = s.doc.project.texture_set_mut(ts)?;
    if !set.has_channel(c) {
        set.channels.push((c, ChannelFormat::default_for(c)));
        set.channels.sort_by_key(|(c, _)| c.index());
    }
    // Existing layers get the new channel enabled.
    let ids: Vec<_> = s.doc.project.walk(ts).into_iter().map(|(id, _)| id).collect();
    for id in ids {
        if let Ok(l) = s.doc.project.layer_mut(id) {
            let mode = if l.is_group() { texturecraft_core::BlendMode::Passthrough } else { texturecraft_core::BlendMode::Normal };
            l.channels.entry(c).or_insert(ChannelBlend { mode, opacity: 1.0 });
        }
    }
    Ok(())
}

fn visible(s: &mut Session, p: &Value) -> Result<Value> {
    let ts = ts_param(s, p, "textureSet.visible")?;
    let set = s.doc.project.texture_set_mut(ts)?;
    set.visible = boolp(p, "visible").unwrap_or(!set.visible);
    let v = set.visible;
    s.revision = s.revision.wrapping_add(1);
    Ok(json!({ "visible": v }))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let ts = ts_param(s, p, "textureSet.rename")?;
    let name = strp(p, "name")
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.chars().count() <= 128)
        .ok_or_else(|| bad("textureSet.rename", "`name` is required"))?
        .to_string();
    s.edit("Rename texture set", |s| {
        s.doc.project.texture_set_mut(ts)?.name = name;
        Ok(())
    })?;
    Ok(Value::Null)
}

fn settings(s: &mut Session, p: &Value) -> Result<Value> {
    let ts = ts_param(s, p, "textureSet.settings")?;
    let nf = match strp(p, "normalFormat") {
        Some("openGl") | Some("opengl") => Some(NormalFormat::OpenGl),
        Some("directX") | Some("directx") => Some(NormalFormat::DirectX),
        Some(o) => return Err(bad("textureSet.settings", format!("unknown normal format `{o}`"))),
        None => None,
    };
    s.edit("Texture set settings", |s| {
        if let Some(n) = nf {
            s.doc.project.normal_format = n;
        }
        let set = s.doc.project.texture_set_mut(ts)?;
        if let Some(d) = strp(p, "description") {
            set.description = d.chars().take(4096).collect();
        }
        if let Some(h) = f32p(p, "heightStrength") {
            set.height_strength = h.clamp(0.0, 100.0);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
