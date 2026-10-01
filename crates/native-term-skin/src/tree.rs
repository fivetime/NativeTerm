//! A tree of folders (or anything nested), as a file manager's
//! navigation pane: a chevron to open and close what has something in
//! it, a line down beside each level, the current one in the accent's
//! tint with a bar at its start, what is still being read with a
//! spinner, only the rows in view drawn. The keys: Up and Down move, Left
//! closes (or goes up a level), Right opens (or goes into it). egui has
//! collapsing headers, no tree: the program lays its tree out as rows
//! (the open ones' children after them) and this draws them.

use crate::{thin, Palette};

/// The design's sizes (`.node`, `.children`).
pub const NODE: f32 = 28.0;
pub const INDENT: f32 = 14.0;
const PAD: f32 = 6.0;
const CHEVRON: f32 = 14.0;

/// Whether a row has rows under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kids {
    No,
    Yes,
    /// Not read yet (it may).
    Unknown,
}

/// A row of the tree as laid out: what it is, how deep, its name and
/// icon (a glyph and its colour), and its state.
#[derive(Clone, Debug)]
pub struct TreeRow<K> {
    pub key: K,
    pub depth: usize,
    pub label: String,
    pub icon: Option<(String, egui::Color32)>,
    pub kids: Kids,
    pub open: bool,
    pub loading: bool,
}

/// What happened in the tree: a row to go to (clicked, or moved to with
/// the keys), a row to open or close.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeShown {
    pub go: Option<usize>,
    pub toggle: Option<usize>,
}

pub struct TreeView<'a> {
    id: egui::Id,
    palette: &'a Palette,
    accent: egui::Color32,
    ink: Option<egui::Color32>,
    keyboard: bool,
}

impl<'a> TreeView<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, palette: &'a Palette) -> TreeView<'a> {
        TreeView { id: egui::Id::new(id), palette, accent: palette.accent, ink: None, keyboard: false }
    }

    pub fn accent(mut self, accent: egui::Color32) -> Self {
        self.accent = accent;
        self
    }

    /// The current row's text and icon (the accent's, unless said).
    pub fn ink(mut self, ink: egui::Color32) -> Self {
        self.ink = Some(ink);
        self
    }

    pub fn keyboard(mut self, keyboard: bool) -> Self {
        self.keyboard = keyboard;
        self
    }

    /// `rows` as laid out, `current` the one shown; `each` gets each row
    /// drawn (for dropping on it, a menu).
    pub fn show<K: std::hash::Hash + std::fmt::Debug>(
        self,
        ui: &mut egui::Ui,
        rows: &[TreeRow<K>],
        current: Option<usize>,
        mut each: impl FnMut(usize, &egui::Response),
    ) -> TreeShown {
        let mut shown = TreeShown::default();
        let palette = *self.palette;
        let dark = ui.visuals().dark_mode;
        let soft = thin(self.accent, if dark { 41 } else { 26 });
        let hover = thin(palette.text, 12);
        let ink = self.ink.unwrap_or(self.accent);

        if self.keyboard && !ui.ctx().egui_wants_keyboard_input() {
            if let Some(c) = current.filter(|&c| c < rows.len()) {
                let pressed = |k| ui.input(|i| i.key_pressed(k));
                let row = &rows[c];
                if pressed(egui::Key::ArrowDown) && c + 1 < rows.len() {
                    shown.go = Some(c + 1);
                } else if pressed(egui::Key::ArrowUp) && c > 0 {
                    shown.go = Some(c - 1);
                } else if pressed(egui::Key::ArrowLeft) {
                    if row.open && row.kids != Kids::No {
                        shown.toggle = Some(c);
                    } else {
                        shown.go = rows[..c].iter().rposition(|r| r.depth < row.depth);
                    }
                } else if pressed(egui::Key::ArrowRight) {
                    if !row.open && row.kids != Kids::No {
                        shown.toggle = Some(c);
                    } else if row.open && rows.get(c + 1).is_some_and(|r| r.depth > row.depth) {
                        shown.go = Some(c + 1);
                    }
                }
            }
        }

        // the current row into view each time it changes, and while the
        // rows above it are still being read (it moves down as they come)
        let seen = self.id.with("current");
        let this = current.and_then(|c| rows.get(c)).map(|r| egui::Id::new(&r.key));
        let settling = rows.iter().any(|r| r.loading);
        let follow = ui.data_mut(|d| {
            let (before, settled) = d.get_temp::<(Option<egui::Id>, bool)>(seen).unwrap_or((None, false));
            this.is_some() && (before != this || !settled)
        });
        // the view's height as it was drawn last (what is left of the
        // panel may be more than the scroll area shows)
        let viewport = self.id.with("viewport");
        let view = ui.data(|d| d.get_temp::<f32>(viewport)).unwrap_or_else(|| ui.available_height());
        let mut area = egui::ScrollArea::vertical().id_salt(self.id).auto_shrink([false, false]);
        if let (true, Some(c)) = (follow, current) {
            let offset = egui::scroll_area::State::load(ui.ctx(), ui.id().with(self.id)).map_or(0.0, |s| s.offset.y);
            let (top, bottom) = (c as f32 * NODE, (c + 1) as f32 * NODE);
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if bottom > offset + view {
                area = area.vertical_scroll_offset(bottom - view);
            }
        }
        let font = egui::FontId::proportional(12.0);
        let current_font = crate::font(ui.ctx(), 12.0, crate::Weight::Medium);
        // (seen in view, once nothing above it is being read: settled;
        // what the panel around shows, which may be less than the area)
        let outer = ui.clip_rect();
        let mut in_view = false;
        // no space between rows, for the rows' places too (`show_rows`
        // places them by its ui's spacing)
        let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing, egui::Vec2::ZERO);
        let output = area.show_rows(ui, NODE, rows.len(), |ui, range| {
            for i in range {
                let row = &rows[i];
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), NODE), egui::Sense::click());
                let is_current = current == Some(i);
                in_view |= is_current && ui.clip_rect().intersect(outer).contains_rect(rect);
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_current, &row.label)
                });
                let painter = ui.painter_at(rect);
                if is_current {
                    painter.rect_filled(rect, 0.0, soft);
                    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(crate::list::BAR, rect.height()));
                    painter.rect_filled(bar, 0.0, self.accent);
                } else if response.hovered() {
                    painter.rect_filled(rect, 0.0, hover);
                }
                // a line down beside each level above this one
                for level in 0..row.depth {
                    let x = rect.left() + PAD + level as f32 * INDENT + CHEVRON / 2.0;
                    painter.vline(x, rect.y_range(), egui::Stroke::new(1.0, palette.line));
                }
                let x = rect.left() + PAD + row.depth as f32 * INDENT;
                let y = rect.center().y;
                let chevron = egui::Rect::from_center_size(egui::pos2(x + CHEVRON / 2.0, y), egui::vec2(CHEVRON, NODE));
                if row.kids != Kids::No {
                    let at = chevron.center();
                    let s = 3.5;
                    let points = if row.open {
                        vec![at + egui::vec2(-s, -s / 2.0), at + egui::vec2(0.0, s / 2.0), at + egui::vec2(s, -s / 2.0)]
                    } else {
                        vec![at + egui::vec2(-s / 2.0, -s), at + egui::vec2(s / 2.0, 0.0), at + egui::vec2(-s / 2.0, s)]
                    };
                    painter.add(egui::Shape::line(points, egui::Stroke::new(1.3, palette.weak)));
                }
                let (text_color, text_font) = if is_current {
                    (ink, current_font.clone())
                } else if response.hovered() {
                    (palette.text, font.clone())
                } else {
                    (palette.weak, font.clone())
                };
                let mut tx = x + CHEVRON + 6.0;
                if let Some((glyph, color)) = &row.icon {
                    let color = if is_current { ink } else { *color };
                    let g = painter.layout_no_wrap(glyph.clone(), egui::FontId::proportional(14.0), color);
                    painter.galley(egui::pos2(tx, y - g.size().y / 2.0), g, color);
                    tx += 20.0;
                }
                let room = (rect.right() - tx - 6.0).max(0.0);
                let mut job = egui::text::LayoutJob::simple_singleline(row.label.clone(), text_font, text_color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                let galley = painter.layout_job(job);
                let elided = galley.elided;
                painter.galley(egui::pos2(tx, y - galley.size().y / 2.0), galley.clone(), text_color);
                if row.loading {
                    let s = 10.0;
                    let spin = egui::Rect::from_center_size(
                        egui::pos2(tx + galley.size().x + 6.0 + s / 2.0, y),
                        egui::vec2(s, s),
                    );
                    egui::Spinner::new().size(s).color(palette.weak).paint_at(ui, spin);
                }
                let response = if elided { response.on_hover_text(&row.label) } else { response };
                if response.clicked() {
                    let on_chevron = response.interact_pointer_pos().is_some_and(|p| chevron.contains(p));
                    if on_chevron && row.kids != Kids::No {
                        shown.toggle = Some(i);
                    } else {
                        shown.go = Some(i);
                    }
                }
                if response.double_clicked() && row.kids != Kids::No {
                    shown.toggle = Some(i);
                }
                each(i, &response);
            }
        });
        ui.spacing_mut().item_spacing = spacing;
        ui.data_mut(|d| d.insert_temp(viewport, output.inner_rect.intersect(outer).height()));
        if follow {
            ui.data_mut(|d| d.insert_temp(seen, (this, in_view && !settling)));
            ui.ctx().request_repaint();
        }
        shown
    }
}
