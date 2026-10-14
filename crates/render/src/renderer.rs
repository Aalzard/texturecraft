//! The wgpu viewport renderer. Independent of any UI toolkit: the shell calls [`Renderer::prepare`]
//! with an encoder (offscreen render into per-view targets) and [`Renderer::blit`] inside its
//! own render pass.
//!
//! Per view and frame: the scene pass (MSAA, linear HDR premultiplied by scene coverage, resolved)
//! → the bloom pyramid (only when bloom is on) → the post pass (flat studio backdrop, exposure,
//! tone mapping, effects, sRGB) into the view's RGBA8 target that [`Renderer::blit`] and
//! [`Renderer::read_view`] use. The key-light shadow map is re-rendered only when the mesh, the
//! light direction or the visible texture sets change.
//!
//! Temporal anti-aliasing (idle accumulation): while a 3D view's frame and the scene content stay
//! the same, each new frame renders with a sub-pixel jitter and is averaged into the view's
//! accumulation target, up to [`TAA_SAMPLES`] frames (4× MSAA each); then the view stops
//! rendering until something changes. [`Renderer::accumulating`] tells the shell to keep redrawing.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use texturecraft_compose::{DisplayKind, ImageRes};
use texturecraft_core::{DisplaySettings, ToneMapping};
use texturecraft_geom::glam::{Mat4, Vec3};
use texturecraft_geom::{Mesh, OrbitCamera};
use texturecraft_texel::TILE;

use crate::ibl;

pub const MSAA: u32 = 4;
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Scene (pre tone mapping) format where float targets are renderable.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Prefiltered environment levels (roughness 0..1).
pub const ENV_LEVELS: u32 = 6;
const SHADOW_SIZE: u32 = 2048;
/// Bloom pyramid levels (1/2 .. 1/32 resolution).
const BLOOM_LEVELS: u32 = 5;
/// Bright-pass threshold on the exposed scene value, and the glow gain at strength 1.
const BLOOM_THRESHOLD: f32 = 1.0;
const BLOOM_GAIN: f32 = 0.15;
/// Frames averaged by idle accumulation (temporal anti-aliasing).
pub const TAA_SAMPLES: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlobalsRaw {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    env: [f32; 4],
    sh: [[f32; 4]; 9],
    mode: [f32; 4],
    bg: [f32; 4],
    uv_xform: [f32; 4],
    light: [f32; 4],
    light_vp: [[f32; 4]; 4],
    shadow: [f32; 4],
    material: [f32; 4],
}

/// The post pass uniform (see `Post` in post.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostRaw {
    a: [f32; 4],
    b: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    tan: [f32; 4],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlitRaw {
    rect: [f32; 4],
    dst: [f32; 4],
}

/// How the material is shaded (from the project's display settings and the view mode).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shade {
    /// Exposure in EV, applied in the post pass (not to data views).
    pub exposure_ev: f32,
    /// ACES filmic tone mapping (else linear, clipped).
    pub aces: bool,
    pub env_rotation_deg: f32,
    /// 0..1 → background blur level.
    pub env_blur: f32,
    pub env_opacity: f32,
    pub ao_intensity: f32,
    pub height_strength: f32,
    /// -1 for DirectX normal maps in the normal channel, 1 for OpenGL.
    pub normal_y: f32,
    /// 0 material, 1 channel, 2 mesh map (aux texture), 3 UV checker.
    pub view_mode: u32,
    /// Channel index for view mode 1 (Painter channel order, see the shader).
    pub channel: u32,
    /// Direction to an optional key light and its intensity (0 = off). The direction is in
    /// environment space: it turns with `env_rotation_deg` like the lights painted into the
    /// environment, so rotating the environment sweeps the key light and its shadows too.
    pub light: [f32; 4],
    /// Draw the UV wireframe in the 2D view.
    pub wireframe: bool,
    /// Key-light shadow map (needs `light[3] > 0`; 3D material view only). Shadows dim the key
    /// light, not the image-based lighting.
    pub shadows: bool,
    /// Master switch of the effects below (Painter's "Post effects"); off costs no extra pass.
    /// Effects never apply to data views (channels, mesh maps, UV checker, 2D view).
    pub post_effects: bool,
    /// Effect strengths, 0..1.
    pub bloom: f32,
    pub vignette: f32,
    pub sharpen: f32,
    pub grain: f32,
    /// Draw the mesh edges over the shaded 3D view (depth tested).
    pub wireframe_3d: bool,
    /// Specular/glossiness shading: base color is the diffuse color and the Specular channel the
    /// F0 color (roughness arrives as 1 - glossiness from the display conversion).
    pub spec_gloss: bool,
    /// Parallax occlusion mapping from the height channel (3D view).
    pub parallax: bool,
    /// Idle accumulation of jittered frames in the 3D view.
    pub taa: bool,
}

impl Shade {
    /// The project-level shading (display and shader settings); the caller adds the view mode,
    /// channel, height strength and wireframe toggles.
    pub fn from_display(d: &DisplaySettings) -> Shade {
        Shade {
            exposure_ev: d.env_exposure,
            aces: d.tone_mapping == ToneMapping::AcesFilmic,
            env_rotation_deg: d.env_rotation,
            env_blur: d.env_blur,
            env_opacity: d.env_opacity,
            ao_intensity: d.ao_intensity,
            // The key light (on with shadows), in environment space.
            light: if d.shadows { [0.5, 0.8, 0.4, 2.5] } else { [0.0; 4] },
            shadows: d.shadows,
            post_effects: d.post_effects,
            bloom: d.bloom,
            vignette: d.vignette,
            sharpen: d.sharpen,
            grain: d.grain,
            spec_gloss: d.spec_gloss,
            parallax: d.parallax,
            taa: d.taa,
            ..Shade::default()
        }
    }
}

impl Default for Shade {
    fn default() -> Self {
        Shade {
            exposure_ev: 0.0,
            aces: true,
            env_rotation_deg: 0.0,
            env_blur: 0.35,
            env_opacity: 0.0,
            ao_intensity: 0.75,
            height_strength: 1.0,
            normal_y: 1.0,
            view_mode: 0,
            channel: 0,
            light: [0.0; 4],
            wireframe: false,
            shadows: false,
            post_effects: false,
            bloom: 0.0,
            vignette: 0.0,
            sharpen: 0.0,
            grain: 0.0,
            wireframe_3d: false,
            spec_gloss: false,
            parallax: false,
            taa: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewKind {
    /// The 3D viewport.
    Scene,
    /// The 2D UV view of texture set `ts`: zoom (1 = fit) and pan (clip-space offset).
    Uv { ts: usize, zoom: f32, pan: [f32; 2] },
}

/// One view to render this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub view_id: u64,
    /// Target size in physical pixels.
    pub size: [u32; 2],
    pub kind: ViewKind,
    pub camera: OrbitCamera,
    pub shade: Shade,
    /// Visibility per texture set.
    pub visible: Vec<bool>,
}

struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    mips: u32,
}

fn mip_count(size: u32) -> u32 {
    32 - size.max(1).leading_zeros()
}

impl Tex {
    fn new(device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, mips: u32) -> Tex {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Tex { texture, view, format, mips }
    }
}

struct SetGpu {
    res: u32,
    /// The five display textures in [`DisplayKind::ALL`] order, then the aux (mesh map) texture.
    tex: Vec<Tex>,
    bind_group: wgpu::BindGroup,
    mips_dirty: Vec<bool>,
}

struct MeshGpu {
    id: u64,
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    /// (first index, count, material)
    draws: Vec<(u32, u32, usize)>,
    /// Unique UV edges as a line list, per material: (first index, count, material).
    lines: wgpu::Buffer,
    line_draws: Vec<(u32, u32, usize)>,
    /// Bounding sphere (center, radius) for fitting the shadow map.
    center: Vec3,
    radius: f32,
}

/// The bloom pyramid of a view, created the first time bloom is used.
struct Bloom {
    /// 1/2, 1/4, ... resolution targets.
    levels: Vec<wgpu::TextureView>,
    /// Per level: reads the level above (the HDR scene for level 0).
    down: Vec<wgpu::BindGroup>,
    /// Per level but the last: reads the level below, added onto this one.
    up: Vec<wgpu::BindGroup>,
}

struct ViewTargets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// The resolved HDR scene of a jittered accumulation frame.
    hdr: wgpu::TextureView,
    /// The HDR scene the post pass reads: resolved directly, or the running average of jittered
    /// frames (see [`TAA_SAMPLES`]).
    accum: wgpu::TextureView,
    /// Reads `hdr` for the accumulation blend.
    accum_bg: wgpu::BindGroup,
    /// What `accum` holds: the frame and content revision, and the sample count.
    history: Option<(Frame, u64)>,
    samples: u32,
    /// The display image (RGBA8, sRGB-encoded): post pass output, blit and read-back source.
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    globals: wgpu::Buffer,
    globals_bg: Option<wgpu::BindGroup>,
    post_buf: wgpu::Buffer,
    post_bg: wgpu::BindGroup,
    bloom: Option<Bloom>,
    blit_buf: wgpu::Buffer,
    blit_bg: wgpu::BindGroup,
    /// Set by [`Renderer::prepare`]: the view has content to blit.
    ready: bool,
}

/// Key-light shadow map, shared by all views (1×1 until shadows are first used).
struct ShadowMap {
    view: wgpu::TextureView,
    size: u32,
    /// What the map was rendered for: mesh id, light direction bits, visible sets.
    key: Option<(u64, [u32; 3])>,
    visible: Vec<bool>,
    vp: Mat4,
    buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
}

pub struct Renderer {
    target_format: wgpu::TextureFormat,
    /// [`HDR_FORMAT`], or [`COLOR_FORMAT`] where float targets are not renderable (WebGL2
    /// without float color buffers).
    hdr_format: wgpu::TextureFormat,
    globals_bgl: wgpu::BindGroupLayout,
    set_bgl: wgpu::BindGroupLayout,
    blit_bgl: wgpu::BindGroupLayout,
    mip_bgl: wgpu::BindGroupLayout,
    fx_bgl: wgpu::BindGroupLayout,
    mesh_pipeline: wgpu::RenderPipeline,
    uv_pipeline: wgpu::RenderPipeline,
    wire_pipeline: wgpu::RenderPipeline,
    wire3d_pipeline: wgpu::RenderPipeline,
    bg_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    post_pipeline: wgpu::RenderPipeline,
    /// Bright pass + first downsample, downsample, additive upsample.
    bloom_pipelines: [wgpu::RenderPipeline; 3],
    shadow_pipeline: wgpu::RenderPipeline,
    mip_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    util_module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    env_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    shadow: ShadowMap,
    /// 1×1 black texture for unused effect inputs.
    black: wgpu::TextureView,
    accum_pipeline: wgpu::RenderPipeline,
    env: Tex,
    env_name: String,
    sh: [[f32; 4]; 9],
    /// Bumped by every change of what the views show (uploads, mesh, environment): restarts
    /// accumulation.
    content_rev: u64,
    mesh: Option<MeshGpu>,
    sets: Vec<SetGpu>,
    /// Texture set index per mesh material.
    material_sets: Vec<Option<usize>>,
    views: HashMap<u64, ViewTargets>,
}

fn fragment_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None }
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    fragment_entry(
        binding,
        wgpu::BindingType::Texture {
            multisampled: false,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
        },
    )
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    fragment_entry(binding, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering))
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

fn display_format(k: DisplayKind) -> wgpu::TextureFormat {
    match k {
        DisplayKind::BaseColor | DisplayKind::Emissive => wgpu::TextureFormat::Rgba8UnormSrgb,
        DisplayKind::Orm | DisplayKind::Normal => wgpu::TextureFormat::Rgba8Unorm,
        DisplayKind::Height => wgpu::TextureFormat::Rgba16Float,
    }
}

/// Default (neutral) texel of a display texture before any upload.
fn default_texel(k: DisplayKind) -> Vec<u8> {
    match k {
        DisplayKind::BaseColor => vec![188, 188, 188, 255],
        DisplayKind::Orm => vec![255, 128, 0, 128],
        DisplayKind::Normal => vec![128, 128, 255, 255],
        DisplayKind::Height => [0.0, 0.04, 0.04, 0.04].iter().flat_map(|v| half::f16::from_f32(*v).to_le_bytes()).collect(),
        DisplayKind::Emissive => vec![0, 0, 0, 255],
    }
}

/// The color output of a pipeline: fragment entry point, format, blending. `fs_wire*` entry
/// points draw line lists.
type ColorOut<'a> = (&'a str, wgpu::TextureFormat, Option<wgpu::BlendState>);

const ADD: wgpu::BlendComponent = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
const ADDITIVE: wgpu::BlendState = wgpu::BlendState { color: ADD, alpha: ADD };

#[allow(clippy::too_many_arguments)]
fn pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    vs: &str,
    buffers: &[Option<wgpu::VertexBufferLayout>],
    color: Option<ColorOut<'_>>,
    samples: u32,
    depth: Option<wgpu::DepthStencilState>,
) -> wgpu::RenderPipeline {
    let lines = color.is_some_and(|(fs, ..)| fs.starts_with("fs_wire"));
    let topology = if lines { wgpu::PrimitiveTopology::LineList } else { wgpu::PrimitiveTopology::TriangleList };
    let targets = color.map(|(_, format, blend)| [Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })]);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState { module, entry_point: Some(vs), buffers, compilation_options: Default::default() },
        primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
        depth_stencil: depth,
        multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
        fragment: color.zip(targets.as_ref()).map(|((fs, ..), targets)| wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            targets,
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Whether [`HDR_FORMAT`] works as an MSAA render target. Guaranteed on WebGPU and the native
/// backends; WebGL2/GLES need float color buffer extensions, so probe there (wgpu-core reports the
/// validation error synchronously, so the future is ready at once).
fn hdr_renderable(device: &wgpu::Device) -> bool {
    if device.adapter_info().backend != wgpu::Backend::Gl {
        return true;
    }
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _probe = texture(device, "tc_probe", [1, 1], HDR_FORMAT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let mut error = std::pin::pin!(scope.pop());
    matches!(error.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())), std::task::Poll::Ready(None))
}

fn texture(device: &wgpu::Device, label: &str, size: [u32; 2], format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn target_view(device: &wgpu::Device, label: &str, size: [u32; 2], format: wgpu::TextureFormat, samples: u32) -> wgpu::TextureView {
    let usage = if samples > 1 || format == DEPTH_FORMAT {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    } else {
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
    };
    texture(device, label, size, format, samples, usage).create_view(&Default::default())
}

fn shadow_view(device: &wgpu::Device, size: u32) -> wgpu::TextureView {
    texture(device, "tc_shadow", [size, size], DEPTH_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default())
}

/// One fullscreen-triangle pass into `target`.
fn fullscreen(encoder: &mut wgpu::CommandEncoder, label: &str, target: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>, pipe: &wgpu::RenderPipeline, bg: &wgpu::BindGroup) {
    fullscreen_blend(encoder, label, target, load, pipe, bg, 1.0);
}

/// [`fullscreen`] with a blend constant (for pipelines that blend by it).
fn fullscreen_blend(encoder: &mut wgpu::CommandEncoder, label: &str, target: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>, pipe: &wgpu::RenderPipeline, bg: &wgpu::BindGroup, constant: f64) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipe);
    pass.set_bind_group(0, bg, &[]);
    pass.set_blend_constant(wgpu::Color { r: constant, g: constant, b: constant, a: constant });
    pass.draw(0..3, 0..1);
}

/// Halton low-discrepancy value of `i` in `base` (0..1).
fn halton(mut i: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

const CLEAR: wgpu::LoadOp<wgpu::Color> = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);

impl Renderer {
    /// `target_format`: the format of the pass [`Renderer::blit`] draws into.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat) -> Renderer {
        let shader = |label: &str, source: &str| device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let module = shader("tc_mesh", include_str!("shaders/mesh.wgsl"));
        let util_module = shader("tc_util", include_str!("shaders/util.wgsl"));
        let post_module = shader("tc_post", concat!(include_str!("shaders/util.wgsl"), include_str!("shaders/post.wgsl")));
        let shadow_module = shader("tc_shadow", include_str!("shaders/shadow.wgsl"));
        let bgl = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries });
        let globals_bgl = bgl(
            "tc_globals",
            &[
                uniform_entry(0),
                tex_entry(1),
                sampler_entry(2),
                fragment_entry(3, wgpu::BindingType::Texture { multisampled: false, sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2 }),
                fragment_entry(4, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
            ],
        );
        let set_bgl = bgl("tc_set", &[tex_entry(0), tex_entry(1), tex_entry(2), tex_entry(3), tex_entry(4), tex_entry(5), sampler_entry(6)]);
        let blit_bgl = bgl("tc_blit", &[tex_entry(0), sampler_entry(1), uniform_entry(2)]);
        let mip_bgl = bgl("tc_mip", &[tex_entry(0), sampler_entry(1)]);
        let fx_bgl = bgl("tc_fx", &[tex_entry(0), sampler_entry(1), uniform_entry(3), tex_entry(4)]);
        let shadow_bgl = bgl("tc_shadow", &[uniform_entry(0)]);
        let pl = |label: &str, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: groups, immediate_size: 0 })
        };
        let layout = pl("tc_mesh", &[Some(&globals_bgl), Some(&set_bgl)]);
        let bg_layout = pl("tc_bg", &[Some(&globals_bgl)]);
        let blit_layout = pl("tc_blit", &[Some(&blit_bgl)]);
        let fx_layout = pl("tc_fx", &[Some(&fx_bgl)]);
        let shadow_layout = pl("tc_shadow", &[Some(&shadow_bgl)]);
        let vb = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x2],
        })];
        // shortcut: the RGBA8 fallback clips highlights above 1 and bands in the darks; encode
        // (e.g. RGBM) if WebGL2 without float color buffers turns out to matter.
        let hdr_format = if hdr_renderable(device) { HDR_FORMAT } else { COLOR_FORMAT };
        let hdr = |fs, blend| Some((fs, hdr_format, blend));
        let always = || Some(depth_state(false, wgpu::CompareFunction::Always));
        let mesh_pipeline = pipeline(device, "tc_mesh", &layout, &module, "vs_mesh", &vb, hdr("fs_mesh", None), MSAA, Some(depth_state(true, wgpu::CompareFunction::Less)));
        let uv_pipeline = pipeline(device, "tc_uv", &layout, &module, "vs_uv", &vb, hdr("fs_uv", None), MSAA, always());
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let wire_pipeline = pipeline(device, "tc_wire", &layout, &module, "vs_uv", &vb, hdr("fs_wire", blend), MSAA, always());
        let wire3d_depth = Some(depth_state(false, wgpu::CompareFunction::LessEqual));
        let wire3d_pipeline = pipeline(device, "tc_wire3d", &layout, &module, "vs_wire3d", &vb, hdr("fs_wire3d", blend), MSAA, wire3d_depth);
        let bg_pipeline = pipeline(device, "tc_bg", &bg_layout, &module, "vs_bg", &[], hdr("fs_bg", None), MSAA, always());
        let blit_pipeline = pipeline(device, "tc_blit", &blit_layout, &util_module, "vs_blit", &[], Some(("fs_blit", target_format, None)), 1, None);
        let post_pipeline = pipeline(device, "tc_post", &fx_layout, &post_module, "vs_full", &[], Some(("fs_post", COLOR_FORMAT, None)), 1, None);
        let bloom_pipelines = [("fs_bloom_pre", None), ("fs_bloom_down", None), ("fs_bloom_up", Some(ADDITIVE))]
            .map(|(fs, blend)| pipeline(device, "tc_bloom", &fx_layout, &post_module, "vs_full", &[], hdr(fs, blend), 1, None));
        let shadow_depth = wgpu::DepthStencilState {
            bias: wgpu::DepthBiasState { constant: 0, slope_scale: 1.5, clamp: 0.0 },
            ..depth_state(true, wgpu::CompareFunction::Less)
        };
        let shadow_pipeline = pipeline(device, "tc_shadow", &shadow_layout, &shadow_module, "vs_shadow", &vb, None, 1, Some(shadow_depth));
        // accum = hdr * k + accum * (1 - k) with k = 1 / (sample + 1): a running average.
        let average = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Constant, dst_factor: wgpu::BlendFactor::OneMinusConstant, operation: wgpu::BlendOperation::Add };
        let accum_layout = pl("tc_accum", &[Some(&mip_bgl)]);
        let accum_blend = Some(wgpu::BlendState { color: average, alpha: average });
        let accum_pipeline = pipeline(device, "tc_accum", &accum_layout, &util_module, "vs_full", &[], hdr("fs_mip", accum_blend), 1, None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tc_tex"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tc_env"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tc_shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tc_shadow"),
            size: std::mem::size_of::<[[f32; 4]; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow = ShadowMap {
            view: shadow_view(device, 1),
            size: 1,
            key: None,
            visible: Vec::new(),
            vp: Mat4::IDENTITY,
            bg: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tc_shadow"),
                layout: &shadow_bgl,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: shadow_buf.as_entire_binding() }],
            }),
            buf: shadow_buf,
        };
        let black = Tex::new(device, "tc_black", 1, 1, COLOR_FORMAT, 1).view;
        let env = Tex::new(device, "tc_env", 512, 256, wgpu::TextureFormat::Rgba16Float, ENV_LEVELS);
        let mut r = Renderer {
            target_format,
            hdr_format,
            globals_bgl,
            set_bgl,
            blit_bgl,
            mip_bgl,
            fx_bgl,
            mesh_pipeline,
            uv_pipeline,
            wire_pipeline,
            wire3d_pipeline,
            bg_pipeline,
            blit_pipeline,
            post_pipeline,
            bloom_pipelines,
            shadow_pipeline,
            accum_pipeline,
            mip_pipelines: HashMap::new(),
            util_module,
            sampler,
            env_sampler,
            shadow_sampler,
            shadow,
            black,
            env,
            env_name: String::new(),
            sh: [[0.0; 4]; 9],
            content_rev: 0,
            mesh: None,
            sets: Vec::new(),
            material_sets: Vec::new(),
            views: HashMap::new(),
        };
        r.set_environment(device, queue, "Studio Soft", None);
        r
    }

    pub fn environment_name(&self) -> &str {
        &self.env_name
    }

    /// Use environment `name`: with `image`, that equirect image (an imported `.hdr`/`.exr`, or
    /// any image, linearized), prefiltered on the CPU (rayon); otherwise the built-in of that
    /// name (no-op when already current; unknown names give the default studio).
    pub fn set_environment(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, name: &str, image: Option<&ImageRes>) {
        let img = match image.and_then(|i| ibl::EnvImage::from_rgba(i.width, i.height, &i.rgba, i.srgb)) {
            Some(img) => img,
            None if self.env_name == name => return,
            None => ibl::environment(name, 512),
        };
        let levels = ibl::prefilter(&img, ENV_LEVELS);
        if self.env.texture.width() != img.width || self.env.texture.height() != img.height {
            self.env = Tex::new(device, "tc_env", img.width, img.height, wgpu::TextureFormat::Rgba16Float, ENV_LEVELS);
        }
        for (l, lv) in levels.iter().enumerate() {
            let bytes: Vec<u8> = lv.data.iter().flat_map(|c| [c[0], c[1], c[2], 1.0]).flat_map(|v| half::f16::from_f32(v).to_le_bytes()).collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.env.texture, mip_level: l as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &bytes,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lv.width * 8), rows_per_image: Some(lv.height) },
                wgpu::Extent3d { width: lv.width, height: lv.height, depth_or_array_layers: 1 },
            );
        }
        let sh = ibl::sh9(&img);
        self.sh = sh.map(|c| [c[0], c[1], c[2], 0.0]);
        self.env_name = name.to_string();
        self.content_rev += 1;
        for v in self.views.values_mut() {
            v.globals_bg = None;
        }
    }

    /// Upload the mesh when `id` changed.
    pub fn set_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh, id: u64) {
        if self.mesh.as_ref().is_some_and(|m| m.id == id) {
            return;
        }
        let verts: Vec<Vertex> = (0..mesh.positions.len())
            .map(|i| Vertex {
                pos: mesh.positions[i].to_array(),
                nrm: mesh.normals.get(i).map(|n| n.to_array()).unwrap_or([0.0, 1.0, 0.0]),
                tan: mesh.tangents.get(i).map(|t| t.to_array()).unwrap_or([1.0, 0.0, 0.0, 1.0]),
                uv: mesh.uvs.get(i).map(|u| u.to_array()).unwrap_or([0.0, 0.0]),
            })
            .collect();
        let mk = |label: &str, data: &[u8], usage: wgpu::BufferUsages| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (data.len() as u64).max(4).next_multiple_of(4),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buf, 0, data);
            buf
        };
        let mut idx_bytes = bytemuck::cast_slice::<u32, u8>(&mesh.indices).to_vec();
        idx_bytes.resize(idx_bytes.len().next_multiple_of(4), 0);
        let vb = mk("tc_vb", bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX);
        let ib = mk("tc_ib", &idx_bytes, wgpu::BufferUsages::INDEX);
        let draws = mesh.submeshes.iter().map(|s| (s.start, s.count, s.material)).collect();
        // UV edges (each shared edge once) per material, for the 2D view wireframe.
        let mut line_idx: Vec<u32> = Vec::new();
        let mut line_draws = Vec::new();
        for m in 0..mesh.materials.len().max(1) {
            let start = line_idx.len() as u32;
            let mut seen = std::collections::HashSet::new();
            for t in mesh.triangles_of(m) {
                let Some(v) = mesh.tri(t) else { continue };
                for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
                    let key = (a.min(b), a.max(b));
                    if seen.insert(key) {
                        line_idx.extend_from_slice(&[key.0 as u32, key.1 as u32]);
                    }
                }
            }
            line_draws.push((start, line_idx.len() as u32 - start, m));
        }
        let lines = mk("tc_lines", bytemuck::cast_slice(&line_idx), wgpu::BufferUsages::INDEX);
        let (lo, hi) = mesh.bounds();
        let (center, radius) = ((lo + hi) * 0.5, ((hi - lo).length() * 0.5).max(1e-3));
        self.mesh = Some(MeshGpu { id, vb, ib, draws, lines, line_draws, center, radius });
        self.content_rev += 1;
    }

    /// Make the per-texture-set textures match `sets` (resolution per set) and map mesh
    /// materials to sets. Re-created textures start neutral; returns the sets that were reset
    /// (the caller must upload all their tiles).
    pub fn set_texture_sets(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, sets: &[(u32, usize)], material_count: usize) -> Vec<usize> {
        let mut reset = Vec::new();
        self.content_rev += 1;
        self.sets.truncate(sets.len());
        for (i, &(res, _)) in sets.iter().enumerate() {
            if self.sets.get(i).is_some_and(|s| s.res == res) {
                continue;
            }
            let mips = mip_count(res);
            let mut tex: Vec<Tex> = DisplayKind::ALL.iter().map(|k| Tex::new(device, "tc_set", res, res, display_format(*k), mips)).collect();
            tex.push(Tex::new(device, "tc_aux", res.min(2048), res.min(2048), wgpu::TextureFormat::Rgba8Unorm, mip_count(res.min(2048))));
            for (t, k) in tex.iter().zip(DisplayKind::ALL.iter().map(|k| Some(*k)).chain(std::iter::once(None))) {
                let texel = k.map(default_texel).unwrap_or_else(|| vec![128, 128, 128, 255]);
                let (w, h) = (t.texture.width(), t.texture.height());
                let row: Vec<u8> = texel.iter().copied().cycle().take(texel.len() * w as usize).collect();
                let data: Vec<u8> = row.iter().copied().cycle().take(row.len() * h as usize).collect();
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &t.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    &data,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * texel.len() as u32), rows_per_image: Some(h) },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            }
            let bind_group = self.set_bind_group(device, &tex);
            let n = tex.len();
            let gpu = SetGpu { res, tex, bind_group, mips_dirty: vec![true; n] };
            if i < self.sets.len() {
                self.sets[i] = gpu;
            } else {
                self.sets.push(gpu);
            }
            reset.push(i);
        }
        self.material_sets = vec![None; material_count];
        for (i, &(_, m)) in sets.iter().enumerate() {
            if let Some(slot) = self.material_sets.get_mut(m) {
                *slot = Some(i);
            }
        }
        reset
    }

    fn set_bind_group(&self, device: &wgpu::Device, tex: &[Tex]) -> wgpu::BindGroup {
        let mut entries: Vec<wgpu::BindGroupEntry> =
            tex.iter().enumerate().map(|(i, t)| wgpu::BindGroupEntry { binding: i as u32, resource: wgpu::BindingResource::TextureView(&t.view) }).collect();
        entries.push(wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.sampler) });
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("tc_set"), layout: &self.set_bgl, entries: &entries })
    }

    /// Upload one 64×64 tile of a display texture (bytes from `compose::tile_bytes`).
    pub fn upload_tile(&mut self, queue: &wgpu::Queue, ts: usize, kind: DisplayKind, tx: u32, ty: u32, bytes: &[u8]) {
        let Some(set) = self.sets.get_mut(ts) else { return };
        let Some(ki) = DisplayKind::ALL.iter().position(|k| *k == kind) else { return };
        let Some(t) = set.tex.get(ki) else { return };
        let bpt = kind.bytes_per_texel() as u32;
        let (x, y) = (tx * TILE, ty * TILE);
        if x >= set.res || y >= set.res || bytes.len() < (TILE * TILE * bpt) as usize {
            return;
        }
        let (w, h) = (TILE.min(set.res - x), TILE.min(set.res - y));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &t.texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(TILE * bpt), rows_per_image: Some(TILE) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.content_rev += 1;
        if let Some(d) = set.mips_dirty.get_mut(ki) {
            *d = true;
        }
    }

    /// Upload the aux (mesh map / debug) texture of a set: RGBA8, `size × size`.
    pub fn upload_aux(&mut self, queue: &wgpu::Queue, ts: usize, size: u32, rgba: &[u8]) {
        let Some(set) = self.sets.get_mut(ts) else { return };
        let Some(t) = set.tex.last() else { return };
        let (w, h) = (t.texture.width(), t.texture.height());
        if size != w || rgba.len() < (w * h * 4) as usize {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &t.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.content_rev += 1;
        if let Some(d) = set.mips_dirty.last_mut() {
            *d = true;
        }
    }

    /// Size of the aux texture of a set.
    pub fn aux_size(&self, ts: usize) -> Option<u32> {
        self.sets.get(ts).and_then(|s| s.tex.last()).map(|t| t.texture.width())
    }

    fn generate_mips(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        for si in 0..self.sets.len() {
            for ti in 0..self.sets[si].tex.len() {
                if !self.sets[si].mips_dirty.get(ti).copied().unwrap_or(false) {
                    continue;
                }
                let format = self.sets[si].tex[ti].format;
                if !self.mip_pipelines.contains_key(&format) {
                    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("tc_mip"),
                        bind_group_layouts: &[Some(&self.mip_bgl)],
                        immediate_size: 0,
                    });
                    let p = pipeline(device, "tc_mip", &layout, &self.util_module, "vs_full", &[], Some(("fs_mip", format, None)), 1, None);
                    self.mip_pipelines.insert(format, p);
                }
                let Some(pipe) = self.mip_pipelines.get(&format) else { continue };
                let t = &self.sets[si].tex[ti];
                for level in 1..t.mips {
                    let src = t.texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: level - 1, mip_level_count: Some(1), ..Default::default() });
                    let dst = t.texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: level, mip_level_count: Some(1), ..Default::default() });
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("tc_mip"),
                        layout: &self.mip_bgl,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        ],
                    });
                    fullscreen(encoder, "tc_mip", &dst, CLEAR, pipe, &bg);
                }
                if let Some(d) = self.sets[si].mips_dirty.get_mut(ti) {
                    *d = false;
                }
            }
        }
    }

    /// Effect bind group (`fx_bgl`): source texture, the post uniform, an extra texture.
    fn fx_bind_group(&self, device: &wgpu::Device, src: &wgpu::TextureView, aux: &wgpu::TextureView, post: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tc_fx"),
            layout: &self.fx_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: post.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(aux) },
            ],
        })
    }

    fn create_targets(&self, device: &wgpu::Device, size: [u32; 2]) -> ViewTargets {
        let hdr = target_view(device, "tc_hdr", size, self.hdr_format, 1);
        let accum = target_view(device, "tc_accum", size, self.hdr_format, 1);
        let accum_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tc_accum"),
            layout: &self.mip_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hdr) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
        let resolve = texture(device, "tc_resolve", size, COLOR_FORMAT, 1, usage);
        let resolve_view = resolve.create_view(&Default::default());
        let uniform = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let post_buf = uniform("tc_post", std::mem::size_of::<PostRaw>());
        let blit_buf = uniform("tc_blit", std::mem::size_of::<BlitRaw>());
        let blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tc_blit"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&resolve_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: blit_buf.as_entire_binding() },
            ],
        });
        ViewTargets {
            size,
            msaa: target_view(device, "tc_msaa", size, self.hdr_format, MSAA),
            depth: target_view(device, "tc_depth", size, DEPTH_FORMAT, MSAA),
            post_bg: self.fx_bind_group(device, &accum, &self.black, &post_buf),
            hdr,
            accum,
            accum_bg,
            history: None,
            samples: 0,
            resolve,
            resolve_view,
            globals: uniform("tc_globals", std::mem::size_of::<GlobalsRaw>()),
            globals_bg: None,
            post_buf,
            bloom: None,
            blit_buf,
            blit_bg,
            ready: false,
        }
    }

    /// The bloom pyramid of `v`, and the post bind group that reads it.
    fn create_bloom(&self, device: &wgpu::Device, v: &ViewTargets) -> (Bloom, wgpu::BindGroup) {
        let levels: Vec<wgpu::TextureView> =
            (1..=BLOOM_LEVELS).map(|i| target_view(device, "tc_bloom", [(v.size[0] >> i).max(1), (v.size[1] >> i).max(1)], self.hdr_format, 1)).collect();
        let fx = |src: &wgpu::TextureView| self.fx_bind_group(device, src, &self.black, &v.post_buf);
        let down = std::iter::once(&v.accum).chain(&levels).take(levels.len()).map(fx).collect();
        let up = levels.iter().skip(1).map(fx).collect();
        let post_bg = self.fx_bind_group(device, &v.accum, levels.first().unwrap_or(&self.black), &v.post_buf);
        (Bloom { levels, down, up }, post_bg)
    }

    fn targets(&mut self, device: &wgpu::Device, id: u64, size: [u32; 2], bloom: bool) -> Option<&mut ViewTargets> {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        if self.views.get(&id).is_none_or(|v| v.size != size) {
            let v = self.create_targets(device, size);
            self.views.insert(id, v);
        }
        if bloom
            && let Some(v) = self.views.get(&id)
            && v.bloom.is_none()
        {
            let (b, post_bg) = self.create_bloom(device, v);
            let v = self.views.get_mut(&id)?;
            v.bloom = Some(b);
            v.post_bg = post_bg;
        }
        let (env_view, shadow_view) = (&self.env.view, &self.shadow.view);
        let (bgl, env_sampler, shadow_sampler) = (&self.globals_bgl, &self.env_sampler, &self.shadow_sampler);
        let v = self.views.get_mut(&id)?;
        if v.globals_bg.is_none() {
            v.globals_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tc_globals"),
                layout: bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: v.globals.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(env_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(env_sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(shadow_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(shadow_sampler) },
                ],
            }));
        }
        Some(v)
    }

    /// Index ranges of `ranges` (first, count, material) whose texture set is visible (and is
    /// `only`, when given), with that set.
    fn drawable<'a>(&'a self, ranges: &'a [(u32, u32, usize)], visible: &'a [bool], only: Option<usize>) -> impl Iterator<Item = (std::ops::Range<u32>, &'a SetGpu)> + 'a {
        ranges.iter().filter_map(move |&(start, count, material)| {
            let ts = self.material_sets.get(material).copied().flatten()?;
            if only.is_some_and(|o| o != ts) || !visible.get(ts).copied().unwrap_or(true) {
                return None;
            }
            Some((start..start + count, self.sets.get(ts)?))
        })
    }

    /// Keep the key-light shadow map current; returns its view-projection. `light` points to
    /// the light (world space). Re-renders only when the mesh, light or visible sets changed.
    /// shortcut: casters are fully opaque (the dithered opacity cut-out is ignored) and the map is
    /// one shared buffer, so two 3D views with different lights in one frame would share the last
    /// light; upgrade to a per-view map if split 3D views get their own lights.
    fn update_shadow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, light: Vec3, visible: &[bool]) -> Option<Mat4> {
        let l = light.try_normalize()?;
        let (id, center, radius) = self.mesh.as_ref().map(|m| (m.id, m.center, m.radius))?;
        if self.shadow.size != SHADOW_SIZE {
            self.shadow.view = shadow_view(device, SHADOW_SIZE);
            self.shadow.size = SHADOW_SIZE;
            self.shadow.key = None;
            for v in self.views.values_mut() {
                v.globals_bg = None;
            }
        }
        let key = (id, l.to_array().map(f32::to_bits));
        if self.shadow.key == Some(key) && self.shadow.visible == visible {
            return Some(self.shadow.vp);
        }
        // An orthographic light frustum around the mesh's bounding sphere.
        let up = if l.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let vp = Mat4::orthographic_rh(-radius, radius, -radius, radius, radius * 0.5, radius * 3.5) * Mat4::look_at_rh(center + l * radius * 2.0, center, up);
        queue.write_buffer(&self.shadow.buf, 0, bytemuck::bytes_of(&vp.to_cols_array_2d()));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tc_shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow.view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, &self.shadow.bg, &[]);
        if let Some(m) = &self.mesh {
            pass.set_vertex_buffer(0, m.vb.slice(..));
            pass.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32);
            for (range, _) in self.drawable(&m.draws, visible, None) {
                pass.draw_indexed(range, 0, 0..1);
            }
        }
        drop(pass);
        self.shadow.key = Some(key);
        self.shadow.visible = visible.to_vec();
        self.shadow.vp = vp;
        Some(vp)
    }

    /// Whether view `id` is still accumulating anti-aliasing samples: the shell should draw it
    /// again even though nothing changed.
    pub fn accumulating(&self, id: u64) -> bool {
        self.views.get(&id).is_some_and(|v| v.history.as_ref().is_some_and(|(f, _)| f.shade.taa && f.kind == ViewKind::Scene) && v.samples + 1 < TAA_SAMPLES)
    }

    /// Render `frame` into its offscreen target (and regenerate dirty mips). A 3D frame with
    /// `shade.taa` that repeats the previous one adds a jittered sample to the average instead,
    /// and renders nothing once [`TAA_SAMPLES`] are in.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, frame: &Frame) {
        self.generate_mips(device, encoder);
        let rev = self.content_rev;
        let sample = match self.views.get(&frame.view_id) {
            Some(v) if frame.shade.taa && frame.kind == ViewKind::Scene && v.history.as_ref().is_some_and(|(f, r)| *r == rev && f == frame) => v.samples + 1,
            _ => 0,
        };
        if sample >= TAA_SAMPLES {
            return;
        }
        let aspect = frame.size[0].max(1) as f32 / frame.size[1].max(1) as f32;
        let mut vp = frame.camera.view_proj(aspect);
        if sample > 0 {
            // Sub-pixel jitter in clip space: a box filter over the pixel once averaged.
            let (jx, jy) = (halton(sample, 2) - 0.5, halton(sample, 3) - 0.5);
            vp = Mat4::from_translation(Vec3::new(2.0 * jx / frame.size[0].max(1) as f32, 2.0 * jy / frame.size[1].max(1) as f32, 0.0)) * vp;
        }
        let s = &frame.shade;
        let (uv_view, uv_xform) = match frame.kind {
            ViewKind::Scene => (0.0, [1.0, 1.0, 0.0, 0.0]),
            ViewKind::Uv { zoom, pan, .. } => {
                // Fit the 0..1 square into the view, keeping texels square.
                let z = zoom.clamp(0.05, 64.0) * 0.92;
                let (sx, sy) = if aspect >= 1.0 { (z / aspect, z) } else { (z, z * aspect) };
                (1.0, [sx, sy, pan[0], pan[1]])
            }
        };
        // Data views (channels, mesh maps, UV checker, the 2D view) show values as they are: no
        // exposure, tone mapping, effects or shadows.
        let data = uv_view > 0.5 || s.view_mode != 0;
        let fx = |v: f32| if s.post_effects && !data { v.clamp(0.0, 1.0) } else { 0.0 };
        let bloom = fx(s.bloom) > 0.0;
        // The key light lives in the environment: turn it with it (the inverse of env_rotate in
        // mesh.wgsl maps environment directions to world directions).
        let (sin, cos) = s.env_rotation_deg.to_radians().sin_cos();
        let [lx, ly, lz, lw] = s.light;
        let light = Vec3::new(cos * lx + sin * lz, ly, cos * lz - sin * lx);
        let shadow_vp = if !data && s.shadows && lw > 0.0 { self.update_shadow(device, queue, encoder, light, &frame.visible) } else { None };
        let radius = self.mesh.as_ref().map_or(1.0, |m| m.radius);
        // Background rays come from a perspective camera even in orthographic views (an ortho
        // ray grid would show a single flat environment color).
        let bg_vp = OrbitCamera { ortho: false, ..frame.camera }.view_proj(aspect);
        let e = frame.camera.eye();
        let raw = GlobalsRaw {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: bg_vp.inverse().to_cols_array_2d(),
            eye: [e.x, e.y, e.z, 0.0],
            env: [s.env_rotation_deg.to_radians(), (ENV_LEVELS - 1) as f32, s.ao_intensity, 0.0],
            sh: self.sh,
            mode: [s.view_mode as f32, s.channel as f32, s.height_strength, s.normal_y],
            bg: [s.env_opacity, s.env_blur * (ENV_LEVELS - 1) as f32, uv_view, 0.0],
            uv_xform,
            light: [light.x, light.y, light.z, lw],
            light_vp: shadow_vp.unwrap_or(Mat4::IDENTITY).to_cols_array_2d(),
            // Normal offset: 1.5 shadow texels of the light frustum (2 × radius wide).
            shadow: [if shadow_vp.is_some() { 1.0 } else { 0.0 }, 1.0 / SHADOW_SIZE as f32, radius * 3.0 / SHADOW_SIZE as f32, 0.0],
            material: [if s.spec_gloss { 1.0 } else { 0.0 }, if s.parallax { 1.0 } else { 0.0 }, sample as f32, 0.0],
        };
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        let post = PostRaw {
            a: [2f32.powf(s.exposure_ev), flag(s.aces), flag(data), fx(s.bloom) * BLOOM_GAIN],
            b: [fx(s.vignette), fx(s.sharpen), fx(s.grain), BLOOM_THRESHOLD],
        };
        let Some(v) = self.targets(device, frame.view_id, frame.size, bloom) else { return };
        queue.write_buffer(&v.globals, 0, bytemuck::bytes_of(&raw));
        queue.write_buffer(&v.post_buf, 0, bytemuck::bytes_of(&post));
        v.ready = true;
        v.history = Some((frame.clone(), rev));
        v.samples = sample;
        let Some(v) = self.views.get(&frame.view_id) else { return };
        let Some(globals_bg) = v.globals_bg.as_ref() else { return };
        if sample == 0 {
            self.scene_pass(encoder, v, &v.accum, globals_bg, frame);
        } else {
            self.scene_pass(encoder, v, &v.hdr, globals_bg, frame);
            fullscreen_blend(encoder, "tc_accum", &v.accum, wgpu::LoadOp::Load, &self.accum_pipeline, &v.accum_bg, 1.0 / f64::from(sample + 1));
        }
        if bloom && let Some(b) = &v.bloom {
            let [pre, down, up] = &self.bloom_pipelines;
            for (i, (level, bg)) in b.levels.iter().zip(&b.down).enumerate() {
                fullscreen(encoder, "tc_bloom_down", level, CLEAR, if i == 0 { pre } else { down }, bg);
            }
            for (level, bg) in b.levels.iter().zip(&b.up).rev() {
                fullscreen(encoder, "tc_bloom_up", level, wgpu::LoadOp::Load, up, bg);
            }
        }
        fullscreen(encoder, "tc_post", &v.resolve_view, CLEAR, &self.post_pipeline, &v.post_bg);
    }

    /// Background, mesh and wireframe into the view's MSAA HDR target, resolved to `resolve`.
    fn scene_pass(&self, encoder: &mut wgpu::CommandEncoder, v: &ViewTargets, resolve: &wgpu::TextureView, globals_bg: &wgpu::BindGroup, frame: &Frame) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tc_view"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v.msaa,
                resolve_target: Some(resolve),
                depth_slice: None,
                ops: wgpu::Operations { load: CLEAR, store: wgpu::StoreOp::Discard },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &v.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, globals_bg, &[]);
        pass.set_pipeline(&self.bg_pipeline);
        pass.draw(0..3, 0..1);
        let Some(m) = &self.mesh else { return };
        let uv_ts = match frame.kind {
            ViewKind::Scene => None,
            ViewKind::Uv { ts, .. } => Some(ts),
        };
        let (pipe, wire) = match uv_ts {
            None => (&self.mesh_pipeline, frame.shade.wireframe_3d.then_some(&self.wire3d_pipeline)),
            Some(_) => (&self.uv_pipeline, frame.shade.wireframe.then_some(&self.wire_pipeline)),
        };
        pass.set_vertex_buffer(0, m.vb.slice(..));
        for (pipe, ib, ranges) in [(Some(pipe), &m.ib, &m.draws), (wire, &m.lines, &m.line_draws)] {
            let Some(pipe) = pipe else { continue };
            pass.set_pipeline(pipe);
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            for (range, set) in self.drawable(ranges, &frame.visible, uv_ts) {
                pass.set_bind_group(1, &set.bind_group, &[]);
                pass.draw_indexed(range, 0, 0..1);
            }
        }
    }

    /// Draw view `id` into the current pass at `rect` (target pixels: x, y, w, h).
    pub fn blit(&self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>, id: u64, rect: [f32; 4], target_size: [u32; 2]) {
        let Some(v) = self.views.get(&id).filter(|v| v.ready) else { return };
        let raw = BlitRaw { rect, dst: [target_size[0] as f32, target_size[1] as f32, if self.target_format.is_srgb() { 1.0 } else { 0.0 }, 0.0] };
        queue.write_buffer(&v.blit_buf, 0, bytemuck::bytes_of(&raw));
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &v.blit_bg, &[]);
        pass.draw(0..6, 0..1);
    }

    /// Write the blit rectangle for view `id` (call from `prepare` when the pass can't write
    /// buffers itself).
    pub fn set_blit_rect(&self, queue: &wgpu::Queue, id: u64, rect: [f32; 4], target_size: [u32; 2]) {
        if let Some(v) = self.views.get(&id) {
            let raw = BlitRaw { rect, dst: [target_size[0] as f32, target_size[1] as f32, if self.target_format.is_srgb() { 1.0 } else { 0.0 }, 0.0] };
            queue.write_buffer(&v.blit_buf, 0, bytemuck::bytes_of(&raw));
        }
    }

    /// Draw view `id` with the rectangle set by [`Renderer::set_blit_rect`].
    pub fn blit_prepared(&self, pass: &mut wgpu::RenderPass<'_>, id: u64) {
        let Some(v) = self.views.get(&id).filter(|v| v.ready) else { return };
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &v.blit_bg, &[]);
        pass.draw(0..6, 0..1);
    }

    /// Read back the rendered view as tightly packed RGBA8 (sRGB-encoded), blocking.
    pub fn read_view(&self, device: &wgpu::Device, queue: &wgpu::Queue, id: u64) -> Option<(u32, u32, Vec<u8>)> {
        let v = self.views.get(&id)?;
        let (w, h) = (v.size[0], v.size[1]);
        let row = (w * 4).next_multiple_of(256);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tc_read"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("tc_read") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &v.resolve, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let data = buf.slice(..).get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let start = y * row as usize;
            out.extend_from_slice(data.get(start..start + (w * 4) as usize)?);
        }
        Some((w, h, out))
    }

    /// Forget a view's targets (e.g. a closed panel).
    pub fn drop_view(&mut self, id: u64) {
        self.views.remove(&id);
    }
}
