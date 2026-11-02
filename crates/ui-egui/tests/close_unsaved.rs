//! Closing the app with unsaved changes asks whether to save them first.

use egui::{ViewportCommand, ViewportEvent, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use texturecraft_ui_egui::TextureCraftApp;

fn harness(modified: bool) -> Harness<'static, TextureCraftApp> {
    let mut session = texturecraft_engine::sample_session(256, 64).unwrap();
    session.modified = modified;
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 960.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.run_steps(4);
    h
}

/// The window's close button, for one frame.
fn request_close(h: &mut Harness<'_, TextureCraftApp>) {
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().events.push(ViewportEvent::Close);
    h.step();
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().events.clear();
}

/// The viewport commands of the last frame.
fn sent(h: &Harness<'_, TextureCraftApp>, cmd: &ViewportCommand) -> bool {
    h.output().viewport_output.get(&ViewportId::ROOT).is_some_and(|v| v.commands.contains(cmd))
}

#[test]
fn closing_with_changes_asks_and_dont_save_closes() {
    let mut h = harness(true);
    request_close(&mut h);
    assert!(sent(&h, &ViewportCommand::CancelClose), "the close waits for an answer");
    h.run_steps(2);
    assert!(h.state().ui.windows.confirm_close, "the dialog is open");
    h.get_by_label("Don't save").click();
    h.step();
    assert!(sent(&h, &ViewportCommand::Close), "Don't save closes the app");
    assert!(!h.state().ui.windows.confirm_close);
}

#[test]
fn cancel_keeps_the_app_open() {
    let mut h = harness(true);
    request_close(&mut h);
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.step();
    assert!(!sent(&h, &ViewportCommand::Close));
    assert!(!h.state().ui.windows.confirm_close, "Cancel closes the dialog");
    assert!(h.state().session.modified, "and keeps the changes");
}

#[test]
fn closing_without_changes_does_not_ask() {
    let mut h = harness(false);
    request_close(&mut h);
    assert!(!sent(&h, &ViewportCommand::CancelClose));
    assert!(!h.state().ui.windows.confirm_close);
}
