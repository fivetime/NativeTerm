//! Small controls a window's bars are made of, in the design's look:
//! a button that is only an icon, a field to filter by, a segmented
//! switch, a badge, a count. egui's own button and field are under
//! them; what the skin adds is the look (no frame until the pointer is
//! on it, the icon inside the field, the switch's sliding tile).

use crate::{thin, Palette};

/// The design's sizes (`.icon-btn`, `.icon-btn.sm`).
pub const ICON_BUTTON: f32 = 30.0;
pub const SMALL_ICON_BUTTON: f32 = 24.0;

/// What an icon button is for: anything, or taking something away (red
/// under the pointer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Danger,
}

/// A button that is only an icon: no frame until the pointer is on it;
/// its hint is its tooltip and what a screen reader says.
pub struct IconButton {
    glyph: String,
    hint: String,
    size: f32,
    kind: Kind,
    enabled: bool,
    on: bool,
    /// Round, outlined, filled with this colour under the pointer
    /// (`.rail button`).
    round: Option<egui::Color32>,
}

impl IconButton {
    pub fn new(glyph: impl Into<String>, hint: impl Into<String>) -> IconButton {
        IconButton {
            glyph: glyph.into(),
            hint: hint.into(),
            size: ICON_BUTTON,
            kind: Kind::Plain,
            enabled: true,
            on: false,
            round: None,
        }
    }

    /// Round and outlined, filled with `hover` under the pointer.
    pub fn round(mut self, hover: egui::Color32) -> Self {
        self.round = Some(hover);
        self
    }

    /// The small one (`.icon-btn.sm`).
    pub fn small(mut self) -> Self {
        self.size = SMALL_ICON_BUTTON;
        self
    }

    pub fn danger(mut self) -> Self {
        self.kind = Kind::Danger;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Drawn as switched on.
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn show(self, ui: &mut egui::Ui, palette: &Palette) -> egui::Response {
        let IconButton { glyph, hint, size, kind, enabled, on, round } = self;
        let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), sense);
        let hovered = enabled && response.hovered();
        let (fill, color) = match (kind, hovered, on) {
            (Kind::Danger, true, _) => (thin(palette.danger, 36), palette.danger),
            (_, true, _) => (palette.raised, palette.text),
            (_, false, true) => (palette.card, palette.text),
            _ => (egui::Color32::TRANSPARENT, palette.weak),
        };
        let (fill, color) = match round {
            Some(hover_fill) if hovered => (hover_fill, palette.on_primary),
            Some(_) if on => (thin(palette.accent, 41), palette.accent),
            Some(_) => (palette.page, palette.weak),
            None => (fill, color),
        };
        let color = if enabled { color } else { palette.weak.gamma_multiply(0.45) };
        if round.is_some() {
            let stroke = egui::Stroke::new(1.0, if hovered { fill } else { palette.line });
            ui.painter().circle(rect.center(), size / 2.0 - 0.5, fill, stroke);
        } else {
            let radius = if size >= ICON_BUTTON { 8.0 } else { 6.0 };
            ui.painter().rect_filled(rect, radius, fill);
        }
        let font = egui::FontId::proportional((size * 0.5).round().max(12.0));
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, font, color);
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, &hint));
        if enabled {
            response.on_hover_text(hint)
        } else {
            response.on_disabled_hover_text(hint)
        }
    }
}

/// A field to filter by: an icon in it, `hint` while empty, the accent
/// round it while typed in. Returns the field's response.
pub fn filter_field(
    ui: &mut egui::Ui,
    palette: &Palette,
    text: &mut String,
    glyph: &str,
    hint: &str,
    width: f32,
) -> egui::Response {
    let height = 26.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 6.0, palette.card);
    let painter = ui.painter_at(rect.expand(1.0));
    painter.text(
        egui::pos2(rect.left() + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        glyph,
        egui::FontId::proportional(12.0),
        palette.weak,
    );
    let inner = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 26.0, rect.top()),
        egui::pos2(rect.right() - 6.0, rect.bottom()),
    );
    let edit = egui::TextEdit::singleline(text)
        .frame(egui::Frame::NONE)
        .hint_text(egui::RichText::new(hint).color(palette.weak))
        .font(egui::FontId::proportional(12.0))
        .vertical_align(egui::Align::Center)
        .desired_width(inner.width());
    // (in a child of its own: `put` would move the row back to the
    // field's end, under the box's right edge)
    let layout = egui::Layout::centered_and_justified(egui::Direction::TopDown);
    let response = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(layout)).add(edit);
    let stroke = if response.has_focus() { egui::Stroke::new(1.0, palette.accent) } else { egui::Stroke::NONE };
    ui.painter().rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Inside);
    response
}

/// A pill of text in a tint (a state: running, waiting, done).
pub fn badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32, color: egui::Color32) -> egui::Response {
    let font = crate::font(ui.ctx(), 11.0, crate::Weight::Medium);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = egui::vec2(galley.size().x + 16.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

/// A small count in a pill (`.count`); `on` in the accent.
pub fn count(ui: &mut egui::Ui, palette: &Palette, n: usize, on: bool) -> egui::Response {
    let (fill, color) = if on { (palette.accent, palette.on_primary) } else { (palette.line, palette.weak) };
    let galley = ui.painter().layout_no_wrap(n.to_string(), egui::FontId::monospace(10.0), color);
    let size = egui::vec2((galley.size().x + 10.0).max(18.0), 16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 8.0, fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

/// One of a segmented switch's choices: an icon, a name, a count.
pub struct Segment<'a> {
    pub glyph: &'a str,
    pub label: &'a str,
    pub count: Option<usize>,
}

/// A segmented switch (`.seg`): the choices side by side on a tile, the
/// one chosen raised. Returns the one clicked.
pub fn segmented(ui: &mut egui::Ui, palette: &Palette, segments: &[Segment<'_>], chosen: usize) -> Option<usize> {
    let mut clicked = None;
    let pad = 3.0;
    let font = crate::font(ui.ctx(), 12.0, crate::Weight::Medium);
    let glyph_font = egui::FontId::proportional(13.0);
    // measure, then draw on one tile
    let widths: Vec<f32> = segments
        .iter()
        .map(|s| {
            let label = ui.painter().layout_no_wrap(s.label.to_string(), font.clone(), palette.text).size().x;
            let count = s.count.map_or(0.0, |n| 6.0 + (n.to_string().len() as f32 * 6.5 + 10.0).max(18.0));
            20.0 + 19.0 + label + count
        })
        .collect();
    let total = widths.iter().sum::<f32>() + pad * 2.0 + 2.0 * (segments.len().saturating_sub(1)) as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, 32.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 8.0, palette.card);
    let mut x = rect.left() + pad;
    for (i, (segment, w)) in segments.iter().zip(&widths).enumerate() {
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top() + pad), egui::vec2(*w, rect.height() - 2.0 * pad));
        let id = ui.id().with(("segment", i, segment.label));
        let response = ui.interact(r, id, egui::Sense::click());
        let on = i == chosen;
        if on {
            ui.painter().rect_filled(r, 6.0, palette.page);
        }
        let color = if on || response.hovered() { palette.text } else { palette.weak };
        let mut tx = r.left() + 10.0;
        ui.painter().text(
            egui::pos2(tx, r.center().y),
            egui::Align2::LEFT_CENTER,
            segment.glyph,
            glyph_font.clone(),
            color,
        );
        tx += 19.0;
        let galley = ui.painter().layout_no_wrap(segment.label.to_string(), font.clone(), color);
        let gw = galley.size().x;
        ui.painter().galley(egui::pos2(tx, r.center().y - galley.size().y / 2.0), galley, color);
        tx += gw + 6.0;
        if let Some(n) = segment.count {
            let (fill, text) = if on { (palette.accent, palette.on_primary) } else { (palette.line, palette.weak) };
            let g = ui.painter().layout_no_wrap(n.to_string(), egui::FontId::monospace(10.0), text);
            let pill = egui::Rect::from_min_size(
                egui::pos2(tx, r.center().y - 8.0),
                egui::vec2((g.size().x + 10.0).max(18.0), 16.0),
            );
            ui.painter().rect_filled(pill, 8.0, fill);
            ui.painter().galley(pill.center() - g.size() / 2.0, g, text);
        }
        let label = segment.label.to_string();
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, &label));
        if response.clicked() {
            clicked = Some(i);
        }
        x += w + 2.0;
    }
    clicked
}
