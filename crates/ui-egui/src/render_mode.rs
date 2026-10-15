//! Rendering mode (F10): a progressive path-traced view of the current camera. The tracer runs on
//! a worker thread (one pass per frame on the single-threaded web) and restarts whenever the
//! scene, camera, view size or settings change.

use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use egui::{Color32, ColorImage, Rect, RichText, TextureHandle, TextureOptions, Ui, vec2};
use texturecraft_engine::geom::OrbitCamera;
use texturecraft_render::trace::{TraceDisplay, TraceScene, TraceSet, TraceSettings, Tracer};

use crate::TextureCraftApp;
use crate::theme::TOKENS;

/// A finished pass, ready to show.
struct Frame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    samples: u32,
}

/// The tracer and everything it needs.
struct Work {
    scene: TraceScene,
    camera: OrbitCamera,
    settings: TraceSettings,
    display: TraceDisplay,
    max_samples: u32,
    tracer: Tracer,
}

impl Work {
    fn frame(&self) -> Frame {
        Frame { width: self.tracer.width, height: self.tracer.height, rgba: self.tracer.rgba8(&self.display), samples: self.tracer.samples }
    }

    /// One more sample per pixel, or None when done.
    #[cfg(not(target_arch = "wasm32"))]
    fn step(&mut self) -> Option<Frame> {
        if self.tracer.samples >= self.max_samples {
            return None;
        }
        self.tracer.pass(&self.scene, &self.camera, &self.settings);
        Some(self.frame())
    }

    /// Trace for about `budget_ms` of this frame (the web has one thread: the UI must keep
    /// running); a frame to show when a pass finished.
    #[cfg(target_arch = "wasm32")]
    fn step_for(&mut self, budget_ms: f64) -> Option<Frame> {
        let start = js_sys::Date::now();
        let mut rows = 4;
        while self.tracer.samples < self.max_samples {
            let t = js_sys::Date::now();
            if self.tracer.pass_rows(&self.scene, &self.camera, &self.settings, rows) {
                return Some(self.frame());
            }
            let now = js_sys::Date::now();
            if now - start >= budget_ms {
                break;
            }
            // Grow the slice while rows are cheap, staying inside the budget.
            if now - t < budget_ms / 8.0 {
                rows *= 2;
            }
        }
        None
    }
}

/// The running render of one scene state.
struct Job {
    key: String,
    latest: Arc<Mutex<Option<Frame>>>,
    started: f64,
    #[cfg(not(target_arch = "wasm32"))]
    cancel: Arc<AtomicBool>,
    #[cfg(target_arch = "wasm32")]
    work: Work,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Rendering mode state kept by the app.
#[derive(Default)]
pub struct RenderView {
    job: Option<Job>,
    texture: Option<TextureHandle>,
    shown: Option<(u32, u32, Vec<u8>)>,
    samples: u32,
    elapsed: f64,
}

/// Viewport resolution of the render (the web renders at half size: it has one thread).
fn render_size(rect: Rect) -> (u32, u32) {
    let scale = if cfg!(target_arch = "wasm32") { 0.5 } else { 1.0 };
    ((rect.width() * scale).max(16.0) as u32, (rect.height() * scale).max(16.0) as u32)
}

fn work(app: &TextureCraftApp, size: (u32, u32)) -> Work {
    let s = &app.session;
    let d = &s.doc.project.display;
    let sets = s
        .doc
        .project
        .texture_sets
        .iter()
        .zip(&s.caches)
        .map(|(ts, c)| TraceSet { material: ts.material, composite: c.composite.clone(), height_strength: ts.atlas_height_strength() })
        .collect();
    let env_image = d.environment_resource().and_then(|n| s.images.get(n));
    Work {
        scene: TraceScene::for_display(s.mesh.clone(), s.bvh.clone(), sets, d, env_image.map(|i| i.as_ref())),
        camera: s.camera,
        settings: TraceSettings { bounces: app.ui.render.bounces },
        display: TraceDisplay { denoise: app.ui.render.denoise, ..TraceDisplay::from_display(d) },
        max_samples: app.ui.render.max_samples.max(1),
        tracer: Tracer::new(size.0, size.1),
    }
}

/// What the render depends on; a change restarts it.
fn key(app: &TextureCraftApp, size: (u32, u32)) -> String {
    let s = &app.session;
    let revisions: Vec<u64> = s.caches.iter().map(|c| c.revision).collect();
    format!("{}|{revisions:?}|{:?}|{size:?}|{:?}|{:?}", s.mesh_id, s.camera, s.doc.project.display, app.ui.render)
}

fn start(app: &TextureCraftApp, ctx: &egui::Context, key: String, size: (u32, u32)) -> Job {
    let latest = Arc::new(Mutex::new(None));
    let started = ctx.input(|i| i.time);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut w = work(app, size);
        let (out, stop, ctx) = (latest.clone(), cancel.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("texturecraft-render".into()).spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let Some(frame) = w.step() else { break };
                if stop.load(Ordering::Acquire) {
                    break;
                }
                *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(frame);
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            log::error!("could not start the render thread: {e}");
        }
        Job { key, latest, started, cancel }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        Job { key, latest, started, work: work(app, size) }
    }
}

/// Draw Rendering mode over the 3D view in `rect` (navigation is handled by the 3D view).
pub fn show(app: &mut TextureCraftApp, ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let size = render_size(rect);
    let k = key(app, size);
    if app.render.job.as_ref().is_none_or(|j| j.key != k) {
        app.render.job = Some(start(app, &ctx, k, size));
        app.render.samples = 0;
    }
    let now = ctx.input(|i| i.time);
    let mut running = false;
    if let Some(job) = app.render.job.as_mut() {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(f) = job.work.step_for(30.0) {
                *job.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(f);
            }
            if job.work.tracer.samples < job.work.max_samples {
                ctx.request_repaint();
            }
        }
        if let Some(f) = job.latest.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let img = ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.rgba);
            match app.render.texture.as_mut() {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => app.render.texture = Some(ctx.load_texture("render-view", img, TextureOptions::LINEAR)),
            }
            app.render.samples = f.samples;
            app.render.elapsed = now - job.started;
            app.render.shown = Some((f.width, f.height, f.rgba));
        }
        running = app.render.samples < app.ui.render.max_samples;
    }
    let p = ui.painter_at(rect);
    match &app.render.texture {
        Some(t) if app.render.samples > 0 => {
            p.image(t.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        _ => {
            p.rect_filled(rect, 0.0, TOKENS.viewport);
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "Starting render...", egui::FontId::proportional(14.0), TOKENS.text_dim);
        }
    }
    overlay(app, ui, rect, running);
}

/// The render status and settings card in the top-left corner.
fn overlay(app: &mut TextureCraftApp, ui: &mut Ui, rect: Rect, running: bool) {
    let card = Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(250.0, 176.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(card).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::Frame::popup(child.style()).show(&mut child, |ui| {
        ui.set_width(226.0);
        let max = app.ui.render.max_samples.max(1);
        ui.label(RichText::new("RENDERING").size(11.0).strong());
        let status = if running { "Rendering" } else { "Done" };
        ui.label(RichText::new(format!("{status}: {} / {max} samples, {:.1} s", app.render.samples, app.render.elapsed)).size(11.0).color(TOKENS.text_dim));
        ui.add(egui::ProgressBar::new(app.render.samples as f32 / max as f32).desired_height(4.0));
        ui.horizontal(|ui| {
            ui.label("Samples");
            ui.add(egui::DragValue::new(&mut app.ui.render.max_samples).range(1..=16384));
            ui.label("Bounces");
            ui.add(egui::DragValue::new(&mut app.ui.render.bounces).range(1..=32));
        });
        ui.checkbox(&mut app.ui.render.denoise, "Denoise").on_hover_text("Filter the remaining noise while keeping edges and texture detail");
        ui.horizontal(|ui| {
            if ui.add_enabled(app.render.shown.is_some(), egui::Button::new("Save render...")).clicked() {
                save(app);
            }
            if ui.button("Back to painting").clicked() {
                app.ui.mode = crate::state::Mode::Painting;
            }
        });
    });
}

/// Save the shown render as PNG.
fn save(app: &mut TextureCraftApp) {
    let Some((w, h, rgba)) = app.render.shown.clone() else { return };
    let Some(png) = crate::control::encode_png(w, h, rgba).map(|(_, _, png)| png) else {
        app.toast("Couldn't encode the render", true);
        return;
    };
    let name = format!("{}_render.png", texturecraft_engine::formats::export::sanitize_file_stem(&app.session.doc.project.name));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name(name).save_file() else { return };
        match std::fs::write(&path, png) {
            Ok(()) => app.toast(format!("Saved {}", path.display()), false),
            Err(e) => app.toast(format!("Couldn't save the render: {e}"), true),
        }
    }
    #[cfg(target_arch = "wasm32")]
    match crate::web_files::download(&name, &png) {
        Ok(()) => app.toast(format!("Downloaded {name}"), false),
        Err(e) => app.toast(format!("Couldn't save the render: {e}"), true),
    }
}

impl RenderView {
    pub fn is_active(&self) -> bool {
        self.job.is_some() || self.texture.is_some()
    }
}

/// Stop rendering (leaving Rendering mode frees the worker and the image).
pub fn stop(app: &mut TextureCraftApp) {
    app.render = RenderView::default();
}
