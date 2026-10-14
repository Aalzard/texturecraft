//! Built-in export presets and user preset JSON. Preset names are ours; their contents follow
//! each engine's public material conventions.

use texturecraft_core::{BakedMap, Channel, ChannelKind};

use crate::export::{Dest, ExportPreset, FileFormat, MapSpec, SourceRef, VirtualMap};
use crate::{FormatError, Result};

/// Largest preset JSON accepted.
const MAX_PRESET_JSON: usize = 1 << 20;

fn ch(channel: Channel, component: u8) -> SourceRef {
    SourceRef::Channel { channel, component }
}

fn virt(map: VirtualMap, component: u8) -> SourceRef {
    SourceRef::Virtual { map, component }
}

/// `$mesh_$textureSet_<suffix>` PNG.
fn map(suffix: &str, bit_depth: u8, channels: Vec<(Dest, SourceRef)>) -> MapSpec {
    MapSpec { file: format!("$mesh_$textureSet_{suffix}"), format: FileFormat::Png, bit_depth, channels }
}

fn rgb(f: impl Fn(u8) -> SourceRef) -> Vec<(Dest, SourceRef)> {
    vec![(Dest::R, f(0)), (Dest::G, f(1)), (Dest::B, f(2))]
}

fn gray(s: SourceRef) -> Vec<(Dest, SourceRef)> {
    vec![(Dest::L, s)]
}

/// Base color RGB with opacity in alpha (alpha is dropped when the set has no opacity).
fn base_with_alpha() -> Vec<(Dest, SourceRef)> {
    let mut c = rgb(|k| ch(Channel::BaseColor, k));
    c.push((Dest::A, ch(Channel::Opacity, 0)));
    c
}

fn normal(map_kind: VirtualMap) -> Vec<(Dest, SourceRef)> {
    rgb(|k| virt(map_kind, k))
}

/// One file per channel: RGB for colors and normals, grayscale (16-bit for height) otherwise.
fn channel_map(c: Channel) -> MapSpec {
    let (channels, depth) = match c.kind() {
        ChannelKind::Color | ChannelKind::NormalMap => (rgb(|k| ch(c, k)), 8),
        ChannelKind::Signed => (gray(ch(c, 0)), 16),
        ChannelKind::Scalar => (gray(ch(c, 0)), 8),
    };
    map("$channel", depth, channels)
}

fn orm() -> Vec<(Dest, SourceRef)> {
    vec![(Dest::R, virt(VirtualMap::AoMixed, 0)), (Dest::G, ch(Channel::Roughness, 0)), (Dest::B, ch(Channel::Metallic, 0))]
}

/// File stems of the glTF metallic-roughness maps (also the GLB export's texture slots).
pub(crate) const GLTF_BASE_COLOR: &str = "BaseColor";
pub(crate) const GLTF_ORM: &str = "OcclusionRoughnessMetallic";
pub(crate) const GLTF_NORMAL: &str = "Normal";
pub(crate) const GLTF_EMISSIVE: &str = "Emissive";

/// glTF metallic-roughness maps (8-bit PNG); `file` turns a stem above into a file template.
pub(crate) fn gltf_pbr_maps(file: impl Fn(&str) -> String) -> Vec<MapSpec> {
    let spec = |stem: &str, channels| MapSpec { file: file(stem), format: FileFormat::Png, bit_depth: 8, channels };
    vec![
        spec(GLTF_BASE_COLOR, base_with_alpha()),
        spec(GLTF_ORM, orm()),
        spec(GLTF_NORMAL, normal(VirtualMap::NormalOpenGl)),
        spec(GLTF_EMISSIVE, rgb(|k| ch(Channel::Emissive, k))),
    ]
}

/// Every built-in preset, in menu order.
pub fn builtin_presets() -> Vec<ExportPreset> {
    let preset = |name: &str, maps: Vec<MapSpec>| ExportPreset { name: name.into(), maps };
    vec![
        preset("Document channels + Normal + AO", {
            let mut maps: Vec<MapSpec> =
                Channel::ALL.into_iter().filter(|c| !matches!(c, Channel::Normal | Channel::AmbientOcclusion)).map(channel_map).collect();
            maps.push(map("normal", 8, rgb(|k| ch(Channel::Normal, k))));
            maps.push(map("ambientOcclusion", 8, gray(virt(VirtualMap::AoMixed, 0))));
            maps
        }),
        preset("glTF PBR Metal Roughness", gltf_pbr_maps(|s| format!("$mesh_$textureSet_{s}"))),
        preset(
            "Unreal Engine (Packed)",
            vec![map("BaseColor", 8, base_with_alpha()), map("OcclusionRoughnessMetallic", 8, orm()), map("Normal", 8, normal(VirtualMap::NormalDirectX))],
        ),
        preset(
            "Unity HDRP (Mask Map)",
            vec![
                map("BaseMap", 8, base_with_alpha()),
                map(
                    "MaskMap",
                    8,
                    vec![
                        (Dest::R, ch(Channel::Metallic, 0)),
                        (Dest::G, virt(VirtualMap::AoMixed, 0)),
                        // Detail mask: none.
                        (Dest::B, SourceRef::Constant(0.0)),
                        (Dest::A, virt(VirtualMap::Glossiness, 0)),
                    ],
                ),
                map("Normal", 8, normal(VirtualMap::NormalOpenGl)),
            ],
        ),
        preset(
            "Unity Standard (Metallic Smoothness)",
            vec![
                map("Albedo", 8, base_with_alpha()),
                map(
                    "MetallicSmoothness",
                    8,
                    vec![
                        (Dest::R, ch(Channel::Metallic, 0)),
                        (Dest::G, SourceRef::Constant(0.0)),
                        (Dest::B, SourceRef::Constant(0.0)),
                        (Dest::A, virt(VirtualMap::Glossiness, 0)),
                    ],
                ),
                map("Normal", 8, normal(VirtualMap::NormalOpenGl)),
                map("Occlusion", 8, gray(virt(VirtualMap::AoMixed, 0))),
            ],
        ),
        preset(
            "Blender (Principled BSDF)",
            vec![
                map("BaseColor", 8, base_with_alpha()),
                map("Metallic", 8, gray(ch(Channel::Metallic, 0))),
                map("Roughness", 8, gray(ch(Channel::Roughness, 0))),
                map("Normal", 8, normal(VirtualMap::NormalOpenGl)),
                map("Height", 16, gray(ch(Channel::Height, 0))),
                map("Emission", 8, rgb(|k| ch(Channel::Emissive, k))),
                map("Specular", 8, gray(ch(Channel::SpecularLevel, 0))),
            ],
        ),
        preset("Separate maps", Channel::ALL.into_iter().map(channel_map).collect()),
        preset(
            "Mesh maps",
            BakedMap::ALL
                .into_iter()
                .map(|m| {
                    let src = |component| SourceRef::Baked { map: m, component };
                    map("$channel", 16, if m.components() == 3 { rgb(src) } else { gray(src(0)) })
                })
                .collect(),
        ),
    ]
}

/// A built-in preset by name (case-insensitive).
pub fn builtin_preset(name: &str) -> Option<ExportPreset> {
    builtin_presets().into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

/// Parse and validate a user preset.
pub fn preset_from_json(json: &str) -> Result<ExportPreset> {
    if json.len() > MAX_PRESET_JSON {
        return Err(FormatError::TooLarge("export preset JSON over 1 MiB".into()));
    }
    let p: ExportPreset = serde_json::from_str(json).map_err(|e| crate::malformed("export preset", e))?;
    p.validate()?;
    Ok(p)
}

pub fn preset_to_json(preset: &ExportPreset) -> Result<String> {
    serde_json::to_string_pretty(preset).map_err(|e| FormatError::Invalid(e.to_string()))
}
