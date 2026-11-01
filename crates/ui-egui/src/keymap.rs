//! Keyboard shortcuts: every menu item and every command with a default shortcut, the user's
//! overrides (`UiState::keymap`, id → "Ctrl+Shift+E" or "" for unbound), conflict detection and
//! the shortcut editor window (Edit ▸ Settings ▸ Keyboard, Help ▸ Shortcuts list).

use std::collections::BTreeMap;

use egui::{Event, Id, Key, KeyboardShortcut, Modifiers, RichText, vec2};
use egui_phosphor::regular as ic;

use crate::TextureCraftApp;
use crate::i18n::{tr, trf};
use crate::menus::{CATALOG, parse_shortcut};
use crate::theme::Tokens;
use crate::widgets::{icon_button, section};

pub type Keymap = BTreeMap<String, String>;

/// Something a shortcut can trigger.
pub struct Binding {
    pub id: &'static str,
    /// Menu (or command group) it belongs to.
    pub group: &'static str,
    pub label: &'static str,
    pub default: Option<&'static str>,
}

/// Every bindable item: the menu catalog, then commands with a default shortcut.
pub fn bindings() -> &'static [Binding] {
    static ALL: std::sync::OnceLock<Vec<Binding>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut v: Vec<Binding> = Vec::new();
        for i in CATALOG.iter().filter(|i| i.label != "-") {
            if !v.iter().any(|b| b.id == i.id) {
                v.push(Binding { id: i.id, group: i.menu, label: i.label, default: i.shortcut });
            }
        }
        for c in texturecraft_engine::command_specs().iter().filter(|c| c.shortcut.is_some()) {
            if !v.iter().any(|b| b.id == c.id) {
                v.push(Binding { id: c.id, group: c.menu.first().copied().unwrap_or("Commands"), label: c.label, default: c.shortcut });
            }
        }
        v
    })
}

/// The shortcut `b` currently has.
pub fn shortcut<'a>(keymap: &'a Keymap, b: &'a Binding) -> Option<&'a str> {
    match keymap.get(b.id) {
        Some(s) => (!s.is_empty()).then_some(s.as_str()),
        None => b.default,
    }
}

/// The shortcut of item `id` (for menu labels and tooltips).
pub fn shortcut_of<'a>(keymap: &'a Keymap, id: &str) -> Option<&'a str> {
    bindings().iter().find(|b| b.id == id).and_then(|b| shortcut(keymap, b))
}

/// "Label (Shortcut)" for an icon button's tooltip.
pub fn tip(keymap: &Keymap, label: &str, id: &str) -> String {
    match shortcut_of(keymap, id) {
        Some(s) => format!("{label} ({s})"),
        None => label.to_owned(),
    }
}

/// "Ctrl+Shift+E" notation (the inverse of `menus::parse_shortcut`).
pub fn format_shortcut(sc: &KeyboardShortcut) -> String {
    let m = sc.modifiers;
    let mut s = String::new();
    if m.command || m.ctrl || m.mac_cmd {
        s.push_str("Ctrl+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    s.push_str(sc.logical_key.name());
    s
}

fn modifier_count(m: Modifiers) -> u8 {
    u8::from(m.command || m.ctrl || m.mac_cmd) + u8::from(m.shift) + u8::from(m.alt)
}

/// Every bound shortcut with its item id, most modifiers first: egui ignores extra Shift and
/// Alt when matching, so Shift+C has to be tried before C.
pub fn active(keymap: &Keymap) -> Vec<(&'static str, KeyboardShortcut)> {
    let mut v: Vec<(&'static str, KeyboardShortcut)> = bindings().iter().filter_map(|b| Some((b.id, parse_shortcut(shortcut(keymap, b)?)?))).collect();
    v.sort_by_key(|(_, sc)| std::cmp::Reverse(modifier_count(sc.modifiers)));
    v
}

/// Each binding's parsed shortcut, in `bindings()` order.
pub fn resolve(keymap: &Keymap) -> Vec<Option<KeyboardShortcut>> {
    bindings().iter().map(|b| shortcut(keymap, b).and_then(parse_shortcut)).collect()
}

/// Another binding with the same shortcut as binding `i`.
pub fn conflict(resolved: &[Option<KeyboardShortcut>], i: usize) -> Option<usize> {
    let sc = resolved.get(i).copied().flatten()?;
    resolved.iter().enumerate().position(|(j, o)| j != i && *o == Some(sc))
}

/// Bind `b` to `value` ("" = no shortcut); the default is stored as no override.
pub fn rebind(keymap: &mut Keymap, b: &Binding, value: &str) {
    if b.default.unwrap_or("") == value {
        keymap.remove(b.id);
    } else {
        keymap.insert(b.id.to_owned(), value.to_owned());
    }
}

/// The bound shortcuts the browser must not act on, for the web build's `keydown` listener:
/// the keymap they were taken from and the shortcuts.
static CAPTURED: std::sync::Mutex<Option<(Keymap, Vec<KeyboardShortcut>)>> = std::sync::Mutex::new(None);

/// Refresh [`captured`] when the keymap changed (once per frame; cheap when it didn't).
pub fn sync_captured(keymap: &Keymap) {
    let mut c = CAPTURED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if c.as_ref().is_some_and(|(k, _)| k == keymap) {
        return;
    }
    // Copy, cut and paste stay with the browser: egui reads them as clipboard events.
    let clipboard = [Key::C, Key::V, Key::X];
    let keys: Vec<KeyboardShortcut> = active(keymap)
        .into_iter()
        .map(|(_, sc)| sc)
        .filter(|sc| {
            let fkey = sc.logical_key.name().strip_prefix('F').is_some_and(|n| n.parse::<u8>().is_ok());
            (sc.modifiers.command || sc.modifiers.alt || fkey) && !(sc.modifiers.command && !sc.modifiers.shift && clipboard.contains(&sc.logical_key))
        })
        .collect();
    log::debug!("browser shortcuts taken over: {}", keys.len());
    *c = Some((keymap.clone(), keys));
}

/// Whether a key press is one of the app's shortcuts the browser must leave alone.
pub fn captured(mods: Modifiers, key: Key) -> bool {
    let c = CAPTURED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    c.as_ref().is_some_and(|(_, keys)| keys.iter().any(|sc| sc.logical_key == key && mods.matches_logically(sc.modifiers)))
}

const CAPTURE: &str = "keymap_capture";

/// Whether the editor is waiting for a key combination (global shortcuts pause meanwhile).
pub fn capturing(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<usize>(Id::unique(CAPTURE))).is_some()
}

/// The first key press this frame as a shortcut.
fn pressed(ctx: &egui::Context) -> Option<KeyboardShortcut> {
    ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            Event::Key { key, pressed: true, modifiers, .. } => Some(KeyboardShortcut::new(*modifiers, *key)),
            _ => None,
        })
    })
}

/// The shortcut editor window.
pub fn window(app: &mut TextureCraftApp, ctx: &egui::Context) {
    let tk = Tokens::get(ctx);
    if !app.ui.windows.shortcuts {
        return;
    }
    let capture_id = Id::unique(CAPTURE);
    let filter_id = Id::unique("keymap_filter");
    let mut capture: Option<usize> = ctx.data(|d| d.get_temp(capture_id));
    if let (Some(i), Some(sc)) = (capture, pressed(ctx)) {
        if (sc.logical_key != Key::Escape || !sc.modifiers.is_none())
            && let Some(b) = bindings().get(i)
        {
            rebind(&mut app.ui.keymap, b, &format_shortcut(&sc));
        }
        capture = None;
    }
    let mut filter: String = ctx.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    let mut open = true;
    crate::windows::window("keyboard_shortcuts", tr("Keyboard shortcuts"), &mut open).default_width(480.0).show(ctx, |ui| {
        let resolved = resolve(&app.ui.keymap);
        ui.horizontal(|ui| {
            ui.label(RichText::new(ic::MAGNIFYING_GLASS).color(tk.text_dim));
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text(tr("Search commands or keys")).desired_width(300.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!app.ui.keymap.is_empty(), egui::Button::new(tr("Reset all"))).on_hover_text(tr("Restore every default shortcut")).clicked() {
                    app.ui.keymap.clear();
                }
            });
        });
        let clashes = (0..resolved.len()).filter(|i| conflict(&resolved, *i).is_some()).count();
        if clashes > 0 {
            ui.label(
                RichText::new(format!("{} {}", ic::WARNING, trf("{n} items share a shortcut; the first one listed wins.", &[("n", &clashes)])))
                    .color(tk.danger),
            );
        }
        ui.add_space(4.0);
        let needle = filter.to_lowercase();
        egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("keymap_grid").num_columns(3).striped(true).spacing(vec2(10.0, 4.0)).show(ui, |ui| {
                for (i, b) in bindings().iter().enumerate() {
                    let current = shortcut(&app.ui.keymap, b);
                    if !needle.is_empty()
                        && ![b.label, b.group, tr(b.label), tr(b.group), b.id, current.unwrap_or("")].iter().any(|t| t.to_lowercase().contains(&needle))
                    {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        ui.label(tr(b.label));
                        ui.label(RichText::new(tr(b.group)).size(10.5).color(tk.text_dim));
                    });
                    let waiting = capture == Some(i);
                    let text = if waiting { tr("Press a key...") } else { current.unwrap_or("-") };
                    let key = egui::Button::new(RichText::new(text).monospace()).selected(waiting).min_size(vec2(130.0, 0.0));
                    if ui.add(key).on_hover_text(tr("Click, then press the new shortcut (Esc cancels)")).clicked() {
                        capture = if waiting { None } else { Some(i) };
                    }
                    let bound = current.is_some();
                    ui.horizontal(|ui| {
                        if let Some(other) = conflict(&resolved, i).and_then(|j| bindings().get(j)) {
                            ui.label(RichText::new(ic::WARNING).color(tk.danger))
                                .on_hover_text(trf("Also bound to {item} ({menu})", &[("item", &tr(other.label)), ("menu", &tr(other.group))]));
                        }
                        if bound && icon_button(ui, ic::X, tr("Remove the shortcut"), false, 18.0).clicked() {
                            rebind(&mut app.ui.keymap, b, "");
                        }
                        if app.ui.keymap.contains_key(b.id)
                            && icon_button(
                                ui,
                                ic::ARROW_COUNTER_CLOCKWISE,
                                &trf("Reset to {shortcut}", &[("shortcut", &b.default.unwrap_or(tr("none")))]),
                                false,
                                18.0,
                            )
                            .clicked()
                        {
                            app.ui.keymap.remove(b.id);
                        }
                    });
                    ui.end_row();
                }
            });
        });
        ui.add_space(4.0);
        section(ui, "keymap_nav", tr("Viewport and brush (fixed)"), false, |ui| {
            egui::Grid::new("keymap_fixed").striped(true).show(ui, |ui| {
                for (k, v) in [
                    (tr("Alt + LMB drag"), tr("Orbit")),
                    (tr("MMB drag / Alt + MMB"), tr("Pan")),
                    (tr("Wheel / Alt + RMB drag"), tr("Zoom")),
                    (tr("LMB"), tr("Paint with the current tool")),
                    ("[ / ]", tr("Brush size down / up")),
                    (tr("Shift + wheel"), tr("Brush size")),
                    ("Esc", tr("Cancel the stroke")),
                ] {
                    ui.label(RichText::new(k).strong());
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
    });
    app.ui.windows.shortcuts = open;
    if !open {
        capture = None;
    }
    ctx.data_mut(|d| {
        d.insert_temp(filter_id, filter);
        match capture {
            Some(i) => {
                d.insert_temp(capture_id, i);
            }
            None => {
                d.remove::<usize>(capture_id);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_leaves_app_shortcuts_alone_but_keeps_the_clipboard() {
        let mut keymap = Keymap::new();
        keymap.insert("file.close".into(), "Alt+W".into());
        sync_captured(&keymap);
        let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
        assert!(captured(ctrl, Key::S) && captured(ctrl, Key::O) && captured(ctrl, Key::D));
        assert!(captured(Modifiers::NONE, Key::F8) && captured(Modifiers::ALT, Key::W));
        assert!(!captured(ctrl, Key::C) && !captured(ctrl, Key::V) && !captured(ctrl, Key::X), "clipboard keys reach the browser");
        assert!(!captured(Modifiers::NONE, Key::M) && !captured(Modifiers::NONE, Key::F), "plain keys need no capture");
    }

    #[test]
    fn shortcuts_round_trip_through_parse_and_format() {
        for b in bindings() {
            if let Some(s) = b.default {
                let sc = parse_shortcut(s).unwrap_or_else(|| panic!("{} has an unparsable shortcut {s}", b.id));
                assert_eq!(format_shortcut(&sc), s);
            }
        }
        for s in ["Ctrl+Shift+Alt+F12", "Alt+OpenBracket", "Shift+Delete", "Ctrl+9"] {
            assert_eq!(parse_shortcut(s).map(|sc| format_shortcut(&sc)).as_deref(), Some(s));
        }
        assert!(parse_shortcut("Ctrl+").is_none() && parse_shortcut("Ctrl+NoSuchKey").is_none());
    }

    #[test]
    fn default_shortcuts_do_not_conflict() {
        let resolved = resolve(&Keymap::new());
        for (i, b) in bindings().iter().enumerate() {
            assert!(conflict(&resolved, i).is_none(), "{} shares its default shortcut", b.id);
        }
    }

    #[test]
    fn rebinding_detects_conflicts_and_resets() {
        let all = bindings();
        let save = all.iter().position(|b| b.id == "file.save").unwrap();
        let undo = all.iter().position(|b| b.id == "edit.undo").unwrap();
        let mut keymap = Keymap::new();
        rebind(&mut keymap, &all[save], "Ctrl+Z");
        let resolved = resolve(&keymap);
        assert_eq!(conflict(&resolved, save), Some(undo));
        assert_eq!(conflict(&resolved, undo), Some(save));
        assert_eq!(shortcut_of(&keymap, "file.save"), Some("Ctrl+Z"));
        // Unbinding clears the conflict; binding back to the default drops the override.
        rebind(&mut keymap, &all[save], "");
        assert_eq!(shortcut_of(&keymap, "file.save"), None);
        assert!(conflict(&resolve(&keymap), undo).is_none());
        rebind(&mut keymap, &all[save], "Ctrl+S");
        assert!(keymap.is_empty());
    }

    #[test]
    fn specific_shortcuts_are_tried_first() {
        let active = active(&Keymap::new());
        let pos = |id: &str| active.iter().position(|(x, _)| *x == id).unwrap();
        assert!(pos("viewport.previousChannel") < pos("viewport.nextChannel"));
        assert!(pos("file.saveAs") < pos("file.save"));
    }
}
