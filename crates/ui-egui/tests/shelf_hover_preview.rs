//! Hovering a shelf tile shows a big preview of the asset over its name, rendered once the pointer
//! rests on the tile.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use texturecraft_engine::materials::{MATERIALS, SmartMaterial};
use texturecraft_ui_egui::TextureCraftApp;
use texturecraft_ui_egui::shelf_preview::ShelfPreview;

/// The shelf's Materials tab.
const MATERIALS_TAB: usize = 0;
/// The hover preview's size in points (pixels at this scale).
const BIG_PREVIEW_SIZE: f32 = 256.0;
/// Simulated seconds per frame, fine enough to stop between the rest and the popup's delay.
const STEP_SECONDS: f32 = 0.001;
/// The popup's delay (egui's tooltip delay) and the rest after which the big preview renders.
const POPUP_DELAY: f32 = 0.5;
const REST: f32 = 0.1;
/// Real time the worker gets to render.
const WORKER_TIMEOUT: Duration = Duration::from_secs(60);

/// The app on the Materials tab, `step_seconds` per frame, and two materials whose names show
/// nowhere else.
fn materials_shelf(step_seconds: f32) -> (Harness<'static, TextureCraftApp>, Vec<&'static SmartMaterial>) {
    let session = texturecraft_engine::sample_session(256, 64).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 960.0)).with_step_dt(step_seconds).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.state_mut().ui.shelf_tab = MATERIALS_TAB;
    h.run_steps(4);

    let unique: Vec<_> = MATERIALS.iter().filter(|m| !m.smart && h.query_all_by_label(m.label).count() == 1).take(2).collect();
    assert_eq!(unique.len(), 2, "two materials with unique names");
    (h, unique)
}

/// The pixel size of the largest texture (not the font atlas) the last frame drew at least
/// `min_width` points wide.
fn largest_drawn_texture(h: &Harness<'_, TextureCraftApp>, min_width: f32) -> Option<[usize; 2]> {
    let textures = h.ctx.tex_manager();
    let textures = textures.read();
    h.output()
        .shapes
        .iter()
        .filter(|clipped| clipped.shape.texture_id() != egui::TextureId::default() && clipped.shape.visual_bounding_rect().width() >= min_width)
        .filter_map(|clipped| textures.meta(clipped.shape.texture_id()).map(|meta| meta.size))
        .max()
}

/// Whether `material`'s big preview has arrived from the worker (asks for nothing).
fn big_preview_ready(h: &mut Harness<'static, TextureCraftApp>, material: &SmartMaterial) -> bool {
    let ctx = h.ctx.clone();
    h.state_mut().thumbs.rendered_big(&ctx, &ShelfPreview::Material(material.id), BIG_PREVIEW_SIZE as usize).is_some()
}

#[test]
fn hovering_a_shelf_tile_shows_its_big_preview() {
    let (mut h, materials) = materials_shelf(1.0 / 4.0);
    let material = materials[0];
    let tile = h.get_by_label(material.label).rect();
    assert!(largest_drawn_texture(&h, BIG_PREVIEW_SIZE).is_none(), "nothing big before the hover");
    h.get_by_label(material.label).hover();

    // The popup's delay, then the worker
    let big_px = BIG_PREVIEW_SIZE as usize;
    let start = Instant::now();
    while largest_drawn_texture(&h, BIG_PREVIEW_SIZE) != Some([big_px, big_px]) && start.elapsed() < WORKER_TIMEOUT {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(largest_drawn_texture(&h, BIG_PREVIEW_SIZE), Some([big_px, big_px]), "the popup draws the big preview");
    // The popup's name sits under the big image, below the tile
    let popup_name = h.get_all_by_label(material.label).map(|n| n.rect()).find(|r| r.min.y > tile.max.y).expect("the popup shows the name");
    assert!(popup_name.min.y - tile.max.y >= BIG_PREVIEW_SIZE, "{popup_name:?} under {tile:?}");
}

#[test]
fn the_big_preview_renders_after_a_rest_but_not_during_a_sweep() {
    let (mut h, materials) = materials_shelf(STEP_SECONDS);
    let (first, second) = (materials[0], materials[1]);
    let (from, to) = (h.get_by_label(first.label).rect().center(), h.get_by_label(second.label).rect().center());

    // A sweep: the pointer moves every frame across both tiles, then leaves the shelf
    let sweep_frames = 40;
    for frame in 0..=sweep_frames {
        h.hover_at(from.lerp(to, frame as f32 / sweep_frames as f32));
        h.run_steps(1);
    }
    h.hover_at(egui::pos2(900.0, 500.0));
    h.run_steps(1);
    std::thread::sleep(Duration::from_secs(1));
    h.run_steps(1);
    assert!(!big_preview_ready(&mut h, first) && !big_preview_ready(&mut h, second), "a sweep renders nothing");

    // A rest: the preview renders before the popup opens
    h.hover_at(from);
    h.run_steps((REST * 1.2 / STEP_SECONDS) as usize);
    let start = Instant::now();
    let headroom_frames = ((POPUP_DELAY - REST * 1.2) / STEP_SECONDS) as usize - 1;
    for _ in 0..headroom_frames {
        if big_preview_ready(&mut h, first) || start.elapsed() > WORKER_TIMEOUT {
            break;
        }
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(big_preview_ready(&mut h, first), "the preview rendered during the popup's delay");
    assert_eq!(h.query_all_by_label(first.label).count(), 1, "the popup is not open yet");
}

#[test]
fn high_dpi_smart_mask_hover_stays_within_the_renderer_limit() {
    use texturecraft_engine::mask_preview::MAX_PREVIEW_SIZE_PX;
    use texturecraft_engine::smart_masks::SMART_MASKS;

    let (mut harness, _) = materials_shelf(1.0 / 4.0);
    harness.ctx.set_pixels_per_point(3.0);
    harness.state_mut().ui.shelf_tab = 2;
    harness.run_steps(4);
    harness.get_by_label(SMART_MASKS[0].label).hover();

    let expected = Some([MAX_PREVIEW_SIZE_PX; 2]);
    let started = Instant::now();
    while largest_drawn_texture(&harness, BIG_PREVIEW_SIZE) != expected && started.elapsed() < WORKER_TIMEOUT {
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(largest_drawn_texture(&harness, BIG_PREVIEW_SIZE), expected, "the popup uses a bounded big preview at 300% scale");
}
