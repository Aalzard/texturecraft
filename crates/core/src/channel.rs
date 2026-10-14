//! Material channels of a texture set.

use serde::{Deserialize, Serialize};

/// A material channel. Order is the display order in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Channel {
    BaseColor,
    Metallic,
    Roughness,
    Normal,
    Height,
    Emissive,
    Opacity,
    AmbientOcclusion,
    SpecularLevel,
    /// Specular/glossiness workflow: the F0 color.
    Specular,
    /// Specular/glossiness workflow: 1 − roughness.
    Glossiness,
}

/// How a channel's values are stored and interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// Linear RGB color, displayed and exported sRGB-encoded.
    Color,
    /// Tangent-space normal packed into 0..1 (`n * 0.5 + 0.5`).
    NormalMap,
    /// A 0..1 scalar.
    Scalar,
    /// A signed scalar (height).
    Signed,
}

impl Channel {
    pub const ALL: [Channel; 11] = [
        Channel::BaseColor,
        Channel::Metallic,
        Channel::Roughness,
        Channel::Normal,
        Channel::Height,
        Channel::Emissive,
        Channel::Opacity,
        Channel::AmbientOcclusion,
        Channel::SpecularLevel,
        Channel::Specular,
        Channel::Glossiness,
    ];

    /// The channels of the default "PBR - Metallic Roughness" template.
    pub const DEFAULT_SET: [Channel; 5] = [Channel::BaseColor, Channel::Metallic, Channel::Roughness, Channel::Normal, Channel::Height];

    pub fn kind(self) -> ChannelKind {
        match self {
            Channel::BaseColor | Channel::Emissive | Channel::Specular => ChannelKind::Color,
            Channel::Normal => ChannelKind::NormalMap,
            Channel::Height => ChannelKind::Signed,
            _ => ChannelKind::Scalar,
        }
    }

    /// Value components (without coverage): 3 for colors and normals, 1 for scalars.
    pub fn components(self) -> usize {
        match self.kind() {
            ChannelKind::Color | ChannelKind::NormalMap => 3,
            ChannelKind::Scalar | ChannelKind::Signed => 1,
        }
    }

    /// The value a texel has when no layer covers it.
    pub fn default_value(self) -> [f32; 3] {
        match self {
            Channel::BaseColor => [0.5, 0.5, 0.5],
            Channel::Metallic | Channel::Height | Channel::Emissive => [0.0; 3],
            Channel::Roughness => [0.5; 3],
            Channel::Normal => [0.5, 0.5, 1.0],
            Channel::Opacity | Channel::AmbientOcclusion => [1.0; 3],
            Channel::SpecularLevel | Channel::Glossiness => [0.5; 3],
            // F0 of a common dielectric (4 %).
            Channel::Specular => [0.04; 3],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::BaseColor => "Base color",
            Channel::Metallic => "Metallic",
            Channel::Roughness => "Roughness",
            Channel::Normal => "Normal",
            Channel::Height => "Height",
            Channel::Emissive => "Emissive",
            Channel::Opacity => "Opacity",
            Channel::AmbientOcclusion => "Ambient occlusion",
            Channel::SpecularLevel => "Specular level",
            Channel::Specular => "Specular",
            Channel::Glossiness => "Glossiness",
        }
    }

    /// Short label used on material channel toggle buttons.
    pub fn short(self) -> &'static str {
        match self {
            Channel::BaseColor => "color",
            Channel::Metallic => "metal",
            Channel::Roughness => "rough",
            Channel::Normal => "nrm",
            Channel::Height => "height",
            Channel::Emissive => "emiss",
            Channel::Opacity => "op",
            Channel::AmbientOcclusion => "ao",
            Channel::SpecularLevel => "spec",
            Channel::Specular => "specular",
            Channel::Glossiness => "gloss",
        }
    }

    /// Stable machine id (commands, export templates, file names).
    pub fn id(self) -> &'static str {
        match self {
            Channel::BaseColor => "baseColor",
            Channel::Metallic => "metallic",
            Channel::Roughness => "roughness",
            Channel::Normal => "normal",
            Channel::Height => "height",
            Channel::Emissive => "emissive",
            Channel::Opacity => "opacity",
            Channel::AmbientOcclusion => "ambientOcclusion",
            Channel::SpecularLevel => "specularLevel",
            Channel::Specular => "specular",
            Channel::Glossiness => "glossiness",
        }
    }

    /// Parse an id, label or short label (case-insensitive, spaces/underscores ignored).
    pub fn parse(s: &str) -> Option<Channel> {
        let norm = |x: &str| x.to_ascii_lowercase().replace([' ', '_', '-'], "");
        let want = norm(s);
        Channel::ALL.into_iter().find(|c| norm(c.id()) == want || norm(c.label()) == want || norm(c.short()) == want || norm(&format!("{c:?}")) == want)
    }

    pub fn index(self) -> usize {
        Channel::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// Storage format of a channel in a texture set (what export and the UI report).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelFormat {
    Srgb8,
    L8,
    Rgb8,
    L16,
    Rgb16,
    L16F,
    Rgb16F,
    L32F,
    Rgb32F,
}

impl ChannelFormat {
    pub fn default_for(c: Channel) -> ChannelFormat {
        match c.kind() {
            ChannelKind::Color => ChannelFormat::Srgb8,
            ChannelKind::NormalMap => ChannelFormat::Rgb16F,
            ChannelKind::Scalar => ChannelFormat::L8,
            ChannelKind::Signed => ChannelFormat::L16F,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ChannelFormat::Srgb8 => "sRGB8",
            ChannelFormat::L8 => "L8",
            ChannelFormat::Rgb8 => "RGB8",
            ChannelFormat::L16 => "L16",
            ChannelFormat::Rgb16 => "RGB16",
            ChannelFormat::L16F => "L16F",
            ChannelFormat::Rgb16F => "RGB16F",
            ChannelFormat::L32F => "L32F",
            ChannelFormat::Rgb32F => "RGB32F",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_ids_labels_and_shorts() {
        for c in Channel::ALL {
            assert_eq!(Channel::parse(c.id()), Some(c));
            assert_eq!(Channel::parse(c.label()), Some(c));
            assert_eq!(Channel::parse(c.short()), Some(c));
        }
        assert_eq!(Channel::parse("base color"), Some(Channel::BaseColor));
        assert_eq!(Channel::parse("nope"), None);
    }

    #[test]
    fn index_matches_all_order() {
        for (i, c) in Channel::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
        }
    }
}
