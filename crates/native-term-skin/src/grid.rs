//! Items across and down, as a file manager's list view: as many columns
//! as fit items at least so wide, each item the same width, only the
//! lines in view drawn, chosen as [`Selection`] says (the arrows go
//! across and down), the chosen ones in the accent's tint with a bar at
//! their start. egui has a grid for laying things out, no view of items:
//! this is one, on its scroll area. What is in an item is the program's.

use crate::list::{ListShown, BAR};
use crate::selection::{Keyed, Selection};
use crate::{thin, Palette};

/// The design's sizes (`.grid-list`, `.gl-item`).
pub const ITEM: f32 = 28.0;
pub const ITEM_WIDTH: f32 = 200.0;
const GAP_ACROSS: f32 = 8.0;
const GAP_DOWN: f32 = 2.0;
const PAD: f32 = 8.0;
const ITEM_PAD: f32 = 8.0;

/// What an item is drawn for.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub index: usize,
    pub chosen: bool,
}

pub struct GridView<'a> {
    id: egui::Id,
    palette: &'a Palette,
    accent: egui::Color32,
    width: f32,
    height: f32,
    keyboard: bool,
}

impl<'a> GridView<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, palette: &'a Palette) -> GridView<'a> {
        GridView {
            id: egui::Id::new(id),
            palette,
            accent: palette.accent,
            width: ITEM_WIDTH,
            height: ITEM,
            keyboard: false,
        }
    }

    pub fn accent(mut self, accent: egui::Color32) -> Self {
        self.accent = accent;
        self
    }

    /// The least width of an item.
    pub fn item_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn keyboard(mut self, keyboard: bool) -> Self {
        self.keyboard = keyboard;
        self
    }

    /// As [`crate::ListView::show`], an item for a row.
    pub fn show<K: Clone + Eq + std::hash::Hash>(
        self,
        ui: &mut egui::Ui,
        keys: &[K],
        selection: &mut Selection<K>,
        name: impl Fn(usize) -> String,
        mut item: impl FnMut(&mut egui::Ui, Item),
        mut each: impl FnMut(usize, bool, &egui::Response),
    ) -> ListShown {
        let mut shown = ListShown::default();
        let dark = ui.visuals().dark_mode;
        let soft = thin(self.accent, if dark { 41 } else { 26 });
        let hover = thin(self.palette.text, 12);

        let width = ui.available_width();
        let across = (((width - 2.0 * PAD + GAP_ACROSS) / (self.width + GAP_ACROSS)).floor() as usize).max(1);
        let item_width = ((width - 2.0 * PAD - GAP_ACROSS * (across - 1) as f32) / across as f32).max(1.0);
        let line = self.height + GAP_DOWN;
        let lines = keys.len().div_ceil(across);
        let view = ui.available_height();

        let scroll_id = self.id.with("scroll");
        let mut offset = egui::scroll_area::State::load(ui.ctx(), ui.id().with(scroll_id)).map(|s| s.offset.y);
        if self.keyboard && !ui.ctx().egui_wants_keyboard_input() {
            let page = ((view / line).floor().max(1.0) as usize) * across;
            let before = selection.len();
            match ui.input(|i| selection.keys(i, keys, across, page, &name)) {
                Keyed::Moved(to) => {
                    shown.changed = true;
                    // into view: the line it is on
                    let (top, bottom) = ((to / across) as f32 * line, (to / across + 1) as f32 * line);
                    let now = offset.unwrap_or(0.0);
                    if top < now {
                        offset = Some(top);
                    } else if bottom > now + view {
                        offset = Some(bottom - view);
                    }
                }
                Keyed::Open => {
                    shown.open = selection.cursor().or_else(|| keys.iter().position(|k| selection.contains(k)))
                }
                Keyed::Nothing => shown.changed |= selection.len() != before,
            }
        }

        let mut clicked: Option<(usize, egui::Modifiers, bool)> = None;
        let mut area = egui::ScrollArea::vertical().id_salt(scroll_id).auto_shrink([false, false]);
        if let Some(y) = offset {
            area = area.vertical_scroll_offset(y.max(0.0));
        }
        area.show_rows(ui, line, lines, |ui, range| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            for l in range {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), line), egui::Sense::hover());
                for c in 0..across {
                    let i = l * across + c;
                    let Some(key) = keys.get(i) else { break };
                    let x = rect.left() + PAD + c as f32 * (item_width + GAP_ACROSS);
                    let cell = egui::Rect::from_min_size(
                        egui::pos2(x, rect.top() + GAP_DOWN),
                        egui::vec2(item_width, self.height),
                    );
                    let response = ui.interact(cell, self.id.with(("item", i)), egui::Sense::click_and_drag());
                    let chosen = selection.contains(key);
                    if chosen {
                        ui.painter().rect_filled(cell, 0.0, soft);
                        let bar = egui::Rect::from_min_size(cell.min, egui::vec2(BAR, cell.height()));
                        ui.painter().rect_filled(bar, 0.0, self.accent);
                    } else if response.hovered() {
                        ui.painter().rect_filled(cell, 0.0, hover);
                    }
                    let inner = cell.shrink2(egui::vec2(ITEM_PAD, 0.0));
                    let layout = egui::Layout::left_to_right(egui::Align::Center);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(layout));
                    child.set_clip_rect(inner.intersect(ui.clip_rect()));
                    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    item(&mut child, Item { index: i, chosen });
                    if response.clicked() || response.secondary_clicked() {
                        let modifiers = ui.input(|i| i.modifiers);
                        clicked = Some((i, modifiers, response.secondary_clicked()));
                    }
                    if response.double_clicked() {
                        shown.open = Some(i);
                    }
                    each(i, chosen, &response);
                }
            }
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
