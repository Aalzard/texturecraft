//! The dock host: every side panel (assets, texture set list, layers, texture set settings,
//! properties, display/shader settings, history, log) is a tab that can be docked, tabbed,
//! resized, floated and closed around the viewport. craft-ui's shared docking
//! (`craft_ui::docking`) draws the frames, tab strips, floating windows and drop previews and
//! owns the gestures; this module names the panels, builds the default arrangement, applies
//! the dock's changes and hosts the panel bodies. The bodies live in `dock.rs`, `shelf.rs` and
//! `windows.rs`.
//!
//! The viewport is the central canvas: it stays docked, never takes tabs and is the only panel
//! that cannot close or float. The layout persists with `UiState` as a craft-ui `Workspace`;
//! a layout that does not load or validate becomes the default, and a layout saved before the
//! shared docking (`{main, windows, closed}`) is converted.

use std::collections::HashSet;

use craft_ui::docking::{Action, DockArea, DockContent, DockStyle, Floating, Layout, Location, Node, PanelLimits, PanelState, Permissions, Placement, Zone};
use craft_ui::layout::{SplitAxis, SplitSize};
use egui::{Id, Margin, RichText, Ui, vec2};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TextureCraftApp;
use crate::i18n::tr;
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Panel {
    Viewport,
    Assets,
    TextureSetList,
    Layers,
    TextureSetSettings,
    Properties,
    Display,
    Shader,
    History,
    Log,
}

/// Window ▸ Views items and the panels they show or hide.
pub const VIEWS: [(&str, Panel); 9] = [
    ("ui.view.assets", Panel::Assets),
    ("ui.view.display", Panel::Display),
    ("ui.view.history", Panel::History),
    ("ui.view.layers", Panel::Layers),
    ("ui.view.log", Panel::Log),
    ("ui.view.properties", Panel::Properties),
    ("ui.view.shader", Panel::Shader),
    ("ui.view.textureSetList", Panel::TextureSetList),
    ("ui.view.textureSetSettings", Panel::TextureSetSettings),
];

/// The panel a Window ▸ Views item toggles.
pub fn view_panel(id: &str) -> Option<Panel> {
    VIEWS.iter().find(|(v, _)| *v == id).map(|(_, p)| *p)
}

impl Panel {
    pub const ALL: [Panel; 10] = [
        Panel::Viewport,
        Panel::Assets,
        Panel::TextureSetList,
        Panel::Layers,
        Panel::TextureSetSettings,
        Panel::Properties,
        Panel::Display,
        Panel::Shader,
        Panel::History,
        Panel::Log,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Panel::Viewport => "Viewport",
            Panel::Assets => "Assets",
            Panel::TextureSetList => "Texture Set List",
            Panel::Layers => "Layers",
            Panel::TextureSetSettings => "Texture Set Settings",
            Panel::Properties => "Properties",
            Panel::Display => "Display Settings",
            Panel::Shader => "Shader Settings",
            Panel::History => "History",
            Panel::Log => "Log",
        }
    }

    /// The panel named `name` in a saved layout.
    fn parse(name: &str) -> Option<Panel> {
        serde_json::from_value(Value::String(name.to_owned())).ok()
    }

    /// Where the panel floats when it has never been laid out: `[x, y, width, height]`, cascaded.
    fn float_rect(self) -> Option<[f32; 4]> {
        let slot = match self {
            Panel::Display => 0.0,
            Panel::Shader => 1.0,
            Panel::History => 2.0,
            Panel::Log => 3.0,
            _ => return None,
        };
        Some([960.0, 90.0 + 50.0 * slot, 300.0, 380.0])
    }
}

/// The saved panel arrangement: craft-ui's workspace over TextureCraft's panels.
pub type Workspace = craft_ui::docking::Workspace<Panel>;

/// The viewport never moves, floats, closes or takes tabs; panels dock beside it.
pub fn permissions(panel: &Panel) -> Permissions {
    if *panel == Panel::Viewport { Permissions { accepts_splits: true, ..Permissions::PROTECTED } } else { Permissions::default() }
}

fn tabs(panels: &[Panel]) -> Node<Panel> {
    Node::Tabs { panels: panels.to_vec(), active: 0 }
}

fn split(axis: SplitAxis, ratio: f32, first: Node<Panel>, second: Node<Panel>) -> Node<Panel> {
    Node::Split { axis, size: SplitSize::Ratio(ratio), first: Box::new(first), second: Box::new(second) }
}

/// The default: assets left of the viewport; texture set list, layers (tabbed with the texture
/// set settings) and properties down the right. Ratios match the 1600x960 window. Display,
/// shader, history and log are closed and open as floating windows.
pub fn default_workspace() -> Workspace {
    use Panel::*;
    let column =
        split(SplitAxis::Vertical, 0.125, tabs(&[TextureSetList]), split(SplitAxis::Vertical, 0.57, tabs(&[Layers, TextureSetSettings]), tabs(&[Properties])));
    let root = split(SplitAxis::Horizontal, 0.115, tabs(&[Assets]), split(SplitAxis::Horizontal, 0.78, tabs(&[Viewport]), column));
    let mut workspace = Workspace::new(Layout { root: Some(root), floating: Vec::new() });
    for panel in Panel::ALL {
        if let Some(rect) = panel.float_rect() {
            let location = Location {
                floating: Some(rect),
                anchor: None,
                placement: Placement::Tab { before: None },
                size: None,
                stack: None,
                sibling: Vec::new(),
                group: None,
            };
            workspace.hidden.push((panel, location));
        }
    }
    workspace
}

/// craft-ui's checks plus the viewport's: a saved or hand-edited file cannot hide, float, group
/// or collapse it.
pub fn validate(workspace: &Workspace) -> Result<(), String> {
    workspace.validate().map_err(|error| error.to_string())?;
    let layout = &workspace.layout;
    if layout.floating.iter().any(|window| window.contains(&Panel::Viewport)) {
        return Err("The viewport cannot float".into());
    }
    if workspace.hidden.iter().any(|(panel, _)| *panel == Panel::Viewport) {
        return Err("The viewport cannot be hidden".into());
    }
    let mut found = false;
    let mut nodes: Vec<&Node<Panel>> = layout.root.iter().collect();
    while let Some(node) = nodes.pop() {
        match node {
            Node::Split { first, second, .. } => {
                nodes.push(first);
                nodes.push(second);
            }
            Node::Tabs { panels, .. } if panels.contains(&Panel::Viewport) => {
                if panels.len() != 1 {
                    return Err("The viewport cannot share a tab group".into());
                }
                found = true;
            }
            Node::Stack { entries } if entries.iter().any(|entry| entry.panel == Panel::Viewport) => {
                return Err("The viewport cannot be collapsed".into());
            }
            Node::Dock { columns, .. } if columns.iter().flat_map(|column| &column.groups).any(|group| group.panels.contains(&Panel::Viewport)) => {
                return Err("The viewport cannot be in a dock".into());
            }
            _ => {}
        }
    }
    if found { Ok(()) } else { Err("The panel layout must contain the viewport".into()) }
}

// ---------------------------------------------------------------------------------------------
// Layouts saved before the shared docking

/// `{main, windows, closed}`: a tree of tab groups and splits, floating windows and where each
/// closed panel was. Panels are names, so a panel from a newer file is dropped, not an error.
#[derive(Deserialize)]
struct OldLayout {
    main: OldNode,
    #[serde(default)]
    windows: Vec<OldWindow>,
    #[serde(default)]
    closed: Vec<(String, OldPlace)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum OldNode {
    Tabs {
        tabs: Vec<String>,
        #[serde(default)]
        active: usize,
    },
    Split {
        vertical: bool,
        fraction: f32,
        first: Box<OldNode>,
        second: Box<OldNode>,
    },
}

#[derive(Deserialize)]
struct OldWindow {
    #[serde(default)]
    rect: Option<[f32; 4]>,
    root: OldNode,
}

#[derive(Deserialize)]
#[serde(default)]
struct OldPlace {
    with: Vec<String>,
    index: usize,
    beside: Vec<String>,
    vertical: bool,
    first: bool,
    fraction: f32,
    window: Option<[f32; 4]>,
}

impl Default for OldPlace {
    fn default() -> Self {
        OldPlace { with: Vec::new(), index: 0, beside: Vec::new(), vertical: false, first: false, fraction: 0.5, window: None }
    }
}

fn ratio(fraction: f32) -> f32 {
    if fraction.is_finite() { fraction.clamp(0.05, 0.95) } else { 0.5 }
}

fn rect_ok(rect: [f32; 4]) -> Option<[f32; 4]> {
    (rect.iter().all(|v| v.is_finite()) && rect[2] > 0.0 && rect[3] > 0.0).then_some(rect)
}

impl OldNode {
    /// The shared node: unknown and repeated panels (and the viewport outside the main area),
    /// empty groups and one-sided splits dropped, ratios clamped. A viewport that shared a group
    /// moves to a frame of its own beside it.
    fn convert(self, seen: &mut HashSet<Panel>, main: bool) -> Option<Node<Panel>> {
        match self {
            OldNode::Tabs { tabs: names, active } => {
                let mut panels = Vec::new();
                let mut front = None;
                for (i, name) in names.iter().enumerate() {
                    let Some(panel) = Panel::parse(name) else { continue };
                    if (panel == Panel::Viewport && !main) || !seen.insert(panel) {
                        continue;
                    }
                    if i == active {
                        front = Some(panels.len());
                    }
                    panels.push(panel);
                }
                let last = panels.len().checked_sub(1)?;
                let active = front.unwrap_or(0).min(last);
                match panels.iter().position(|p| *p == Panel::Viewport) {
                    Some(at) if panels.len() > 1 => {
                        panels.remove(at);
                        let active = active.min(panels.len() - 1);
                        Some(split(SplitAxis::Horizontal, 0.78, tabs(&[Panel::Viewport]), Node::Tabs { panels, active }))
                    }
                    _ => Some(Node::Tabs { panels, active }),
                }
            }
            OldNode::Split { vertical, fraction, first, second } => {
                let first = first.convert(seen, main);
                let second = second.convert(seen, main);
                match (first, second) {
                    (Some(a), Some(b)) => Some(split(if vertical { SplitAxis::Vertical } else { SplitAxis::Horizontal }, ratio(fraction), a, b)),
                    (a, b) => a.or(b),
                }
            }
        }
    }
}

impl OldPlace {
    /// Where the closed panel goes back: its tab group, a split beside the panels it was split
    /// off from, or its floating window. `None` when the file said nothing usable.
    fn location(self) -> Option<Location<Panel>> {
        let known = |names: &[String]| names.iter().filter_map(|n| Panel::parse(n)).filter(|p| *p != Panel::Viewport).collect::<Vec<_>>();
        let with = known(&self.with);
        let beside = known(&self.beside);
        let floating = self.window.and_then(rect_ok);
        let mut location =
            Location { floating, anchor: None, placement: Placement::Tab { before: None }, size: None, stack: None, sibling: Vec::new(), group: None };
        if let Some(anchor) = with.first() {
            location.anchor = Some(*anchor);
            location.placement = Placement::Tab { before: with.get(self.index).copied() };
        } else if let Some(anchor) = beside.first() {
            let zone = match (self.vertical, self.first) {
                (false, true) => Zone::Left,
                (false, false) => Zone::Right,
                (true, true) => Zone::Top,
                (true, false) => Zone::Bottom,
            };
            location.anchor = Some(*anchor);
            location.placement = Placement::Split(zone);
            location.size = Some(SplitSize::Ratio(ratio(self.fraction)));
            location.sibling = beside;
        } else if floating.is_none() {
            return None;
        }
        Some(location)
    }
}

impl OldLayout {
    fn convert(self) -> Workspace {
        let mut seen = HashSet::new();
        let root = self.main.convert(&mut seen, true);
        let floating = self
            .windows
            .into_iter()
            .filter_map(|window| {
                let root = window.root.convert(&mut seen, false)?;
                Some(Floating::new(root, window.rect.and_then(rect_ok).unwrap_or([960.0, 90.0, 300.0, 380.0])))
            })
            .collect();
        let mut workspace = Workspace::new(Layout { root, floating });
        let mut hidden: Vec<(Panel, Location<Panel>)> = Vec::new();
        for (name, place) in self.closed {
            let Some(panel) = Panel::parse(&name) else { continue };
            if panel == Panel::Viewport || seen.contains(&panel) || hidden.iter().any(|(p, _)| *p == panel) {
                continue;
            }
            if let Some(location) = place.location() {
                hidden.push((panel, location));
            }
        }
        // A panel the file neither shows nor lists was closed: it stays closed and reopens at its
        // default place.
        let default = default_workspace();
        for panel in Panel::ALL {
            let listed = seen.contains(&panel) || hidden.iter().any(|(p, _)| *p == panel);
            if panel != Panel::Viewport && !listed {
                let location = default.layout.location(&panel).ok().or_else(|| default.hidden.iter().find(|(p, _)| *p == panel).map(|(_, l)| l.clone()));
                hidden.extend(location.map(|l| (panel, l)));
            }
        }
        workspace.hidden = hidden;
        workspace
    }
}

// ---------------------------------------------------------------------------------------------
// The live dock

/// The dock layout (part of `UiState`; serialized as a craft-ui `Workspace`).
#[derive(Clone, Debug)]
pub struct Dock {
    workspace: Workspace,
    /// A panel to bring in front of the other floating windows on the next frame.
    raise: Option<Panel>,
}

impl Default for Dock {
    fn default() -> Self {
        Dock { workspace: default_workspace(), raise: None }
    }
}

impl PartialEq for Dock {
    fn eq(&self, other: &Self) -> bool {
        self.workspace == other.workspace
    }
}

impl Serialize for Dock {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.workspace.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Dock {
    /// Never fails: a layout that cannot be read or fails the viewport's rules is the default.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Dock::from_value(Value::deserialize(d)?))
    }
}

impl Dock {
    /// The dock for a saved layout, in the current or the earlier format.
    pub fn from_value(value: Value) -> Dock {
        let default = default_workspace();
        let loaded = if value.get("main").is_some() {
            serde_json::from_value::<OldLayout>(value).ok().map(OldLayout::convert)
        } else {
            serde_json::from_value::<Workspace>(value).ok()
        };
        let Some(mut workspace) = loaded.filter(|w| validate(w).is_ok()) else { return Dock::default() };
        workspace.reconcile(|_| true, permissions, &default);
        if validate(&workspace).is_err() {
            return Dock::default();
        }
        Dock { workspace, raise: None }
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn is_open(&self, p: Panel) -> bool {
        self.workspace.layout.contains(&p)
    }

    /// Open and the front tab of its group.
    pub fn is_visible(&self, p: Panel) -> bool {
        self.workspace.layout.panel_state(&p) == PanelState::Front
    }

    /// Hide a visible panel, otherwise bring it up.
    pub fn toggle(&mut self, p: Panel) {
        if self.is_visible(p) { self.close(p) } else { self.focus(p) }
    }

    pub fn close(&mut self, p: Panel) {
        if let Err(error) = self.workspace.set_visible(p, false, None, permissions) {
            log::warn!("panel {p:?} was not closed: {error}");
        }
    }

    /// Open `p` (where it was when closed, else where it lives by default) and bring it to the
    /// front.
    pub fn focus(&mut self, p: Panel) {
        let mut next = self.workspace.clone();
        match next.set_visible(p, true, None, permissions) {
            Ok(()) if validate(&next).is_ok() => {
                self.workspace = next;
                self.raise = Some(p);
            }
            Ok(()) => log::warn!("panel {p:?} was not opened: it would break the viewport's rules"),
            Err(error) => log::warn!("panel {p:?} was not opened: {error}"),
        }
    }

    /// One change from the dock area's gestures. A change that is rejected leaves the layout as
    /// it was.
    fn apply(&mut self, action: Action<Panel>) {
        let mut next = self.workspace.clone();
        match next.apply(action, permissions) {
            Ok(_) if validate(&next).is_ok() => self.workspace = next,
            Ok(_) => log::warn!("a panel move would break the viewport's rules"),
            Err(error) => log::warn!("a panel move was not applied: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing

fn area_id() -> Id {
    Id::unique("texturecraft-docking")
}

/// The dock look: every panel group is a card on the window's frame colour, its tab strip
/// without hard lines, the shown tab a pill, uppercase captions.
fn style(ui: &Ui, t: &Tokens) -> DockStyle {
    let mut s = DockStyle::from_ui(ui);
    s.gap = 6.0;
    // Smallest panel extent a divider drag leaves.
    s.min_pane = 60.0;
    s.background = t.panel;
    s.tab_background = t.panel;
    s.active_background = t.button;
    s.text = t.text_strong;
    s.inactive_text = t.text_dim;
    s.border = egui::Stroke::NONE;
    s.accent = t.accent;
    s.floating_background = t.panel;
    s.floating_stroke = egui::Stroke::new(1.0, t.button_border);
    s.font = egui::FontId::proportional(10.5);
    s.min_tab_width = 54.0;
    s.tab_height = 30.0;
    // Panels arrange in a split tree around the viewport; floating title bars only move windows.
    s.new_dock_zones = false;
    s.title_bar_docks = false;
    s
}

/// Draw the dock (viewport and panels) into `ui`.
pub fn show(app: &mut TextureCraftApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let tk = Tokens::get(&ctx);
    let style = style(ui, &tk);
    let layout = app.ui.dock.workspace.layout.clone();
    let mut area = DockArea::new(area_id()).input_blocked(ctx.memory(|m| m.top_modal_layer()).is_some());
    if let Some(panel) = app.ui.dock.raise.take() {
        area = area.raise_panel(panel);
    }
    let output = area.show_content(ui, &layout, &style, &mut Panels { app, tk });
    #[cfg(not(target_arch = "wasm32"))]
    crate::menus::forwarded_shortcuts(app, &ctx, output.forwarded_events, output.forwarded_modifiers.unwrap_or_default());
    if let Some(error) = &output.error {
        log::warn!("docking: {error}");
    }
    // A body that changed the layout (a panel opened from its own menu) wins over this frame's
    // gestures, which were made against the old one.
    if app.ui.dock.workspace.layout != layout {
        craft_ui::docking::cancel_drag::<Panel>(&ctx, area_id());
        return;
    }
    for action in output.actions {
        app.ui.dock.apply(action);
    }
}

/// The panels' contents, names and limits for the dock area.
struct Panels<'a> {
    app: &'a mut TextureCraftApp,
    tk: Tokens,
}

impl DockContent<Panel> for Panels<'_> {
    fn body(&mut self, ui: &mut Ui, panel: &Panel) {
        let app = &mut *self.app;
        if *panel == Panel::Viewport {
            ui.painter().rect_filled(ui.max_rect(), 0.0, self.tk.viewport);
            crate::viewport::central(app, ui);
            return;
        }
        egui::Frame::new().inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| match panel {
            Panel::Assets if app.ui.mode == crate::state::Mode::Baking => crate::windows::bake_body(app, ui),
            Panel::Assets => crate::shelf::body(app, ui),
            Panel::TextureSetList => crate::dock::texture_set_list(app, ui),
            Panel::Layers => crate::dock::layers(app, ui),
            Panel::TextureSetSettings => crate::dock::texture_set_settings(app, ui),
            Panel::Properties => crate::dock::properties(app, ui),
            // The other panels scroll their own lists.
            Panel::Display => {
                egui::ScrollArea::vertical().id_salt("dock-display").auto_shrink([false, false]).show(ui, |ui| crate::windows::display(app, ui)).inner
            }
            Panel::Shader => {
                egui::ScrollArea::vertical().id_salt("dock-shader").auto_shrink([false, false]).show(ui, |ui| crate::windows::shader(app, ui)).inner
            }
            Panel::History => crate::windows::history(app, ui),
            Panel::Log => crate::windows::log(app, ui),
            Panel::Viewport => {}
        });
    }

    fn logical_label(&self, panel: &Panel) -> String {
        match panel {
            Panel::Properties => crate::dock::properties_title(self.app),
            // Bake mode shows its settings where the shelf lives.
            Panel::Assets if self.app.ui.mode == crate::state::Mode::Baking => tr("Bake Mesh Maps").to_string(),
            p => tr(p.name()).to_string(),
        }
    }

    fn label(&mut self, _ui: &Ui, _panel: &Panel, logical: &str, font: &egui::FontId, color: egui::Color32) -> egui::WidgetText {
        RichText::new(logical.to_uppercase()).font(font.clone()).strong().color(color).into()
    }

    fn permissions(&self, panel: &Panel) -> Permissions {
        permissions(panel)
    }

    fn limits(&self, panel: &Panel) -> PanelLimits {
        let min = if *panel == Panel::Viewport { vec2(200.0, 150.0) } else { vec2(120.0, 60.0) };
        PanelLimits { min, ..PanelLimits::default() }
    }

    fn floating_size(&self, _panel: &Panel) -> Option<egui::Vec2> {
        Some(vec2(300.0, 380.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn panels_of(w: &Workspace) -> Vec<Panel> {
        w.layout.panels().into_iter().copied().collect()
    }

    /// A layout as the dock of egui_dock wrote it: the default arrangement, Layers closed from
    /// its tab group, Log in a floating window.
    fn saved_before_shared_docking() -> Value {
        json!({
            "main": {"split": {"vertical": false, "fraction": 0.115,
                "first": {"tabs": {"tabs": ["Assets"], "active": 0}},
                "second": {"split": {"vertical": false, "fraction": 0.78,
                    "first": {"tabs": {"tabs": ["Viewport"], "active": 0}},
                    "second": {"split": {"vertical": true, "fraction": 0.3,
                        "first": {"tabs": {"tabs": ["TextureSetList"], "active": 0}},
                        "second": {"tabs": {"tabs": ["TextureSetSettings", "Properties"], "active": 1}}}}}}}},
            "windows": [{"rect": [100.0, 120.0, 320.0, 400.0], "root": {"tabs": {"tabs": ["Log", "Bogus"], "active": 0}}}],
            "closed": [["Layers", {"with": ["TextureSetSettings", "Properties"], "index": 0}]]
        })
    }

    #[test]
    fn default_workspace_has_every_panel_and_is_valid() {
        let w = default_workspace();
        assert!(validate(&w).is_ok());
        for panel in Panel::ALL {
            assert!(w.layout.contains(&panel) || w.hidden.iter().any(|(p, _)| *p == panel), "{panel:?} is neither shown nor hidden");
        }
        assert_eq!(panels_of(&w), [Panel::Assets, Panel::Viewport, Panel::TextureSetList, Panel::Layers, Panel::TextureSetSettings, Panel::Properties]);
    }

    #[test]
    fn default_dock_round_trips() {
        let dock = Dock::default();
        let json = serde_json::to_string(&dock).unwrap();
        let back: Dock = serde_json::from_str(&json).unwrap();
        assert_eq!(back, dock);
    }

    #[test]
    fn a_layout_saved_before_the_shared_docking_loads() {
        let mut dock: Dock = serde_json::from_value(saved_before_shared_docking()).unwrap();
        let w = dock.workspace();
        assert!(validate(w).is_ok());
        // The tree is kept (Layers is closed), and the log keeps its window and rect.
        assert_eq!(panels_of(w), [Panel::Assets, Panel::Viewport, Panel::TextureSetList, Panel::TextureSetSettings, Panel::Properties, Panel::Log]);
        assert_eq!(w.layout.floating.len(), 1);
        assert_eq!(w.layout.floating[0].rect, [100.0, 120.0, 320.0, 400.0]);
        assert!(!dock.is_open(Panel::Layers) && dock.workspace().hidden.iter().any(|(p, _)| *p == Panel::Layers));
        // Panels the file never mentioned are hidden at their default place.
        assert!(!dock.is_open(Panel::Display) && dock.workspace().hidden.iter().any(|(p, _)| *p == Panel::Display));
        // Reopening Layers puts it back in its tab group, first.
        dock.focus(Panel::Layers);
        assert!(dock.is_visible(Panel::Layers));
        assert_eq!(dock.workspace().layout.floating.len(), 1);
        let mut nodes: Vec<&Node<Panel>> = dock.workspace().layout.root.iter().collect();
        let mut group = None;
        while let Some(node) = nodes.pop() {
            match node {
                Node::Split { first, second, .. } => nodes.extend([&**first, &**second]),
                Node::Tabs { panels, .. } if panels.contains(&Panel::Layers) => group = Some(panels.clone()),
                _ => {}
            }
        }
        assert_eq!(group.unwrap(), [Panel::Layers, Panel::TextureSetSettings, Panel::Properties]);
    }

    #[test]
    fn unreadable_layouts_become_the_default() {
        for text in [
            // No viewport at all.
            r#"{"main":{"tabs":{"tabs":["Layers"]}}}"#,
            // Not a layout.
            r#"{"main":7}"#,
            r#"[1,2]"#,
            r#""dock""#,
            // A shared workspace with a panel this build does not know.
            r#"{"version":2,"layout":{"root":{"Tabs":{"panels":["Bogus"],"active":0}},"floating":[]}}"#,
            // A shared workspace from a newer format.
            r#"{"version":99,"layout":{"root":{"Tabs":{"panels":["Viewport"],"active":0}},"floating":[]}}"#,
            // A viewport that floats.
            r#"{"version":2,"layout":{"root":{"Tabs":{"panels":["Assets"],"active":0}},"floating":[{"panels":["Viewport"],"active":0,"rect":[0,0,200,200]}]}}"#,
        ] {
            let dock: Dock = serde_json::from_str(text).unwrap();
            assert_eq!(dock, Dock::default(), "{text}");
        }
    }

    #[test]
    fn hostile_old_layouts_are_repaired() {
        // Duplicates, unknown panels, a viewport in a window, bad fractions and rects, empty groups.
        let json = r#"{"main":{"split":{"vertical":false,"fraction":0.3,"first":{"tabs":{"tabs":["Layers","Layers","Bogus"],"active":9}},
            "second":{"split":{"vertical":true,"fraction":7,"first":{"tabs":{"tabs":["Viewport"]}},"second":{"tabs":{"tabs":[]}}}}}},
            "windows":[{"rect":[0,0,-5,10],"root":{"tabs":{"tabs":["Viewport","Log","Layers"]}}}]}"#;
        let dock: Dock = serde_json::from_str(json).unwrap();
        let w = dock.workspace();
        assert!(validate(w).is_ok());
        assert_eq!(panels_of(w), [Panel::Layers, Panel::Viewport, Panel::Log]);
        assert_eq!(w.layout.floating.len(), 1);
    }

    #[test]
    fn the_viewport_stays_put() {
        let mut dock = Dock::default();
        dock.close(Panel::Viewport);
        assert!(dock.is_open(Panel::Viewport));
        let before = dock.clone();
        dock.apply(Action::Float { panel: Panel::Viewport, rect: [0.0, 0.0, 300.0, 300.0] });
        dock.apply(Action::Move { panel: Panel::Layers, anchor: Panel::Viewport, placement: Placement::Tab { before: None } });
        assert_eq!(dock, before);
    }

    #[test]
    fn closed_panels_come_back_where_they_were() {
        let mut dock = Dock::default();
        let before = dock.clone();
        for p in [Panel::Properties, Panel::Layers, Panel::Assets] {
            dock.close(p);
            assert!(!dock.is_open(p));
            // The places persist with the layout.
            dock = serde_json::from_str(&serde_json::to_string(&dock).unwrap()).unwrap();
            dock.focus(p);
            assert!(dock.is_visible(p), "{p:?}");
            assert_eq!(dock, before, "{p:?} came back elsewhere");
        }
    }

    #[test]
    fn closed_panels_without_a_place_open_floating() {
        let mut dock = Dock::default();
        dock.focus(Panel::Display);
        assert!(dock.is_visible(Panel::Display));
        assert_eq!(dock.workspace().layout.floating.len(), 1);
        assert!(dock.workspace().layout.floating[0].contains(&Panel::Display));
        // Hiding and showing the views one by one.
        dock.toggle(Panel::Display);
        assert!(!dock.is_open(Panel::Display));
        dock.toggle(Panel::TextureSetSettings);
        assert!(dock.is_visible(Panel::TextureSetSettings) && !dock.is_visible(Panel::Layers));
    }
}
