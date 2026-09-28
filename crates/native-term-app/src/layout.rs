//! The main window's layout, after the design the person brought
//! (`interactive_modern_tree_view_ui-v2.html`, a web page written with
//! Tailwind's classes; the numbers here are those classes', the colours
//! its variables, `looks::Tones`): a rail of
//! icons at the left, a header, the tree under its search field and its
//! filters, a bar below it, and what is chosen at the right. Nothing of
//! it is a picture: rectangles, lines, the text font and the icon font.
//!
//! This file draws the parts; `app.rs` puts them together, the tree's
//! rows are `tree_view.rs`'s and what is chosen is `properties.rs`'s.

use std::sync::Arc;

use crate::icons;
use crate::looks::{Tint, Tones};

/// The rail's width (`w-14`).
pub const RAIL: f32 = 56.0;
/// A button of the rail: its icon (`w-5`) with `py-2.5` above and below,
/// and what is between two of them (`space-y-4`; less in a low window).
const RAIL_ICON: f32 = 20.0;
pub const RAIL_BUTTON: f32 = 40.0;
pub const RAIL_GAP: f32 = 16.0;
/// The header's tile: an icon of 24 with `p-2.5` around it.
const HEADER_TILE: f32 = 44.0;
/// From this width on the design has more room around the header's
/// content and a larger title (its `sm:`).
const WIDE: f32 = 640.0;

/// The room around the header's content (`p-4 sm:p-5`) in a window
/// `width` wide.
#[must_use]
pub fn header_pad(width: f32) -> i8 {
    if width < WIDE {
        16
    } else {
        20
    }
}
/// The bar below the tree (`h-9`, `px-4`).
pub const FOOTER: f32 = 36.0;
/// What is chosen, at the right (`w-80`; the design has `w-96` from a
/// width of 640 on, which leaves a docked window's tree too little).
pub const PROPERTIES: f32 = 320.0;
/// A window narrower than this has what is chosen under the tree.
pub const NARROW: f32 = 680.0;
/// A window narrower than this has its header's buttons as icons.
pub const SHORT_HEADER: f32 = 720.0;

/// The small text (`text-xs`), a badge's (`text-[11px]`), what is said
/// in the middle of an empty place (`text-sm`), a title's (`text-base
/// sm:text-lg`).
pub const SMALL: f32 = 12.0;
pub const TINY: f32 = 11.0;
pub const MIDDLE: f32 = 14.0;

#[must_use]
pub fn title_size(width: f32) -> f32 {
    if width < WIDE {
        16.0
    } else {
        18.0
    }
}

fn font(size: f32) -> egui::FontId {
    egui::FontId::proportional(size)
}

/// `color` as thin as `alpha` of 255 says.
#[must_use]
pub fn thin(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// One line of `text`, ending in "…" where it is wider than `width`.
pub fn elided(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    painter.layout_job(job)
}

/// What is between two of the rail's buttons, for `count` of them in a
/// rail `height` high: the design's, or less where they would not fit.
#[must_use]
pub fn rail_gap(height: f32, count: usize) -> f32 {
    if count < 2 {
        return RAIL_GAP;
    }
    let free = height - count as f32 * RAIL_BUTTON;
    (free / (count - 1) as f32).clamp(0.0, RAIL_GAP)
}

/// One of the rail's buttons, as it is now.
pub struct Rail<'a> {
    pub icon: char,
    /// Said under the pointer.
    pub hint: &'a str,
    /// It is what is shown.
    pub active: bool,
    /// In a bubble at its corner (none for 0).
    pub count: usize,
    /// Its colour under the pointer.
    pub near: egui::Color32,
}

/// One of the rail's buttons: an icon, in the accent's colour with a bar
/// at the window's edge while it is what is shown.
pub fn rail_button(ui: &mut egui::Ui, tones: &Tones, rail: Rail<'_>) -> egui::Response {
    let Rail { icon, hint, active, count, near } = rail;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(RAIL, RAIL_BUTTON), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, hint));
    if ui.is_rect_visible(rect) {
        let color = if active {
            tones.accent
        } else if response.hovered() {
            near
        } else {
            tones.weak
        };
        let painter = ui.painter();
        if active {
            // from a quarter of its height to three (`top: 25%; bottom: 25%`)
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + rect.height() * 0.25),
                egui::pos2(rect.left() + 3.0, rect.bottom() - rect.height() * 0.25),
            );
            let round = egui::CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 };
            painter.rect_filled(bar, round, tones.accent);
        }
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon, font(RAIL_ICON), color);
        if count > 0 {
            let text = if count > 99 { "99+".to_string() } else { count.to_string() };
            let galley = painter.layout_no_wrap(text, font(9.5), tones.on_primary);
            let size = egui::vec2((galley.size().x + 8.0).max(15.0), 15.0);
            let center = rect.center() + egui::vec2(11.0, -10.0);
            let bubble = egui::Rect::from_center_size(center, size);
            painter.rect_filled(bubble, 7.5, tones.primary);
            painter.galley(bubble.center() - galley.size() / 2.0, galley, tones.on_primary);
        }
    }
    response.on_hover_text(hint)
}

/// A square with an icon in it: the header's (the accent's colours) and
/// the chosen thing's (a button's).
pub fn tile(ui: &mut egui::Ui, side: f32, icon: char, fill: egui::Color32, line: egui::Color32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 12.0, fill);
        painter.rect_stroke(rect, 12.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon, font(24.0), color);
    }
}

/// The header's tile.
pub fn header_tile(ui: &mut egui::Ui, tones: &Tones, icon: char) {
    tile(ui, HEADER_TILE, icon, tones.tile.fill, tones.tile.line, tones.tile.text);
}

/// The header's two lines, `width` wide: what is shown, in letters
/// `size` high, and a word about it.
pub fn header_title(ui: &mut egui::Ui, tones: &Tones, title: &str, about: &str, width: f32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(0.0), HEADER_TILE), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect);
        let title = elided(&painter, title, font(size), tones.text, rect.width());
        let about = elided(&painter, about, font(SMALL), tones.weak, rect.width());
        let height = title.size().y + about.size().y;
        let top = rect.center().y - height / 2.0;
        let under = top + title.size().y;
        painter.galley(egui::pos2(rect.left(), top), title, tones.text);
        painter.galley(egui::pos2(rect.left(), under), about, tones.weak);
    }
}

/// The button that changes between light and dark, in the header (`p-2`
/// around an icon of 16; `hover:text-amber-500`).
pub fn theme_button(ui: &mut egui::Ui, tones: &Tones, icon: char, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, hint));
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() { tones.sun } else { tones.weak };
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, tones.card);
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.0_f32, tones.line), egui::StrokeKind::Inside);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon, font(16.0), color);
    }
    response.on_hover_text(hint)
}

/// What a button is for, which is how it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One among others (`--bg-card`, `--border-color`).
    Plain,
    /// What makes something new, or does what the place is for
    /// (`bg-blue-600 text-white`).
    Primary,
    /// What takes something away (`bg-red-500/10 hover:bg-red-500/20
    /// border-red-500/30 text-red-500`).
    Danger,
}

/// Where a button is, which is how much room it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Room {
    /// The header's (`px-3.5 py-1.5`).
    Header,
    /// Among what is chosen's (`py-2`), as wide as it needs.
    Panel,
    /// The same, this wide.
    Wide(f32),
}

/// A button: an icon, a text, or both.
pub fn button(
    ui: &mut egui::Ui,
    tones: &Tones,
    kind: Kind,
    room: Room,
    icon: Option<char>,
    text: &str,
) -> egui::Response {
    let (pad, height) = match room {
        Room::Header => (14.0, 28.0),
        Room::Panel | Room::Wide(_) => (12.0, 32.0),
    };
    let color = match kind {
        Kind::Plain => tones.text,
        Kind::Primary => tones.on_primary,
        Kind::Danger => tones.danger,
    };
    let words = (!text.is_empty()).then(|| ui.painter().layout_no_wrap(text.to_string(), font(SMALL), color));
    let sign = icon.map(|icon| ui.painter().layout_no_wrap(icon.to_string(), font(14.0), color));
    let between = if words.is_some() && sign.is_some() { 6.0 } else { 0.0 };
    let content = words.as_ref().map_or(0.0, |g| g.size().x) + sign.as_ref().map_or(0.0, |g| g.size().x) + between;
    let width = match room {
        Room::Wide(width) => width,
        _ => content + 2.0 * pad,
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        let near = response.hovered() || response.has_focus();
        let (fill, line) = match kind {
            Kind::Plain => (if near { tones.raised } else { tones.card }, tones.line),
            Kind::Primary => {
                let fill = if near { tones.primary_near } else { tones.primary };
                (fill, fill)
            }
            Kind::Danger => (thin(tones.danger, if near { 0x33 } else { 0x1a }), thin(tones.danger, 0x4d)),
        };
        let (fill, line, color) = if ui.is_enabled() {
            (fill, line, color)
        } else {
            (fill.gamma_multiply(0.5), line.gamma_multiply(0.5), color.gamma_multiply(0.5))
        };
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 8.0, fill);
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside);
        let mut x = rect.center().x - content / 2.0;
        if let Some(sign) = sign {
            let width = sign.size().x;
            painter.galley(egui::pos2(x, rect.center().y - sign.size().y / 2.0), sign, color);
            x += width + between;
        }
        if let Some(words) = words {
            painter.galley(egui::pos2(x, rect.center().y - words.size().y / 2.0), words, color);
        }
    }
    response
}

/// One of the filters above the tree (`px-3 py-1 rounded-lg border`);
/// the one that is on in the accent's colours (the design's
/// `chip-active`).
pub fn chip(ui: &mut egui::Ui, tones: &Tones, text: &str, on: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font(SMALL), tones.weak);
    let size = egui::vec2(galley.size().x + 24.0, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, text));
    if ui.is_rect_visible(rect) {
        let near = response.hovered();
        let (fill, line, color) = if on {
            (tones.chip_on.fill, tones.chip_on.line, tones.chip_on.text)
        } else if near {
            (tones.chip, tones.near, tones.text)
        } else {
            (tones.chip, tones.line, tones.weak)
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, fill);
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response
}

/// What is said before a row of chips, and above a part of what is
/// chosen (`text-[11px] font-semibold uppercase tracking-wider`).
pub fn caption(ui: &mut egui::Ui, tones: &Tones, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).size(TINY).color(tones.weak).extra_letter_spacing(0.6));
}

/// A badge's size with `text` in it (`px-2 py-0.5`).
#[must_use]
pub fn badge_size(painter: &egui::Painter, text: &str) -> egui::Vec2 {
    let galley = painter.layout_no_wrap(text.to_string(), font(TINY), egui::Color32::WHITE);
    egui::vec2(galley.size().x + 16.0, 18.0)
}

/// A badge at `rect`: a tint's colours, or those of a mark that says
/// nothing about how it goes.
pub fn paint_badge(painter: &egui::Painter, tones: &Tones, rect: egui::Rect, text: &str, tint: Option<Tint>) {
    let tint = tint.unwrap_or(tones.plain);
    painter.rect_filled(rect, 4.0, tint.fill);
    painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, tint.line), egui::StrokeKind::Inside);
    let galley = painter.layout_no_wrap(text.to_string(), font(TINY), tint.text);
    painter.galley(rect.center() - galley.size() / 2.0, galley, tint.text);
}

/// A badge among other widgets.
pub fn badge(ui: &mut egui::Ui, tones: &Tones, text: &str, tint: Option<Tint>) {
    let size = badge_size(ui.painter(), text);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_badge(ui.painter(), tones, rect, text, tint);
    }
}

/// The search field (`pl-10 pr-9 py-2 rounded-xl`): the sign in it, a
/// line in the accent's colour around it while it is typed into, and
/// what clears it at its end once something was typed. The field's
/// response, and whether it was cleared.
pub fn search_field(ui: &mut egui::Ui, tones: &Tones, text: &mut String, hint: &str) -> (egui::Response, bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
    // (under what is in it, in a colour known once that was drawn)
    let under = ui.painter().add(egui::Shape::Noop);
    let within = rect.shrink2(egui::vec2(12.0, 0.0));
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(within).layout(layout));
    inside.spacing_mut().item_spacing.x = 8.0;
    inside.label(egui::RichText::new(icons::SEARCH.to_string()).size(14.0).color(tones.weak));
    let clear_width = if text.is_empty() { 0.0 } else { 24.0 };
    let edit = inside.add(
        egui::TextEdit::singleline(text)
            .hint_text(egui::RichText::new(hint).color(tones.weak))
            .frame(egui::Frame::NONE)
            .text_color(tones.text)
            .desired_width(inside.available_width() - clear_width),
    );
    let mut cleared = false;
    if !text.is_empty() {
        let (at, clear) = inside.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
        let color = if clear.hovered() { tones.text } else { tones.weak };
        inside.painter().text(at.center(), egui::Align2::CENTER_CENTER, icons::CLEAR, font(12.0), color);
        cleared = clear.on_hover_text(native_term_app::t!("tree-search-clear")).clicked();
    }
    // (the design's `focus:border-blue-500/60`)
    let line = if edit.has_focus() { thin(tones.accent, 0x99) } else { tones.line };
    let shape = egui::epaint::RectShape::new(
        rect,
        12.0,
        tones.card,
        egui::Stroke::new(1.0_f32, line),
        egui::StrokeKind::Inside,
    );
    ui.painter().set(under, shape);
    (edit, cleared)
}

/// One of the things the bar below says: a sign, a name, a number.
pub fn footer_count(ui: &mut egui::Ui, tones: &Tones, icon: char, name: &str, count: usize) {
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.label(egui::RichText::new(icon.to_string()).size(14.0).color(tones.weak));
    ui.label(egui::RichText::new(name).size(SMALL).color(tones.weak));
    ui.label(egui::RichText::new(count.to_string()).size(SMALL).color(tones.text));
    ui.add_space(10.0);
}

/// A round dot (`w-2 h-2 rounded-full`).
pub fn dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// One thing about what is chosen: its name at the left, what it is at
/// the right, a line under both (`py-1.5 border-b`).
pub fn property(ui: &mut egui::Ui, tones: &Tones, name: &str, value: impl FnOnce(&mut egui::Ui)) {
    let row = ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        ui.label(egui::RichText::new(name).size(SMALL).color(tones.weak));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), value);
    });
    let rect = row.response.rect;
    let line = egui::Stroke::new(1.0_f32, tones.line);
    ui.painter().hline(egui::Rangef::new(rect.left(), ui.max_rect().right()), rect.bottom() + 0.5, line);
}

/// What a property is, as text (one line; its end is cut where the
/// panel is narrow, and the whole of it is said under the pointer).
pub fn value(ui: &mut egui::Ui, tones: &Tones, text: &str, fixed: bool) {
    let mut rich = egui::RichText::new(text).size(SMALL).color(tones.text);
    if fixed {
        rich = rich.monospace();
    }
    ui.add(egui::Label::new(rich).truncate()).on_hover_text(text);
}

/// A box with a text in it (the design's description: `p-3 rounded-lg
/// border`).
pub fn text_box(ui: &mut egui::Ui, tones: &Tones, text: &str) {
    egui::Frame::new()
        .fill(tones.card)
        .stroke(egui::Stroke::new(1.0_f32, tones.line))
        .corner_radius(8.0)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).size(SMALL).color(tones.text));
        });
}

/// A part of the window with nothing in it yet: a sign, and what to do
/// (the design's empty properties).
pub fn empty(ui: &mut egui::Ui, tones: &Tones, icon: char, text: &str) {
    ui.vertical_centered(|ui| {
        let free = ui.available_height();
        ui.add_space((free / 2.0 - 60.0).max(12.0));
        ui.label(egui::RichText::new(icon.to_string()).size(40.0).color(tones.weak));
        ui.add_space(6.0);
        ui.set_max_width(ui.available_width().min(320.0));
        ui.label(egui::RichText::new(text).size(MIDDLE).color(tones.weak));
    });
}

/// A card: a part of a page with a line around it (`p-4 rounded-xl
/// border`).
pub fn card<R>(ui: &mut egui::Ui, tones: &Tones, inside: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(tones.card)
        .stroke(egui::Stroke::new(1.0_f32, tones.line))
        .corner_radius(12.0)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            inside(ui)
        })
        .inner
}

/// A bar's frame: its colour, and the room at its sides.
pub fn bar(tones: &Tones, margin: impl Into<egui::Margin>) -> egui::Frame {
    egui::Frame::new().fill(tones.bar).inner_margin(margin)
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

    #[test]
    fn a_long_text_ends_in_dots() {
        let ctx = egui::Context::default();
        let mut cut = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = "control1.cloud.local, the first of three";
            let galley = elided(ui.painter(), text, font(SMALL), egui::Color32::WHITE, 80.0);
            cut = Some((galley.size().x, galley.rows.len(), galley.elided));
        });
        let (width, rows, elided) = cut.unwrap();
        assert!(width <= 80.0, "{width}");
        assert_eq!(rows, 1);
        assert!(elided);
    }
}
