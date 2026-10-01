//! A list of rows in columns, as a file manager's details view: a header
//! whose columns sort when clicked (an arrow at the one sorted by), rows
//! chosen as [`Selection`] says (the chosen ones in the accent's tint
//! with a bar at their start), only the rows in view drawn. Built on
//! egui's own table (`egui_extras::TableBuilder`: column widths, the
//! header kept at the top, rows drawn while scrolled to); what is in a
//! cell is the program's.

use crate::selection::{Keyed, Selection};
use crate::{thin, Palette};

/// How wide a column is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    /// What the others leave, at least this much.
    Rest(f32),
    Fixed(f32),
}

/// A column: its title, how wide, its text at the end (numbers) or the
/// start, and whether a click on its title sorts by it.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub title: String,
    pub width: Width,
    pub end: bool,
    pub sorts: bool,
}

impl Column {
    pub fn new(title: impl Into<String>, width: Width) -> Column {
        Column { title: title.into(), width, end: false, sorts: true }
    }

    /// Its text at the end of the cell (numbers).
    pub fn at_end(mut self) -> Column {
        self.end = true;
        self
    }

    pub fn unsorted(mut self) -> Column {
        self.sorts = false;
        self
    }
}

/// Sorted by this column, down (A to Z, small to large) or up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

/// What a cell is drawn for.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub row: usize,
    pub column: usize,
    pub chosen: bool,
}

/// The design's sizes (`thead th`, `tbody td`).
pub const HEADER: f32 = 32.0;
pub const ROW: f32 = 34.0;
/// The space at a cell's sides.
pub const CELL_PAD: f32 = 10.0;
/// The bar at a chosen row's start.
pub const BAR: f32 = 2.0;

pub struct ListView<'a> {
    id: egui::Id,
    columns: &'a [Column],
    palette: &'a Palette,
    accent: egui::Color32,
    row: f32,
    sort: Option<Sort>,
    keyboard: bool,
}

/// What happened in the list.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListShown {
    /// A title clicked: the sort now asked for.
    pub sort: Option<Sort>,
    /// A row double-clicked, or Enter on what is chosen (its row or the
    /// cursor's).
    pub open: Option<usize>,
    /// The chosen rows changed.
    pub changed: bool,
    /// A click under the last row.
    pub clicked_empty: bool,
}

impl<'a> ListView<'a> {
    /// `accent`: the chosen rows' colour (a side's own, or the palette's).
    pub fn new(
        id: impl std::hash::Hash + std::fmt::Debug,
        columns: &'a [Column],
        palette: &'a Palette,
    ) -> ListView<'a> {
        ListView {
            id: egui::Id::new(id),
            columns,
            palette,
            accent: palette.accent,
            row: ROW,
            sort: None,
            keyboard: false,
        }
    }

    pub fn accent(mut self, accent: egui::Color32) -> Self {
        self.accent = accent;
        self
    }

    pub fn row_height(mut self, height: f32) -> Self {
        self.row = height;
        self
    }

    pub fn sorted(mut self, sort: Option<Sort>) -> Self {
        self.sort = sort;
        self
    }

    /// The keys go to this list (it is the one clicked last).
    pub fn keyboard(mut self, keyboard: bool) -> Self {
        self.keyboard = keyboard;
        self
    }

    /// `keys`: the rows' keys in the order shown; `name(i)`: what typing
    /// finds row `i` by; `cell` draws a cell; `each_row` gets each row
    /// drawn, whether it is chosen and its response (for a menu,
    /// dragging, dropping).
    pub fn show<K: Clone + Eq + std::hash::Hash>(
        self,
        ui: &mut egui::Ui,
        keys: &[K],
        selection: &mut Selection<K>,
        name: impl Fn(usize) -> String,
        mut cell: impl FnMut(&mut egui::Ui, Cell),
        mut each_row: impl FnMut(usize, bool, &egui::Response),
    ) -> ListShown {
        let mut shown = ListShown::default();
        let dark = ui.visuals().dark_mode;
        let palette = *self.palette;
        let soft = thin(self.accent, if dark { 41 } else { 26 });
        let hover = thin(palette.text, 12);

        // the keys first: a row moved to is scrolled into view this frame
        let page = ((ui.available_height() - HEADER) / self.row).floor().max(1.0) as usize;
        let mut scroll_to = None;
        if self.keyboard && !ui.ctx().egui_wants_keyboard_input() {
            let before = selection.len();
            match ui.input(|i| selection.keys(i, keys, 1, page, &name)) {
                Keyed::Moved(to) => {
                    scroll_to = Some(to);
                    shown.changed = true;
                }
                Keyed::Open => {
                    shown.open = selection.cursor().or_else(|| keys.iter().position(|k| selection.contains(k)))
                }
                Keyed::Nothing => shown.changed |= selection.len() != before,
            }
        }

        let mut clicked: Option<(usize, egui::Modifiers, bool)> = None;
        ui.scope(|ui| {
            let visuals = ui.visuals_mut();
            visuals.selection.bg_fill = soft;
            visuals.selection.stroke.color = palette.text;
            visuals.widgets.hovered.bg_fill = hover;
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            let mut table = egui_extras::TableBuilder::new(ui)
                .id_salt(self.id)
                .striped(false)
                .resizable(false)
                .sense(egui::Sense::click_and_drag())
                .auto_shrink([false, false])
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
            for column in self.columns {
                table = table.column(match column.width {
                    Width::Rest(least) => egui_extras::Column::remainder().at_least(least).clip(true),
                    Width::Fixed(w) => egui_extras::Column::exact(w).clip(true),
                });
            }
            if let Some(to) = scroll_to {
                table = table.scroll_to_row(to, None);
            }
            let sort = self.sort;
            table
                .header(HEADER, |mut header| {
                    for (c, column) in self.columns.iter().enumerate() {
                        header.col(|ui| {
                            if let Some(s) = title(ui, self.id, c, column, sort, &palette) {
                                shown.sort = Some(s);
                            }
                        });
                    }
                    let rect = header.response().rect;
                    ui_line(&header, rect, palette.line);
                })
                .body(|body| {
                    body.rows(self.row, keys.len(), |mut row| {
                        let i = row.index();
                        let chosen = selection.contains(&keys[i]);
                        row.set_selected(chosen);
                        for c in 0..self.columns.len() {
                            let end = self.columns[c].end;
                            let accent = self.accent;
                            row.col(|ui| {
                                if c == 0 && chosen {
                                    let r = ui.max_rect();
                                    let bar = egui::Rect::from_min_size(r.min, egui::vec2(BAR, r.height()));
                                    ui.painter().rect_filled(bar, 0.0, accent);
                                }
                                let layout = if end {
                                    egui::Layout::right_to_left(egui::Align::Center)
                                } else {
                                    egui::Layout::left_to_right(egui::Align::Center)
                                };
                                let inner = ui.max_rect().shrink2(egui::vec2(CELL_PAD, 0.0));
                                ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(layout), |ui| {
                                    cell(ui, Cell { row: i, column: c, chosen });
                                });
                            });
                        }
                        let response = row.response();
                        if response.clicked() || response.secondary_clicked() {
                            let modifiers = response.ctx.input(|i| i.modifiers);
                            clicked = Some((i, modifiers, response.secondary_clicked()));
                        }
                        if response.double_clicked() {
                            shown.open = Some(i);
                        }
                        each_row(i, chosen, &response);
                    });
                });
        });
        if let Some((i, modifiers, secondary)) = clicked {
            if secondary {
                selection.context_click(keys, i);
            } else {
                selection.click(keys, i, modifiers);
            }
            shown.changed = true;
        }
        shown
    }
}

/// A line under the header, across the table.
fn ui_line(header: &egui_extras::TableRow<'_, '_>, rect: egui::Rect, color: egui::Color32) {
    let response = header.response();
    let painter = response.ctx.layer_painter(response.layer_id);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, color));
}

/// A column's title, clickable where it sorts: the sort asked for.
fn title(
    ui: &mut egui::Ui,
    id: egui::Id,
    c: usize,
    column: &Column,
    sort: Option<Sort>,
    palette: &Palette,
) -> Option<Sort> {
    let rect = ui.max_rect();
    let response = if column.sorts {
        ui.interact(rect, id.with(("title", c)), egui::Sense::click())
    } else {
        ui.interact(rect, id.with(("title", c)), egui::Sense::hover())
    };
    let sorted = sort.filter(|s| s.column == c);
    let color = if sorted.is_some() || (column.sorts && response.hovered()) { palette.text } else { palette.weak };
    let font = crate::font(ui.ctx(), 11.0, crate::Weight::Medium);
    let mut text = column.title.clone();
    if let Some(s) = sorted {
        text.push_str(if s.descending { " ↑" } else { " ↓" });
    }
    let galley = ui.painter().layout_no_wrap(text, font, color);
    let x = if column.end { rect.right() - CELL_PAD - galley.size().x } else { rect.left() + CELL_PAD };
    let pos = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(rect).galley(pos, galley, color);
    if column.sorts {
        let label = column.title.clone();
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    }
    let next = Sort { column: c, descending: sorted.is_some_and(|s| !s.descending) };
    response.clicked().then_some(next)
}
