//! A strip of tabs across a pane's top, as a browser's (`.ptabs`): each
//! tab a number, an icon, a name, what is under it once chosen (an
//! address), a dot for its state, a close button; the chosen one on the
//! page's colour with a line in the accent under it. Tabs that don't fit
//! scroll sideways; tools (add, a list of all) stay at the end.

use crate::{thin, Palette};

/// The design's sizes.
pub const STRIP: f32 = 38.0;
const MAX_TAB: f32 = 260.0;

/// A tab as shown.
#[derive(Clone, Debug, Default)]
pub struct StripTab {
    /// A number at its start (the same on both sides of a window that
    /// pairs them).
    pub number: Option<usize>,
    pub glyph: Option<String>,
    pub label: String,
    /// Under the name once chosen (an address).
    pub sub: Option<String>,
    /// A dot after the name: its colour (connected, gone).
    pub dot: Option<egui::Color32>,
    /// Its tooltip.
    pub hint: Option<String>,
    pub closable: bool,
    /// Shown chosen on the other side (when the sides aren't paired).
    pub marked: bool,
}

/// What happened in the strip.
#[derive(Clone, Copy, Debug, Default)]
pub struct StripShown {
    pub clicked: Option<usize>,
    pub closed: Option<usize>,
}

pub struct TabStrip<'a> {
    id: egui::Id,
    palette: &'a Palette,
    accent: egui::Color32,
}

impl<'a> TabStrip<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, palette: &'a Palette) -> TabStrip<'a> {
        TabStrip { id: egui::Id::new(id), palette, accent: palette.accent }
    }

    pub fn accent(mut self, accent: egui::Color32) -> Self {
        self.accent = accent;
        self
    }

    /// `tools` draws the buttons at the strip's end (right to left).
    pub fn show(
        self,
        ui: &mut egui::Ui,
        tabs: &[StripTab],
        chosen: Option<usize>,
        close_hint: &str,
        tools: impl FnOnce(&mut egui::Ui),
    ) -> StripShown {
        let mut shown = StripShown::default();
        let palette = *self.palette;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), STRIP), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, palette.bar);
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, palette.line));
        let mut strip = ui
            .new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        // the tools first, at the end, so the tabs know their room
        let tools_rect = {
            let mut end = strip.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(6.0, 0.0)))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            end.spacing_mut().item_spacing.x = 2.0;
            tools(&mut end);
            end.min_rect()
        };
        let room = egui::Rect::from_min_max(rect.min, egui::pos2(tools_rect.left() - 6.0, rect.bottom()));
        let mut tabs_ui = strip
            .new_child(egui::UiBuilder::new().max_rect(room).layout(egui::Layout::left_to_right(egui::Align::Center)));
        egui::ScrollArea::horizontal()
            .id_salt(self.id)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut tabs_ui, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                for (i, tab) in tabs.iter().enumerate() {
                    let out = self.tab(ui, i, tab, chosen == Some(i), close_hint);
                    if out.0 {
                        shown.clicked = Some(i);
                    }
                    if out.1 {
                        shown.closed = Some(i);
                    }
                }
            });
        shown
    }

    /// One tab: (clicked, closed).
    fn tab(&self, ui: &mut egui::Ui, i: usize, tab: &StripTab, on: bool, close_hint: &str) -> (bool, bool) {
        let palette = self.palette;
        let font = crate::font(ui.ctx(), 12.5, crate::Weight::Semibold);
        let color = if on || tab.marked { palette.text } else { palette.weak };
        let label = ui.painter().layout_no_wrap(tab.label.clone(), font, color);
        let sub = tab
            .sub
            .as_ref()
            .filter(|_| on)
            .map(|s| ui.painter().layout_no_wrap(s.clone(), egui::FontId::monospace(11.0), palette.weak));
        // number, icon, name, address, dot, close
        let mut width = 12.0 + 8.0;
        if tab.number.is_some() {
            width += 18.0 + 8.0;
        }
        if tab.glyph.is_some() {
            width += 15.0 + 8.0;
        }
        width += label.size().x;
        if let Some(s) = &sub {
            width += 8.0 + s.size().x;
        }
        if tab.dot.is_some() {
            width += 8.0 + 6.0;
        }
        if tab.closable {
            width += 8.0 + 18.0;
        }
        let width = width.min(MAX_TAB);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, STRIP), egui::Sense::click());
        let painter = ui.painter_at(rect);
        let hovered = response.hovered();
        if on {
            painter.rect_filled(rect, 0.0, palette.page);
            let line = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.max);
            painter.rect_filled(line, 0.0, self.accent);
        } else if hovered {
            painter.rect_filled(rect, 0.0, thin(palette.text, 12));
        }
        let y = rect.center().y;
        let mut x = rect.left() + 12.0;
        if let Some(n) = tab.number {
            let (fill, text) = if on { (thin(self.accent, 41), self.accent) } else { (palette.card, palette.weak) };
            let chip = egui::Rect::from_min_size(egui::pos2(x, y - 9.0), egui::vec2(18.0, 18.0));
            painter.rect_filled(chip, 4.0, fill);
            painter.text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                n.to_string(),
                egui::FontId::monospace(10.0),
                text,
            );
            x += 18.0 + 8.0;
        }
        if let Some(g) = &tab.glyph {
            let tint = if on { self.accent } else { palette.weak };
            painter.text(egui::pos2(x, y), egui::Align2::LEFT_CENTER, g, egui::FontId::proportional(14.0), tint);
            x += 15.0 + 8.0;
        }
        let close_room = if tab.closable { 8.0 + 18.0 + 8.0 } else { 8.0 };
        let dot_room = if tab.dot.is_some() { 14.0 } else { 0.0 };
        let text_right = rect.right() - close_room - dot_room;
        let label_w = label.size().x;
        let text_clip = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(text_right, rect.bottom()));
        painter.with_clip_rect(text_clip).galley(egui::pos2(x, y - label.size().y / 2.0), label, color);
        x += label_w;
        if let Some(s) = sub {
            x += 8.0;
            let sw = s.size().x;
            painter.with_clip_rect(text_clip).galley(egui::pos2(x, y - s.size().y / 2.0), s, palette.weak);
            x += sw;
        }
        if let Some(dot) = tab.dot {
            let at = egui::pos2((x + 8.0 + 3.0).min(text_right + 8.0), y);
            painter.circle_filled(at, 3.0, dot);
        }
        let mut closed = false;
        if tab.closable {
            let close = egui::Rect::from_center_size(egui::pos2(rect.right() - 8.0 - 9.0, y), egui::vec2(18.0, 18.0));
            let close_response = ui.interact(close, self.id.with(("close", i)), egui::Sense::click());
            if on || hovered || close_response.hovered() {
                if close_response.hovered() {
                    painter.rect_filled(close, 4.0, palette.raised);
                }
                let c = if close_response.hovered() { palette.text } else { palette.weak };
                let s = 3.5;
                let center = close.center();
                let stroke = egui::Stroke::new(1.4, c);
                painter.line_segment([center + egui::vec2(-s, -s), center + egui::vec2(s, s)], stroke);
                painter.line_segment([center + egui::vec2(-s, s), center + egui::vec2(s, -s)], stroke);
            }
            let hint = close_hint.to_string();
            close_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &hint));
            closed = close_response.on_hover_text(close_hint).clicked();
        }
        let label = tab.label.clone();
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, &label));
        let response = match &tab.hint {
            Some(h) => response.on_hover_text(h),
            None => response,
        };
        (response.clicked() && !closed, closed)
    }
}
