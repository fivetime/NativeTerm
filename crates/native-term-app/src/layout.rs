//! The main window's layout, after the design the person brought
//! (`interactive_modern_tree_view_ui-v3.html`, a web page written with
//! Tailwind's classes; the numbers here are those classes', the colours
//! its variables, `looks::Tones`): a rail of icons at the left, a header
//! that is the window's title bar (the system's is gone), the tree under
//! its search field and its filters, a bar below it, and what is chosen
//! at the right. Nothing of it is a picture: rectangles, lines, the text
//! font and the icon font.
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
/// in the middle of an empty place and the header's title (`text-sm`).
pub const SMALL: f32 = 12.0;
pub const TINY: f32 = 11.0;
pub const MIDDLE: f32 = 14.0;

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

/// What a tile's picture is 24 a side.
pub const TILE_PICTURE: f32 = 24.0;

/// A square with an icon in it: the header's (the accent's colours) and
/// the chosen thing's (a button's). With `logo` a picture is in the
/// icon's place (`logos.rs`).
pub fn tile(
    ui: &mut egui::Ui,
    side: f32,
    icon: char,
    logo: Option<(egui::TextureId, egui::Color32)>,
    (fill, line, color): (egui::Color32, egui::Color32, egui::Color32),
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 12.0, fill);
        painter.rect_stroke(rect, 12.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside);
        match logo {
            Some((logo, tint)) => {
                let place = crate::logos::place(ui.ctx(), rect.center(), TILE_PICTURE);
                crate::logos::paint(painter, logo, place, tint);
            }
            None => {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon, font(TILE_PICTURE), color);
            }
        }
    }
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
    /// The header's (`px-3 py-1.5`).
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
        Room::Header => (12.0, 28.0),
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
    let galley = elided(ui.painter(), text, font(SMALL), tones.weak, CHIP_MOST - 24.0);
    let size = egui::vec2(galley.size().x + 24.0, CHIP);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if galley.elided {
        // (all of it, where the chip has its beginning only)
        response = response.on_hover_text(text);
    }
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

/// A chip's height, how wide one is at most (a longer text ends in
/// dots), and how wide one with `text` in it is.
pub const CHIP: f32 = 26.0;
pub const CHIP_MOST: f32 = 240.0;

#[must_use]
pub fn chip_width(ui: &egui::Ui, text: &str) -> f32 {
    elided(ui.painter(), text, font(SMALL), egui::Color32::WHITE, CHIP_MOST - 24.0).size().x + 24.0
}

/// How many lines chips as wide as `widths` take in a row `room` wide,
/// `between` of room between two of them, `before` of the first line
/// taken by what is said before them.
#[must_use]
pub fn chip_lines(widths: &[f32], before: f32, room: f32, between: f32) -> usize {
    let mut lines = 1;
    let mut used = before;
    for width in widths {
        if used + between + width > room {
            lines += 1;
            used = *width;
        } else {
            used += between + width;
        }
    }
    lines
}

fn caption_text(tones: &Tones, text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).size(TINY).color(tones.weak).extra_letter_spacing(0.6)
}

/// What is said before a row of chips, and above a part of what is
/// chosen (`text-[11px] font-semibold uppercase tracking-wider`).
pub fn caption(ui: &mut egui::Ui, tones: &Tones, text: &str) {
    ui.label(caption_text(tones, text));
}

/// How wide `caption` is with `text`.
#[must_use]
pub fn caption_width(ui: &egui::Ui, tones: &Tones, text: &str) -> f32 {
    let text = egui::WidgetText::from(caption_text(tones, text));
    text.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x
}

/// What opens a row of chips that has more of them than its one line
/// shows, and closes it again: two chevrons, down while it is closed
/// and up while it is open.
pub fn fold_button(ui: &mut egui::Ui, tones: &Tones, open: bool, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(CHIP, CHIP), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, hint));
    if ui.is_rect_visible(rect) {
        let near = response.hovered();
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, if near { tones.raised } else { tones.chip });
        let line = if near { tones.near } else { tones.line };
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside);
        let stroke = egui::Stroke::new(1.5_f32, if near { tones.text } else { tones.weak });
        // (each 8 wide and 4 high, 5 from the other; their points down, or up)
        let way = if open { -1.0 } else { 1.0 };
        for middle in [-2.5_f32, 2.5] {
            let tip = rect.center() + egui::vec2(0.0, middle + 2.0 * way);
            let arms = [tip + egui::vec2(-4.0, -4.0 * way), tip, tip + egui::vec2(4.0, -4.0 * way)];
            painter.add(egui::Shape::line(arms.to_vec(), stroke));
        }
    }
    response.on_hover_text(hint)
}

/// What a switch at the end of the chips' row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// The checkboxes, shown (`on`) or not: a box with a tick in it.
    Checks { on: bool },
    /// Every folder opened: two arrows away from a line.
    OpenAll,
    /// Every folder closed: two arrows toward a line.
    CloseAll,
}

/// A switch's side, and how wide two of them are in their frame (`p-0.5`
/// around them, a line of 1 between them, `gap-1`).
pub const SWITCH: f32 = 22.0;
pub const SWITCHES: f32 = 2.0 * SWITCH + 2.0 * 4.0 + 1.0 + 2.0 * 2.0;

/// The two switches' frame at `rect` (`bg-dark-800/80 border rounded-md`),
/// and the line between them.
pub fn switches_frame(painter: &egui::Painter, tones: &Tones, rect: egui::Rect) {
    painter.rect_filled(rect, 6.0, tones.chip);
    painter.rect_stroke(rect, 6.0, egui::Stroke::new(1.0_f32, tones.line), egui::StrokeKind::Inside);
    let middle = rect.center().x;
    painter.vline(middle, rect.center().y - 6.0..=rect.center().y + 6.0, egui::Stroke::new(1.0_f32, tones.line));
}

/// A switch at `rect`: drawn with lines, as the window's buttons are.
pub fn switch(ui: &mut egui::Ui, tones: &Tones, rect: egui::Rect, what: Switch, hint: &str) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(("switch", hint)), egui::Sense::click());
    let on = what == Switch::Checks { on: true };
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, hint));
    if ui.is_rect_visible(rect) {
        let near = response.hovered();
        let painter = ui.painter();
        if on {
            painter.rect_filled(rect, 4.0, tones.chip_on.fill);
        } else if near {
            painter.rect_filled(rect, 4.0, tones.raised);
        }
        let color = match (on, near) {
            (true, _) => tones.chip_on.text,
            (false, true) => tones.text,
            (false, false) => tones.weak,
        };
        let stroke = egui::Stroke::new(1.5_f32, color);
        let c = rect.center();
        let at = |x: f32, y: f32| c + egui::vec2(x, y);
        match what {
            Switch::Checks { on } => {
                // (a box of 11 with a tick; the tick's end out of it, as
                // Phosphor's check-square-offset)
                let square = egui::Rect::from_center_size(at(-0.5, 0.5), egui::Vec2::splat(10.0));
                painter.rect_stroke(square, 2.0, stroke, egui::StrokeKind::Middle);
                painter.add(egui::Shape::line(vec![at(-3.0, 0.5), at(-0.5, 3.0), at(5.5, -4.5)], stroke));
                if on {
                    // the dot at its corner (`bg-brand-500`)
                    painter.circle_filled(rect.right_top() + egui::vec2(-2.0, 2.0), 2.5, tones.accent);
                }
            }
            Switch::OpenAll | Switch::CloseAll => {
                // a line across, and an arrow above and under it: away
                // from it to open, toward it to close
                painter.hline(c.x - 5.0..=c.x + 5.0, c.y, stroke);
                let away = what == Switch::OpenAll;
                for side in [-1.0_f32, 1.0] {
                    let (from, to) = if away { (2.5, 7.5) } else { (7.5, 2.5) };
                    painter.line_segment([at(0.0, side * from), at(0.0, side * to)], stroke);
                    let tip = at(0.0, side * to);
                    let back = if away { -side } else { side };
                    painter.add(egui::Shape::line(
                        vec![tip + egui::vec2(-2.5, back * 2.5), tip, tip + egui::vec2(2.5, back * 2.5)],
                        stroke,
                    ));
                }
            }
        }
    }
    response.on_hover_text(hint)
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
