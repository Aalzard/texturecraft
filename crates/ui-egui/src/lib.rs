//! # texturecraft-ui-egui
//!
//! The TextureCraft desktop and web shell: a dense dark professional layout (menu bar,
//! tool-settings bar, tool shelf, asset shelf, 3D + 2D viewports, texture set list, layer stack,
//! properties, display/shader/history windows, bake mode). It is a thin layer: every action goes
//! through `texturecraft-engine` commands, so the CLI, control channel and MCP can do the same.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod control;
pub mod dock;
pub mod docking;
pub mod export_templates;
pub mod fonts;
pub mod gpu;
pub mod i18n;
pub mod keymap;
pub mod menus;
pub mod parity;
pub mod profile;
pub mod render_mode;
pub mod shelf;
pub mod state;
pub mod theme;
pub mod thumbs;
pub mod titlebar;
pub mod toolbar;
pub mod viewport;
pub mod web_files;
pub mod widgets;
pub mod windows;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::Value;
use texturecraft_engine::Session;
use texturecraft_engine::bake::Progress;

pub use control::ControlRequest;
pub use state::UiState;

/// A texture set resize running on a worker thread (`Session::resize_task`).
pub struct ResizeJob {
    pub progress: Arc<Progress>,
    pub rx: std::sync::mpsc::Receiver<texturecraft_engine::Result<texturecraft_engine::ResizeOutput>>,
}

/// A background bake of one or more texture sets.
pub struct BakeJob {
    pub queue: Vec<usize>,
    pub current: Option<(usize, Arc<Progress>, std::sync::mpsc::Receiver<texturecraft_engine::Result<texturecraft_engine::bake::BakeOutput>>)>,
    pub total: usize,
}

/// The browser build's notice when WebGPU is unavailable and wgpu fell back to WebGL2.
pub const WEBGL_NOTICE: &str = i18n::n_(
    "WebGPU isn't available in this browser, so TextureCraft runs on WebGL2: Rendering mode path traces on the CPU (slower) and large texture sets show at reduced resolution. A current Chrome, Edge or Safari has WebGPU.",
);

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: f64,
}

pub struct TextureCraftApp {
    pub session: Session,
    pub ui: UiState,
    pub gpu: Option<gpu::GpuLink>,
    pub control: Option<std::sync::mpsc::Receiver<ControlRequest>>,
    pub toasts: Vec<Toast>,
    pub bake: Option<BakeJob>,
    pub resize: Option<ResizeJob>,
    pub thumbs: thumbs::Thumbs,
    /// Last frame's viewport rect (logical points), for scripted input.
    pub viewport_rect: egui::Rect,
    pub fps: f32,
    /// User smart materials found in the library folder (refreshed on demand).
    pub library: Option<Vec<std::path::PathBuf>>,
    /// User brush presets found in the brushes folder (refreshed on demand).
    pub brushes: Option<Vec<std::path::PathBuf>>,
    /// Rendering mode's path tracer.
    pub render: render_mode::RenderView,
    /// Smoothed CPU time spent building the UI per frame (ms).
    pub ui_ms: f32,
    /// Frame timing and the scripted paint test (`debug.*` control methods).
    pub profiler: profile::Profiler,
    /// Where the last 3D stroke ended (viewport pixels), for Shift+click straight lines.
    pub last_paint_point: Option<texturecraft_engine::geom::glam::Vec2>,
    /// The decal layer the next click in the 3D view places.
    pub placing_decal: Option<texturecraft_engine::core::LayerId>,
    /// The wgpu state (screenshots read the renderer's targets).
    pub render_state: Option<eframe::egui_wgpu::RenderState>,
    /// CJK fallback fonts for the UI language.
    pub fonts: fonts::UiFonts,
    /// A notice across the top of the window until dismissed (English key, shown translated).
    pub banner: Option<&'static str>,
    /// Who draws the window's title bar (decided at startup, before the window opens).
    pub title_bar: titlebar::TitleBar,
    last_time: f64,
    /// When the next autosave is due (seconds of app time; desktop only).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    next_autosave: f64,
    /// The brightness and accent the style was last built for.
    styled: Option<(theme::Brightness, theme::Accent)>,
}

impl TextureCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, session: Session) -> TextureCraftApp {
        theme::install(&cc.egui_ctx);
        // Bakes cast their rays on a GPU device of their own (created on the first bake).
        #[cfg(not(target_arch = "wasm32"))]
        texturecraft_engine::bake_cmds::install_gpu_tracer(Arc::new(texturecraft_render::gpu_bake::GpuBaker::new()));
        let gpu = cc.wgpu_render_state.as_ref().map(gpu::init);
        // The first run speaks the OS (or browser) language.
        let ui = cc.storage.and_then(|s| eframe::get_value::<UiState>(s, "ui")).unwrap_or_else(|| {
            let mut ui = UiState::default();
            ui.prefs.language = i18n::os_language().into();
            ui
        });
        let next_autosave = ui.prefs.autosave_minutes as f64 * 60.0;
        let mut app = TextureCraftApp {
            session,
            ui,
            gpu,
            control: None,
            toasts: Vec::new(),
            bake: None,
            resize: None,
            thumbs: thumbs::Thumbs::default(),
            viewport_rect: egui::Rect::NOTHING,
            fps: 0.0,
            ui_ms: 0.0,
            profiler: profile::Profiler::default(),
            library: None,
            brushes: None,
            render: render_mode::RenderView::default(),
            last_paint_point: None,
            placing_decal: None,
            render_state: cc.wgpu_render_state.clone(),
            fonts: fonts::UiFonts::default(),
            banner: None,
            title_bar: titlebar::TitleBar::default(),
            last_time: 0.0,
            next_autosave,
            styled: None,
        };
        app.ui.rename = None;
        app.ui.windows.new_project = false;
        i18n::set_language(&app.ui.prefs.language);
        export_templates::load_library(&mut app, cc.storage);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let skipped = app.session.load_templates(user_dir("templates"));
            app.session.log.extend(skipped.into_iter().map(|w| format!("Project template skipped: {w}")));
        }
        app
    }

    /// Run an engine command, reporting errors as toasts. Returns the result on success.
    pub fn run(&mut self, id: &str, params: Value) -> Option<Value> {
        match texturecraft_engine::execute(&mut self.session, id, &params) {
            Ok(v) => Some(v),
            Err(e) => {
                self.toast(e.to_string(), true);
                None
            }
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, until: self.last_time + if error { 6.0 } else { 3.0 } });
    }

    /// Start baking texture sets in the background (one after another).
    pub fn start_bake(&mut self, sets: Vec<usize>) {
        if self.bake.is_some() || sets.is_empty() {
            return;
        }
        let total = sets.len();
        self.bake = Some(BakeJob { queue: sets, current: None, total });
    }

    fn poll_bake(&mut self, ctx: &egui::Context) {
        let Some(job) = self.bake.as_mut() else { return };
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        if let Some((ts, _, rx)) = &job.current {
            match rx.try_recv() {
                Ok(Ok(out)) => {
                    let ts = *ts;
                    job.current = None;
                    if let Err(e) = self.session.apply_bake(ts, out) {
                        self.toast(e.to_string(), true);
                    }
                }
                Ok(Err(e)) => {
                    job.current = None;
                    let msg = e.to_string();
                    self.bake = None;
                    self.toast(msg, true);
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.bake = None;
                    self.toast(i18n::tr("The bake worker stopped"), true);
                    return;
                }
            }
        }
        let Some(job) = self.bake.as_mut() else { return };
        if job.current.is_none() {
            if job.queue.is_empty() {
                self.bake = None;
                self.toast(i18n::tr("Mesh maps baked"), false);
                return;
            }
            let ts = job.queue.remove(0);
            match self.session.bake_task(ts) {
                Ok(task) => {
                    let progress = Arc::new(Progress::new());
                    let p = progress.clone();
                    let rx = spawn_worker("texturecraft-bake", move || task.run(&p));
                    if let Some(job) = self.bake.as_mut() {
                        job.current = Some((ts, progress, rx));
                    }
                }
                Err(e) => self.toast(e.to_string(), true),
            }
        }
    }

    pub fn cancel_bake(&mut self) {
        if let Some(job) = self.bake.take()
            && let Some((_, p, _)) = job.current
        {
            p.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Resize texture set `ts` to `resolution` on a worker thread (the document changes when
    /// it is done, as one undo step).
    pub fn start_resize(&mut self, ts: usize, resolution: u32) {
        if self.resize.is_some() {
            return;
        }
        match self.session.resize_task(ts, resolution) {
            Ok(task) => {
                let progress = Arc::new(Progress::new());
                let p = progress.clone();
                self.resize = Some(ResizeJob { progress, rx: spawn_worker("texturecraft-resize", move || task.run(&p)) });
            }
            Err(e) => self.toast(e.to_string(), true),
        }
    }

    /// Apply a finished resize (after any stroke in progress ends).
    fn poll_resize(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.resize else { return };
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        if self.session.stroke_active() {
            return;
        }
        let result = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(texturecraft_engine::EngineError::Other(i18n::tr("The resize worker stopped").into())),
        };
        self.resize = None;
        if let Err(e) = result.and_then(|out| self.session.apply_resize(out)) {
            self.toast(e.to_string(), true);
        }
    }

    /// Bake progress 0..1 across all queued sets.
    pub fn bake_progress(&self) -> Option<f32> {
        let job = self.bake.as_ref()?;
        let done = job.total - job.queue.len() - usize::from(job.current.is_some());
        let cur = job.current.as_ref().map(|(_, p, _)| p.fraction()).unwrap_or(0.0);
        Some(((done as f32 + cur) / job.total.max(1) as f32).clamp(0.0, 1.0))
    }
}

/// Run `f` on a worker thread named `name`; its result arrives on the returned channel.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_worker<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
        let _ = tx.send(f());
    });
    if let Err(e) = spawned {
        // The receiver reports a stopped worker.
        log::warn!("could not start {name}: {e}");
    }
    rx
}

#[cfg(target_arch = "wasm32")]
fn spawn_worker<T: Send + 'static>(_name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::sync::mpsc::Receiver<T> {
    // shortcut: no threads on the web; the work runs on the UI thread (the window pauses).
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(f());
    rx
}

impl TextureCraftApp {
    /// Every `prefs.autosave_minutes`, write a modified project to the autosave folder (encoded
    /// and written on a worker thread; the project data is shared copy-on-write).
    #[cfg(not(target_arch = "wasm32"))]
    fn autosave(&mut self, now: f64) {
        let period = self.ui.prefs.autosave_minutes as f64 * 60.0;
        // The clock starts at the first unsaved change.
        if period <= 0.0 || !self.session.modified {
            self.next_autosave = now + period;
            return;
        }
        if now < self.next_autosave {
            return;
        }
        self.next_autosave = now + period;
        let dir = user_dir("autosave");
        let path = dir.join(format!(
            "{}.{}",
            texturecraft_engine::formats::export::sanitize_file_stem(&self.session.doc.project.name),
            texturecraft_engine::core::PROJECT_EXT
        ));
        let pf = self.session.project_file();
        let spawned = std::thread::Builder::new().name("texturecraft-autosave".into()).spawn(move || {
            let result = std::fs::create_dir_all(&dir)
                .map_err(|e| e.to_string())
                .and_then(|_| texturecraft_engine::formats::save_project(&path, &pf).map_err(|e| e.to_string()));
            match result {
                Ok(()) => log::info!("autosaved {}", path.display()),
                Err(e) => log::warn!("autosave to {} failed: {e}", path.display()),
            }
        });
        if let Err(e) = spawned {
            log::warn!("could not start autosave: {e}");
        }
    }
}

impl eframe::App for TextureCraftApp {
    /// The browser never tells eframe the page is closing, so save often there.
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if cfg!(target_arch = "wasm32") { 5 } else { 30 })
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "ui", &self.ui);
        export_templates::save_library(self, storage);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t0 = now_ms();
        let now = ctx.input(|i| i.time);
        let dt = (now - self.last_time) as f32;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.last_time = now;
        let style = (self.ui.prefs.brightness, self.ui.prefs.accent);
        if self.styled != Some(style) {
            theme::apply(ctx, style.0, style.1);
            self.styled = Some(style);
        }
        control::poll(self, ctx);
        web_files::poll(self);
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in &dropped {
            web_files::dropped(self, ctx, f);
        }
        self.poll_bake(ctx);
        self.poll_resize(ctx);
        let scale = self.ui.prefs.ui_scale.clamp(0.5, 3.0);
        if (ctx.zoom_factor() - scale).abs() > 1e-3 {
            ctx.set_zoom_factor(scale);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.autosave(now);
        i18n::set_language(&self.ui.prefs.language);
        #[cfg(target_arch = "wasm32")]
        {
            keymap::sync_captured(&self.ui.keymap);
            web_files::sync_unsaved(self);
        }
        self.fonts.update(ctx, i18n::current().code, self.ui.windows.settings);
        menus::shortcuts(self, ctx);
        profile::drive(self, ctx);
        self.toasts.retain(|t| t.until > now);
        self.profiler.logic_ms = (now_ms() - t0) as f32;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t0 = now_ms();
        if !self.ui.hide_ui {
            titlebar::show(self, ui);
            banner(self, ui);
            toolbar::top_bar(self, ui);
            toolbar::status_bar(self, ui);
            toolbar::tool_shelf(self, ui);
            // Bake settings take the Assets slot; a side panel only when Assets is closed.
            if self.ui.mode == state::Mode::Baking && !self.ui.dock.is_open(docking::Panel::Assets) {
                windows::bake_left(self, ui);
            }
            toolbar::rail(self, ui);
        }
        let hide = self.ui.hide_ui;
        let tk = theme::Tokens::get(&ctx);
        // The dock keeps the cards' gutter from the bars and rails around it.
        let frame = if hide { egui::Frame::NONE.fill(tk.viewport) } else { egui::Frame::NONE.fill(tk.chrome).inner_margin(theme::GUTTER) };
        egui::CentralPanel::default().frame(frame).show(ui, |ui| if hide { viewport::central(self, ui) } else { docking::show(self, ui) });
        windows::show(self, &ctx);
        toasts(self, &ctx);
        let end = now_ms();
        let dt = (end - t0) as f32;
        self.ui_ms = if self.ui_ms == 0.0 { dt } else { self.ui_ms * 0.9 + dt * 0.1 };
        self.profiler.end_frame(end, dt);
    }
}

/// `Documents/TextureCraft/<sub>` in the user's home folder.
#[cfg(not(target_arch = "wasm32"))]
pub fn user_dir(sub: &str) -> std::path::PathBuf {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join("Documents").join("TextureCraft").join(sub)
}

/// Folder holding the user's smart materials.
#[cfg(not(target_arch = "wasm32"))]
pub fn library_dir() -> std::path::PathBuf {
    user_dir("library")
}

/// Milliseconds since the first call (0 on the web, where `std::time` is unavailable).
pub fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
    }
    #[cfg(target_arch = "wasm32")]
    0.0
}

/// The dismissable notice under the menu bar.
fn banner(app: &mut TextureCraftApp, ui: &mut egui::Ui) {
    let Some(text) = app.banner else { return };
    let tk = theme::Tokens::get(ui.ctx());
    egui::Panel::top("banner").show_separator_line(false).frame(egui::Frame::NONE.fill(tk.row_selected).inner_margin(egui::Margin::symmetric(10, 5))).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(egui_phosphor::regular::INFO).color(tk.accent));
                // The text wraps in the width the dismiss button leaves.
                let width = (ui.available_width() - 28.0).max(80.0);
                ui.allocate_ui_with_layout(egui::vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(i18n::tr(text)).color(tk.text_strong)).wrap());
                });
                if widgets::icon_button(ui, egui_phosphor::regular::X, i18n::tr("Dismiss"), false, 18.0).clicked() {
                    app.banner = None;
                }
            });
        },
    );
}

fn toasts(app: &mut TextureCraftApp, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    let tk = theme::Tokens::get(ctx);
    egui::Area::new(egui::Id::unique("toasts")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -36.0)).show(ctx, |ui| {
        for t in &app.toasts {
            egui::Frame::NONE
                .fill(tk.panel)
                .stroke(egui::Stroke::new(1.0, if t.error { tk.danger } else { tk.accent }))
                .corner_radius(crate::theme::CARD_RADIUS)
                .shadow(ui.visuals().popup_shadow)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_max_width(360.0);
                    ui.label(egui::RichText::new(&t.text).color(tk.text_strong));
                });
            ui.add_space(4.0);
        }
    });
}
