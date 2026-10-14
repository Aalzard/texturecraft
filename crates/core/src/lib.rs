//! # texturecraft-core
//!
//! The TextureCraft document model: a [`Project`] holds texture sets (one per mesh material),
//! each with a stack of [`Layer`]s (paint, fill, group) whose channels blend with one of
//! Painter's [`BlendMode`]s. Pure data with serde; pixels live in `texturecraft-texel` images
//! owned by the engine. [`blend`] is the CPU reference for every blend mode.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod blend;
pub mod channel;
pub mod layer;
pub mod project;

pub use blend::{BlendMode, blend1, blend3, lum, mix3};
pub use channel::{Channel, ChannelFormat, ChannelKind};
pub use layer::{
    ChannelBlend, CompareOp, Effect, EffectKind, Fill, Filter, Generator, GeneratorKind, HeightMask, Layer, LayerId, LayerKind, Levels, Mask, MaskBackground, Procedural,
    ProceduralKind, Projection, Source, hsl_adjust,
};
pub use project::{AoMixing, BakeMatch, BakeSettings, BakedMap, DisplaySettings, FORMAT_VERSION, NormalFormat, NormalMixing, Project, TextureSet, ToneMapping};

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CoreError {
    #[error("no layer with id {}", .0.0)]
    NoLayer(LayerId),
    #[error("no texture set {0}")]
    NoTextureSet(usize),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Product name for user-facing text.
pub const APP_NAME: &str = "TextureCraft";
/// Native project file extension.
pub const PROJECT_EXT: &str = "tcraft";
