//! A row of a list of things that are not files (hosts, folders of them,
//! saved commands), as the design's lists have them: a card that is
//! chosen, a lighter one under the pointer, a line down each level it is
//! under; before the name a checkbox, a chevron (or a dot), a picture;
//! after it a weak word and a sign; at the row's end marks (badges) and,
//! in the fixed font, where it is. What does not fit is left out from the
//! end: the address first, then the marks; the name is cut short last.
//! The view lays the rows out ([`crate::Selection`] keeps what is
//! chosen); this draws one.

use std::sync::Arc;

use crate::Tint;

/// Around the list (`p-3`), before the first level (`level * 20 + 10`),
/// the checkbox (`w-3.5`), the sign that opens a folder (`w-3.5` with
/// `p-0.5`), a picture (`w-4`), between them (`gap-2`); between two rows
/// (`space-y-0.5`).
pub const LIST_PAD: f32 = 12.0;
const ROW_PAD: f32 = 10.0;
const LEVEL: f32 = 20.0;
const CHECK: f32 = 14.0;
const CARET: f32 = 18.0;
pub const PICTURE: f32 = 16.0;
const GAP: f32 = 8.0;
pub const ROW_GAP: f32 = 2.0;
/// The name keeps this much of a row before a mark, and before the
/// address, is left out.
const NAME_BEFORE_MARK: f32 = 120.0;
const NAME_BEFORE_ADDRESS: f32 = 200.0;
/// The marks' and the address's size.
const TINY: f32 = 11.0;

/// The colours a row is drawn in: the program's theme's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemColors {
    /// What a checkbox that is off is filled with, its line, the line
    /// under the pointer.
    pub card: egui::Color32,
    pub line: egui::Color32,
    pub near: egui::Color32,
    /// A checkbox that is on (and under the pointer), its sign.
    pub primary: egui::Color32,
    pub primary_near: egui::Color32,
    pub on_primary: egui::Color32,
    /// The chosen row's card; the row under the pointer.
    pub chosen: Tint,
    pub raised: egui::Color32,
    /// Where what is dragged would land.
    pub accent: egui::Color32,
    /// The lines down the levels.
    pub guide: egui::Color32,
    pub text: egui::Color32,
    pub weak: egui::Color32,
    /// A mark that says nothing about how it goes.
    pub plain: Tint,
}

/// A checkbox: nothing of what the row stands for is chosen, some of it,
/// all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Off,
    Partly,
    On,
}

impl Check {
    pub fn of(chosen: usize, all: usize) -> Check {
        match chosen {
            0 => Check::Off,
            n if n >= all => Check::On,
            _ => Check::Partly,
        }
    }
}

/// What the program paints in a picture's place, given its middle.
pub type Paint<'a> = Box<dyn FnOnce(&egui::Painter, egui::Pos2) + 'a>;

/// The row's picture: a glyph in a colour, or the program's own.
pub enum Picture<'a> {
    Glyph(char, egui::Color32),
    Paint(Paint<'a>),
}

/// What happened to a row: its response, and whether a click on it was
/// on its checkbox.
pub struct ItemShown {
    pub response: egui::Response,
    pub on_check: bool,
}

#[derive(Default)]
pub struct ItemRow<'a> {
    text: &'a str,
    level: usize,
    check: Option<Check>,
    chevron: Option<char>,
    picture: Option<Picture<'a>>,
    after: Option<String>,
    sign: Option<(String, egui::Color32)>,
    stripe: Option<egui::Color32>,
    marks: Vec<(String, Option<Tint>)>,
    address: Option<String>,
    chosen: bool,
    weak: bool,
    draggable: bool,
    drop_here: bool,
}

impl<'a> ItemRow<'a> {
    pub fn new(text: &'a str) -> ItemRow<'a> {
        ItemRow { text, ..ItemRow::default() }
    }

    /// How deep in a tree it is.
    pub fn level(mut self, level: usize) -> Self {
        self.level = level;
        self
    }

    pub fn check(mut self, check: Option<Check>) -> Self {
        self.check = check;
        self
    }

    /// The sign that opens and closes it (a folder); without one, beside
    /// checkboxes, a dot.
    pub fn chevron(mut self, chevron: char) -> Self {
        self.chevron = Some(chevron);
        self
    }

    pub fn picture(mut self, picture: Picture<'a>) -> Self {
        self.picture = Some(picture);
        self
    }

    /// Said after the name, weakly.
    pub fn after(mut self, after: Option<String>) -> Self {
        self.after = after;
        self
    }

    /// A sign right after the name (a star).
    pub fn sign(mut self, glyph: impl Into<String>, color: egui::Color32) -> Self {
        self.sign = Some((glyph.into(), color));
        self
    }

    /// A bar at the row's start (its colour).
    pub fn stripe(mut self, stripe: Option<egui::Color32>) -> Self {
        self.stripe = stripe;
        self
    }

    /// Badges at the row's end, before the address; a mark without a
    /// tint says nothing about how it goes.
    pub fn marks(mut self, marks: Vec<(String, Option<Tint>)>) -> Self {
        self.marks = marks;
        self
    }

    /// Where it is, at the row's end in the fixed font.
    pub fn address(mut self, address: String) -> Self {
        self.address = Some(address);
        self
    }

    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    /// Its name weakly (a row that says there is nothing).
    pub fn weak(mut self) -> Self {
        self.weak = true;
        self
    }

    /// It can be dragged (a click is still a click).
    pub fn draggable(mut self) -> Self {
        self.draggable = true;
        self
    }

    /// Something that could be dropped on it is being carried: the row
    /// shows it would take it while the pointer is on it.
    pub fn drop_target(mut self, carrying: bool) -> Self {
        self.drop_here = carrying;
        self
    }

    pub fn show(self, ui: &mut egui::Ui, colors: &ItemColors, height: f32) -> ItemShown {
        let look = self;
        let width = ui.available_width();
        // a host can be dragged into another folder; a click is still a click
        // (egui only calls it a drag once the pointer has moved)
        let sense = if look.draggable { egui::Sense::click_and_drag() } else { egui::Sense::click() };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), sense);
        let drop_here = look.drop_here && response.contains_pointer();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, look.chosen, look.text)
        });
        let within = rect.shrink2(egui::vec2(LIST_PAD, 0.0));
        let mut x = within.left() + ROW_PAD + look.level as f32 * LEVEL;
        let check_at =
            egui::Rect::from_min_size(egui::pos2(x, rect.center().y - CHECK / 2.0), egui::vec2(CHECK, CHECK));
        // (a little around it counts: it is small)
        let on = |at: Option<egui::Pos2>| at.is_some_and(|at| check_at.expand(5.0).contains(at));
        let on_check = look.check.is_some() && response.clicked() && on(response.interact_pointer_pos());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter().with_clip_rect(rect);
            let middle = rect.center().y;
            let near = response.hovered() || response.highlighted();
            if look.chosen {
                painter.rect_filled(within, 8.0, colors.chosen.fill);
                let line = egui::Stroke::new(1.0_f32, colors.chosen.line);
                painter.rect_stroke(within, 8.0, line, egui::StrokeKind::Inside);
            } else if near {
                painter.rect_filled(within, 8.0, colors.raised);
            }
            if drop_here {
                // where what is carried would land
                painter.rect_filled(within, 8.0, crate::thin(colors.accent, 0x40));
                let line = egui::Stroke::new(1.0_f32, colors.accent);
                painter.rect_stroke(within, 8.0, line, egui::StrokeKind::Inside);
            }
            // a line down each level the row is under, from under that
            // level's checkbox (its chevron, where there are none)
            let first = if look.check.is_some() { CHECK } else { CARET };
            for level in 0..look.level {
                let under = (within.left() + ROW_PAD + level as f32 * LEVEL + first / 2.0).round() + 0.5;
                let whole = egui::Rangef::new(rect.top() - ROW_GAP, rect.bottom());
                ui.painter().vline(under, whole, egui::Stroke::new(1.0_f32, colors.guide));
            }
            if let Some(stripe) = look.stripe {
                let bar = egui::Rect::from_min_size(
                    egui::pos2(within.left() + 2.0, rect.top() + 5.0),
                    egui::vec2(3.0, rect.height() - 10.0),
                );
                painter.rect_filled(bar, 1.5, stripe);
            }
            if let Some(check) = look.check {
                paint_check(&painter, colors, check_at, check, near && on(ui.ctx().pointer_hover_pos()));
                x += CHECK + GAP;
            }
            // a folder's chevron; a host's dot beside the checkboxes (without
            // them, nothing: the picture is where the host begins)
            if look.check.is_some() || look.chevron.is_some() {
                let sign = egui::pos2(x + CARET / 2.0, middle);
                let (glyph, size, color) = match look.chevron {
                    Some(chevron) => (chevron, 12.0, if near { colors.text } else { colors.weak }),
                    None => ('•', 13.0, colors.weak),
                };
                painter.text(sign, egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(size), color);
                x += CARET + GAP;
            }
            if let Some(picture) = look.picture {
                let at = egui::pos2(x + PICTURE / 2.0, middle);
                match picture {
                    Picture::Paint(paint) => paint(&painter, at),
                    Picture::Glyph(glyph, tint) => {
                        let font = egui::FontId::proportional(PICTURE);
                        painter.text(at, egui::Align2::CENTER_CENTER, glyph, font, tint);
                    }
                }
                x += PICTURE + GAP;
            }
            // the row's end, from the right: what does not fit is left out
            let mut end = within.right() - ROW_PAD;
            if let Some(address) = &look.address {
                let galley = painter.layout_no_wrap(address.clone(), egui::FontId::monospace(TINY), colors.weak);
                let start = end - galley.size().x;
                if start - GAP - x >= NAME_BEFORE_ADDRESS {
                    painter.galley(egui::pos2(start, middle - galley.size().y / 2.0), galley, colors.weak);
                    end = start - GAP;
                }
            }
            for (mark, tint) in look.marks.iter().rev() {
                let size = badge_size(&painter, mark);
                let start = end - size.x;
                if start - GAP - x >= NAME_BEFORE_MARK {
                    let at = egui::Rect::from_min_size(egui::pos2(start, middle - size.y / 2.0), size);
                    paint_badge(&painter, at, mark, tint.unwrap_or(colors.plain));
                    end = start - GAP;
                }
            }
            let color = match (look.weak, look.chosen) {
                (true, _) => colors.weak,
                (false, true) => colors.chosen.text,
                (false, false) => colors.text,
            };
            let font = egui::TextStyle::Body.resolve(ui.style());
            let name = elided(&painter, look.text, font.clone(), color, end - x);
            let name_width = name.size().x;
            painter.galley(egui::pos2(x, middle - name.size().y / 2.0), name, color);
            x += name_width;
            if let Some((glyph, tint)) = look.sign {
                let sign = painter.layout_no_wrap(glyph, egui::FontId::proportional(12.0), tint);
                if x + 6.0 + sign.size().x <= end {
                    let width = sign.size().x;
                    painter.galley(egui::pos2(x + 6.0, middle - sign.size().y / 2.0), sign, tint);
                    x += 6.0 + width;
                }
            }
            if let Some(after) = look.after.as_deref().filter(|_| end - x - GAP > 24.0) {
                let after = elided(&painter, after, font, colors.weak, end - x - GAP);
                painter.galley(egui::pos2(x + GAP, middle - after.size().y / 2.0), after, colors.weak);
            }
        }
        ItemShown { response, on_check }
    }
}

fn paint_check(painter: &egui::Painter, colors: &ItemColors, rect: egui::Rect, check: Check, near: bool) {
    if check == Check::Off {
        painter.rect_filled(rect, 4.0, colors.card);
        let line = if near { colors.near } else { colors.line };
        painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside);
        return;
    }
    painter.rect_filled(rect, 4.0, if near { colors.primary_near } else { colors.primary });
    let sign = egui::Stroke::new(1.6_f32, colors.on_primary);
    let at = |x: f32, y: f32| rect.min + egui::vec2(rect.width() * x, rect.height() * y);
    if check == Check::On {
        painter.line_segment([at(0.22, 0.52), at(0.42, 0.72)], sign);
        painter.line_segment([at(0.42, 0.72), at(0.78, 0.30)], sign);
    } else {
        painter.line_segment([at(0.25, 0.5), at(0.75, 0.5)], sign);
    }
}

/// One line of `text`, ending in "…" where it is wider than `width`.
fn elided(
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

/// How large a badge (a mark: a word on a tint) of `text` is.
pub fn badge_size(painter: &egui::Painter, text: &str) -> egui::Vec2 {
    let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(TINY), egui::Color32::WHITE);
    egui::vec2(galley.size().x + 16.0, 18.0)
}

/// A badge at `rect`, in a tint's colours.
pub fn paint_badge(painter: &egui::Painter, rect: egui::Rect, text: &str, tint: Tint) {
    painter.rect_filled(rect, 4.0, tint.fill);
    painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, tint.line), egui::StrokeKind::Inside);
    let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(TINY), tint.text);
    painter.galley(rect.center() - galley.size() / 2.0, galley, tint.text);
}
