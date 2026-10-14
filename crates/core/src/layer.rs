//! Layers, masks, effects and fill sources.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BlendMode, Channel};

/// Id of a layer or effect (one id space per project).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId(pub u64);

/// Blend mode and opacity of one channel of a layer. A channel missing from
/// [`Layer::channels`] is disabled for that layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelBlend {
    pub mode: BlendMode,
    pub opacity: f32,
}

impl Default for ChannelBlend {
    fn default() -> Self {
        ChannelBlend { mode: BlendMode::Normal, opacity: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub kind: LayerKind,
    pub channels: BTreeMap<Channel, ChannelBlend>,
    #[serde(default)]
    pub mask: Option<Mask>,
    /// Effects applied to the layer's content, bottom → top.
    #[serde(default)]
    pub content_effects: Vec<Effect>,
    /// Painter-style color tag (0..8), shown as a stripe on the layer row.
    #[serde(default)]
    pub color_tag: Option<u8>,
    /// Combine the mask with the layer's height ("mask with height").
    #[serde(default)]
    pub height_mask: HeightMask,
}

/// Mask with height: partial mask values follow the layer's own height, so a transition
/// reveals the high parts first. With mask `m` and the layer's height `h` (−1..1):
/// `m' = saturate(m + contrast · 4m(1 − m) · (h + offset))`. Fully hidden (0) and fully shown
/// (1) texels are unchanged, as is everything where the layer has no height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeightMask {
    pub enabled: bool,
    /// How strongly the height pushes the mask (0 = plain mask), 0..64.
    pub contrast: f32,
    /// Added to the height before it pushes the mask, −1..1.
    pub offset: f32,
}

impl Default for HeightMask {
    fn default() -> Self {
        HeightMask { enabled: false, contrast: 1.0, offset: 0.0 }
    }
}

impl HeightMask {
    /// The mask `m` combined with height `h` (see [`HeightMask`]).
    pub fn apply(&self, m: f32, h: f32) -> f32 {
        if !self.enabled || !h.is_finite() {
            return m;
        }
        (m + self.contrast * 4.0 * m * (1.0 - m) * (h + self.offset)).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayerKind {
    /// Painted pixels, stored per channel by the engine under the layer id.
    Paint,
    /// A procedural or uniform material applied everywhere (shaped by the mask).
    Fill(Fill),
    /// A folder of layers, bottom → top.
    Group { children: Vec<LayerId>, collapsed: bool },
    /// A linked instance of `source` (possibly in another texture set): renders the source's
    /// content, content effects and channel blending, and its mask unless the instance has its
    /// own. Edits to the source show in every instance.
    Instance { source: LayerId },
}

impl LayerKind {
    pub fn name(&self) -> &'static str {
        match self {
            LayerKind::Paint => "paint",
            LayerKind::Fill(_) => "fill",
            LayerKind::Group { .. } => "group",
            LayerKind::Instance { .. } => "instance",
        }
    }
}

impl Layer {
    /// A layer with every channel of `channels` enabled at Normal/100%.
    pub fn new(id: LayerId, name: impl Into<String>, kind: LayerKind, channels: &[Channel]) -> Layer {
        Layer {
            id,
            name: name.into(),
            visible: true,
            kind,
            channels: channels.iter().map(|c| (*c, ChannelBlend::default())).collect(),
            mask: None,
            content_effects: Vec::new(),
            color_tag: None,
            height_mask: HeightMask::default(),
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self.kind, LayerKind::Group { .. })
    }

    pub fn children(&self) -> &[LayerId] {
        match &self.kind {
            LayerKind::Group { children, .. } => children,
            _ => &[],
        }
    }

    pub fn blend(&self, c: Channel) -> Option<ChannelBlend> {
        self.channels.get(&c).copied()
    }

    /// The layer this one instances, if it is an instance.
    pub fn instance_source(&self) -> Option<LayerId> {
        match self.kind {
            LayerKind::Instance { source } => Some(source),
            _ => None,
        }
    }
}

/// Where a texture or procedural is sampled from on the surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    /// Texture coordinates of the mesh.
    Uv { scale: [f32; 2], rotation: f32, offset: [f32; 2] },
    /// World-space box projection blended by the normal.
    Triplanar { scale: f32, sharpness: f32, offset: [f32; 3] },
    /// World-space planar projection along an axis (0 = X, 1 = Y, 2 = Z).
    Planar { scale: f32, axis: u8, offset: [f32; 3] },
    /// Spherical projection around the mesh center.
    Spherical { scale: f32 },
    /// Cylindrical projection around an axis.
    Cylindrical { scale: f32, axis: u8 },
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Uv { scale: [1.0, 1.0], rotation: 0.0, offset: [0.0, 0.0] }
    }
}

impl Projection {
    pub fn label(&self) -> &'static str {
        match self {
            Projection::Uv { .. } => "UV projection",
            Projection::Triplanar { .. } => "Tri-planar projection",
            Projection::Planar { .. } => "Planar projection",
            Projection::Spherical { .. } => "Spherical projection",
            Projection::Cylindrical { .. } => "Cylindrical projection",
        }
    }
}

/// Procedural patterns available to fills, generators and masks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProceduralKind {
    /// Smooth fractal value noise.
    Clouds,
    /// Gradient (Perlin-style) noise.
    Perlin,
    /// Fine high-frequency noise.
    Noise,
    /// Worley / cellular noise.
    Cells,
    /// Blotchy grunge: thresholded multi-scale noise.
    Grunge,
    /// Thin random scratches.
    Scratches,
    /// Checkerboard.
    Checker,
    /// Stripes.
    Stripes,
    /// Linear gradient along U (or the projection axis).
    Gradient,
    /// Wood grain: growth rings with fibers along X.
    Wood,
    /// Running-bond bricks with mortar joints (0 = mortar).
    Bricks,
    /// Square tiles with grout joints (0 = grout).
    Tiles,
    /// Plain fabric weave.
    Weave,
    /// Leather grain: pebbled cells with creases.
    Leather,
    /// Hammered metal dimples.
    Hammered,
    /// Fine brushed-metal lines along X.
    Brushed,
    /// Marble veins.
    Marble,
    /// Flat random tone per Voronoi cell (crystals, spangle, stones).
    Voronoi,
    /// Vertical streaks and leaks (along the projection's up axis).
    Streaks,
    /// Fingerprints and smudges.
    Smudges,
    /// Small round pores and pits (0 = pore).
    Pores,
    /// Sharp-edged paint chips.
    Chips,
}

impl ProceduralKind {
    pub const ALL: [ProceduralKind; 22] = [
        ProceduralKind::Clouds,
        ProceduralKind::Perlin,
        ProceduralKind::Noise,
        ProceduralKind::Cells,
        ProceduralKind::Grunge,
        ProceduralKind::Scratches,
        ProceduralKind::Checker,
        ProceduralKind::Stripes,
        ProceduralKind::Gradient,
        ProceduralKind::Wood,
        ProceduralKind::Bricks,
        ProceduralKind::Tiles,
        ProceduralKind::Weave,
        ProceduralKind::Leather,
        ProceduralKind::Hammered,
        ProceduralKind::Brushed,
        ProceduralKind::Marble,
        ProceduralKind::Voronoi,
        ProceduralKind::Streaks,
        ProceduralKind::Smudges,
        ProceduralKind::Pores,
        ProceduralKind::Chips,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ProceduralKind::Clouds => "Clouds",
            ProceduralKind::Perlin => "Perlin noise",
            ProceduralKind::Noise => "Fine noise",
            ProceduralKind::Cells => "Cells",
            ProceduralKind::Grunge => "Grunge",
            ProceduralKind::Scratches => "Scratches",
            ProceduralKind::Checker => "Checker",
            ProceduralKind::Stripes => "Stripes",
            ProceduralKind::Gradient => "Gradient",
            ProceduralKind::Wood => "Wood grain",
            ProceduralKind::Bricks => "Bricks",
            ProceduralKind::Tiles => "Tiles",
            ProceduralKind::Weave => "Fabric weave",
            ProceduralKind::Leather => "Leather grain",
            ProceduralKind::Hammered => "Hammered",
            ProceduralKind::Brushed => "Brushed lines",
            ProceduralKind::Marble => "Marble veins",
            ProceduralKind::Voronoi => "Voronoi",
            ProceduralKind::Streaks => "Streaks",
            ProceduralKind::Smudges => "Fingerprints",
            ProceduralKind::Pores => "Pores",
            ProceduralKind::Chips => "Paint chips",
        }
    }
    pub fn parse(s: &str) -> Option<ProceduralKind> {
        let norm = |x: &str| x.to_ascii_lowercase().replace([' ', '_', '-'], "");
        ProceduralKind::ALL.into_iter().find(|k| norm(k.label()) == norm(s) || norm(&format!("{k:?}")) == norm(s))
    }
}

/// A procedural grayscale pattern. Its output maps 0..1 between `low` and `high` values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Procedural {
    pub kind: ProceduralKind,
    /// Pattern frequency (repeats per projection unit).
    pub scale: f32,
    pub seed: u32,
    /// Contrast around 0.5 (1 = unchanged).
    pub contrast: f32,
    /// Shifts the pattern brighter (+) or darker (−).
    pub balance: f32,
    pub invert: bool,
}

impl Procedural {
    pub fn new(kind: ProceduralKind) -> Procedural {
        Procedural { kind, scale: 4.0, seed: 1, contrast: 1.0, balance: 0.0, invert: false }
    }
}

/// What a fill writes into one channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Source {
    /// A constant value (linear RGB for colors; `[v, v, v]` for scalars).
    Uniform([f32; 3]),
    /// A procedural pattern mixing two values.
    Procedural { pattern: Procedural, low: [f32; 3], high: [f32; 3] },
    /// An imported image resource, by name.
    Image { name: String },
    /// The stack value recorded by an anchor point effect (`anchor` is the effect id) for
    /// `channel`; colors read as gray where a scalar is needed.
    Anchor { anchor: LayerId, channel: Channel },
}

impl Source {
    pub fn uniform_of(c: Channel) -> Source {
        Source::Uniform(c.default_value())
    }

    /// The anchor point this source reads, if any.
    pub fn anchor(&self) -> Option<LayerId> {
        match self {
            Source::Anchor { anchor, .. } => Some(*anchor),
            _ => None,
        }
    }
}

/// A fill layer's material: one source per channel, plus how it is projected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Fill {
    pub sources: BTreeMap<Channel, Source>,
    pub projection: Projection,
}

impl Fill {
    /// Value used for `c` when the fill has no source for it: the channel default.
    pub fn source(&self, c: Channel) -> Source {
        self.sources.get(&c).cloned().unwrap_or_else(|| Source::uniform_of(c))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MaskBackground {
    #[default]
    White,
    Black,
}

/// A layer mask: a background color with a stack of effects on top.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub enabled: bool,
    pub background: MaskBackground,
    /// Effects bottom → top. Paint effects hold bitmap data keyed by their id.
    pub effects: Vec<Effect>,
}

impl Mask {
    pub fn new(background: MaskBackground) -> Mask {
        Mask { enabled: true, background, effects: Vec::new() }
    }
}

/// Levels remap: input range, gamma, output range.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    pub in_low: f32,
    pub in_high: f32,
    pub gamma: f32,
    pub out_low: f32,
    pub out_high: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Levels { in_low: 0.0, in_high: 1.0, gamma: 1.0, out_low: 0.0, out_high: 1.0 }
    }
}

impl Levels {
    pub fn apply(&self, x: f32) -> f32 {
        let t = ((x - self.in_low) / (self.in_high - self.in_low).max(1e-5)).clamp(0.0, 1.0);
        let t = t.powf(1.0 / self.gamma.max(1e-3));
        self.out_low + (self.out_high - self.out_low) * t
    }
}

/// Mesh-map driven mask generators (the "smart mask" building blocks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeneratorKind {
    /// Convex edges from the curvature map.
    EdgeWear,
    /// Crevices from ambient occlusion.
    Dirt,
    /// Thin parts from the thickness map.
    ThinAreas,
    /// Raw curvature.
    Curvature,
    /// Raw ambient occlusion.
    AmbientOcclusion,
    /// Gradient along a world axis (bottom → top by default).
    Position,
    /// Surfaces facing a direction (e.g. up for dust).
    Direction,
}

impl GeneratorKind {
    pub const ALL: [GeneratorKind; 7] = [
        GeneratorKind::EdgeWear,
        GeneratorKind::Dirt,
        GeneratorKind::ThinAreas,
        GeneratorKind::Curvature,
        GeneratorKind::AmbientOcclusion,
        GeneratorKind::Position,
        GeneratorKind::Direction,
    ];
    pub fn label(self) -> &'static str {
        match self {
            GeneratorKind::EdgeWear => "Mask Editor: Edge wear",
            GeneratorKind::Dirt => "Dirt",
            GeneratorKind::ThinAreas => "Thin areas",
            GeneratorKind::Curvature => "Curvature",
            GeneratorKind::AmbientOcclusion => "Ambient occlusion",
            GeneratorKind::Position => "Position gradient",
            GeneratorKind::Direction => "Light / direction",
        }
    }
    pub fn parse(s: &str) -> Option<GeneratorKind> {
        let norm = |x: &str| x.to_ascii_lowercase().replace([' ', '_', '-', ':'], "");
        GeneratorKind::ALL.into_iter().find(|k| norm(&format!("{k:?}")) == norm(s) || norm(k.label()) == norm(s))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Generator {
    pub kind: GeneratorKind,
    /// Where the mask starts (0..1, generator specific threshold).
    pub level: f32,
    /// Edge softness (0 = hard).
    pub softness: f32,
    /// Breaks the result up with world-space noise (0 = none).
    pub noise_amount: f32,
    pub noise_scale: f32,
    pub seed: u32,
    /// Direction for `Position` / `Direction` generators (world space).
    pub direction: [f32; 3],
    pub invert: bool,
}

impl Generator {
    pub fn new(kind: GeneratorKind) -> Generator {
        let (level, softness, noise_amount) = match kind {
            GeneratorKind::EdgeWear => (0.6, 0.12, 0.5),
            GeneratorKind::Dirt => (0.55, 0.25, 0.35),
            GeneratorKind::ThinAreas => (0.3, 0.2, 0.0),
            _ => (0.5, 0.25, 0.0),
        };
        Generator { kind, level, softness, noise_amount, noise_scale: 8.0, seed: 7, direction: [0.0, 1.0, 0.0], invert: false }
    }
}

/// Filters usable on masks and content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    Blur {
        radius: f32,
    },
    Sharpen {
        amount: f32,
    },
    Invert,
    Threshold {
        level: f32,
    },
    /// Hue/saturation/lightness shift (color channels; lightness only on grays and masks).
    /// `hue` is in turns, `saturation` and `lightness` in −1..1; all zero = unchanged.
    Hsl {
        hue: f32,
        saturation: f32,
        lightness: f32,
    },
}

/// Push `x` (0..1) towards 1 (`amount` > 0) or 0 (`amount` < 0); `amount` in −1..1.
fn shift_unit(x: f32, amount: f32) -> f32 {
    let a = amount.clamp(-1.0, 1.0);
    if a >= 0.0 { x + (1.0 - x) * a } else { x * (1.0 + a) }
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// HSL adjustment of a 0..1 color (see [`Filter::Hsl`]). A gray stays gray and only its
/// lightness changes.
pub fn hsl_adjust(rgb: [f32; 3], hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    let [r, g, b] = rgb.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let (d, l) = (mx - mn, (mx + mn) * 0.5);
    let (h, s) = if d <= 1e-6 {
        (0.0, 0.0)
    } else {
        let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
        let h = if mx == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h / 6.0, s)
    };
    let h = h + if hue.is_finite() { hue } else { 0.0 };
    let (s, l) = (if d <= 1e-6 { 0.0 } else { shift_unit(s, saturation) }, shift_unit(l, lightness));
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    [hue_to_rgb(p, q, h + 1.0 / 3.0), hue_to_rgb(p, q, h), hue_to_rgb(p, q, h - 1.0 / 3.0)]
}

impl Filter {
    pub fn label(&self) -> &'static str {
        match self {
            Filter::Blur { .. } => "Blur",
            Filter::Sharpen { .. } => "Sharpen",
            Filter::Invert => "Invert",
            Filter::Threshold { .. } => "Threshold",
            Filter::Hsl { .. } => "HSL Perceptive",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EffectKind {
    /// Painted grayscale (mask) or material (content) pixels keyed by the effect id.
    Paint,
    /// A uniform or procedural grayscale fill.
    Fill {
        source: Source,
        projection: Projection,
    },
    Generator(Generator),
    Levels(Levels),
    Filter(Filter),
    /// Select ID-map colors within a tolerance.
    ColorSelection {
        colors: Vec<[f32; 3]>,
        tolerance: f32,
    },
    /// Compare the mask with a constant.
    Compare {
        op: CompareOp,
        value: f32,
        tolerance: f32,
    },
    /// Content only: a material fill (one source per channel; channels without a source are
    /// left as they are).
    Material(Fill),
    /// Content only: records the composited stack right after its layer (that layer and
    /// everything below) so masks and fills above can read it ([`Source::Anchor`]).
    Anchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareOp {
    LesserThan,
    WithinTolerance,
    GreaterThan,
}

impl EffectKind {
    pub fn name(&self) -> &'static str {
        match self {
            EffectKind::Paint => "Paint",
            EffectKind::Fill { .. } => "Fill",
            EffectKind::Generator(_) => "Generator",
            EffectKind::Levels(_) => "Levels",
            EffectKind::Filter(f) => f.label(),
            EffectKind::ColorSelection { .. } => "Color selection",
            EffectKind::Compare { .. } => "Compare mask",
            EffectKind::Material(_) => "Fill",
            EffectKind::Anchor => "Anchor point",
        }
    }

    /// Whether the effect belongs in a layer's content stack (`true`), its mask (`false`), or
    /// both (`None`).
    pub fn content_only(&self) -> Option<bool> {
        match self {
            EffectKind::Material(_) | EffectKind::Anchor => Some(true),
            EffectKind::Fill { .. } | EffectKind::Generator(_) | EffectKind::ColorSelection { .. } | EffectKind::Compare { .. } => Some(false),
            EffectKind::Paint | EffectKind::Levels(_) | EffectKind::Filter(_) => None,
        }
    }

    /// Anchor points read by this effect's sources.
    fn reads_anchor(&self) -> bool {
        match self {
            EffectKind::Fill { source, .. } => source.anchor().is_some(),
            EffectKind::Material(f) => f.sources.values().any(|s| s.anchor().is_some()),
            _ => false,
        }
    }
}

impl Layer {
    /// Whether any source of the layer (fill, mask or content effects) reads an anchor point.
    pub fn reads_anchor(&self) -> bool {
        let fill = match &self.kind {
            LayerKind::Fill(f) => f.sources.values().any(|s| s.anchor().is_some()),
            _ => false,
        };
        fill || self.mask.iter().flat_map(|m| m.effects.iter()).chain(self.content_effects.iter()).any(|e| e.kind.reads_anchor())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub kind: EffectKind,
    pub blend: BlendMode,
    pub opacity: f32,
}

impl Effect {
    pub fn new(id: LayerId, kind: EffectKind) -> Effect {
        // Generators and fills that build a mask usually add on top of the background.
        Effect { id, name: kind.name().to_string(), visible: true, kind, blend: BlendMode::Normal, opacity: 1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_range_and_gamma() {
        let l = Levels { in_low: 0.2, in_high: 0.6, gamma: 1.0, out_low: 0.0, out_high: 1.0 };
        assert!((l.apply(0.4) - 0.5).abs() < 1e-6);
        assert_eq!(l.apply(0.0), 0.0);
        assert_eq!(l.apply(1.0), 1.0);
        let g = Levels { gamma: 2.0, ..Levels::default() };
        assert!((g.apply(0.25) - 0.5).abs() < 1e-6);
        // Degenerate ranges never divide by zero.
        let d = Levels { in_low: 0.5, in_high: 0.5, ..Levels::default() };
        assert!(d.apply(0.7).is_finite());
    }

    #[test]
    fn hsl_adjust_shifts_hue_saturation_and_lightness() {
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4);
        let c = [0.8, 0.3, 0.1];
        assert!(close(hsl_adjust(c, 0.0, 0.0, 0.0), c), "identity");
        assert!(close(hsl_adjust([1.0, 0.0, 0.0], 1.0 / 3.0, 0.0, 0.0), [0.0, 1.0, 0.0]), "red → green");
        assert!(close(hsl_adjust([1.0, 0.0, 0.0], -1.0 / 3.0, 0.0, 0.0), [0.0, 0.0, 1.0]), "red → blue");
        let gray = hsl_adjust(c, 0.0, -1.0, 0.0);
        assert!((gray[0] - gray[1]).abs() < 1e-6 && (gray[1] - gray[2]).abs() < 1e-6);
        // Grays only change lightness, whatever the hue and saturation.
        assert!(close(hsl_adjust([0.4; 3], 0.3, 1.0, 0.5), [0.7; 3]));
        assert!(close(hsl_adjust([0.4; 3], 0.0, 0.0, -0.5), [0.2; 3]));
        assert!(hsl_adjust([f32::NAN, 2.0, -1.0], f32::NAN, 5.0, -5.0).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn effect_targets_and_anchor_reads() {
        assert_eq!(EffectKind::Anchor.content_only(), Some(true));
        assert_eq!(EffectKind::Generator(Generator::new(GeneratorKind::Dirt)).content_only(), Some(false));
        assert_eq!(EffectKind::Filter(Filter::Invert).content_only(), None);
        let mut l = Layer::new(LayerId(1), "l", LayerKind::Paint, &[Channel::BaseColor]);
        assert!(!l.reads_anchor());
        let source = Source::Anchor { anchor: LayerId(9), channel: Channel::Height };
        l.mask = Some(Mask { effects: vec![Effect::new(LayerId(2), EffectKind::Fill { source, projection: Projection::default() })], ..Mask::new(MaskBackground::Black) });
        assert!(l.reads_anchor());
    }

    #[test]
    fn height_mask_keeps_the_ends_and_follows_height() {
        let off = HeightMask::default();
        assert_eq!(off.apply(0.3, 1.0), 0.3, "disabled");
        let hm = HeightMask { enabled: true, contrast: 1.0, offset: 0.0 };
        for h in [-1.0, 0.0, 1.0] {
            assert_eq!(hm.apply(0.0, h), 0.0);
            assert_eq!(hm.apply(1.0, h), 1.0);
        }
        assert_eq!(hm.apply(0.5, 0.0), 0.5, "no height, no change");
        assert_eq!(hm.apply(0.5, 0.5), 1.0);
        assert_eq!(hm.apply(0.5, -0.5), 0.0);
        assert!(hm.apply(0.5, 0.1) > 0.5 && hm.apply(0.5, -0.1) < 0.5);
        assert!(HeightMask { offset: 0.5, ..hm }.apply(0.5, 0.0) > 0.5);
        assert_eq!(hm.apply(0.5, f32::NAN), 0.5);
    }

    #[test]
    fn generator_kinds_parse() {
        for k in GeneratorKind::ALL {
            assert_eq!(GeneratorKind::parse(&format!("{k:?}")), Some(k));
        }
        for k in ProceduralKind::ALL {
            assert_eq!(ProceduralKind::parse(k.label()), Some(k));
        }
    }
}
