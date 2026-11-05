use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use texturecraft_engine::core::{BakeResolution, BakedMap, CageType, CurvatureMethod, HeightNormalization};
use texturecraft_ui_egui::TextureCraftApp;
use texturecraft_ui_egui::docking::Panel;
use texturecraft_ui_egui::state::Mode;

/// A bake-mode app with only `map` enabled and Mesh Map Settings in front
fn harness(map: Option<BakedMap>) -> Harness<'static, TextureCraftApp> {
    let mut session = texturecraft_engine::sample_session(256, 64).unwrap();
    session.doc.project.texture_sets[0].bake = Default::default();
    session.doc.project.texture_sets[0].bake.common.resolution = BakeResolution::R64;
    session.doc.project.texture_sets[0].bake.maps.select(map);
    let mut harness = Harness::builder().with_size(egui::vec2(1600.0, 1500.0)).build_eframe(|context| TextureCraftApp::new(context, session));
    harness.state_mut().ui.mode = Mode::Baking;
    harness.run_steps(4);
    harness.state_mut().ui.dock.focus(Panel::MeshMapSettings);
    harness.run_steps(3);
    harness
}

/// The maps the first texture set will bake
fn enabled_maps(harness: &Harness<'_, TextureCraftApp>) -> Vec<BakedMap> {
    harness.state().session.doc.project.texture_sets[0].bake.maps.enabled().collect()
}

/// Click a map's row in Mesh Map Bakers, which shows its settings
fn select_map(harness: &mut Harness<'_, TextureCraftApp>, name: &str) {
    harness.get_by_role_and_label(Role::Button, name).click();
    harness.run_steps(3);
}

/// Click a checkbox in Mesh Map Bakers: a map's row or Check All
fn click_checkbox(harness: &mut Harness<'_, TextureCraftApp>, name: &str) {
    harness.get_by_role_and_label(Role::CheckBox, name).click();
    harness.run_steps(3);
}

/// Scroll the widget labelled `label` into view and click it
fn click(harness: &mut Harness<'_, TextureCraftApp>, label: &str) {
    harness.get_by_label(label).scroll_to_me();
    harness.run_steps(3);
    harness.get_by_label(label).click();
    harness.run_steps(3);
}

#[test]
fn output_axes_unlock_independently_and_relock_to_width() {
    let mut harness = harness(None);
    harness.get_by_label("Texture Sets");
    harness.get_by_label("Mesh Map Settings");
    harness.state_mut().ui.dock.focus(Panel::CommonSettings);
    harness.run_steps(3);
    let width = harness.state().session.doc.project.texture_sets[0].bake.common.resolution.pixels().to_string();
    assert!(harness.get_all_by_value(&width).nth(1).unwrap().accesskit_node().is_disabled());

    click(&mut harness, "Lock Output Size");
    assert!(!harness.get_all_by_value(&width).nth(1).unwrap().accesskit_node().is_disabled());
    harness.get_all_by_value(&width).next().unwrap().click();
    harness.run_steps(3);
    click(&mut harness, "128");
    let settings = &harness.state().session.doc.project.texture_sets[0].bake.common;
    assert_eq!(settings.resolution, BakeResolution::R128);
    assert_eq!(settings.resolution_height.map(BakeResolution::pixels), Some(width.parse().unwrap()));

    click(&mut harness, "Lock Output Size");
    assert_eq!(harness.state().session.doc.project.texture_sets[0].bake.common.output_size(), (128, 128));
    assert!(harness.get_all_by_value("128").nth(1).unwrap().accesskit_node().is_disabled());
    harness.state_mut().ui.dock.focus(Panel::MeshMapSettings);
    harness.run_steps(3);
    harness.state_mut().ui.dock.focus(Panel::CommonSettings);
    harness.run_steps(3);
    assert!(harness.get_all_by_value("128").nth(1).unwrap().accesskit_node().is_disabled());

    harness.state_mut().ui.mode = Mode::Painting;
    harness.run_steps(3);
    assert!(!harness.state().ui.dock.is_open(Panel::CommonSettings));
    assert!(harness.state().ui.dock.is_open(Panel::Assets));
}

#[test]
fn height_controls_follow_manual_normalization_and_the_row_checkbox_enables_height() {
    let mut harness = harness(None);
    assert!(harness.query_by_label("Normalization").is_none());
    select_map(&mut harness, "Height");
    assert!(!harness.state().session.doc.project.texture_sets[0].bake.maps.height.is_enabled, "selecting is not enabling");
    assert!(harness.query_by_label("Scaling Divisor").is_none());
    click(&mut harness, "Normalization");
    click(&mut harness, "Manual");
    harness.get_by_label("Scaling Divisor");
    assert_eq!(harness.state().session.doc.project.texture_sets[0].bake.maps.height.parameters.normalization, HeightNormalization::Manual);

    click_checkbox(&mut harness, "Height");
    assert!(harness.state().session.doc.project.texture_sets[0].bake.maps.height.is_enabled);
    click(&mut harness, "Restore Defaults");
    assert!(harness.state().session.doc.project.texture_sets[0].bake.maps.height.is_enabled, "Restore Defaults keeps the map enabled");
    assert_eq!(harness.state().session.doc.project.texture_sets[0].bake.maps.height.parameters.normalization, HeightNormalization::RelativeToLowMesh);
    assert!(harness.query_by_label("Scaling Divisor").is_none());
    click_checkbox(&mut harness, "Height");
    assert!(!harness.state().session.doc.project.texture_sets[0].bake.maps.height.is_enabled);
}

#[test]
fn clicking_a_row_shows_that_maps_settings_without_changing_what_bakes() {
    let mut harness = harness(Some(BakedMap::Curvature));
    harness.state_mut().ui.dock.focus(Panel::CommonSettings);
    harness.run_steps(3);
    let before = enabled_maps(&harness);

    select_map(&mut harness, "Ambient Occlusion");
    assert!(harness.state().ui.dock.is_visible(Panel::MeshMapSettings), "selecting a map fronts its settings");
    harness.get_by_label("Attenuation");
    assert!(harness.query_by_label("Method").is_none());
    select_map(&mut harness, "Opacity");
    harness.get_by_label("No Parameters");
    assert!(harness.query_by_label("Attenuation").is_none());
    assert_eq!(enabled_maps(&harness), before);

    click(&mut harness, &format!("{} Common Settings", egui_phosphor::regular::GEAR));
    assert!(harness.state().ui.dock.is_visible(Panel::CommonSettings));
    assert_eq!(enabled_maps(&harness), before);
}

#[test]
fn check_all_enables_every_map_then_disables_them_all() {
    let mut harness = harness(Some(BakedMap::Curvature));
    click_checkbox(&mut harness, "Check All");
    assert_eq!(enabled_maps(&harness), BakedMap::ALL);
    click_checkbox(&mut harness, "Check All");
    assert!(enabled_maps(&harness).is_empty());
}

#[test]
fn the_breadcrumb_switches_the_shown_map() {
    let mut harness = harness(Some(BakedMap::Curvature));
    harness.get_by_label("Details");
    assert_eq!(harness.get_all_by_role_and_label(Role::Button, "Position").count(), 1, "only the list row before the menu opens");
    let dropdowns: Vec<_> = harness.get_all_by_label(egui_phosphor::regular::CARET_RIGHT).collect();
    let [_texture_set_dropdown, map_dropdown] = dropdowns.as_slice() else { panic!("the breadcrumb has a texture set and a map dropdown") };
    map_dropdown.click();
    harness.run_steps(3);
    assert_eq!(harness.get_all_by_role_and_label(Role::Button, "Position").count(), 2, "the map menu lists Position");
    // The menu draws its items after the list rows
    harness.get_all_by_role_and_label(Role::Button, "Position").last().unwrap().click();
    harness.run_steps(3);
    harness.get_by_label("Normalization Scale");
    assert!(harness.query_by_label("Details").is_none());
    assert_eq!(enabled_maps(&harness), [BakedMap::Curvature]);
}

#[test]
fn curvature_controls_follow_the_method_and_notes_visibility_rules() {
    let mut harness = harness(Some(BakedMap::Curvature));
    harness.get_by_label("Details");
    harness.get_by_label("Seams Intensity");
    click(&mut harness, "Enable Seams");
    assert!(harness.query_by_label("Seams Intensity").is_none());
    click(&mut harness, "Method");
    click(&mut harness, "Generate from Mesh");
    assert_eq!(harness.state().session.doc.project.texture_sets[0].bake.maps.curvature.parameters.method, CurvatureMethod::Mesh);
    assert!(harness.query_by_label("Details").is_none());
    harness.get_by_label("Sampling Radius");
    assert!(harness.query_by_label("Value Bounds Min").is_none());
    assert!(harness.query_by_label("Value Bounds Max").is_none());
    click(&mut harness, "Auto Tonemapping");
    harness.get_by_label("Value Bounds Min");
    harness.get_by_label("Value Bounds Max");
    assert!(!harness.state().session.doc.project.texture_sets[0].bake.maps.curvature.parameters.mesh.auto_tonemapping);
}

#[test]
fn cage_and_skew_controls_require_a_high_poly_and_follow_cage_type() {
    let mut harness = harness(None);
    harness.state_mut().ui.dock.focus(Panel::CommonSettings);
    harness.run_steps(3);
    assert!(harness.query_by_label("Cage Type").is_none());
    assert!(harness.query_by_label("Skew Correction").is_none());
    harness.state_mut().session.add_high_poly_bytes("High.obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n".to_vec()).unwrap();
    harness.run_steps(3);
    harness.get_by_label("Max Frontal Distance");
    harness.get_by_label("Skew Correction");
    click(&mut harness, "Cage Type");
    click(&mut harness, "Custom File");
    assert_eq!(harness.state().session.doc.project.texture_sets[0].bake.cage.kind, CageType::CustomFile);
    harness.get_by_label("Cage Scene");
    assert!(harness.query_by_label("Max Frontal Distance").is_none());
    assert!(harness.query_by_label("Skew Correction").is_none());
}
