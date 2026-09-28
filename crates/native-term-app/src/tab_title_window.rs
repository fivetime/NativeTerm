//! The window "Rename Tab…" asks in: the tab's title for as long as the
//! tab lives. A window of its own, small and above the others, where the
//! pointer is: the terminal that asked is another program's window.

use native_term_app::{t, tab_title};

/// Asks for the question `ticket` names (see `tab_title::answer`).
pub fn open(ticket: u64, current: String) {
    let viewport = egui::ViewportBuilder::default()
        .with_title(t!("tab-title-title"))
        .with_inner_size([440.0, 170.0])
        .with_resizable(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_always_on_top();
    let window = TitleWindow { ticket, title: current, focused: false, answered: false };
    crate::window::open_at_pointer("tab-title", viewport, move |_| Box::new(window));
}

struct TitleWindow {
    ticket: u64,
    title: String,
    /// The field gets the keyboard when the window opens.
    focused: bool,
    answered: bool,
}

impl TitleWindow {
    fn answer(&mut self, rename: bool) {
        if self.answered {
            return;
        }
        self.answered = true;
        tab_title::answer(self.ticket, rename.then(|| self.title.clone()));
    }
}

impl Drop for TitleWindow {
    fn drop(&mut self) {
        // closed by its close button, or with the program: the tab keeps
        // its title
        self.answer(false);
    }
}

impl crate::window::Ui for TitleWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let frame = egui::Frame::NONE.inner_margin(14.0_f32).fill(ui.visuals().panel_fill);
        // the buttons first, at the bottom: there whatever room the window
        // system leaves the rest
        egui::Panel::bottom("tab-title-buttons").frame(frame).show_separator_line(false).show_inside(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t!("button-cancel")).clicked() || escape {
                    self.answer(false);
                }
                if ui.button(t!("button-ok")).clicked() || enter {
                    self.answer(true);
                }
            });
        });
        let frame = frame.inner_margin(egui::Margin { bottom: 0, ..egui::Margin::same(14) });
        egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(t!("tab-title-name"));
            let edit = ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(f32::INFINITY));
            if !self.focused {
                self.focused = true;
                edit.request_focus();
                // what is there is taken whole: typing replaces it
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit.id) {
                    let all = egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(self.title.chars().count()),
                    );
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), edit.id);
                }
            }
            ui.weak(t!("tab-title-note"));
        });
    }

    fn wants_close(&self) -> bool {
        self.answered
    }
}
