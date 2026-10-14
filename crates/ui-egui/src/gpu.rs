//! Bridge between the session and the wgpu renderer: per-frame scene sync (mesh, texture set
//! layout, dirty tiles, environment) and the egui paint callback that renders and blits a view.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use rayon::prelude::*;
use texturecraft_engine::Session;
use texturecraft_engine::compose::{DisplayKind, ImageRes, tile_bytes};
use texturecraft_engine::core::BakedMap;
use texturecraft_engine::geom::Mesh;
use texturecraft_render::{Frame, Renderer, wgpu};

/// Content shown through the aux texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AuxView {
    MeshMap(BakedMap),
    Mask(texturecraft_engine::core::LayerId),
    /// The quick mask being edited, over the base color.
    QuickMask,
}

/// What the renderer must catch up with before drawing.
#[derive(Default)]
pub struct SceneSync {
    pub mesh: Option<(Arc<Mesh>, u64)>,
    /// (resolution, material) per texture set, and the mesh material count.
    pub sets: Option<(Vec<(u32, usize)>, usize)>,
    /// The environment name and, for `resource:<name>`, its decoded image.
    pub env: Option<(String, Option<Arc<ImageRes>>)>,
    pub tiles: Vec<(usize, DisplayKind, u32, u32, Vec<u8>)>,
    pub aux: Vec<(usize, u32, Vec<u8>)>,
}

/// Lives in egui's callback resources.
pub struct GpuState {
    pub renderer: Renderer,
    /// Raised when texture-set textures were recreated: the UI must upload every tile again.
    pub reset: Arc<AtomicBool>,
}

/// UI-side bookkeeping of what the GPU already has.
pub struct GpuLink {
    pub reset: Arc<AtomicBool>,
    /// Raised when a view callback was dropped before the GPU applied its scene sync (the
    /// frame was never painted, e.g. a WebGPU surface not ready yet): send everything again.
    pub lost: Arc<AtomicBool>,
    mesh_id: u64,
    sets: Vec<(u32, usize)>,
    /// The environment the GPU has: name and image identity (a re-imported resource differs).
    env: (String, usize),
    /// What each set's aux texture holds: (mesh map or mask key, revision).
    aux: BTreeMap<usize, (String, u64)>,
    pub adapter: String,
    /// Most tiles waiting to be composited since compositing last caught up (progress).
    composite_total: usize,
}

/// Compositing time per frame: a full recomposite of a large texture set spreads over frames.
const COMPOSITE_BUDGET_MS: f64 = 30.0;

pub fn init(rs: &egui_wgpu::RenderState) -> GpuLink {
    let reset = Arc::new(AtomicBool::new(false));
    let renderer = Renderer::new(&rs.device, &rs.queue, rs.target_format);
    rs.renderer.write().callback_resources.insert(GpuState { renderer, reset: reset.clone() });
    let info = rs.adapter.get_info();
    GpuLink {
        reset,
        lost: Arc::new(AtomicBool::new(false)),
        mesh_id: 0,
        sets: Vec::new(),
        env: Default::default(),
        aux: BTreeMap::new(),
        adapter: format!("{} ({:?})", info.name, info.backend),
        composite_total: 0,
    }
}

impl GpuLink {
    /// Collect everything the GPU needs this frame (runs the compositor first).
    /// Compositing progress (0..1) while tiles wait to be composited.
    pub fn composite_progress(&self, s: &Session) -> Option<f32> {
        let pending = s.pending_tiles();
        (pending > 0 && self.composite_total > 0).then(|| 1.0 - pending as f32 / self.composite_total as f32)
    }

    pub fn sync(&mut self, s: &mut Session, aux_map: Option<AuxView>) -> SceneSync {
        let pending = s.pending_tiles();
        self.composite_total = if pending == 0 { 0 } else { self.composite_total.max(pending) };
        s.update_for(COMPOSITE_BUDGET_MS, crate::now_ms);
        let mut out = SceneSync::default();
        if self.lost.swap(false, Ordering::AcqRel) {
            self.mesh_id = 0;
            self.sets.clear();
            self.env = Default::default();
            self.reset.store(true, Ordering::Release);
        }
        if self.mesh_id != s.mesh_id {
            out.mesh = Some((s.mesh.clone(), s.mesh_id));
            self.mesh_id = s.mesh_id;
            self.aux.clear();
        }
        let sets: Vec<(u32, usize)> = s.doc.project.texture_sets.iter().map(|t| (t.resolution, t.material)).collect();
        if sets != self.sets || out.mesh.is_some() {
            out.sets = Some((sets.clone(), s.mesh.materials.len().max(1)));
            self.sets = sets;
        }
        let d = &s.doc.project.display;
        let image = d.environment_resource().and_then(|n| s.images.get(n));
        let ptr = image.map_or(0, |i| Arc::as_ptr(i) as usize);
        if self.env.0 != d.environment || self.env.1 != ptr {
            self.env = (d.environment.clone(), ptr);
            out.env = Some((d.environment.clone(), image.cloned()));
        }
        if self.reset.swap(false, Ordering::AcqRel) {
            for c in &mut s.caches {
                c.upload.extend(texturecraft_engine::compose::all_tiles(c.resolution));
            }
            self.aux.clear();
        }
        for ts in 0..s.caches.len() {
            let tiles = s.take_uploads(ts);
            if tiles.is_empty() {
                continue;
            }
            let Some(cache) = s.caches.get(ts) else { continue };
            let empty = Default::default();
            let baked = s.doc.baked.get(&ts).unwrap_or(&empty);
            let comp = &cache.composite;
            let bytes: Vec<_> = tiles
                .par_iter()
                .flat_map_iter(|&(tx, ty)| DisplayKind::ALL.into_iter().map(move |k| (k, tx, ty)))
                .map(|(k, tx, ty)| (ts, k, tx, ty, tile_bytes(comp, baked, k, tx, ty)))
                .collect();
            out.tiles.extend(bytes);
        }
        match aux_map {
            Some(AuxView::MeshMap(m)) => {
                for ts in 0..s.caches.len() {
                    let rev = s.doc.baked.get(&ts).and_then(|b| b.maps.get(&m)).map(|d| Arc::as_ptr(d) as u64).unwrap_or(0);
                    let key = (format!("{m:?}"), rev);
                    if self.aux.get(&ts) == Some(&key) {
                        continue;
                    }
                    let res = s.caches.get(ts).map(|c| c.resolution).unwrap_or(1024).min(2048);
                    out.aux.push((ts, res, texturecraft_render::mesh_maps::mesh_map_rgba(s.doc.baked.get(&ts), m, res)));
                    self.aux.insert(ts, key);
                }
            }
            Some(AuxView::Mask(id)) => {
                if let Some((ts, _, _)) = s.doc.project.locate(id) {
                    let key = (format!("mask{}", id.0), s.revision);
                    if self.aux.get(&ts) != Some(&key) && !s.stroke_active() {
                        let res = s.caches.get(ts).map(|c| c.resolution).unwrap_or(1024).min(2048);
                        if let Some(rgba) = s.mask_view_rgba(id, res) {
                            out.aux.push((ts, res, rgba));
                        }
                        self.aux.insert(ts, key);
                    }
                }
            }
            Some(AuxView::QuickMask) => {
                for ts in 0..s.caches.len() {
                    let Some(cache) = s.caches.get(ts) else { continue };
                    let key = (format!("quickmask{}", s.quick_mask.revision), cache.revision);
                    if self.aux.get(&ts) == Some(&key) {
                        continue;
                    }
                    let res = cache.resolution.min(2048);
                    if let Some(rgba) = s.quick_mask_rgba(ts, res) {
                        out.aux.push((ts, res, rgba));
                    }
                    self.aux.insert(ts, key);
                }
            }
            None => {}
        }
        out
    }
}

/// Paint callback for one view.
pub struct ViewCallback {
    pub frame: Frame,
    /// Scene changes, applied by the first view drawn this frame.
    pub sync: Mutex<Option<SceneSync>>,
    pub rect: egui::Rect,
    /// [`GpuLink::lost`].
    pub lost: Arc<AtomicBool>,
}

impl Drop for ViewCallback {
    fn drop(&mut self) {
        if self.sync.get_mut().unwrap_or_else(PoisonError::into_inner).is_some() {
            self.lost.store(true, Ordering::Release);
        }
    }
}

impl CallbackTrait for ViewCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<GpuState>() else { return Vec::new() };
        let sync = self.sync.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(sync) = sync {
            if let Some((env, image)) = &sync.env {
                gpu.renderer.set_environment(device, queue, env, image.as_deref());
            }
            if let Some((mesh, id)) = &sync.mesh {
                gpu.renderer.set_mesh(device, queue, mesh, *id);
            }
            if let Some((sets, mats)) = &sync.sets {
                let reset = gpu.renderer.set_texture_sets(device, queue, sets, *mats);
                if !reset.is_empty() {
                    gpu.reset.store(true, Ordering::Release);
                }
            }
            for (ts, k, tx, ty, bytes) in &sync.tiles {
                gpu.renderer.upload_tile(queue, *ts, *k, *tx, *ty, bytes);
            }
            for (ts, size, bytes) in &sync.aux {
                gpu.renderer.upload_aux(queue, *ts, *size, bytes);
            }
        }
        gpu.renderer.prepare(device, queue, encoder, &self.frame);
        let ppp = screen.pixels_per_point;
        let r = self.rect;
        gpu.renderer.set_blit_rect(queue, self.frame.view_id, [r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp], screen.size_in_pixels);
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &CallbackResources) {
        let Some(gpu) = resources.get::<GpuState>() else { return };
        let [sw, sh] = info.screen_size_px;
        let clip = info.clip_rect_in_pixels();
        if clip.width_px <= 0 || clip.height_px <= 0 || sw == 0 || sh == 0 {
            return;
        }
        pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
        pass.set_scissor_rect(clip.left_px.max(0) as u32, clip.top_px.max(0) as u32, clip.width_px as u32, clip.height_px as u32);
        gpu.renderer.blit_prepared(pass, self.frame.view_id);
    }
}
