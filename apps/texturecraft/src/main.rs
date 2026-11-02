//! TextureCraft desktop app.
//!
//! Usage: `texturecraft [project.tcraft | mesh.glb] [--control <port>] [--empty] [--version]`
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod control_server;
mod window_frame;

use texturecraft_engine::Session;
use texturecraft_ui_egui::titlebar::TitleBar;
use texturecraft_ui_egui::{TextureCraftApp, UiState};

/// The app name eframe keeps the settings under (`eframe::storage_dir`).
const APP_NAME: &str = "TextureCraft";

const ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.texturecraft.png");

fn version() -> String {
    let sha = env!("TEXTURECRAFT_BUILD_SHA");
    let short = sha.get(..7).unwrap_or(sha);
    let date = env!("TEXTURECRAFT_BUILD_DATE");
    match (short.is_empty(), date.is_empty()) {
        (true, true) => format!("TextureCraft {}", texturecraft_engine::VERSION),
        (true, false) => format!("TextureCraft {} ({date})", texturecraft_engine::VERSION),
        _ => format!("TextureCraft {} ({short} {date})", texturecraft_engine::VERSION),
    }
}

struct Args {
    file: Option<String>,
    control: Option<u16>,
    empty: bool,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args { file: None, control: None, empty: false };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--version" | "-V" => {
                println!("{}", version());
                return Ok(None);
            }
            "--help" | "-h" => {
                println!("{}\n\nUsage: texturecraft [project.tcraft | mesh file] [--control <port>] [--empty]", version());
                return Ok(None);
            }
            "--control" => {
                let p = it.next().ok_or("--control needs a port")?;
                args.control = Some(p.parse().map_err(|_| format!("invalid port `{p}`"))?);
            }
            "--empty" => args.empty = true,
            f if !f.starts_with('-') => args.file = Some(f.to_string()),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(Some(args))
}

fn initial_session(args: &Args) -> Session {
    if let Some(f) = &args.file {
        match texturecraft_engine::file_cmds::open_path(f) {
            Ok(s) => return s,
            Err(e) => eprintln!("texturecraft: could not open {f}: {e}"),
        }
    }
    if !args.empty
        && let Ok(s) = texturecraft_engine::sample_session(2048, 512)
    {
        return s;
    }
    match Session::new_project("Untitled", texturecraft_engine::geom::primitives::material_ball(), "builtin:material_ball", None, 2048) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("texturecraft: {e}");
            std::process::exit(1);
        }
    }
}

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}

/// Windows shows a window's big icon (`ICON_BIG`) in the taskbar and Alt+Tab; eframe sets only the
/// small one, so the taskbar would show a generic icon. Set the big one too.
#[cfg(windows)]
fn set_taskbar_icon(w: &winit::window::Window) {
    use winit::platform::windows::WindowExtWindows as _;
    // An icon that can't be made leaves the generic one: nothing else depends on it.
    if let Some(i) = icon().and_then(|i| winit::window::Icon::from_rgba(i.rgba, i.width, i.height).ok()) {
        w.set_taskbar_icon(Some(i));
    }
}

/// Settings › Interface › System title bar, read from the saved settings before the window opens
/// (eframe hands the app its storage only afterwards). Missing or unreadable: off.
fn saved_system_title_bar(persistence_path: Option<&std::path::Path>) -> bool {
    let file = persistence_path.map(std::path::Path::to_path_buf).or_else(|| eframe::storage_dir(APP_NAME).map(|d| d.join("app.ron")));
    let saved = file.and_then(|f| std::fs::read_to_string(f).ok());
    let kv = saved.and_then(|s| ron::from_str::<std::collections::HashMap<String, String>>(&s).ok());
    kv.and_then(|kv| ron::from_str::<UiState>(kv.get("ui")?).ok()).is_some_and(|ui| ui.prefs.system_title_bar)
}

/// The app with the window chrome that needs the native window: the Windows borderless guard, and
/// on macOS the traffic lights and AppKit's `movable` flag.
struct Shell {
    app: TextureCraftApp,
    window_frame: Option<window_frame::WindowFrameFn>,
    #[cfg(target_os = "macos")]
    movable: Option<bool>,
}

impl eframe::App for Shell {
    fn auto_save_interval(&self) -> std::time::Duration {
        self.app.auto_save_interval()
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app.save(storage);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, _raw: &mut egui::RawInput) {
        // winit gives the window its caption back when it changes the window's flags (maximize,
        // restore, …): take it off again before the frame is laid out.
        if let Some(guard) = self.window_frame.as_mut() {
            guard();
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
        #[cfg(target_os = "macos")]
        texturecraft_ui_egui::titlebar::sync_native(&self.app.title_bar, ui, frame, &mut self.movable);
    }
}

fn main() {
    let args = match parse_args() {
        Ok(Some(a)) => a,
        Ok(None) => return,
        Err(e) => {
            eprintln!("texturecraft: {e}");
            std::process::exit(2);
        }
    };
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("texturecraft: internal error: {info}");
        prev(info);
    }));
    let session = initial_session(&args);
    // Scripted runs (agents, films, tests) set TEXTURECRAFT_NO_PREFS so they neither read nor
    // overwrite the user's saved UI layout and settings: state goes to a throwaway folder.
    let persistence_path =
        std::env::var_os("TEXTURECRAFT_NO_PREFS").map(|_| std::env::temp_dir().join(format!("texturecraft-noprefs-{}.ron", std::process::id())));
    let title_bar = TitleBar::decide(saved_system_title_bar(persistence_path.as_deref()));
    let mut viewport = egui::ViewportBuilder::default().with_title(APP_NAME).with_inner_size([1600.0, 960.0]).with_min_inner_size([960.0, 600.0]);
    if let Some(i) = icon() {
        viewport = viewport.with_icon(i);
    }
    let viewport = title_bar.viewport(viewport);
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, persistence_path, ..Default::default() };
    let control = args.control;
    // The start window offers new, open and recent projects, unless a file was given or a script drives the app.
    let start = args.file.is_none() && control.is_none();
    let r = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            #[cfg(windows)]
            if let Some(w) = cc.winit_window() {
                set_taskbar_icon(w);
            }
            let window_frame = window_frame::service(cc, matches!(title_bar, TitleBar::App(_)));
            let mut app = TextureCraftApp::new(cc, session);
            app.title_bar = title_bar;
            app.ui.windows.start = start && app.ui.prefs.start_window;
            if let Some(port) = control {
                app.control = Some(control_server::start(port, cc.egui_ctx.clone()));
            }
            Ok(Box::new(Shell {
                app,
                window_frame,
                #[cfg(target_os = "macos")]
                movable: None,
            }))
        }),
    );
    if let Err(e) = r {
        eprintln!("texturecraft: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_title_bar_setting_is_read_from_eframes_settings_file() {
        let path = std::env::temp_dir().join(format!("texturecraft-titlebar-test-{}.ron", std::process::id()));
        for on in [false, true] {
            let mut ui = UiState::default();
            ui.prefs.system_title_bar = on;
            // Written as eframe writes it: a pretty RON map of RON strings.
            let kv = std::collections::HashMap::from([("ui".to_string(), ron::to_string(&ui).unwrap())]);
            std::fs::write(&path, ron::Options::default().to_string_pretty(&kv, Default::default()).unwrap()).unwrap();
            assert_eq!(saved_system_title_bar(Some(&path)), on);
        }
        std::fs::write(&path, "not ron").unwrap();
        assert!(!saved_system_title_bar(Some(&path)));
        std::fs::remove_file(&path).unwrap();
        assert!(!saved_system_title_bar(Some(&path)));
    }
}
