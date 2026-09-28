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
/// The header, which is the window's title bar (`h-14 px-4`), and its
/// tile: an icon of 16 with `p-1.5` around it.
pub const TITLE_BAR: f32 = 56.0;
pub const TITLE_PAD: i8 = 16;
const HEADER_TILE: f32 = 28.0;
/// One of the window's buttons (`p-1.5` around an icon of 14), and what
/// is between two of them (`space-x-1`).
const CAPTION: f32 = 26.0;
const CAPTION_GAP: f32 = 4.0;
/// The window's buttons as macOS has them (`w-3 h-3 rounded-full`,
/// `gap-2`).
const DOT: f32 = 12.0;
const DOT_GAP: f32 = 8.0;
/// Where the window is taken by its edge to be made larger or smaller:
/// the band along each edge and the corners' reach along it, as Chrome
/// has them for a frame of its own without a shadow
/// (`kFrameBorderThickness`, `kResizeAreaCornerSize` in
/// `ui/views/window/default_frame_view.cc`).
pub const FRAME_BAND: f32 = 4.0;
pub const FRAME_CORNER: f32 = 16.0;
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

/// The header's tile (`p-1.5 rounded-lg`, the accent's colours).
pub fn header_tile(ui: &mut egui::Ui, tones: &Tones, icon: char) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(HEADER_TILE, HEADER_TILE), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, tones.tile.fill);
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.0_f32, tones.tile.line), egui::StrokeKind::Inside);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon, font(16.0), tones.tile.text);
    }
}

/// The header's title, `width` wide (`text-sm`): what is shown. Nothing
/// happens to it: the pointer takes the window by it as by the rest of
/// the header.
pub fn header_title(ui: &mut egui::Ui, tones: &Tones, title: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(0.0), HEADER_TILE), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect);
        let title = elided(&painter, title, font(MIDDLE), tones.text, rect.width());
        painter.galley(egui::pos2(rect.left(), rect.center().y - title.size().y / 2.0), title, tones.text);
    }
}

/// One of the window's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caption {
    Minimize,
    Maximize,
    /// What "maximize" is while the window is maximized.
    Restore,
    Close,
}

impl Caption {
    fn hint(self) -> String {
        match self {
            Caption::Minimize => native_term_app::t!("window-minimize"),
            Caption::Maximize => native_term_app::t!("window-maximize"),
            Caption::Restore => native_term_app::t!("window-restore"),
            Caption::Close => native_term_app::t!("window-close"),
        }
    }

    /// Its sign, drawn in a square `side` wide around `middle`: the
    /// design's icons (a line, a square, two squares, a cross), which
    /// are lines a twelfth of their size thick.
    fn paint(self, painter: &egui::Painter, middle: egui::Pos2, side: f32, color: egui::Color32) {
        let line = egui::Stroke::new((side / 12.0).max(1.0), color);
        // (the icons are drawn on a square of 24)
        let at = |x: f32, y: f32| middle + egui::vec2(x - 12.0, y - 12.0) * (side / 24.0);
        match self {
            Caption::Minimize => {
                painter.line_segment([at(5.0, 12.0), at(19.0, 12.0)], line);
            }
            Caption::Maximize => {
                let square = egui::Rect::from_min_max(at(3.0, 3.0), at(21.0, 21.0));
                painter.rect_stroke(square, side / 12.0, line, egui::StrokeKind::Middle);
            }
            Caption::Restore => {
                let front = egui::Rect::from_min_max(at(8.0, 8.0), at(22.0, 22.0));
                painter.rect_stroke(front, side / 12.0, line, egui::StrokeKind::Middle);
                let behind = [at(4.0, 16.0), at(2.0, 14.0), at(2.0, 4.0), at(4.0, 2.0), at(14.0, 2.0), at(16.0, 4.0)];
                painter.add(egui::Shape::line(behind.to_vec(), line));
            }
            Caption::Close => {
                painter.line_segment([at(6.0, 6.0), at(18.0, 18.0)], line);
                painter.line_segment([at(18.0, 6.0), at(6.0, 18.0)], line);
            }
        }
    }
}

/// One of the window's buttons as Windows and the Linux desktops have
/// them (`p-1.5 rounded-lg hover:bg-gray-500/10`; the one that closes
/// `hover:bg-red-500 hover:text-white`).
fn caption_button(ui: &mut egui::Ui, tones: &Tones, caption: Caption) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(CAPTION, CAPTION), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, caption.hint()));
    if ui.is_rect_visible(rect) {
        let near = response.hovered();
        let (fill, color) = match (near, caption) {
            (false, _) => (egui::Color32::TRANSPARENT, tones.weak),
            (true, Caption::Close) => (tones.danger, egui::Color32::WHITE),
            (true, _) => (thin(egui::Color32::from_rgb(0x6b, 0x72, 0x80), 0x1a), tones.weak),
        };
        ui.painter().rect_filled(rect, 8.0, fill);
        // (the square is `w-3` where the others are `w-3.5`)
        let side = if matches!(caption, Caption::Maximize | Caption::Restore) { 12.0 } else { 14.0 };
        caption.paint(ui.painter(), rect.center(), side, color);
    }
    response.on_hover_text(caption.hint())
}

/// The window's buttons at the header's end, from the right (the layout
/// they are put into goes that way): a line before them (`pl-2
/// border-l`). The one that was clicked.
pub fn caption_buttons(ui: &mut egui::Ui, tones: &Tones, maximized: bool) -> Option<Caption> {
    let mut clicked = None;
    let middle = if maximized { Caption::Restore } else { Caption::Maximize };
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = CAPTION_GAP;
        for caption in [Caption::Close, middle, Caption::Minimize] {
            if caption_button(ui, tones, caption).clicked() {
                clicked = Some(caption);
            }
        }
        ui.add_space(8.0 - CAPTION_GAP);
        let (line, _) = ui.allocate_exact_size(egui::vec2(1.0, CAPTION), egui::Sense::hover());
        ui.painter().rect_filled(line, 0.0, tones.line);
    });
    clicked
}

/// The window's buttons as macOS has them, at the header's start: three
/// dots (`bg-red-500`, `bg-amber-500`, `bg-emerald-500`; a shade darker
/// under the pointer), their signs in them while the pointer is on any
/// of them; without their colours while the window is not the one in
/// front, as the system's are. The one that was clicked.
pub fn caption_dots(ui: &mut egui::Ui, tones: &Tones, focused: bool) -> Option<Caption> {
    const DOTS: [(Caption, u32, u32, u32); 3] = [
        (Caption::Close, 0xef4444, 0xdc2626, 0x450a0a),
        (Caption::Minimize, 0xf59e0b, 0xd97706, 0x451a03),
        (Caption::Maximize, 0x10b981, 0x059669, 0x022c22),
    ];
    let rgb = |hex: u32| egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    let width = 3.0 * DOT + 2.0 * DOT_GAP;
    let (all, _) = ui.allocate_exact_size(egui::vec2(width, DOT), egui::Sense::hover());
    let over_any = ui.rect_contains_pointer(all.expand(2.0));
    let mut clicked = None;
    for (i, (caption, color, near, sign)) in DOTS.into_iter().enumerate() {
        let left = all.left() + i as f32 * (DOT + DOT_GAP);
        let rect = egui::Rect::from_min_size(egui::pos2(left, all.top()), egui::vec2(DOT, DOT));
        let response = ui.interact(rect.expand(2.0), ui.id().with(("dot", i)), egui::Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, caption.hint()));
        let fill = match (focused || over_any, response.hovered()) {
            (false, _) => tones.line,
            (true, false) => rgb(color),
            (true, true) => rgb(near),
        };
        ui.painter().circle_filled(rect.center(), DOT / 2.0, fill);
        if over_any {
            // (`w-2` with lines of three: thicker than the others')
            caption.paint(ui.painter(), rect.center(), 7.0, rgb(sign));
        }
        if response.on_hover_text(caption.hint()).clicked() {
            clicked = Some(caption);
        }
    }
    clicked
}

/// Which way the window is made larger or smaller when it is taken at
/// `point` (from its top left corner; it is `size` large), if it is
/// taken by its edge there: Chrome's `FrameView::GetHTComponentForFrame`
/// with `FRAME_BAND` along every edge and `FRAME_CORNER` for the corners.
#[must_use]
pub fn frame_hit(point: egui::Vec2, size: egui::Vec2) -> Option<egui::viewport::ResizeDirection> {
    use egui::viewport::ResizeDirection as To;
    let mut top = point.y < FRAME_BAND;
    let bottom = point.y >= size.y - FRAME_BAND;
    let mut left = point.x < FRAME_BAND;
    let mut right = point.x >= size.x - FRAME_BAND;
    if !(top || bottom || left || right) {
        return None;
    }
    // (in a band: the corners reach further along it)
    top |= point.y < FRAME_CORNER;
    left |= point.x < FRAME_CORNER;
    right |= point.x >= size.x - FRAME_CORNER;
    Some(match (top, bottom, left, right) {
        (true, _, true, _) => To::NorthWest,
        (true, _, _, true) => To::NorthEast,
        (true, _, _, _) => To::North,
        (_, true, true, _) => To::SouthWest,
        (_, true, _, true) => To::SouthEast,
        (_, true, _, _) => To::South,
        (_, _, true, _) => To::West,
        _ => To::East,
    })
}

/// The pointer's shape over an edge the window is taken by.
#[must_use]
pub fn frame_cursor(to: egui::viewport::ResizeDirection) -> egui::CursorIcon {
    use egui::viewport::ResizeDirection as To;
    match to {
        To::North | To::South => egui::CursorIcon::ResizeVertical,
        To::East | To::West => egui::CursorIcon::ResizeHorizontal,
        To::NorthWest | To::SouthEast => egui::CursorIcon::ResizeNwSe,
        To::NorthEast | To::SouthWest => egui::CursorIcon::ResizeNeSw,
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
    fn the_window_is_taken_by_its_edges() {
        use egui::viewport::ResizeDirection as To;
        let size = egui::vec2(800.0, 600.0);
        let at = |x: f32, y: f32| frame_hit(egui::vec2(x, y), size);
        assert_eq!(at(400.0, 300.0), None, "in the window");
        assert_eq!(at(400.0, 4.0), None, "the header, under the band");
        assert_eq!(at(400.0, 3.0), Some(To::North));
        assert_eq!(at(400.0, 596.0), Some(To::South));
        assert_eq!(at(3.0, 300.0), Some(To::West));
        assert_eq!(at(796.0, 300.0), Some(To::East));
        // the corners reach 16 along a band
        assert_eq!(at(15.0, 3.0), Some(To::NorthWest));
        assert_eq!(at(3.0, 15.0), Some(To::NorthWest));
        assert_eq!(at(16.0, 3.0), Some(To::North));
        assert_eq!(at(790.0, 2.0), Some(To::NorthEast));
        assert_eq!(at(10.0, 598.0), Some(To::SouthWest));
        assert_eq!(at(799.0, 599.0), Some(To::SouthEast));
        // (as in Chrome: from the side a lower corner begins at the band below)
        assert_eq!(at(2.0, 590.0), Some(To::West));
        assert_eq!(frame_cursor(To::NorthWest), egui::CursorIcon::ResizeNwSe);
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
