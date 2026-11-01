//! Compact widgets matching the app's look: labelled slim sliders, icon buttons, section
//! headers, channel toggle chips and color swatches.

use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use egui::{Color32, CornerRadius, Response, RichText, Sense, Stroke, Ui, vec2};

use crate::theme::Tokens;

/// The drag-and-drop payload of type `T` released on `resp` this frame. Unlike
/// [`Response::dnd_release_payload`] (which clears a payload of any type), it leaves payloads
/// of other types for the next drop target.
pub fn dnd_release<T: std::any::Any + Send + Sync>(resp: &Response) -> Option<std::sync::Arc<T>> {
    resp.dnd_hover_payload::<T>().and_then(|_| resp.dnd_release_payload::<T>())
}

/// A labelled slider in two rows: "Label ........ value" over a slim track with a round knob.
/// Returns the response of the track (`.changed()` when dragged, `.drag_stopped()` at the end).
pub fn slider(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, decimals: usize) -> Response {
    let tk = Tokens::get(ui.ctx());
    let (lo, hi) = (*range.start(), *range.end());
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(tk.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut text = format!("{:.*}", decimals, value);
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(44.0).align(egui::Align2::RIGHT_CENTER).frame(egui::Frame::NONE));
            if r.lost_focus()
                && let Ok(v) = text.trim().parse::<f32>()
                && v.is_finite()
            {
                *value = v.clamp(lo, hi);
            }
        });
    });
    let (rect, mut resp) = ui.allocate_exact_size(vec2(w, 14.0), Sense::click_and_drag());
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let t = ((p.x - rect.left() - 5.0) / (rect.width() - 10.0).max(1.0)).clamp(0.0, 1.0);
        let v = lo + (hi - lo) * t;
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            resp.mark_changed();
        }
    }
    let t = if hi > lo { ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
    track(ui, &resp, rect.left() + 5.0..=rect.right() - 5.0, rect.center().y, t, 2.0, 4.5);
    resp
}

/// A small inline slider for the top tool bar: "Label value" above a short track.
pub fn bar_slider(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, width: f32) -> Response {
    let tk = Tokens::get(ui.ctx());
    let (lo, hi) = (*range.start(), *range.end());
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click_and_drag());
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let t = ((p.x - rect.left() - 4.0) / (rect.width() - 8.0).max(1.0)).clamp(0.0, 1.0);
        let v = lo + (hi - lo) * t;
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            resp.mark_changed();
        }
    }
    let p = ui.painter();
    let font = egui::FontId::proportional(11.0);
    p.text(rect.left_top() + vec2(2.0, 2.0), egui::Align2::LEFT_TOP, label, font.clone(), tk.text);
    p.text(rect.right_top() + vec2(-2.0, 2.0), egui::Align2::RIGHT_TOP, format!("{:.0}", value), font, tk.text_strong);
    let t = if hi > lo { ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
    track(ui, &resp, rect.left() + 4.0..=rect.right() - 4.0, rect.bottom() - 7.0, t, 1.5, 4.0);
    resp
}

/// A slider track from `x` at height `y`: the rail, the accent up to fraction `t` and a round knob.
fn track(ui: &Ui, resp: &Response, x: std::ops::RangeInclusive<f32>, y: f32, t: f32, width: f32, knob: f32) {
    let tk = Tokens::get(ui.ctx());
    let p = ui.painter();
    let (x0, x1) = (*x.start(), *x.end());
    let xk = x0 + (x1 - x0) * t;
    p.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], Stroke::new(width, tk.button_border));
    p.line_segment([egui::pos2(x0, y), egui::pos2(xk, y)], Stroke::new(width, tk.accent));
    let ring = if resp.hovered() || resp.dragged() { tk.text_strong } else { tk.text };
    p.circle(egui::pos2(xk, y), knob, tk.panel, Stroke::new(1.5, ring));
}

/// One line of `text` cut with "…" to fit `max_width`, for list rows that shrink with their panel.
pub fn elided(painter: &egui::Painter, text: impl Into<String>, size: f32, color: Color32, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), egui::FontId::proportional(size), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    painter.layout_job(job)
}

/// A square icon button (Phosphor glyph) in a rounded tool well: `selected` sits in the accent
/// tinted well (the shared Craft icon button, `craft_ui::buttons`).
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str, selected: bool, size: f32) -> Response {
    let tk = Tokens::get(ui.ctx());
    let well = |fill, icon| ButtonVisuals { fill, stroke: Stroke::NONE, icon };
    let style = IconButtonStyle {
        normal: well(Color32::TRANSPARENT, tk.text),
        hovered: well(tk.row_hover, tk.text_strong),
        pressed: Some(well(tk.tool_active, tk.text_strong)),
        selected: well(tk.tool_active, tk.text_strong),
        corner_radius: CornerRadius::same(6),
        focus_stroke: Stroke::new(1.0, tk.accent),
    };
    IconButton::new(tooltip, vec2(size, size), &style)
        .selected(selected)
        .show(ui, |ui, rect, color| {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(size * 0.62), color);
        })
        .on_hover_text(tooltip)
}

/// A flat button filled with the accent, white lettering (a dialog's main action).
pub fn primary_button<'a>(ui: &Ui, text: &str) -> egui::Button<'a> {
    let tk = Tokens::get(ui.ctx());
    egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(tk.accent_strong).stroke(Stroke::NONE)
}

/// A labelled dropdown on one row: the label, then the box filling the rest of the row, so a
/// narrow panel shortens the box (its text truncated) instead of pushing the row past its edge.
pub fn combo(ui: &mut Ui, label: &str, selected: impl Into<egui::WidgetText>, menu: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(label)
            .width(ui.available_width().max(60.0))
            .wrap_mode(egui::TextWrapMode::Truncate)
            .selected_text(selected)
            .show_ui(ui, menu);
    });
}

/// A collapsible section header with a chevron ("> Size").
pub fn section(ui: &mut Ui, id: &str, title: &str, default_open: bool, body: impl FnOnce(&mut Ui)) {
    let tk = Tokens::get(ui.ctx());
    let id = ui.make_persistent_id(id);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
    let open = state.is_open();
    let resp = ui
        .horizontal(|ui| {
            let chev = if open { egui_phosphor::regular::CARET_DOWN } else { egui_phosphor::regular::CARET_RIGHT };
            ui.label(RichText::new(chev).size(10.0).color(tk.text_dim));
            ui.label(RichText::new(title).strong().color(tk.text_strong));
        })
        .response
        .interact(Sense::click());
    if resp.clicked() {
        state.toggle(ui);
    }
    state.show_body_unindented(ui, |ui| {
        ui.indent(id.with("in"), body);
    });
}

/// A channel chip ("color", "metal", "rough"…): toggled = the accent-tinted well.
pub fn chip(ui: &mut Ui, text: &str, on: bool) -> Response {
    let tk = Tokens::get(ui.ctx());
    let galley = ui.painter().layout_no_wrap(text.into(), egui::FontId::proportional(11.0), tk.text);
    let size = vec2(galley.size().x + 14.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let fill = if on {
        tk.tool_active
    } else if resp.hovered() {
        tk.row_hover
    } else {
        tk.row
    };
    p.rect_filled(rect, 5.0, fill);
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(11.0), if on { tk.text_strong } else { tk.text_dim });
    resp
}

/// A color swatch button for a linear RGB value; opens a picker on click. Returns true when
/// the color changed.
pub fn color_swatch(ui: &mut Ui, id: &str, linear: &mut [f32; 3], width: f32) -> bool {
    let tk = Tokens::get(ui.ctx());
    let srgb = linear.map(|v| (texturecraft_engine::texel::linear_to_srgb(v) * 255.0 + 0.5) as u8);
    let mut c = Color32::from_rgb(srgb[0], srgb[1], srgb[2]);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click());
    ui.painter().rect(rect, 4.0, c, Stroke::new(1.0, tk.field_border), egui::StrokeKind::Inside);
    let popup = ui.make_persistent_id(id);
    if resp.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    let mut changed = false;
    // Open state lives in memory (`toggle_id` above); `from_response` alone shows it always.
    egui::Popup::from_response(&resp).id(popup).open_memory(None).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
            let lin = [c.r(), c.g(), c.b()].map(|v| texturecraft_engine::texel::srgb_to_linear(v as f32 / 255.0));
            *linear = lin;
            changed = true;
        }
    });
    changed
}

/// Panel title row with optional trailing content.
pub fn panel_header(ui: &mut Ui, title: &str, trailing: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.label(crate::theme::caption(title));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
    });
}
