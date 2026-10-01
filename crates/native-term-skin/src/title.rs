//! The title bar: the window buttons where the platform has them, the
//! window's icon in its tile and its title; a window of its own is moved
//! by it.

use crate::caption::{caption_buttons, caption_dots, Buttons, Caption};
use crate::{Skin, MAC, TILE, TITLE_BAR, TITLE_PAD, TITLE_TEXT};

/// What a window's title bar is: its title, its icon, which buttons it
/// has, how high it is (the main window's unless said otherwise).
#[derive(Clone, Debug)]
pub struct TitleBar<'a> {
    title: &'a str,
    icon: Option<char>,
    buttons: Buttons,
    height: f32,
    /// Two clicks maximize the window (or make it what it was), where it
    /// has a maximize button; else the program does what it wants with
    /// them ([`TitleShown::double_clicked`]).
    double_click_maximizes: bool,
    /// A line before the window buttons, where other buttons share the
    /// title bar.
    line: bool,
}

/// What happened in the title bar this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TitleShown {
    /// A window button that was clicked (a window of its own has had it
    /// done already: minimized, maximized, asked to close).
    pub caption: Option<Caption>,
    pub double_clicked: bool,
}

impl TitleShown {
    /// The close button was clicked.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.caption == Some(Caption::Close)
    }
}

impl<'a> TitleBar<'a> {
    /// A title bar with the close button only, as high as the main
    /// window's.
    #[must_use]
    pub fn new(title: &'a str) -> TitleBar<'a> {
        TitleBar {
            title,
            icon: None,
            buttons: Buttons::CLOSE,
            height: TITLE_BAR,
            double_click_maximizes: true,
            line: false,
        }
    }

    /// The window's icon, in its tile before the title (a character of
    /// the icon font the program has installed).
    #[must_use]
    pub fn icon(mut self, icon: char) -> TitleBar<'a> {
        self.icon = Some(icon);
        self
    }

    #[must_use]
    pub fn buttons(mut self, buttons: Buttons) -> TitleBar<'a> {
        self.buttons = buttons;
        self
    }

    /// Another height than the main window's.
    #[must_use]
    pub fn height(mut self, height: f32) -> TitleBar<'a> {
        self.height = height;
        self
    }

    /// The program does what two clicks do (`TitleShown::double_clicked`).
    #[must_use]
    pub fn own_double_click(mut self) -> TitleBar<'a> {
        self.double_click_maximizes = false;
        self
    }

    /// A line before the window buttons.
    #[must_use]
    pub fn line_before_buttons(mut self) -> TitleBar<'a> {
        self.line = true;
        self
    }

    /// The title bar of a window of its own, at its top: it moves the
    /// window, and its buttons do what they say.
    pub fn show_window(self, ui: &mut egui::Ui, skin: &Skin) -> TitleShown {
        self.show_window_with(ui, skin, |_| {})
    }

    /// The same, `extra` before the window buttons (laid out from the
    /// right): the program's own buttons in its title bar.
    pub fn show_window_with(self, ui: &mut egui::Ui, skin: &Skin, extra: impl FnOnce(&mut egui::Ui)) -> TitleShown {
        let (maximized, focused) = ui.input(|i| {
            let window = i.viewport();
            (window.maximized.unwrap_or(false), window.focused.unwrap_or(true))
        });
        let maximize_on_double_click = self.double_click_maximizes && self.buttons.maximize;
        let frame = egui::Frame::NONE.fill(skin.palette.bar).inner_margin(egui::Margin::symmetric(TITLE_PAD, 0));
        let shown = egui::Panel::top(ui.id().with("skin-title"))
            .exact_size(self.height)
            .resizable(false)
            .frame(frame)
            .show_inside(ui, |ui| {
                // the window is taken by the title bar: what is put into
                // it afterwards is over this, and is what it is
                let bar = ui.max_rect().expand2(egui::vec2(f32::from(TITLE_PAD), 0.0));
                let taken = ui.interact(bar, ui.id().with("title-bar"), egui::Sense::click_and_drag());
                if taken.drag_started_by(egui::PointerButton::Primary) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                let mut shown = self.row(ui, skin, maximized, focused, extra);
                shown.double_clicked = taken.double_clicked();
                shown
            })
            .inner;
        let command = match shown.caption {
            Some(Caption::Minimize) => Some(egui::ViewportCommand::Minimized(true)),
            Some(Caption::Maximize | Caption::Restore) => Some(egui::ViewportCommand::Maximized(!maximized)),
            Some(Caption::Close) => Some(egui::ViewportCommand::Close),
            None if shown.double_clicked && maximize_on_double_click => {
                Some(egui::ViewportCommand::Maximized(!maximized))
            }
            None => None,
        };
        if let Some(command) = command {
            ui.ctx().send_viewport_cmd(command);
        }
        shown
    }

    /// The title bar inside something else (a modal dialog): it moves
    /// nothing; `width` wide, its top corners as round as `round`.
    pub(crate) fn show_inside(self, ui: &mut egui::Ui, skin: &Skin, width: f32, round: u8) -> TitleShown {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, self.height), egui::Sense::hover());
        let corners = egui::CornerRadius { nw: round, ne: round, sw: 0, se: 0 };
        ui.painter().rect_filled(rect, corners, skin.palette.bar);
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0_f32, skin.palette.line));
        let inner = rect.shrink2(egui::vec2(f32::from(TITLE_PAD), 0.0));
        let mut row = ui
            .new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        self.row(&mut row, skin, false, true, |_| {})
    }

    /// What is in the bar, left to right: macOS's dots, the icon, the
    /// title in what is left, the program's buttons, the window buttons.
    fn row(
        &self,
        ui: &mut egui::Ui,
        skin: &Skin,
        maximized: bool,
        focused: bool,
        extra: impl FnOnce(&mut egui::Ui),
    ) -> TitleShown {
        let mut caption = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if MAC {
                caption = caption_dots(ui, skin, self.buttons, focused);
                ui.add_space(6.0);
            }
            if let Some(icon) = self.icon {
                tile(ui, skin, icon);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if !MAC {
                    caption = caption_buttons(ui, skin, self.buttons, maximized, self.line);
                }
                extra(ui);
                ui.add_space(4.0);
                // the title has what the buttons leave
                title(ui, skin, self.title, ui.available_width());
            });
        });
        TitleShown { caption, double_clicked: false }
    }
}

/// The icon's tile (`p-1.5 rounded-lg`, the accent's colours).
pub(crate) fn tile(ui: &mut egui::Ui, skin: &Skin, icon: char) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(TILE, TILE), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let tint = skin.palette.tile;
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, tint.fill);
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.0_f32, tint.line), egui::StrokeKind::Inside);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(16.0), tint.text);
    }
}

/// The title, `width` wide (`text-sm`), cut short with an ellipsis where
/// it is longer. Nothing happens to it: the pointer takes the window by
/// it as by the rest of the bar.
fn title(ui: &mut egui::Ui, skin: &Skin, text: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(0.0), TILE), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect);
        let mut job = egui::text::LayoutJob::simple_singleline(
            text.to_string(),
            egui::FontId::proportional(TITLE_TEXT),
            skin.palette.text,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
        let galley = painter.layout_job(job);
        painter.galley(egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, skin.palette.text);
    }
}
