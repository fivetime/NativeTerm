//! A window's pages and the bar at its side that chooses among them
//! (tabs, as desktops and phones have them): what is shown is one page,
//! the bar says which and goes to another.
//!
//! Two looks:
//! - [`Style::Rail`]: a narrow rail of icons (the hint under the pointer),
//!   the page shown in the accent's colour with a bar at the window's
//!   edge, a count in a bubble where a page has one; the program's sign at
//!   its top, buttons that are not pages at its bottom (settings, light or
//!   dark). Drawn here: egui has no such control.
//! - [`Style::List`]: a list of icons and names (egui's own selectable
//!   buttons), what goes under it at its bottom (an "about").
//!
//! [`Tabs::bar`] draws the bar only, the program draws the page in what is
//! left of the window; [`Tabs::show`] draws both, the page by `page`.

use crate::Skin;

/// The rail's width (`w-14`), a button of it (an icon of 20 with `py-2.5`)
/// and what is between two of them (`space-y-4`; less in a low window).
pub const RAIL: f32 = 56.0;
const RAIL_ICON: f32 = 20.0;
const RAIL_BUTTON: f32 = 40.0;
const RAIL_GAP: f32 = 16.0;
/// The program's sign at the rail's top: its height, its tile.
const SIGN: f32 = 44.0;
const SIGN_TILE: f32 = 40.0;
/// A row of the list.
const ROW: f32 = 30.0;

/// One page: what it is (`id`), its icon and name, what the pointer says
/// on it (the name if nothing else), a count at its corner (none for 0).
pub struct Tab<'a, T> {
    pub id: T,
    pub icon: char,
    pub label: &'a str,
    pub hint: Option<&'a str>,
    pub badge: usize,
}

impl<'a, T> Tab<'a, T> {
    #[must_use]
    pub fn new(id: T, icon: char, label: &'a str) -> Tab<'a, T> {
        Tab { id, icon, label, hint: None, badge: 0 }
    }

    #[must_use]
    pub fn hint(mut self, hint: &'a str) -> Tab<'a, T> {
        self.hint = Some(hint);
        self
    }

    #[must_use]
    pub fn badge(mut self, count: usize) -> Tab<'a, T> {
        self.badge = count;
        self
    }
}

/// A button at the bar's bottom (or its top: the program's sign) that is
/// not a page: it does something (settings, light or dark, pin). `on`: it
/// is shown as chosen (what it opens is open); `near`: its colour under the
/// pointer, where not the skin's.
pub struct Action<'a> {
    pub icon: char,
    pub hint: &'a str,
    pub on: bool,
    pub near: Option<egui::Color32>,
}

impl<'a> Action<'a> {
    #[must_use]
    pub fn new(icon: char, hint: &'a str) -> Action<'a> {
        Action { icon, hint, on: false, near: None }
    }

    #[must_use]
    pub fn on(mut self, on: bool) -> Action<'a> {
        self.on = on;
        self
    }

    #[must_use]
    pub fn near(mut self, color: egui::Color32) -> Action<'a> {
        self.near = Some(color);
        self
    }
}

/// How the bar looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// Icons only, `RAIL` wide.
    Rail,
    /// Icons and names, this wide.
    List { width: f32 },
}

/// What goes at the bottom of a list.
type Below<'a> = Box<dyn FnOnce(&mut egui::Ui) + 'a>;

/// A window's pages and their bar.
pub struct Tabs<'a, T> {
    id: &'static str,
    style: Style,
    tabs: Vec<Tab<'a, T>>,
    sign: Option<Action<'a>>,
    actions: Vec<Action<'a>>,
    below: Option<Below<'a>>,
}

/// What happened in the bar this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabsShown<T> {
    /// A page chosen (already the current one, for `show`).
    pub chosen: Option<T>,
    /// One of the actions pressed, by its place.
    pub action: Option<usize>,
    /// The program's sign pressed.
    pub sign: bool,
}

impl<'a, T: Copy + PartialEq> Tabs<'a, T> {
    /// A bar `id` (its panel's) of `style`, without pages yet.
    #[must_use]
    pub fn new(id: &'static str, style: Style) -> Tabs<'a, T> {
        Tabs { id, style, tabs: Vec::new(), sign: None, actions: Vec::new(), below: None }
    }

    /// A page, after those given before.
    #[must_use]
    pub fn tab(mut self, tab: Tab<'a, T>) -> Tabs<'a, T> {
        self.tabs.push(tab);
        self
    }

    /// The program's sign at the rail's top (a button too).
    #[must_use]
    pub fn sign(mut self, sign: Action<'a>) -> Tabs<'a, T> {
        self.sign = Some(sign);
        self
    }

    /// A button at the bar's bottom, from the bottom up.
    #[must_use]
    pub fn action(mut self, action: Action<'a>) -> Tabs<'a, T> {
        self.actions.push(action);
        self
    }

    /// What goes at the bottom of a list (an "about").
    #[must_use]
    pub fn below(mut self, below: impl FnOnce(&mut egui::Ui) + 'a) -> Tabs<'a, T> {
        self.below = Some(Box::new(below));
        self
    }

    /// The bar at the side of `ui`, `current` the page shown; the page is
    /// the program's to draw in what is left.
    pub fn bar(self, ui: &mut egui::Ui, skin: &Skin, current: T) -> TabsShown<T> {
        match self.style {
            Style::Rail => self.rail(ui, skin, current),
            Style::List { width } => self.list(ui, skin, current, width),
        }
    }

    /// The bar and the page: `page` draws `current` in what is left. A
    /// page chosen is the current one from this frame on.
    pub fn show<R>(
        self,
        ui: &mut egui::Ui,
        skin: &Skin,
        current: &mut T,
        page: impl FnOnce(&mut egui::Ui, T) -> R,
    ) -> (TabsShown<T>, R) {
        let shown = self.bar(ui, skin, *current);
        if let Some(chosen) = shown.chosen {
            *current = chosen;
        }
        let frame = egui::Frame::NONE.inner_margin(egui::Margin { left: 16, ..egui::Margin::ZERO });
        let inner = egui::CentralPanel::default().frame(frame).show(ui, |ui| page(ui, *current)).inner;
        (shown, inner)
    }

    fn rail(self, ui: &mut egui::Ui, skin: &Skin, current: T) -> TabsShown<T> {
        let p = skin.palette;
        let Tabs { id, tabs, sign, actions, .. } = self;
        let mut shown = TabsShown { chosen: None, action: None, sign: false };
        let whole = ui.max_rect();
        let frame = egui::Frame::new().fill(p.rail).inner_margin(egui::Margin::symmetric(0, 12));
        egui::Panel::left(id).exact_size(RAIL).resizable(false).show_separator_line(false).frame(frame).show(
            ui,
            |ui| {
                // the design's gaps, or less in a low window
                let buttons = tabs.len() + actions.len();
                let top = if sign.is_some() { SIGN } else { 0.0 };
                let gap = rail_gap(ui.available_height() - top - 12.0, buttons);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, gap);
                if let Some(sign) = &sign {
                    shown.sign = sign_button(ui, skin, sign).clicked();
                }
                for tab in &tabs {
                    let hint = tab.hint.unwrap_or(tab.label);
                    let button =
                        RailButton { icon: tab.icon, hint, on: tab.id == current, count: tab.badge, near: None };
                    if rail_button(ui, skin, &button).clicked() {
                        shown.chosen = Some(tab.id);
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    // (`space-y-3`)
                    ui.spacing_mut().item_spacing.y = gap.min(12.0);
                    for (i, action) in actions.iter().enumerate() {
                        let button = RailButton {
                            icon: action.icon,
                            hint: action.hint,
                            on: action.on,
                            count: 0,
                            near: action.near,
                        };
                        if rail_button(ui, skin, &button).clicked() {
                            shown.action = Some(i);
                        }
                    }
                });
            },
        );
        // (the line at its side is the rail's own)
        let side = (whole.left() + RAIL).round() - 0.5;
        ui.painter().vline(side, whole.y_range(), egui::Stroke::new(1.0_f32, p.rail_line));
        shown
    }

    fn list(self, ui: &mut egui::Ui, skin: &Skin, current: T, width: f32) -> TabsShown<T> {
        let Tabs { id, tabs, actions, below, .. } = self;
        let mut shown = TabsShown { chosen: None, action: None, sign: false };
        let frame = egui::Frame::new().inner_margin(egui::Margin { right: 12, ..egui::Margin::ZERO });
        egui::Panel::left(id).exact_size(width).resizable(false).frame(frame).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let row = |ui: &mut egui::Ui, icon: char, label: &str, on: bool| {
                let width = ui.available_width();
                ui.add_sized([width, ROW], egui::Button::selectable(on, format!("{icon}  {label}")))
            };
            for tab in &tabs {
                let mut response = row(ui, tab.icon, tab.label, tab.id == current);
                if let Some(hint) = tab.hint {
                    response = response.on_hover_text(hint);
                }
                if response.clicked() {
                    shown.chosen = Some(tab.id);
                }
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                if let Some(below) = below {
                    below(ui);
                }
                for (i, action) in actions.iter().enumerate() {
                    if row(ui, action.icon, action.hint, action.on).clicked() {
                        shown.action = Some(i);
                    }
                }
            });
        });
        let _ = skin;
        shown
    }
}

/// What is between the rail's buttons in a window `height` high (what the
/// rail has below its sign) with `count` of them: the design's 16 where
/// there is room, less where not, never less than nothing.
#[must_use]
pub fn rail_gap(height: f32, count: usize) -> f32 {
    if count < 2 {
        return RAIL_GAP;
    }
    let room = height - count as f32 * RAIL_BUTTON;
    (room / (count - 1) as f32).clamp(0.0, RAIL_GAP)
}

struct RailButton<'a> {
    icon: char,
    hint: &'a str,
    on: bool,
    count: usize,
    near: Option<egui::Color32>,
}

/// One of the rail's buttons: an icon, in the accent's colour with a bar
/// at the window's edge while it is what is shown, a count in a bubble.
fn rail_button(ui: &mut egui::Ui, skin: &Skin, button: &RailButton<'_>) -> egui::Response {
    let p = skin.palette;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(RAIL, RAIL_BUTTON), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, button.on, button.hint));
    if ui.is_rect_visible(rect) {
        let color = if button.on {
            p.accent
        } else if response.hovered() {
            button.near.unwrap_or(p.rail_near)
        } else {
            p.weak
        };
        let painter = ui.painter();
        if button.on {
            // from a quarter of its height to three (`top: 25%; bottom: 25%`)
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + rect.height() * 0.25),
                egui::pos2(rect.left() + 3.0, rect.bottom() - rect.height() * 0.25),
            );
            let round = egui::CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 };
            painter.rect_filled(bar, round, p.accent);
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            button.icon,
            egui::FontId::proportional(RAIL_ICON),
            color,
        );
        if button.count > 0 {
            let text = if button.count > 99 { "99+".to_string() } else { button.count.to_string() };
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(9.5), p.on_primary);
            let size = egui::vec2((galley.size().x + 8.0).max(15.0), 15.0);
            let bubble = egui::Rect::from_center_size(rect.center() + egui::vec2(11.0, -10.0), size);
            painter.rect_filled(bubble, 7.5, p.primary);
            painter.galley(bubble.center() - galley.size() / 2.0, galley, p.on_primary);
        }
    }
    response.on_hover_text(button.hint)
}

/// The program's sign at the rail's top (`p-2 rounded-xl`, `mb-1`): its
/// icon in the accent's colour, the tile's colour under the pointer.
fn sign_button(ui: &mut egui::Ui, skin: &Skin, sign: &Action<'_>) -> egui::Response {
    let p = skin.palette;
    let (at, response) = ui.allocate_exact_size(egui::vec2(RAIL, SIGN), egui::Sense::click());
    let tile = egui::Rect::from_center_size(at.center(), egui::vec2(SIGN_TILE, SIGN_TILE));
    if response.hovered() {
        // (`hover:bg-blue-500/10`)
        ui.painter().rect_filled(tile, 12.0, p.tile.fill);
    }
    ui.painter().text(
        tile.center(),
        egui::Align2::CENTER_CENTER,
        sign.icon,
        egui::FontId::proportional(24.0),
        p.accent,
    );
    response.on_hover_text(sign.hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_fits_a_low_window() {
        assert_eq!(rail_gap(640.0, 6), RAIL_GAP, "the design's, where there is room");
        // six buttons in 280: 40 left for five gaps
        assert_eq!(rail_gap(280.0, 6), 8.0);
        assert_eq!(rail_gap(200.0, 6), 0.0, "never over each other's place");
        assert_eq!(rail_gap(100.0, 1), RAIL_GAP);
    }
}
