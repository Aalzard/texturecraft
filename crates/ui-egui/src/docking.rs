//! The dock host: every side panel (assets, texture set list, layers, texture set settings,
//! properties, display/shader settings, history, log) is a tab that can be docked, tabbed,
//! resized, floated and closed around the viewport (`egui_dock`). The panel bodies live in
//! `dock.rs`, `shelf.rs` and `windows.rs`; this module only hosts them.
//!
//! The layout persists with `UiState` as a small validated tree (`Layout`), not as the dock
//! crate's internal arrays, so a hand-edited or stale layout can never break the dock.

use egui::{CornerRadius, Id, Margin, Rect, RichText, Stroke, Ui, WidgetText, pos2, vec2};
use egui_dock::{DockArea, DockState, LeafNode, NodeIndex, Split, Style, TabIndex, TabStyle, TabViewer, Tree};
use serde::{Deserialize, Serialize};

use crate::TextureCraftApp;
use crate::i18n::tr;
use crate::theme::TOKENS;

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
    /// A panel from a newer layout file; dropped on load.
    #[serde(other)]
    Unknown,
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

/// Where a closed panel comes back.
enum Home {
    /// Left of the viewport.
    Left,
    /// The right column, top (0) to bottom (2).
    Column(u8),
    /// A floating window, cascaded by index.
    Float(u8),
    Fixed,
}

impl Panel {
    const COUNT: usize = Panel::Unknown as usize + 1;

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
            Panel::Unknown => "",
        }
    }

    fn home(self) -> Home {
        match self {
            Panel::Assets => Home::Left,
            Panel::TextureSetList => Home::Column(0),
            Panel::Layers | Panel::TextureSetSettings => Home::Column(1),
            Panel::Properties => Home::Column(2),
            Panel::Display => Home::Float(0),
            Panel::Shader => Home::Float(1),
            Panel::History => Home::Float(2),
            Panel::Log => Home::Float(3),
            Panel::Viewport | Panel::Unknown => Home::Fixed,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Persisted layout

/// A dock tree: a group of tabs, or two subtrees side by side (`vertical`: one above the other).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Node {
    Tabs {
        tabs: Vec<Panel>,
        #[serde(default)]
        active: usize,
    },
    Split {
        vertical: bool,
        /// Share of the first (left or top) subtree, 0..1.
        fraction: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// A floating window: screen rect `[x, y, width, height]` and its tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Floating {
    #[serde(default)]
    pub rect: Option<[f32; 4]>,
    pub root: Node,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub main: Node,
    #[serde(default)]
    pub windows: Vec<Floating>,
}

fn tabs(tabs: &[Panel]) -> Node {
    Node::Tabs { tabs: tabs.to_vec(), active: 0 }
}

fn split(vertical: bool, fraction: f32, first: Node, second: Node) -> Node {
    Node::Split { vertical, fraction, first: Box::new(first), second: Box::new(second) }
}

impl Default for Layout {
    /// Painter's default: assets left of the viewport; texture set list, layers (tabbed with the
    /// texture set settings) and properties down the right. Fractions match the 1600x960 window.
    fn default() -> Self {
        use Panel::*;
        let column = split(true, 0.125, tabs(&[TextureSetList]), split(true, 0.57, tabs(&[Layers, TextureSetSettings]), tabs(&[Properties])));
        Layout { main: split(false, 0.115, tabs(&[Assets]), split(false, 0.78, tabs(&[Viewport]), column)), windows: Vec::new() }
    }
}

/// Deeper trees are flattened into one tab group (the dock's node array doubles per level).
const MAX_DEPTH: usize = 8;

impl Node {
    fn collect(self, out: &mut Vec<Panel>) {
        match self {
            Node::Tabs { tabs, .. } => out.extend(tabs),
            Node::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    /// Drop unknown and repeated panels (and the viewport outside the main area), empty groups
    /// and one-sided splits; clamp fractions; flatten too-deep trees.
    fn sanitize(self, seen: &mut [bool; Panel::COUNT], depth: usize, main: bool) -> Option<Node> {
        match self {
            Node::Split { .. } if depth >= MAX_DEPTH => {
                let mut all = Vec::new();
                self.collect(&mut all);
                Node::Tabs { tabs: all, active: 0 }.sanitize(seen, depth, main)
            }
            Node::Tabs { tabs, active } => {
                let tabs: Vec<Panel> = tabs
                    .into_iter()
                    .filter(|p| *p != Panel::Unknown && (main || *p != Panel::Viewport) && !std::mem::replace(&mut seen[*p as usize], true))
                    .collect();
                let last = tabs.len().checked_sub(1)?;
                Some(Node::Tabs { tabs, active: active.min(last) })
            }
            Node::Split { vertical, fraction, first, second } => {
                let first = first.sanitize(seen, depth + 1, main);
                let second = second.sanitize(seen, depth + 1, main);
                match (first, second) {
                    (Some(a), Some(b)) => Some(split(vertical, if fraction.is_finite() { fraction.clamp(0.05, 0.95) } else { 0.5 }, a, b)),
                    (a, b) => a.or(b),
                }
            }
        }
    }

    /// Write this (sanitized) tree into `tree` at `at`, which must hold a leaf.
    fn place(self, tree: &mut Tree<Panel>, at: NodeIndex) {
        match self {
            Node::Tabs { tabs, active } => {
                let mut leaf = LeafNode::new(tabs);
                leaf.active = TabIndex(active);
                tree[at] = egui_dock::Node::Leaf(leaf);
            }
            Node::Split { vertical, fraction, first, second } => {
                // The placeholder tab is overwritten by `second`.
                let [a, b] = tree.split(at, if vertical { Split::Below } else { Split::Right }, fraction, egui_dock::Node::leaf(Panel::Unknown));
                first.place(tree, a);
                second.place(tree, b);
            }
        }
    }

    fn read(nodes: &[&egui_dock::Node<Panel>], i: usize) -> Option<Node> {
        let (vertical, s) = match nodes.get(i)? {
            egui_dock::Node::Empty => return None,
            egui_dock::Node::Leaf(l) => return (!l.tabs.is_empty()).then(|| Node::Tabs { tabs: l.tabs.clone(), active: l.active.0 }),
            egui_dock::Node::Vertical(s) => (true, s),
            egui_dock::Node::Horizontal(s) => (false, s),
        };
        let first = Node::read(nodes, i.saturating_mul(2).saturating_add(1));
        let second = Node::read(nodes, i.saturating_mul(2).saturating_add(2));
        match (first, second) {
            (Some(a), Some(b)) => Some(split(vertical, if s.fraction.is_finite() { s.fraction } else { 0.5 }, a, b)),
            (a, b) => a.or(b),
        }
    }

    fn of(tree: &Tree<Panel>) -> Option<Node> {
        Node::read(&tree.iter().collect::<Vec<_>>(), 0)
    }
}

impl Layout {
    /// The live dock for this layout, or `None` when it has no viewport.
    fn build(self) -> Option<DockState<Panel>> {
        let mut seen = [false; Panel::COUNT];
        let main = self.main.sanitize(&mut seen, 0, true)?;
        if !seen[Panel::Viewport as usize] {
            return None;
        }
        let mut state = DockState::new(Vec::new());
        main.place(state.main_surface_mut(), NodeIndex::root());
        for w in self.windows {
            let Some(root) = w.root.sanitize(&mut seen, 0, false) else { continue };
            let s = state.add_window(vec![Panel::Unknown]);
            if let Some(tree) = state.get_surface_mut(s).and_then(|x| x.node_tree_mut()) {
                root.place(tree, NodeIndex::root());
            }
            if let (Some([x, y, width, height]), Some(ws)) = (w.rect, state.get_window_state_mut(s))
                && [x, y, width, height].iter().all(|v| v.is_finite())
                && width > 0.0
                && height > 0.0
            {
                ws.set_position(pos2(x, y)).set_size(vec2(width, height));
            }
        }
        Some(state)
    }

    fn of(state: &DockState<Panel>) -> Layout {
        let Some(main) = Node::of(state.main_surface()) else { return Layout::default() };
        let windows = state
            .iter_surfaces()
            .filter_map(|s| match s {
                egui_dock::Surface::Window(tree, ws) => {
                    let r = ws.rect();
                    let rect = (r.is_finite() && r.is_positive()).then(|| [r.min.x, r.min.y, r.width(), r.height()]);
                    Node::of(tree).map(|root| Floating { rect, root })
                }
                _ => None,
            })
            .collect();
        Layout { main, windows }
    }
}

// ---------------------------------------------------------------------------------------------
// The live dock

/// The dock layout (part of `UiState`; serialized as a `Layout`). Empty only while the dock is
/// being drawn.
#[derive(Clone, Debug)]
pub struct Dock(Option<DockState<Panel>>);

impl Default for Dock {
    fn default() -> Self {
        Dock(Layout::default().build())
    }
}

impl PartialEq for Dock {
    fn eq(&self, other: &Self) -> bool {
        self.layout() == other.layout()
    }
}

impl Serialize for Dock {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.layout().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Dock {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Layout::deserialize(d)?.build().map_or_else(Dock::default, |s| Dock(Some(s))))
    }
}

impl Dock {
    pub fn layout(&self) -> Layout {
        self.0.as_ref().map(Layout::of).unwrap_or_default()
    }

    pub fn is_open(&self, p: Panel) -> bool {
        self.0.as_ref().is_some_and(|s| s.find_tab(&p).is_some())
    }

    /// Open and the front tab of its group.
    pub fn is_visible(&self, p: Panel) -> bool {
        let Some(s) = &self.0 else { return false };
        s.find_tab(&p).is_some_and(|path| s.leaf(path.node_path()).is_ok_and(|l| l.active == path.tab))
    }

    /// Hide a visible panel, otherwise bring it up (`screen` places floating windows).
    pub fn toggle(&mut self, p: Panel, screen: Rect) {
        if self.is_visible(p) { self.close(p) } else { self.focus(p, screen) }
    }

    pub fn close(&mut self, p: Panel) {
        if p == Panel::Viewport {
            return;
        }
        if let Some(s) = &mut self.0
            && let Some(path) = s.find_tab(&p)
        {
            s.remove_tab(path);
        }
    }

    /// Open `p` where it lives by default (if closed) and bring it to the front.
    pub fn focus(&mut self, p: Panel, screen: Rect) {
        let Some(s) = &mut self.0 else { return };
        if s.find_tab(&p).is_none() {
            insert(s, p, screen);
        }
        if let Some(path) = s.find_tab(&p) {
            let _ = s.set_active_tab(path);
            s.set_focused_node_and_surface(path.node_path());
        }
    }
}

fn insert(state: &mut DockState<Panel>, p: Panel, screen: Rect) {
    match p.home() {
        Home::Fixed => {}
        Home::Float(k) => {
            let s = state.add_window(vec![p]);
            if let Some(w) = state.get_window_state_mut(s) {
                w.set_position(screen.right_top() + vec2(-640.0, 90.0 + 50.0 * k as f32)).set_size(vec2(300.0, 380.0));
            }
        }
        Home::Left => beside_viewport(state.main_surface_mut(), Split::Left, 0.13, p),
        Home::Column(order) => {
            let tree = state.main_surface_mut();
            // The right-column group nearest to the panel's slot (its own slot first).
            let mut best: Option<(NodeIndex, u8)> = None;
            for (i, n) in tree.iter().enumerate() {
                let slot = n.get_leaf().and_then(|l| {
                    l.tabs.iter().find_map(|t| match t.home() {
                        Home::Column(o) => Some(o),
                        _ => None,
                    })
                });
                if let Some(o) = slot
                    && best.is_none_or(|(_, b)| o.abs_diff(order) < b.abs_diff(order))
                {
                    best = Some((NodeIndex(i), o));
                }
            }
            match best {
                Some((node, o)) if o == order => {
                    if let Ok(leaf) = tree.leaf_mut(node) {
                        leaf.append_tab(p);
                    }
                }
                Some((node, o)) if o > order => {
                    tree.split_above(node, 0.3, vec![p]);
                }
                Some((node, _)) => {
                    tree.split_below(node, 0.6, vec![p]);
                }
                None => beside_viewport(tree, Split::Right, 0.78, p),
            }
        }
    }
}

/// Split the viewport's group; `fraction` is the left (or top) share.
fn beside_viewport(tree: &mut Tree<Panel>, side: Split, fraction: f32, p: Panel) {
    match tree.find_tab(&Panel::Viewport) {
        Some((node, _)) => {
            tree.split(node, side, fraction, egui_dock::Node::leaf(p));
        }
        None => tree.push_to_first_leaf(p),
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing

struct Host<'a> {
    app: &'a mut TextureCraftApp,
}

impl TabViewer for Host<'_> {
    type Tab = Panel;

    fn id(&mut self, tab: &mut Panel) -> Id {
        Id::new(("panel", *tab))
    }

    fn title(&mut self, tab: &mut Panel) -> WidgetText {
        let text = match tab {
            Panel::Properties => crate::dock::properties_title(self.app),
            // Bake mode shows its settings where the shelf lives, as Painter does.
            Panel::Assets if self.app.ui.mode == crate::state::Mode::Baking => tr("Bake Mesh Maps").to_string(),
            p => tr(p.name()).to_string(),
        };
        RichText::new(text.to_uppercase()).size(10.5).strong().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Panel) {
        let app = &mut *self.app;
        match tab {
            Panel::Viewport => crate::viewport::central(app, ui),
            Panel::Assets if app.ui.mode == crate::state::Mode::Baking => crate::windows::bake_body(app, ui),
            Panel::Assets => crate::shelf::body(app, ui),
            Panel::TextureSetList => crate::dock::texture_set_list(app, ui),
            Panel::Layers => crate::dock::layers(app, ui),
            Panel::TextureSetSettings => crate::dock::texture_set_settings(app, ui),
            Panel::Properties => crate::dock::properties(app, ui),
            Panel::Display => crate::windows::display(app, ui),
            Panel::Shader => crate::windows::shader(app, ui),
            Panel::History => crate::windows::history(app, ui),
            Panel::Log => crate::windows::log(app, ui),
            Panel::Unknown => {}
        }
    }

    fn is_closeable(&self, tab: &Panel) -> bool {
        *tab != Panel::Viewport
    }

    fn allowed_in_windows(&self, tab: &mut Panel) -> bool {
        *tab != Panel::Viewport
    }

    fn scroll_bars(&self, tab: &Panel) -> [bool; 2] {
        // The other panels scroll their own lists.
        [false, matches!(tab, Panel::Display | Panel::Shader)]
    }

    fn tab_style_override(&self, tab: &Panel, global: &TabStyle) -> Option<TabStyle> {
        (*tab == Panel::Viewport).then(|| {
            let mut s = global.clone();
            s.tab_body.inner_margin = Margin::ZERO;
            s.tab_body.bg_fill = TOKENS.viewport;
            s
        })
    }
}

/// The dock look: flat dark tab bars, Painter-style uppercase captions, thin separators.
fn style(egui_style: &egui::Style) -> Style {
    let t = TOKENS;
    let mut s = Style::from_egui(egui_style);
    s.separator.width = 2.0;
    // Smallest panel extent a separator drag leaves (the crate's 175 px would grow the list).
    s.separator.extra = 60.0;
    s.separator.color_idle = t.separator;
    s.separator.color_hovered = t.accent_dim;
    s.separator.color_dragged = t.accent;
    s.tab_bar.bg_fill = t.chrome;
    s.tab_bar.height = 24.0;
    s.tab_bar.hline_color = t.separator;
    s.tab_bar.corner_radius = CornerRadius::ZERO;
    for (tab, text) in [
        (&mut s.tab.active, t.text_strong),
        (&mut s.tab.focused, t.text_strong),
        (&mut s.tab.active_with_kb_focus, t.text_strong),
        (&mut s.tab.focused_with_kb_focus, t.text_strong),
        (&mut s.tab.hovered, t.text),
        (&mut s.tab.inactive, t.text_dim),
        (&mut s.tab.inactive_with_kb_focus, t.text_dim),
    ] {
        tab.text_color = text;
        tab.corner_radius = CornerRadius::ZERO;
        tab.bg_fill = if text == t.text_strong { t.panel } else { t.chrome };
        tab.outline_color = tab.bg_fill;
    }
    let body = &mut s.tab.tab_body;
    body.bg_fill = t.panel;
    body.inner_margin = Margin::symmetric(6, 4);
    body.stroke = Stroke::NONE;
    body.corner_radius = CornerRadius::ZERO;
    // The viewport's group hides its tab bar entirely: no drag strip, no "show tab bar" button.
    body.hidden_tab_bar_drag_height = Some(0.0);
    s.buttons.show_tab_bar_size = 0.0;
    s.buttons.show_tab_bar_hover_expand = 0.0;
    s.buttons.show_tab_bar_color = egui::Color32::TRANSPARENT;
    s.buttons.show_tab_bar_active_color = egui::Color32::TRANSPARENT;
    s.overlay.selection_color = t.accent.gamma_multiply(0.35);
    s
}

/// Draw the dock (viewport and panels) into `ui`.
pub fn show(app: &mut TextureCraftApp, ui: &mut Ui) {
    let Some(mut state) = app.ui.dock.0.take() else {
        app.ui.dock = Dock::default();
        return;
    };
    // Only the viewport alone in its group hides the tab bar.
    for (_, leaf) in state.iter_leaves_mut() {
        leaf.tab_bar_hidden = leaf.tabs == [Panel::Viewport];
    }
    DockArea::new(&mut state)
        .id(Id::new("dock_area"))
        .style(style(ui.style()))
        .hidable_tab_bars(true)
        .tab_context_menus(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .show_secondary_button_hint(false)
        .show_inside(ui, &mut Host { app });
    // A layout adopted while drawing (Window ▸ Load UI Layout) wins over the drawn one.
    if app.ui.dock.0.is_none() {
        app.ui.dock.0 = Some(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_round_trips() {
        let dock = Dock::default();
        assert_eq!(dock.layout(), Layout::default());
        let json = serde_json::to_string(&dock).unwrap();
        let back: Dock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.layout(), Layout::default());
    }

    #[test]
    fn hostile_layouts_are_repaired() {
        // Duplicates, unknown panels, a viewport in a window, bad fractions and rects, empty groups.
        let json = r#"{"main":{"split":{"vertical":false,"fraction":0.3,"first":{"tabs":{"tabs":["Layers","Layers","Bogus"],"active":9}},
            "second":{"split":{"vertical":true,"fraction":7,"first":{"tabs":{"tabs":["Viewport"]}},"second":{"tabs":{"tabs":[]}}}}}},
            "windows":[{"rect":[0,0,-5,10],"root":{"tabs":{"tabs":["Viewport","Log","Layers"]}}}]}"#;
        let dock: Dock = serde_json::from_str(json).unwrap();
        let l = dock.layout();
        assert_eq!(l.main, split(false, 0.3, Node::Tabs { tabs: vec![Panel::Layers], active: 0 }, tabs(&[Panel::Viewport])));
        assert_eq!(l.windows.len(), 1);
        assert_eq!(l.windows[0].root, tabs(&[Panel::Log]));
        // No viewport at all: the default layout.
        let none: Dock = serde_json::from_str(r#"{"main":{"tabs":{"tabs":["Layers"]}}}"#).unwrap();
        assert_eq!(none.layout(), Layout::default());
    }

    #[test]
    fn deep_trees_are_flattened() {
        let mut node = tabs(&[Panel::Viewport]);
        for p in [
            Panel::Assets,
            Panel::Layers,
            Panel::Log,
            Panel::History,
            Panel::Shader,
            Panel::Display,
            Panel::Properties,
            Panel::TextureSetList,
            Panel::TextureSetSettings,
        ] {
            node = split(true, 0.5, tabs(&[p]), node);
        }
        let dock = Dock(Layout { main: node, windows: Vec::new() }.build());
        let mut all = Vec::new();
        dock.layout().main.collect(&mut all);
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn closed_panels_come_back_home() {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 960.0));
        let mut dock = Dock::default();
        assert!(dock.is_visible(Panel::Layers) && !dock.is_visible(Panel::TextureSetSettings));
        dock.toggle(Panel::TextureSetSettings, screen);
        assert!(dock.is_visible(Panel::TextureSetSettings) && !dock.is_visible(Panel::Layers));
        for p in [Panel::Assets, Panel::TextureSetList, Panel::Layers, Panel::TextureSetSettings, Panel::Properties] {
            dock.close(p);
            assert!(!dock.is_open(p));
        }
        for p in [Panel::Properties, Panel::Layers, Panel::TextureSetList, Panel::Assets, Panel::Display] {
            dock.focus(p, screen);
            assert!(dock.is_visible(p), "{p:?}");
        }
        dock.close(Panel::Viewport);
        assert!(dock.is_open(Panel::Viewport));
        let l = dock.layout();
        assert_eq!(l.windows.len(), 1);
        let mut order = Vec::new();
        l.main.collect(&mut order);
        assert_eq!(order, [Panel::Assets, Panel::Viewport, Panel::TextureSetList, Panel::Layers, Panel::Properties]);
    }
}
