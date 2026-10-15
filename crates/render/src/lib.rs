//! # texturecraft-render
//!
//! The wgpu viewport renderer: metal/roughness or specular/glossiness PBR with image-based
//! lighting (procedural environments or imported HDR images, prefiltered on the CPU in [`ibl`]),
//! height-to-normal, optional parallax occlusion, 4× MSAA plus idle temporal accumulation, channel / mesh-map / UV
//! checker debug views, the 2D UV view, a linear HDR scene with a post pass (exposure, Linear /
//! ACES tone mapping, bloom, sharpen, vignette, grain), key-light shadow maps, a 3D wireframe
//! overlay, and a [`headless`] device for renders
//! without a window. No UI toolkit dependency; the egui shell drives it through
//! [`Renderer::prepare`] and [`Renderer::blit`]. Fragment passes only, so it runs on WebGPU and
//! WebGL2 too.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod mesh_maps;
#[cfg(not(target_arch = "wasm32"))]
pub mod headless;
pub mod ibl;
pub mod renderer;
pub mod trace;

pub use renderer::{COLOR_FORMAT, Frame, MSAA, Renderer, Shade, ViewKind, aux_size, uv_view_scale};
pub use wgpu;
