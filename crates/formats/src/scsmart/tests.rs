use serde_json::{Value, json};
use texturecraft_core::{BlendMode, Channel, EffectKind, Filter, GeneratorKind, LayerId, LayerKind, MaskBackground, ProceduralKind, Projection, Source};
use texturecraft_texel::from_u16;

use super::fixture::{self, Builder, blank, raster};
use super::*;

fn import(bytes: &[u8]) -> ScsmartImport {
    read_scsmart(bytes).expect("imports")
}

fn root(i: &ScsmartImport) -> LayerId {
    *i.file.project.texture_sets[0].stack.last().expect("a root layer")
}

fn kids(i: &ScsmartImport, id: LayerId) -> Vec<LayerId> {
    i.file.project.layers[&id].children().to_vec()
}

fn named(i: &ScsmartImport, name: &str) -> LayerId {
    *i.file.project.layers.iter().find(|(_, l)| l.name == name).map(|(id, _)| id).expect("layer exists")
}

fn has_issue(r: &ImportReport, severity: Severity, what: &str) -> bool {
    r.issues.iter().any(|i| i.severity == severity && (i.what.contains(what) || i.message.contains(what)))
}

fn one_layer(layer: Value) -> Builder {
    Builder::new("T", 128).layers(json!([layer]))
}

fn fill_layer(channels: Value) -> Value {
    json!({"id": "a", "name": "F", "visible": true, "blend": [{"channel": "base_color", "mode": "normal", "opacity": 1.0}],
           "content": {"type": "fill", "projection": {"mode": "uv"}, "channels": channels}})
}

// ---- the full sample ----------------------------------------------------------------------

#[test]
fn maps_the_sample_smart_material() {
    let i = import(&fixture::sample());
    let p = &i.file.project;
    p.validate().expect("a valid project");
    let folder = root(&i);
    assert_eq!(p.layers[&folder].name, "Painted Metal");
    assert!(matches!(p.layers[&folder].kind, LayerKind::Group { collapsed: false, .. }));
    assert_eq!(p.layers[&folder].blend(Channel::BaseColor).map(|b| b.mode), Some(BlendMode::Passthrough));
    let layers = kids(&i, folder);
    assert_eq!(layers.len(), 3);
    assert_eq!(p.texture_sets[0].resolution, 128);

    // Steel: uniform, procedural and image sources; the mesh-map channel and `user0` are gone.
    let steel = &p.layers[&named(&i, "Steel")];
    let LayerKind::Fill(fill) = &steel.kind else { panic!("fill") };
    assert_eq!(fill.sources.get(&Channel::BaseColor), Some(&Source::Uniform([0.6, 0.6, 0.65])));
    assert!(matches!(fill.projection, Projection::Triplanar { scale, .. } if scale == 2.0));
    let Some(Source::Procedural { pattern, low, high }) = fill.sources.get(&Channel::Roughness) else { panic!("procedural roughness") };
    assert_eq!((pattern.kind, pattern.scale, pattern.seed), (ProceduralKind::Clouds, 6.0, 3));
    assert!((pattern.contrast - 1.0 / 1.002).abs() < 1e-4 && pattern.balance.abs() < 1e-4, "identity remap: {pattern:?}");
    assert_eq!((*low, *high), ([0.3; 3], [0.6; 3]));
    let Some(Source::Image { name, .. }) = fill.sources.get(&Channel::Normal) else { panic!("image normal") };
    assert!(i.file.resources.contains_key(name), "the image travels with the material");
    assert!(!fill.sources.contains_key(&Channel::Height), "mesh-map source cannot be mapped");
    assert!(!steel.channels.contains_key(&Channel::Height), "an unmappable channel is disabled so it writes nothing");
    assert_eq!(steel.channels.get(&Channel::Normal).map(|b| b.mode), Some(BlendMode::Normal));
    assert_eq!(steel.channels.len(), 4, "base color, metallic, roughness, normal");
    assert_eq!(p.layers[&named(&i, "Steel")].blend(Channel::Height), None);

    // Scratches: multiply at 80 %, painted roughness (premultiplied by coverage), the mask.
    let scratches = named(&i, "Scratches");
    let l = &p.layers[&scratches];
    assert_eq!(l.blend(Channel::Roughness).map(|b| (b.mode, b.opacity)), Some((BlendMode::Multiply, 0.8)));
    let img = i.file.pixels.layer(scratches, Channel::Roughness).expect("painted roughness");
    let tile1 = img.get(64, 0);
    assert!((from_u16(tile1[0]) - 0.9).abs() < 1e-3 && from_u16(tile1[1]) > 0.999, "{tile1:?}");
    assert_eq!(img.get(0, 0), [0; 4], "unpainted tiles stay empty");

    let mask = l.mask.as_ref().expect("mask");
    assert_eq!(mask.background, MaskBackground::White);
    let kinds: Vec<&str> = mask.effects.iter().map(|e| e.kind.name()).collect();
    assert_eq!(kinds, ["Paint", "Generator", "Fill", "Invert", "Blur", "Levels"], "base paint, edges, image fill + inversion, blur (median dropped), levels");
    let base = i.file.pixels.masks.get(&mask.effects[0].id).expect("painted mask base");
    assert_eq!(base.get(64, 64)[0], 0, "the hole in the mask base is black");
    assert!(base.get(64, 64)[1] > 60000, "and covered");
    assert_eq!(base.get(0, 0)[1], 0, "unstored base tiles are not painted");
    let EffectKind::Generator(g) = &mask.effects[1].kind else { panic!("generator") };
    assert_eq!((g.kind, g.level, g.seed, g.noise_scale), (GeneratorKind::EdgeWear, 0.4, 5, 8.0));
    assert_eq!((mask.effects[2].blend, mask.effects[2].opacity), (BlendMode::Multiply, 0.5));
    assert!(matches!(&mask.effects[2].kind, EffectKind::Fill { source: Source::Image { .. }, .. }));
    assert!(matches!(mask.effects[4].kind, EffectKind::Filter(Filter::Blur { radius }) if (radius - 64.0).abs() < 1e-3), "4 px at 128 is 64 texels at 2K");

    // Rust: hidden, unknown blend mode, strength folded into the value, content filter mapped.
    let rust = &p.layers[&named(&i, "Rust")];
    assert!(!rust.visible);
    assert_eq!(rust.blend(Channel::BaseColor).map(|b| (b.mode, b.opacity)), Some((BlendMode::Normal, 0.7)));
    let LayerKind::Fill(f) = &rust.kind else { panic!("fill") };
    assert_eq!(f.sources.get(&Channel::BaseColor), Some(&Source::Uniform([0.225, 0.1, 0.05])));
    assert!(matches!(f.projection, Projection::Uv { scale: [3.0, 3.0], rotation: 15.0, .. }));
    assert!(
        matches!(rust.content_effects.as_slice(), [e] if matches!(e.kind, EffectKind::Filter(Filter::Hsl { hue, saturation, .. }) if (hue - 0.05).abs() < 1e-4 && (saturation - 0.5).abs() < 1e-4))
    );

    // The report.
    let r = &i.report;
    assert_eq!((r.name.as_str(), r.schema_version, r.resolution), ("Painted Metal", 16, 128));
    assert_eq!((r.layers, r.folders, r.masks, r.images), (4, 1, 1, 2));
    assert!(has_issue(r, Severity::Dropped, "mesh-map fill"), "{:#?}", r.issues);
    assert!(has_issue(r, Severity::Dropped, "user0"));
    assert!(has_issue(r, Severity::Dropped, "filter \"median\""));
    assert!(has_issue(r, Severity::Approximated, "blend mode \"wormhole\""));
    assert!(has_issue(r, Severity::Note, "procedural patterns"));
    assert!(!r.is_clean());
    assert!(r.summary().starts_with("Imported \"Painted Metal\": 4 layers, 1 masks, 2 images; 1 approximated"), "{}", r.summary());
    assert!(r.lines().len() > 4);
    assert!(i.report.issues.iter().all(|i| i.count >= 1));
}

#[test]
fn the_report_serializes_for_the_control_protocol() {
    let v = serde_json::to_value(&import(&fixture::sample()).report).expect("serializes");
    assert_eq!(v["name"], "Painted Metal");
    assert!(v["issues"].as_array().is_some_and(|a| a.iter().any(|i| i["severity"] == "dropped")));
}

#[test]
fn recognizes_smart_materials_only() {
    assert!(is_scsmart(&fixture::sample()));
    assert!(!is_scsmart(b"not a zip"));
    assert!(!is_scsmart(&[]));
    let other = Builder::new("x", 128);
    let mut j = other.json.clone();
    j["format"] = json!("subcraft-project");
    assert!(!is_scsmart(&other.build_with(&j.to_string())));
}

// ---- mapping details -----------------------------------------------------------------------

#[test]
fn procedural_balance_and_contrast_follow_subcraft_remap() {
    // SubCraft: out = (v - balance) * k + 0.5 with k = 1 / ((1 - contrast) + 0.002).
    let i = import(
        &one_layer(fill_layer(json!([{"channel": "roughness", "enabled": true, "source": {"type": "procedural",
            "params": {"kind": "wood_rings", "scale": 3.0, "seed": 9, "balance": 0.4, "contrast": 0.5, "invert": true}, "low": [0.1, 0.1, 0.1], "high": [0.8, 0.8, 0.8]}}])))
        .build(),
    );
    let LayerKind::Fill(f) = &i.file.project.layers[&root(&i)].kind else { panic!() };
    let Some(Source::Procedural { pattern, .. }) = f.sources.get(&Channel::Roughness) else { panic!() };
    let k = 1.0 / (0.5 + 0.002);
    assert!((pattern.contrast - k).abs() < 1e-3 && (pattern.balance - k * 0.1).abs() < 1e-3, "{pattern:?}");
    assert_eq!((pattern.kind, pattern.invert), (ProceduralKind::Wood, true));
    assert!(has_issue(&i.report, Severity::Approximated, "wood_rings"));
}

#[test]
fn disabled_and_unmappable_fill_channels_write_nothing() {
    let i = import(
        &Builder::new("T", 128)
            .layers(json!([{"id": "a", "name": "F", "visible": true,
                "blend": [{"channel": "base_color", "mode": "normal", "opacity": 1.0}, {"channel": "metallic", "mode": "normal", "opacity": 1.0}, {"channel": "roughness", "mode": "normal", "opacity": 1.0}],
                "content": {"type": "fill", "projection": {"mode": "uv"}, "channels": [
                    {"channel": "base_color", "enabled": true, "source": {"type": "bitmap", "resource": "missing"}},
                    {"channel": "metallic", "enabled": false, "source": {"type": "uniform", "value": [1.0, 1.0, 1.0]}},
                    {"channel": "roughness", "enabled": true, "source": {"type": "teleport"}}]}}]))
            .build(),
    );
    let l = &i.file.project.layers[&root(&i)];
    assert!(l.channels.is_empty(), "{:?}", l.channels);
    assert!(has_issue(&i.report, Severity::Dropped, "image"));
    assert!(has_issue(&i.report, Severity::Dropped, "fill source \"teleport\""));
}

#[test]
fn unreadable_images_are_reported_not_fatal() {
    let b = Builder::new("T", 128)
        .image("bad", "broken.png", b"definitely not a png".to_vec())
        .layers(json!([fill_layer(json!([{"channel": "base_color", "enabled": true, "source": {"type": "bitmap", "resource": "bad"}}]))]));
    let i = import(&b.build());
    assert!(has_issue(&i.report, Severity::Dropped, "broken.png"));
    assert!(i.file.resources.is_empty());
    // An image listed in the table but missing from the container.
    let mut b = Builder::new("T", 128)
        .layers(json!([fill_layer(json!([{"channel": "base_color", "enabled": true, "source": {"type": "bitmap", "resource": "gone"}}]))]));
    b.json["resources"] = json!([{"id": "gone", "name": "x.png", "kind": "texture", "file": "resources/gone.png"}]);
    assert!(has_issue(&import(&b.build()).report, Severity::Dropped, "x.png"));
}

#[test]
fn identical_images_share_a_name_and_different_ones_never_do() {
    let b = Builder::new("T", 128)
        .image("a", "same.png", fixture::png(4, 4))
        .image("b", "same.png", fixture::png(4, 4))
        .image("c", "same.png", fixture::png(6, 6))
        .layers(json!([{"id": "g", "name": "G", "visible": true, "blend": [], "content": {"type": "folder", "expanded": true, "children": [
            fill_layer(json!([{"channel": "base_color", "enabled": true, "source": {"type": "bitmap", "resource": "a"}}])),
            fill_layer(json!([{"channel": "base_color", "enabled": true, "source": {"type": "bitmap", "resource": "b"}}])),
            fill_layer(json!([{"channel": "base_color", "enabled": true, "source": {"type": "bitmap", "resource": "c"}}]))]}}]));
    let i = import(&b.build());
    assert_eq!(i.file.resources.len(), 2, "{:?}", i.file.resources.keys());
    assert_eq!(i.report.images, 2);
}

#[test]
fn generators_map_or_are_dropped_with_a_reason() {
    let eff = |g: Value| json!({"id": "e", "name": "G", "visible": true, "blend": "normal", "opacity": 1.0, "kind": {"type": "generator", "generator": g}});
    let layer = json!({"id": "a", "name": "L", "visible": true, "blend": [{"channel": "base_color", "mode": "normal", "opacity": 1.0}],
        "mask": {"enabled": true, "base": "rasters/m.tiles", "effects": [
            eff(json!({"kind": "cavity", "level": 0.2, "softness": 0.1, "invert": true, "noise_amount": 0.0, "direction": [0, 1, 0]})),
            eff(json!({"kind": "position_gradient", "level": 0.5, "softness": 0.5, "invert": false, "noise_amount": 0.0, "direction": [0, 1, 0], "axis": "z"})),
            eff(json!({"kind": {"mesh_map": "thickness"}, "level": 0.5, "softness": 0.5, "invert": false, "noise_amount": 0.0, "direction": [0, 1, 0]})),
            eff(json!({"kind": {"mesh_map": "world_space_normal"}, "level": 0.5, "softness": 0.5, "invert": false, "noise_amount": 0.0, "direction": [0, 1, 0]})),
            eff(json!({"kind": "from_the_future", "level": 0.5})),
            eff(json!({"kind": "concave", "level": 0.5, "softness": 0.5, "invert": false, "noise_amount": 0.0, "direction": [0, 1, 0], "edge_spread": {"distance": 1.0, "variation": 0.5}}))]},
        "content": {"type": "paint", "channels": []}});
    let i = import(&Builder::new("T", 128).entry("rasters/m.tiles", blank(128, 1, [0.0; 4])).layers(json!([layer])).build());
    let l = &i.file.project.layers[&root(&i)];
    let mask = l.mask.as_ref().expect("mask");
    assert_eq!(mask.background, MaskBackground::Black);
    let gens: Vec<(GeneratorKind, bool, [f32; 3])> =
        mask.effects.iter().filter_map(|e| if let EffectKind::Generator(g) = &e.kind { Some((g.kind, g.invert, g.direction)) } else { None }).collect();
    assert_eq!(
        gens,
        vec![
            (GeneratorKind::Dirt, true, [0.0, 1.0, 0.0]),
            (GeneratorKind::Position, false, [0.0, 0.0, 1.0]),
            (GeneratorKind::ThinAreas, false, [0.0, 1.0, 0.0]),
            (GeneratorKind::Curvature, true, [0.0, 1.0, 0.0])
        ]
    );
    assert!(has_issue(&i.report, Severity::Dropped, "world_space_normal"));
    assert!(has_issue(&i.report, Severity::Dropped, "from_the_future"));
    assert!(has_issue(&i.report, Severity::Approximated, "concave"));
}

#[test]
fn filters_map_with_units_scaled_and_unmappable_ones_reported() {
    let f = |v: Value| json!({"id": "e", "name": "F", "visible": true, "blend": "normal", "opacity": 1.0, "kind": {"type": "filter", "filter": v}});
    let layer = json!({"id": "a", "name": "L", "visible": true, "blend": [], "mask": {"enabled": true, "base": "rasters/m.tiles", "effects": [
        f(json!({"kind": "threshold", "cutoff": 0.3})),
        f(json!({"kind": "directional_blur", "radius": 8.0, "angle_deg": -90.0})),
        f(json!({"kind": "posterize", "levels": 4})),
        f(json!({"kind": "gradient_map", "gradient": [{"position": 0.0, "color": [0, 0, 0]}, {"position": 0.25, "color": [1, 0, 0]}, {"position": 1.0, "color": [1, 1, 1]}], "to_gray": false})),
        f(json!({"kind": "gradient_map", "gradient": [{"position": 0.0, "color": [0, 0, 0]}, {"position": 0.5, "color": [0.5, 0.5, 0.5]}, {"position": 1.0, "color": [1, 1, 1]}], "to_gray": false})),
        f(json!({"kind": "mask_outline", "radius": 4.0, "threshold": 0.5, "position": "both"})),
        f(json!({"kind": "normal_invert", "flip_x": true, "flip_y": false, "flip_z": false})),
        f(json!({"kind": "sparkle"})),
        f(json!({"kind": "histogram_scan", "position": 0.5, "contrast": 0.5, "invert": true}))]},
        "content": {"type": "paint", "channels": []}});
    let i = import(&Builder::new("T", 1024).entry("rasters/m.tiles", blank(1024, 1, [1.0; 4])).layers(json!([layer])).build());
    let mask = i.file.project.layers[&root(&i)].mask.clone().expect("mask");
    let fs: Vec<&Filter> = mask.effects.iter().filter_map(|e| if let EffectKind::Filter(f) = &e.kind { Some(f) } else { None }).collect();
    assert_eq!(fs.len(), 6, "{fs:?}");
    assert!(matches!(fs[0], Filter::Threshold { level } if (*level - 0.3).abs() < 1e-6));
    assert!(
        matches!(fs[1], Filter::DirectionalBlur { radius, angle } if (*radius - 16.0).abs() < 1e-4 && (*angle - 270.0).abs() < 1e-4),
        "8 px at 1024 is 16 at 2K; -90 wraps"
    );
    assert!(matches!(fs[2], Filter::Posterize { levels } if *levels == 4.0));
    assert!(matches!(fs[3], Filter::GradientMap { colors } if colors.len() == 16), "uneven stops are resampled");
    assert!(matches!(fs[4], Filter::GradientMap { colors } if colors.len() == 3), "even stops are kept");
    assert!(matches!(fs[5], Filter::HistogramScan { .. }));
    assert!(has_issue(&i.report, Severity::Dropped, "mask_outline"));
    assert!(has_issue(&i.report, Severity::Dropped, "normal_invert"));
    assert!(has_issue(&i.report, Severity::Dropped, "sparkle"));
    assert!(has_issue(&i.report, Severity::Approximated, "histogram_scan"));
}

#[test]
fn content_filters_map_only_when_the_layer_writes_that_channel_alone() {
    let fx = |channel: &str| {
        json!([{"id": "x", "name": "Tune", "visible": true, "blend": [], "kind": {"type": "filter", "channel": channel, "filter": {"kind": "invert"}}},
        {"id": "y", "name": "Curve", "visible": true, "blend": [], "kind": {"type": "levels", "channel": channel, "levels": {"in_low": 0.0, "in_high": 1.0, "gamma": 1.0, "out_low": 0.0, "out_high": 1.0}}}])
    };
    let layer = |blend: Value, ch: &str| json!({"id": "a", "name": "L", "visible": true, "blend": blend, "content": {"type": "paint", "channels": []}, "content_effects": fx(ch)});
    let one = import(&one_layer(layer(json!([{"channel": "roughness", "mode": "normal", "opacity": 1.0}]), "roughness")).build());
    assert_eq!(one.file.project.layers[&root(&one)].content_effects.len(), 2);
    let two = import(
        &one_layer(layer(
            json!([{"channel": "roughness", "mode": "normal", "opacity": 1.0}, {"channel": "metallic", "mode": "normal", "opacity": 1.0}]),
            "roughness",
        ))
        .build(),
    );
    assert!(two.file.project.layers[&root(&two)].content_effects.is_empty());
    assert!(has_issue(&two.report, Severity::Dropped, "filter on roughness"));
    assert!(has_issue(&two.report, Severity::Dropped, "levels on roughness"));
}

#[test]
fn content_paint_and_fill_effects_become_effects() {
    let layer = json!({"id": "a", "name": "L", "visible": true, "blend": [{"channel": "roughness", "mode": "normal", "opacity": 1.0}], "content": {"type": "paint", "channels": []},
        "content_effects": [
            {"id": "x", "name": "Dust", "visible": true, "blend": [{"channel": "roughness", "mode": "overlay", "opacity": 0.4}], "kind": {"type": "paint", "channels": [{"channel": "roughness", "raster": "rasters/p.tiles"}]}},
            {"id": "y", "name": "Grain", "visible": true, "blend": [{"channel": "roughness", "mode": "screen", "opacity": 1.0}, {"channel": "metallic", "mode": "multiply", "opacity": 1.0}],
             "kind": {"type": "fill", "projection": {"mode": "uv"}, "channels": [{"channel": "roughness", "enabled": true, "source": {"type": "uniform", "value": [0.5, 0.5, 0.5]}}]}}]});
    let tile = |_: usize| vec![0.5, 0.5];
    let i = import(&one_layer(layer).entry("rasters/p.tiles", raster(128, 2, [0.0; 4], &[(0, &tile)])).build());
    let l = &i.file.project.layers[&root(&i)];
    assert_eq!(l.content_effects.len(), 2);
    let paint = &l.content_effects[0];
    assert_eq!((paint.blend, paint.opacity), (BlendMode::Overlay, 0.4));
    let px = i.file.pixels.layer(paint.id, Channel::Roughness).expect("effect pixels keyed by the effect").get(1, 1);
    assert!((from_u16(px[0]) - 0.25).abs() < 1e-3 && (from_u16(px[1]) - 0.5).abs() < 1e-3, "premultiplied: {px:?}");
    assert!(matches!(l.content_effects[1].kind, EffectKind::Material(_)));
    assert!(has_issue(&i.report, Severity::Approximated, "per-channel blending"));
}

#[test]
fn several_roots_are_wrapped_and_editable_sources_become_notes() {
    let paint = |name: &str| json!({"id": name, "name": name, "visible": true, "blend": [], "content": {"type": "paint", "channels": [], "path": {"anchors": []}, "text": null}});
    let i = import(&Builder::new("Pair", 128).layers(json!([paint("A"), paint("B")])).build());
    let r = root(&i);
    assert_eq!(i.file.project.layers[&r].name, "Pair");
    assert_eq!(kids(&i, r).len(), 2);
    assert!(has_issue(&i.report, Severity::Note, "pen path"));
    assert!(has_issue(&i.report, Severity::Note, "several top-level layers"));
}

#[test]
fn restricted_mesh_parts_are_reported() {
    let layer = json!({"id": "a", "name": "L", "visible": true, "blend": [], "parts": {"enabled": true, "kind": "mesh", "parts": [1, 2]}, "content": {"type": "paint", "channels": []}});
    assert!(has_issue(&import(&one_layer(layer).build()).report, Severity::Dropped, "mesh parts"));
}

// ---- versions and kinds --------------------------------------------------------------------

fn with(f: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut b = Builder::new("V", 128).layers(json!([fill_layer(json!([]))]));
    f(&mut b.json);
    b.build()
}

#[test]
fn schema_versions() {
    for v in [1u32, 2, 5, 9, 16] {
        let i = import(&with(|j| j["schema_version"] = json!(v)));
        assert_eq!(i.report.schema_version, v, "older schemas load unchanged");
    }
    let newer = read_scsmart(&with(|j| j["schema_version"] = json!(17))).expect_err("newer schema");
    assert!(matches!(&newer, FormatError::Unsupported(m) if m.contains("newer SubCraft") && m.contains("schema 17")), "{newer}");
    assert!(read_scsmart(&with(|j| j["schema_version"] = json!(u64::MAX))).is_err());
    assert!(read_scsmart(&with(|j| j["schema_version"] = json!(0))).is_err());
    assert!(read_scsmart(&with(|j| j["schema_version"] = json!("16"))).is_err());
    assert!(
        read_scsmart(&with(|j| {
            j.as_object_mut().map(|o| o.remove("schema_version"));
        }))
        .is_err()
    );
}

#[test]
fn rejects_what_is_not_a_smart_material() {
    assert!(matches!(read_scsmart(&with(|j| j["kind"] = json!("smart_mask"))), Err(FormatError::Unsupported(m)) if m.contains("smart mask")));
    assert!(read_scsmart(&with(|j| j["kind"] = json!("mystery"))).is_err());
    assert!(read_scsmart(&with(|j| j["format"] = json!("subcraft-project"))).is_err());
    assert!(read_scsmart(&with(|j| j["resolution"] = json!(100))).is_err());
    assert!(read_scsmart(&with(|j| j["resolution"] = json!(8192))).is_err());
    assert!(read_scsmart(&with(|j| j["layers"] = json!([]))).is_err(), "no layers");
    assert!(read_scsmart(&with(|j| j["name"] = json!(null))).is_ok(), "a missing name gets a default");
    let unnamed = import(&with(|j| j["name"] = json!("  \u{7}  ")));
    assert_eq!(unnamed.report.name, "SubCraft smart material");
}

// ---- hostile and truncated files -----------------------------------------------------------

#[test]
fn garbage_empty_and_truncated_files_are_errors() {
    assert!(read_scsmart(b"").is_err());
    assert!(read_scsmart(b"PK\x03\x04 and then nonsense").is_err());
    assert!(read_scsmart(&[0u8; 4096]).is_err());
    let good = fixture::sample();
    for cut in (0..good.len()).step_by((good.len() / 40).max(1)) {
        assert!(read_scsmart(&good[..cut]).is_err(), "a file cut at {cut} of {} must not import", good.len());
    }
    assert!(read_scsmart(&good[..good.len() - 1]).is_err());
}

#[test]
fn malformed_documents_are_errors() {
    let b = Builder::new("x", 128);
    for text in [
        "",
        "{",
        "[]",
        "null",
        "{\"format\": 5}",
        &"[".repeat(5000),
        "{\"format\":\"subcraft-fragment\",\"schema_version\":16,\"kind\":\"smart_material\",\"resolution\":128,\"layers\":5}",
    ] {
        let r = read_scsmart(&b.build_with(text));
        assert!(r.is_err() || r.is_ok_and(|i| i.report.layers == 0), "{text:.60}");
    }
}

#[test]
fn damaged_rasters_are_errors_with_the_blob_named() {
    let layer = |raster: &str| json!({"id": "a", "name": "P", "visible": true, "blend": [{"channel": "roughness", "mode": "normal", "opacity": 1.0}], "content": {"type": "paint", "channels": [{"channel": "roughness", "raster": raster}]}});
    let tile = |_: usize| vec![0.5, 1.0];
    let good = raster(128, 2, [0.0; 4], &[(0, &tile)]);
    let try_blob = |blob: Vec<u8>| read_scsmart(&one_layer(layer("rasters/r.tiles")).entry("rasters/r.tiles", blob).build());
    assert!(try_blob(good.clone()).is_ok());
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    let mut bad_version = good.clone();
    bad_version[6] = 9;
    let wrong_size = raster(256, 2, [0.0; 4], &[]);
    let wrong_comps = raster(128, 3, [0.0; 4], &[]);
    let mut huge_count = good.clone();
    huge_count[33..37].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut few = good.clone();
    few[33..37].copy_from_slice(&5u32.to_le_bytes());
    let mut bad_index = good.clone();
    bad_index[37..41].copy_from_slice(&99u32.to_le_bytes());
    let twice = raster(128, 2, [0.0; 4], &[(1, &tile), (1, &tile)]);
    let mut zero_edge = good.clone();
    zero_edge[8..12].copy_from_slice(&0u32.to_le_bytes());
    let mut giant = good.clone();
    giant[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    for (name, blob) in [
        ("bad magic", bad_magic),
        ("bad version", bad_version),
        ("wrong size", wrong_size),
        ("wrong components", wrong_comps),
        ("huge tile count", huge_count),
        ("count beyond data", few),
        ("tile index out of range", bad_index),
        ("duplicate tile", twice),
        ("zero edge", zero_edge),
        ("giant edge", giant),
        ("truncated", good[..good.len() - 7].to_vec()),
        ("empty", Vec::new()),
    ] {
        let e = try_blob(blob).expect_err(name);
        assert!(e.to_string().contains("rasters/r.tiles"), "{name}: {e}");
    }
    let missing = read_scsmart(&one_layer(layer("rasters/none.tiles")).build()).expect_err("missing raster");
    assert!(missing.to_string().contains("rasters/none.tiles"), "{missing}");
}

#[test]
fn non_finite_pixels_and_fills_import_as_zero() {
    let nan = half::f16::NAN.to_f32();
    let weird = |_: usize| vec![nan, 1.0];
    let blob = raster(128, 2, [f32::NAN, f32::INFINITY, 0.0, 0.0], &[(0, &weird)]);
    let layer = json!({"id": "a", "name": "P", "visible": true, "blend": [], "content": {"type": "paint", "channels": [{"channel": "roughness", "raster": "rasters/r.tiles"}]}});
    let i = import(&one_layer(layer).entry("rasters/r.tiles", blob).build());
    let img = i.file.pixels.layer(root(&i), Channel::Roughness).expect("pixels");
    assert_eq!(img.get(0, 0)[0], 0);
}

#[test]
fn an_opaque_fill_value_paints_unstored_tiles() {
    let layer = json!({"id": "a", "name": "P", "visible": true, "blend": [], "content": {"type": "paint", "channels": [{"channel": "roughness", "raster": "rasters/r.tiles"}]}});
    let i = import(&one_layer(layer).entry("rasters/r.tiles", blank(128, 2, [0.5, 1.0, 0.0, 0.0])).build());
    let img = i.file.pixels.layer(root(&i), Channel::Roughness).expect("pixels");
    assert!((from_u16(img.get(100, 100)[0]) - 0.5).abs() < 1e-3 && from_u16(img.get(100, 100)[1]) > 0.999);
}

#[test]
fn too_many_layers_or_images_are_refused() {
    let many: Vec<Value> = (0..MAX_LAYERS + 1)
        .map(|i| json!({"id": i.to_string(), "name": "L", "visible": true, "blend": [], "content": {"type": "paint", "channels": []}}))
        .collect();
    assert!(matches!(read_scsmart(&Builder::new("x", 128).layers(Value::Array(many)).build()), Err(FormatError::TooLarge(_))));
    let mut b = Builder::new("x", 128).layers(json!([fill_layer(json!([]))]));
    b.json["resources"] = Value::Array((0..300).map(|i| json!({"id": i.to_string(), "name": "x.png", "kind": "texture", "file": "resources/x.png"})).collect());
    assert!(matches!(read_scsmart(&b.build()), Err(FormatError::TooLarge(_))));
}

#[test]
fn too_many_container_entries_are_refused() {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..MAX_ENTRIES + 1 {
        use std::io::Write;
        z.start_file(format!("e{i}"), o).expect("entry");
        z.write_all(b"x").expect("write");
    }
    let bytes = z.finish().expect("zip").into_inner();
    assert!(matches!(read_scsmart(&bytes), Err(FormatError::TooLarge(_))));
    assert!(!is_scsmart(&bytes));
}

#[test]
fn deeply_nested_folders_are_cut_not_overflowed() {
    let mut layer = json!({"id": "leaf", "name": "leaf", "visible": true, "blend": [], "content": {"type": "paint", "channels": []}});
    for n in 0..28 {
        layer = json!({"id": n.to_string(), "name": format!("f{n}"), "visible": true, "blend": [], "content": {"type": "folder", "expanded": true, "children": [layer]}});
    }
    let i = import(&one_layer(layer).build());
    i.file.project.validate().expect("within the project's folder depth");
    assert!(has_issue(&i.report, Severity::Dropped, "layer"));
    // Nesting beyond what the JSON parser accepts is an error, not a stack overflow.
    let deep = format!("{}{}", "{\"c\":".repeat(400), "}".repeat(400));
    assert!(read_scsmart(&Builder::new("x", 128).build_with(&deep)).is_err());
}

#[test]
fn unknown_layer_and_effect_kinds_do_not_fail_the_import() {
    let layer = json!({"id": "g", "name": "G", "visible": true, "blend": [], "content": {"type": "folder", "expanded": false, "children": [
        {"id": "h", "name": "Hologram", "visible": true, "blend": [], "content": {"type": "hologram"}},
        {"id": "k", "name": "Kept", "visible": true, "blend": [], "mask": {"enabled": true, "base": "rasters/m.tiles", "effects": [
            {"id": "e", "name": "Weird", "visible": true, "blend": "normal", "opacity": 1.0, "kind": {"type": "warp_core"}}]},
         "content": {"type": "paint", "channels": []},
         "content_effects": [{"id": "x", "name": "Odd", "visible": true, "blend": [], "kind": {"type": "bloom"}}]}]}});
    let i = import(&one_layer(layer).entry("rasters/m.tiles", blank(128, 1, [1.0; 4])).build());
    let folder = root(&i);
    assert!(matches!(i.file.project.layers[&folder].kind, LayerKind::Group { collapsed: true, .. }));
    assert_eq!(kids(&i, folder).len(), 1);
    assert!(has_issue(&i.report, Severity::Dropped, "hologram"));
    assert!(has_issue(&i.report, Severity::Dropped, "warp_core"));
    assert!(has_issue(&i.report, Severity::Dropped, "bloom"));
}

#[test]
fn a_flood_of_identical_problems_is_one_counted_issue() {
    let layers: Vec<Value> = (0..200).map(|i| json!({"id": i.to_string(), "name": "L", "visible": true, "blend": [{"channel": "user1", "mode": "normal", "opacity": 1.0}], "content": {"type": "paint", "channels": []}})).collect();
    let i = import(
        &Builder::new("x", 128)
            .layers(json!([{"id": "r", "name": "R", "visible": true, "blend": [], "content": {"type": "folder", "expanded": true, "children": layers}}]))
            .build(),
    );
    let issue = i.report.issues.iter().find(|i| i.what.contains("user1")).expect("issue");
    assert_eq!(issue.count, 200);
    assert!(i.report.issues.len() < 5);
}

#[test]
fn report_issue_list_is_capped() {
    let mut r = ImportReport::default();
    for n in 0..(MAX_ISSUES + 50) {
        r.issue(Severity::Dropped, "", format!("thing {n}"), "x");
    }
    assert_eq!((r.issues.len(), r.more_issues), (MAX_ISSUES, 50));
    assert!(r.lines().last().is_some_and(|l| l.contains("50 more")));
}

/// Flip bytes everywhere in a good file: whatever happens, it is an `Ok` or an `Err`.
#[test]
fn random_corruption_never_panics() {
    let good = fixture::sample();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..300 {
        let mut bytes = good.clone();
        for _ in 0..(1 + next() % 8) {
            let at = (next() as usize) % bytes.len();
            bytes[at] = next() as u8;
        }
        let _ = read_scsmart(&bytes);
        let _ = is_scsmart(&bytes);
    }
}

/// The same on the document: corrupt `fragment.json` itself so the mapping code sees odd values.
#[test]
fn random_document_corruption_never_panics() {
    let text = {
        let b = Builder::new("x", 128);
        let _ = b;
        // The sample's document, recovered from its container.
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(fixture::sample())).expect("zip");
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("fragment.json").expect("json"), &mut s).expect("read");
        s
    };
    let template = Builder::new("x", 128).entry("rasters/00000.tiles", blank(128, 2, [0.0; 4])).entry("rasters/00001.tiles", blank(128, 1, [1.0; 4]));
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let replacements = ["null", "-1", "1e39", "[]", "{}", "\"x\"", "true", "4294967296", "0.0", "[1,2]", "-0.0", "99999999999999999999"];
    for _ in 0..400 {
        let mut s = text.clone().into_bytes();
        for _ in 0..(1 + next() % 4) {
            // Overwrite a random token-ish span with a different JSON value.
            let at = (next() as usize) % s.len();
            let rep = replacements[(next() as usize) % replacements.len()].as_bytes();
            let end = (at + rep.len()).min(s.len());
            s.splice(at..end, rep.iter().copied());
        }
        let _ = read_scsmart(&template.build_with(&String::from_utf8_lossy(&s)));
    }
}
