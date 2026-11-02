//! A fill channel takes an image resource from its source menu.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use texturecraft_engine::core::{Channel, LayerId, LayerKind, Source};
use texturecraft_ui_egui::TextureCraftApp;

#[test]
fn a_fill_channel_takes_an_image_from_its_source_menu() {
    let mut session = texturecraft_engine::sample_session(256, 64).unwrap();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([90, 90, 90, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let name = session.import_resource_bytes("rough.png", png).unwrap();
    let out = texturecraft_engine::execute(&mut session, "layer.add.fill", &json!({"roughness": 0.6})).unwrap();
    let layer = LayerId(out["layer"].as_u64().unwrap());
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1400.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.run_steps(4);

    // The source menu button sits in the row of the channel's name.
    let row = h.get_all_by_label("Roughness").map(|n| n.rect()).max_by(|a, b| a.right().total_cmp(&b.right())).unwrap().center().y;
    let menu = h.get_all_by_label(egui_phosphor::regular::DOTS_THREE).find(|n| (n.rect().center().y - row).abs() < 6.0).unwrap();
    menu.click();
    h.run_steps(2);
    h.get_by_label(&name).click();
    h.run_steps(3);

    let LayerKind::Fill(fill) = &h.state().session.doc.project.layers[&layer].kind else { panic!("a fill layer") };
    assert_eq!(fill.source(Channel::Roughness), Source::Image { name });
}
