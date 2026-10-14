//! TextureCraft desktop app.
//!
//! Usage: `texturecraft [project.tcraft | mesh.glb] [--control <port>] [--empty] [--version]`
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod control_server;

use texturecraft_engine::Session;
use texturecraft_ui_egui::TextureCraftApp;

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
    let mut viewport = egui::ViewportBuilder::default().with_title("TextureCraft").with_inner_size([1600.0, 960.0]).with_min_inner_size([960.0, 600.0]);
    if let Some(i) = icon() {
        viewport = viewport.with_icon(i);
    }
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, ..Default::default() };
    let control = args.control;
    let r = eframe::run_native(
        "TextureCraft",
        options,
        Box::new(move |cc| {
            let mut app = TextureCraftApp::new(cc, session);
            if let Some(port) = control {
                app.control = Some(control_server::start(port, cc.egui_ctx.clone()));
            }
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = r {
        eprintln!("texturecraft: {e}");
        std::process::exit(1);
    }
}
