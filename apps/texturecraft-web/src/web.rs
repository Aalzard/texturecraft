//! The eframe web runner: canvas lookup, backend selection and startup errors shown in the page.

use texturecraft_engine::Session;
use texturecraft_ui_egui::TextureCraftApp;
use wasm_bindgen::JsCast as _;

const CANVAS_ID: &str = "texturecraft_canvas";
const LOADING_ID: &str = "texturecraft_loading";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let result = match document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) {
            Some(canvas) => run(canvas).await,
            None => Err(format!("the page has no <canvas id=\"{CANVAS_ID}\">")),
        };
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => {
                    log::error!("TextureCraft failed to start: {e}");
                    el.set_text_content(Some(&format!("TextureCraft failed to start: {e}. It needs a browser with WebGPU or WebGL2.")));
                }
            }
        }
    });
}

async fn run(canvas: web_sys::HtmlCanvasElement) -> Result<(), String> {
    let mut options = eframe::WebOptions::default();
    if query().contains("webgl")
        && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
    {
        create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
    }
    eframe::WebRunner::new()
        .start(
            canvas,
            options,
            Box::new(|cc| {
                if let Some(rs) = &cc.wgpu_render_state {
                    log::info!("TextureCraft: wgpu backend {:?}", rs.adapter.get_info().backend);
                }
                Ok(Box::new(TextureCraftApp::new(cc, initial_session()?)))
            }),
        )
        .await
        .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))
}

/// The baked sample project, or an empty material ball if that fails. Texture and bake sizes are
/// kept small: the web build composites and bakes on one thread (bake 256 took ~12 s to start on a
/// fast desktop CPU, 128 ~7.5 s).
fn initial_session() -> Result<Session, String> {
    texturecraft_engine::sample_session(1024, 128)
        .or_else(|e| {
            log::warn!("sample project failed: {e}");
            Session::new_project("Untitled", texturecraft_engine::geom::primitives::material_ball(), "builtin:material_ball", None, 1024)
        })
        .map_err(|e| e.to_string())
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}
