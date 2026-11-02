//! A fill channel takes an image resource from its source menu.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use texturecraft_engine::core::{Channel, LayerId, LayerKind, Source};
use texturecraft_ui_egui::TextureCraftApp;

/// The Roughness channel's name in Properties (the rightmost "Roughness" label).
fn roughness<'a>(h: &'a Harness<'_, TextureCraftApp>) -> egui_kittest::Node<'a> {
    h.get_all_by_label("Roughness").max_by(|a, b| a.rect().right().total_cmp(&b.rect().right())).unwrap()
}

#[test]
fn a_fill_channel_takes_an_image_from_its_source_menu() {
    let mut session = texturecraft_engine::sample_session(256, 64).unwrap();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([90, 90, 90, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let name = session.import_resource_bytes("rough.png", png).unwrap();
    let out = texturecraft_engine::execute(&mut session, "layer.add.fill", &json!({"roughness": 0.6})).unwrap();
    let layer = LayerId(out["layer"].as_u64().unwrap());
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1400.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    // The labels below are English; a first run would speak the system's language.
    h.state_mut().ui.prefs.language = "en".into();
    h.run_steps(4);

    // The source menu button sits in the row of the channel's name, scrolled into view first.
    roughness(&h).scroll_to_me();
    // The scroll animates for about twenty frames.
    h.run_steps(30);
    let row = roughness(&h).rect().center().y;
    let menu = h.get_all_by_label(egui_phosphor::regular::DOTS_THREE).find(|n| (n.rect().center().y - row).abs() < 6.0).unwrap();
    menu.click();
    h.run_steps(2);
    h.get_by_label(&name).click();
    h.run_steps(3);

    let LayerKind::Fill(fill) = &h.state().session.doc.project.layers[&layer].kind else { panic!("a fill layer") };
    assert_eq!(fill.source(Channel::Roughness), Source::Image { name });
}
