//! Design tokens and egui styling: a dense, very dark professional theme (our own palette and
//! widgets; no third-party assets).

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

/// Colors and metrics used across the UI. Never hard-code colors elsewhere.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    /// Menu bar, title strip, tool shelves.
    pub chrome: Color32,
    /// Dock panel backgrounds.
    pub panel: Color32,
    /// Panel title rows ("LAYERS", "PROPERTIES").
    pub header: Color32,
    /// List rows (layers, texture sets).
    pub row: Color32,
    pub row_hover: Color32,
    pub row_selected: Color32,
    /// Separators between panels.
    pub separator: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_strong: Color32,
    /// Selection and focus.
    pub accent: Color32,
    pub accent_dim: Color32,
    pub field: Color32,
    pub danger: Color32,
    pub viewport: Color32,
    pub radius: u8,
}

pub const TOKENS: Tokens = Tokens {
    chrome: Color32::from_rgb(0x1c, 0x1c, 0x1c),
    panel: Color32::from_rgb(0x24, 0x24, 0x24),
    header: Color32::from_rgb(0x24, 0x24, 0x24),
    row: Color32::from_rgb(0x2a, 0x2a, 0x2a),
    row_hover: Color32::from_rgb(0x33, 0x33, 0x33),
    row_selected: Color32::from_rgb(0x2c, 0x34, 0x40),
    separator: Color32::from_rgb(0x14, 0x14, 0x14),
    text: Color32::from_rgb(0xd4, 0xd4, 0xd4),
    text_dim: Color32::from_rgb(0x96, 0x96, 0x96),
    text_strong: Color32::from_rgb(0xf2, 0xf2, 0xf2),
    accent: Color32::from_rgb(0x2f, 0x86, 0xeb),
    accent_dim: Color32::from_rgb(0x1f, 0x4f, 0x8a),
    field: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    danger: Color32::from_rgb(0xe0, 0x55, 0x4a),
    viewport: Color32::from_rgb(0x2a, 0x2a, 0x2a),
    radius: 3,
};

/// The app's fonts: egui's defaults and the Phosphor icon font (`fonts::UiFonts` adds CJK).
pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    fonts
}

/// Install the fonts and the dark style.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
    let t = TOKENS;
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::new(10.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(11.5, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(6.0, 2.0);
        s.spacing.interact_size = egui::vec2(36.0, 20.0);
        s.spacing.slider_width = 120.0;
        s.spacing.menu_margin = egui::Margin::same(4);
        s.spacing.window_margin = egui::Margin::same(8);
        let mut v = Visuals::dark();
        v.panel_fill = t.panel;
        v.window_fill = t.panel;
        v.extreme_bg_color = t.field;
        v.faint_bg_color = t.row;
        v.code_bg_color = t.field;
        v.window_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x3a));
        v.window_corner_radius = CornerRadius::same(4);
        v.menu_corner_radius = CornerRadius::same(3);
        v.selection.bg_fill = t.accent_dim;
        v.selection.stroke = Stroke::new(1.0, t.accent);
        v.hyperlink_color = t.accent;
        let r = CornerRadius::same(t.radius);
        v.widgets.noninteractive.bg_fill = t.panel;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.separator);
        v.widgets.inactive.bg_fill = Color32::from_rgb(0x33, 0x33, 0x33);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x30, 0x30, 0x30);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
        v.widgets.inactive.corner_radius = r;
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x40, 0x40, 0x40);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x3b, 0x3b, 0x3b);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text_strong);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x55, 0x55, 0x55));
        v.widgets.hovered.corner_radius = r;
        v.widgets.active.bg_fill = Color32::from_rgb(0x4a, 0x4a, 0x4a);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(0x45, 0x45, 0x45);
        v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.active.corner_radius = r;
        v.widgets.open.bg_fill = Color32::from_rgb(0x3a, 0x3a, 0x3a);
        v.widgets.open.corner_radius = r;
        v.slider_trailing_fill = true;
        v.striped = false;
        s.visuals = v;
    });
}

/// Painter-style uppercase panel caption.
pub fn caption(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).size(10.5).strong().color(TOKENS.text_strong)
}
