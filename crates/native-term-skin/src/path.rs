//! A path as crumbs to click (`.path`, `.crumbs`): each folder of it a
//! step back to it, the last one the folder shown; a click beside them
//! (or F4, as Explorer's) makes it a field to type a path into, Enter
//! goes there, Esc or a click elsewhere puts the crumbs back. A long path
//! shows its first folder, "…", and its last three.

use crate::Palette;

/// What the path bar was asked for.
#[derive(Clone, Debug, Default)]
pub struct PathShown {
    /// A crumb clicked: the folder at that place of the path.
    pub crumb: Option<usize>,
    /// A path typed and Enter pressed.
    pub typed: Option<String>,
}

pub struct PathBar<'a> {
    id: egui::Id,
    palette: &'a Palette,
}

/// Crumbs shown at most (the first, "…", the last ones).
const SHOWN: usize = 5;

impl<'a> PathBar<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, palette: &'a Palette) -> PathBar<'a> {
        PathBar { id: egui::Id::new(id), palette }
    }

    /// `crumbs`: the path's folders, the first being its root (`C:`,
    /// `/`); `full`: the path as typed (what the field starts with);
    /// `width`: how wide the bar is; `edit`: start typing (F4).
    pub fn show(self, ui: &mut egui::Ui, crumbs: &[String], full: &str, width: f32, edit: bool) -> PathShown {
        let mut shown = PathShown::default();
        let palette = self.palette;
        let editing_id = self.id.with("editing");
        let text_id = self.id.with("text");
        let mut editing = ui.data(|d| d.get_temp::<bool>(editing_id)).unwrap_or(false);
        if edit && !editing {
            editing = true;
            ui.data_mut(|d| d.insert_temp(text_id, full.to_string()));
        }
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
        let mono = egui::FontId::monospace(12.0);
        if editing {
            let mut text = ui.data(|d| d.get_temp::<String>(text_id)).unwrap_or_else(|| full.to_string());
            ui.painter().rect_filled(rect, 6.0, palette.card);
            ui.painter().rect_stroke(rect, 6.0, egui::Stroke::new(1.0, palette.accent), egui::StrokeKind::Inside);
            let inner = rect.shrink2(egui::vec2(8.0, 0.0));
            let field = egui::TextEdit::singleline(&mut text)
                .id(self.id.with("field"))
                .frame(egui::Frame::NONE)
                .font(mono)
                .vertical_align(egui::Align::Center)
                .desired_width(inner.width());
            // (in a child of its own: `put` would move the bar's cursor
            // back to the field's end, and what comes next would be drawn
            // over the box's right edge)
            let layout = egui::Layout::centered_and_justified(egui::Direction::TopDown);
            let field = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(layout)).add(field);
            let started = ui.data(|d| d.get_temp::<bool>(self.id.with("focused"))).is_none();
            if started {
                field.request_focus();
                // all of it chosen, to type over
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field.id) {
                    let all = egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(text.chars().count()),
                    );
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), field.id);
                }
                ui.data_mut(|d| d.insert_temp(self.id.with("focused"), true));
            }
            let done = field.lost_focus() && !started;
            if done {
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    shown.typed = Some(text.clone());
                }
                editing = false;
                ui.data_mut(|d| d.remove::<bool>(self.id.with("focused")));
            }
            ui.data_mut(|d| d.insert_temp(text_id, text));
        } else {
            if response.hovered() {
                ui.painter().rect_filled(rect, 6.0, palette.card);
            }
            // all of them if they fit; else the first, "…", and as many of
            // the last ones as fit (at most a few)
            let n = crumbs.len();
            let painter = ui.painter_at(rect);
            let widths: Vec<f32> = crumbs
                .iter()
                .map(|c| painter.layout_no_wrap(c.clone(), mono.clone(), palette.text).size().x + 10.0)
                .collect();
            let room = rect.width() - 12.0;
            let span = |places: &[Option<usize>]| -> f32 {
                places.iter().map(|p| p.map_or(22.0, |i| widths[i])).sum::<f32>()
                    + 12.0 * places.len().saturating_sub(1) as f32
            };
            let mut places: Vec<Option<usize>> = (0..n).map(Some).collect();
            if n > SHOWN || span(&places) > room {
                let mut from = n.saturating_sub(SHOWN - 2).max(1);
                loop {
                    places = vec![Some(0), None];
                    places.extend((from..n).map(Some));
                    if span(&places) <= room || from + 1 >= n {
                        break;
                    }
                    from += 1;
                }
            }
            let mut x = rect.left() + 6.0;
            let y = rect.center().y;
            let mut over_crumb = false;
            for (k, place) in places.iter().enumerate() {
                if k > 0 {
                    // a chevron between
                    let at = egui::pos2(x + 5.0, y);
                    let s = 2.5;
                    let stroke = egui::Stroke::new(1.2, palette.weak.gamma_multiply(0.6));
                    painter.line_segment([at + egui::vec2(-s / 2.0, -s), at + egui::vec2(s / 2.0, 0.0)], stroke);
                    painter.line_segment([at + egui::vec2(s / 2.0, 0.0), at + egui::vec2(-s / 2.0, s)], stroke);
                    x += 12.0;
                }
                let last = k + 1 == places.len();
                let text = place.map_or("…".to_string(), |i| crumbs[i].clone());
                let color = if last { palette.text } else { palette.weak };
                let galley = painter.layout_no_wrap(text, mono.clone(), color);
                let r = egui::Rect::from_min_size(egui::pos2(x, y - 10.0), egui::vec2(galley.size().x + 10.0, 20.0));
                if r.left() > rect.right() {
                    break;
                }
                if let Some(i) = place {
                    let crumb = ui.interact(r.intersect(rect), self.id.with(("crumb", i)), egui::Sense::click());
                    if crumb.hovered() {
                        painter.rect_filled(r, 4.0, palette.raised);
                        over_crumb = true;
                    }
                    let name = crumbs[*i].clone();
                    crumb.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                    if crumb.clicked() {
                        shown.crumb = Some(*i);
                    }
                }
                let gw = galley.size().x;
                painter.galley(
                    egui::pos2(x + 5.0, y - galley.size().y / 2.0),
                    galley,
                    if last { palette.text } else { palette.weak },
                );
                x += gw + 10.0;
            }
            if response.clicked() && !over_crumb && shown.crumb.is_none() {
                editing = true;
                ui.data_mut(|d| d.insert_temp(text_id, full.to_string()));
            }
            response.on_hover_text(full);
        }
        ui.data_mut(|d| d.insert_temp(editing_id, editing));
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let c = egui::Color32::GRAY;
        let tint = crate::Tint { fill: c, line: c, text: c };
        Palette {
            page: c,
            bar: c,
            line: c,
            card: c,
            raised: c,
            text: c,
            weak: c,
            danger: c,
            primary: c,
            on_primary: c,
            tile: tint,
            rail: c,
            rail_line: c,
            accent: c,
            rail_near: c,
            backdrop: c,
        }
    }

    /// What follows the bar starts after it, while a path is typed too
    /// (the field's own placing moved the row back over the box's end).
    #[test]
    fn what_follows_starts_after_the_bar() {
        let palette = palette();
        let ctx = egui::Context::default();
        let crumbs = ["/".to_string(), "root".to_string(), "build".to_string()];
        for edit in [false, true] {
            let mut gap = None;
            for _ in 0..2 {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let start = ui.cursor().left();
                        PathBar::new("path", &palette).show(ui, &crumbs, "/root/build", 300.0, edit);
                        let next = ui.label("filter");
                        gap = Some(next.rect.left() - (start + 300.0));
                    });
                });
                output.textures_delta.clear();
            }
            assert_eq!(gap, Some(2.0), "typing: {edit}");
        }
    }

    /// The same after the filter field.
    #[test]
    fn what_follows_starts_after_the_filter() {
        let palette = palette();
        let ctx = egui::Context::default();
        let mut gap = None;
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let start = ui.cursor().left();
                    crate::filter_field(ui, &palette, &mut String::new(), "?", "filter", 140.0);
                    gap = Some(ui.label("star").rect.left() - (start + 140.0));
                });
            });
            output.textures_delta.clear();
        }
        assert_eq!(gap, Some(2.0));
    }
}
