//! The floating navigation bar: zoom, page stepping and page display in a small bar over the
//! document area, for when the right rail is out of reach (Read mode, full screen, a wide window).
//!
//! It can be dragged anywhere inside the document area (double-click the grip to put it back at
//! the bottom centre), drawn small, medium or large, laid out horizontally or vertically, and
//! collapsed to a single button. All of that is a preference, saved with the other settings; the
//! whole bar can be turned off from View ▸ Floating navigation bar.
//!
//! Everything here acts on the active tab's [`DocView`] through the same methods as the rail, so
//! the two always agree.

use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::canvas::{DocView, Fit};
use crate::icons;
use crate::theme::{self, Tokens};

/// Gap kept between the bar and the edges of the document area.
const MARGIN: f32 = 8.0;
/// Distance of the default (bottom-centre) position from the bottom of the document area.
const BOTTOM_GAP: f32 = 16.0;

/// How large the bar's buttons are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BarSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl BarSize {
    /// The side of one square button, in points.
    pub fn button(self) -> f32 {
        match self {
            Self::Small => 24.0,
            Self::Medium => 30.0,
            Self::Large => 38.0,
        }
    }

    /// The text size of the zoom and page read-outs.
    fn text(self) -> f32 {
        match self {
            Self::Small => 10.0,
            Self::Medium => 11.0,
            Self::Large => 13.0,
        }
    }

    pub fn larger(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium | Self::Large => Self::Large,
        }
    }

    pub fn smaller(self) -> Self {
        match self {
            Self::Large => Self::Medium,
            Self::Medium | Self::Small => Self::Small,
        }
    }
}

/// The bar's preferences. Unknown or missing fields in saved settings fall back to the defaults.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FloatNav {
    /// View ▸ Floating navigation bar.
    pub enabled: bool,
    /// Folded away to a single button.
    pub collapsed: bool,
    pub size: BarSize,
    pub vertical: bool,
    /// The bar's top-left corner relative to the document area's top-left, once it has been
    /// dragged; `None` keeps it at the bottom centre.
    pub offset: Option<[f32; 2]>,
}

impl Default for FloatNav {
    fn default() -> Self {
        Self { enabled: true, collapsed: false, size: BarSize::default(), vertical: false, offset: None }
    }
}

impl FloatNav {
    /// Read saved settings leniently: anything malformed becomes the default, and an offset that
    /// isn't a pair of finite numbers is dropped (it would place the bar nowhere).
    pub fn restore(value: &serde_json::Value) -> Self {
        let mut nav: Self = serde_json::from_value(value.clone()).unwrap_or_default();
        nav.offset = nav.offset.filter(|[x, y]| x.is_finite() && y.is_finite());
        nav
    }
}

/// Where the bar of `size` goes in `area`: at `offset` from the area's top-left, or at the bottom
/// centre, then kept inside the area with a margin. A bar larger than the area sticks to its
/// top-left corner. Total for every input: non-finite numbers fall back to the default position
/// or the area's corner.
pub fn place(area: Rect, size: Vec2, offset: Option<[f32; 2]>) -> Pos2 {
    let size = if size.x.is_finite() && size.y.is_finite() { size.max(Vec2::ZERO) } else { Vec2::ZERO };
    let wanted = match offset.filter(|[x, y]| x.is_finite() && y.is_finite()) {
        Some([x, y]) => area.min + vec2(x, y),
        None => pos2(area.center().x - size.x / 2.0, area.max.y - BOTTOM_GAP - size.y),
    };
    let lo = area.min + Vec2::splat(MARGIN);
    let hi = area.max - size - Vec2::splat(MARGIN);
    let clamp = |v: f32, lo: f32, hi: f32| if !v.is_finite() || hi < lo { lo } else { v.clamp(lo, hi) };
    let p = pos2(clamp(wanted.x, lo.x, hi.x), clamp(wanted.y, lo.y, hi.y));
    if p.x.is_finite() && p.y.is_finite() { p } else { area.min }
}

/// What a click on the bar asks for. Collected while the bar is drawn and applied afterwards, when
/// the view and the preferences are no longer borrowed by the drawing closure.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    ZoomStep(bool),
    ActualSize,
    ToggleFit,
    Page(bool),
    ToggleContinuous,
    Rotate,
    Size(BarSize),
    ToggleVertical,
    Collapse(bool),
    MoveTo(Option<[f32; 2]>),
}

fn apply(action: Action, view: &mut DocView, nav: &mut FloatNav) {
    match action {
        Action::ZoomStep(up) => view.zoom_step(up),
        Action::ActualSize => view.set_zoom(1.0),
        Action::ToggleFit => view.set_fit(if view.fit == Fit::Width { Fit::Page } else { Fit::Width }),
        Action::Page(forward) => view.step_page(forward),
        Action::ToggleContinuous => view.set_continuous(!view.continuous()),
        Action::Rotate => view.rotate_view(true),
        Action::Size(size) => nav.size = size,
        Action::ToggleVertical => nav.vertical = !nav.vertical,
        Action::Collapse(collapsed) => nav.collapsed = collapsed,
        Action::MoveTo(offset) => nav.offset = offset,
    }
}

/// Draw the bar over `area` for the view of tab `index`, and apply what it was asked to do.
pub fn show(app: &mut crate::PdfCraftApp, index: usize, area: Rect, ui: &mut egui::Ui) {
    if !app.float_nav.enabled || !area.is_positive() {
        return;
    }
    let Some(page_count) = app.views.get(index).and_then(|v| app.session.get(v.id)).map(|d| d.info.pages.len()) else { return };
    let mut nav = app.float_nav;
    let Some(view) = app.views.get_mut(index) else { return };
    let t = Tokens::get(ui.ctx());
    // The bar's size from the last frame places it this frame (its first frame guesses).
    let size_id = egui::Id::new("float-nav").with("size");
    let guess = Vec2::splat(nav.size.button() + 12.0);
    let last = ui.ctx().data(|d| d.get_temp::<Vec2>(size_id)).unwrap_or(guess);
    let pos = place(area, last, nav.offset);
    let mut actions: Vec<Action> = Vec::new();
    let shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 100 } else { 28 }) };

    let shown = egui::Area::new(egui::Id::new("float-nav")).order(egui::Order::Middle).fixed_pos(pos).show(ui.ctx(), |ui| {
        let b = nav.size.button();
        if nav.collapsed {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(255))
                .shadow(shadow)
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    if icons::button(ui, "sliders-horizontal", b, false, tl!("Show floating bar")).clicked() {
                        actions.push(Action::Collapse(false));
                    }
                });
            return;
        }
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(12))
            .shadow(shadow)
            .inner_margin(egui::Margin::same(5))
            .show(ui, |ui| {
                let layout = if nav.vertical { Layout::top_down(Align::Center) } else { Layout::left_to_right(Align::Center) };
                ui.with_layout(layout, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(2.0);
                    bar_contents(ui, view, &nav, page_count, pos - area.min, &t, &mut actions);
                });
            });
    });
    let rect = shown.response.rect;
    if rect.is_positive() {
        ui.ctx().data_mut(|d| d.insert_temp(size_id, rect.size()));
    }
    for action in actions {
        apply(action, view, &mut nav);
    }
    app.float_nav = nav;
}

/// The bar's buttons, in reading order: grip, zoom, pages, page display, bar settings.
fn bar_contents(ui: &mut egui::Ui, view: &DocView, nav: &FloatNav, pages: usize, at: Vec2, t: &Tokens, actions: &mut Vec<Action>) {
    let b = nav.size.button();
    let separator = |ui: &mut egui::Ui| {
        let (rect, _) = ui.allocate_exact_size(if nav.vertical { vec2(b * 0.6, 5.0) } else { vec2(5.0, b * 0.6) }, Sense::hover());
        let stroke = Stroke::new(1.0, t.divider);
        if nav.vertical {
            ui.painter().hline(rect.x_range(), rect.center().y, stroke);
        } else {
            ui.painter().vline(rect.center().x, rect.y_range(), stroke);
        }
    };
    let readout = |ui: &mut egui::Ui, text: String, tip: &str| -> egui::Response {
        let width = if nav.vertical { b } else { b * 1.6 };
        let (rect, resp) = ui.allocate_exact_size(vec2(width, b), Sense::click());
        let resp = theme::hand(resp);
        if resp.hovered() {
            ui.painter().rect_filled(rect, t.radius, t.field);
        }
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::medium(nav.size.text()), t.text_muted);
        resp.on_hover_text(tip.to_string())
    };

    // Grip: drag to move; double-click puts the bar back at the bottom centre.
    let grip = icons::button(ui, "move", b, false, tl!("Drag to move. Double-click to put it back.")).interact(Sense::drag());
    if grip.double_clicked() {
        actions.push(Action::MoveTo(None));
    } else if grip.dragged() {
        let to = at + grip.drag_delta();
        if to.x.is_finite() && to.y.is_finite() {
            actions.push(Action::MoveTo(Some([to.x, to.y])));
        }
    }
    separator(ui);

    let tip = crate::commands::key_tip(ui.ctx(), tl!("Zoom out ({key})"), crate::commands::ZOOM_OUT);
    if icons::button(ui, "zoom-out", b, false, &tip).clicked() {
        actions.push(Action::ZoomStep(false));
    }
    // Zoom is finite and positive (`DocView::set_zoom` clamps it), so the cast is in range.
    let percent = (view.zoom * 100.0).round().clamp(0.0, 100_000.0) as u32;
    if readout(ui, format!("{percent}%"), tl!("Actual size")).clicked() {
        actions.push(Action::ActualSize);
    }
    let tip = crate::commands::key_tip(ui.ctx(), tl!("Zoom in ({key})"), crate::commands::ZOOM_IN);
    if icons::button(ui, "zoom-in", b, false, &tip).clicked() {
        actions.push(Action::ZoomStep(true));
    }
    let fit_icon = if view.fit == Fit::Width { "maximize" } else { "maximize-2" };
    if icons::button(ui, fit_icon, b, false, tl!("Toggle fit page / fit width")).clicked() {
        actions.push(Action::ToggleFit);
    }
    separator(ui);

    let first = view.current == 0;
    let last = view.current.saturating_add(1) >= pages;
    if ui.add_enabled_ui(!first, |ui| icons::button(ui, "chevron-up", b, false, tl!("Previous page"))).inner.clicked() {
        actions.push(Action::Page(false));
    }
    let page = view.current.saturating_add(1).min(pages.max(1));
    let text = if nav.vertical { format!("{page}\n{pages}") } else { format!("{page} / {pages}") };
    readout(ui, text, tl!("Page"));
    if ui.add_enabled_ui(!last, |ui| icons::button(ui, "chevron-down", b, false, tl!("Next page"))).inner.clicked() {
        actions.push(Action::Page(true));
    }
    // Selected while scrolling continuously; off is one page (or spread) at a time.
    let continuous = view.continuous();
    if icons::button(ui, if continuous { "rows-3" } else { "file-text" }, b, continuous, tl!("Continuous scrolling")).clicked() {
        actions.push(Action::ToggleContinuous);
    }
    let tip = crate::commands::key_tip(ui.ctx(), tl!("Rotate view clockwise ({key})"), crate::commands::ROTATE_CW);
    if icons::button(ui, "rotate-cw", b, false, &tip).clicked() {
        actions.push(Action::Rotate);
    }
    separator(ui);

    if ui.add_enabled_ui(nav.size != BarSize::Small, |ui| icons::button(ui, "minus", b, false, tl!("Smaller floating bar"))).inner.clicked() {
        actions.push(Action::Size(nav.size.smaller()));
    }
    if ui.add_enabled_ui(nav.size != BarSize::Large, |ui| icons::button(ui, "plus", b, false, tl!("Larger floating bar"))).inner.clicked() {
        actions.push(Action::Size(nav.size.larger()));
    }
    let turn = if nav.vertical { "arrow-left-right" } else { "arrow-up-down" };
    if icons::button(ui, turn, b, false, tl!("Switch between horizontal and vertical")).clicked() {
        actions.push(Action::ToggleVertical);
    }
    if icons::button(ui, "eye-off", b, false, tl!("Hide floating bar")).clicked() {
        actions.push(Action::Collapse(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0))
    }

    /// No offset: centred horizontally, just above the bottom of the area.
    #[test]
    fn the_default_place_is_the_bottom_centre() {
        let p = place(area(), vec2(200.0, 40.0), None);
        assert_eq!(p, pos2(100.0 + 400.0 - 100.0, 50.0 + 600.0 - BOTTOM_GAP - 40.0));
    }

    /// A dragged bar stays where it was put, relative to the area.
    #[test]
    fn an_offset_is_kept_inside_the_area() {
        assert_eq!(place(area(), vec2(200.0, 40.0), Some([30.0, 40.0])), pos2(130.0, 90.0));
        // Past every edge: pulled back in, a margin from the edge.
        assert_eq!(place(area(), vec2(200.0, 40.0), Some([-500.0, -500.0])), pos2(100.0 + MARGIN, 50.0 + MARGIN));
        assert_eq!(place(area(), vec2(200.0, 40.0), Some([5000.0, 5000.0])), pos2(900.0 - 200.0 - MARGIN, 650.0 - 40.0 - MARGIN));
    }

    /// A bar wider or taller than the area sticks to its top-left instead of leaving it.
    #[test]
    fn a_bar_larger_than_the_area_sticks_to_the_corner() {
        let small = Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 20.0));
        assert_eq!(place(small, vec2(200.0, 40.0), None), pos2(10.0 + MARGIN, 10.0 + MARGIN));
    }

    /// Non-finite input never yields a non-finite position.
    #[test]
    fn nonsense_numbers_fall_back() {
        for offset in [Some([f32::NAN, 1.0]), Some([f32::INFINITY, f32::NEG_INFINITY]), None] {
            for size in [vec2(f32::NAN, 10.0), vec2(f32::INFINITY, 1.0), vec2(-5.0, -5.0)] {
                let p = place(area(), size, offset);
                assert!(p.x.is_finite() && p.y.is_finite(), "{offset:?} {size:?} → {p:?}");
                assert!(area().expand(1.0).contains(p), "{offset:?} {size:?} → {p:?}");
            }
        }
    }

    /// Saved settings: well-formed ones round-trip, malformed ones become the default, and a
    /// non-finite offset is dropped.
    #[test]
    fn settings_restore_leniently() {
        let nav = FloatNav { enabled: false, collapsed: true, size: BarSize::Large, vertical: true, offset: Some([12.0, 34.0]) };
        let saved = serde_json::to_value(nav).unwrap_or_default();
        assert_eq!(FloatNav::restore(&saved), nav);
        assert_eq!(FloatNav::restore(&serde_json::json!("garbage")), FloatNav::default());
        assert_eq!(FloatNav::restore(&serde_json::Value::Null), FloatNav::default());
        let partial = FloatNav::restore(&serde_json::json!({ "size": "small" }));
        assert_eq!(partial, FloatNav { size: BarSize::Small, ..FloatNav::default() });
        let bad = FloatNav::restore(&serde_json::json!({ "offset": [1.0e40, 2.0] }));
        assert_eq!(bad.offset, None, "1e40 overflows f32 to infinity");
        assert_eq!(FloatNav::restore(&serde_json::json!({ "size": "huge" })), FloatNav::default());
    }

    /// The size steps stop at both ends.
    #[test]
    fn sizes_step_and_stop() {
        assert_eq!(BarSize::Small.smaller(), BarSize::Small);
        assert_eq!(BarSize::Small.larger(), BarSize::Medium);
        assert_eq!(BarSize::Medium.larger(), BarSize::Large);
        assert_eq!(BarSize::Large.larger(), BarSize::Large);
        assert_eq!(BarSize::Large.smaller(), BarSize::Medium);
        assert!(BarSize::Small.button() < BarSize::Medium.button() && BarSize::Medium.button() < BarSize::Large.button());
    }
}
