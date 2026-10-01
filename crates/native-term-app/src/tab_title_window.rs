//! The window "Rename Tab…" asks in: the tab's title for as long as the
//! tab lives. A window of its own, small and above the others, where the
//! pointer is: the terminal that asked is another program's window.

use native_term_app::{t, tab_title};

/// Asks for the question `ticket` names (see `tab_title::answer`).
pub fn open(ticket: u64, current: String) {
    let viewport =
        crate::skinned::viewport(&t!("tab-title-title"), 440.0, 170.0).with_resizable(false).with_always_on_top();
    let window = TitleWindow { ticket, title: current, focused: false, answered: false };
    crate::window::open_at_pointer("tab-title", viewport, move |_| Box::new(window));
}

/// The window for a picture of it (`snapshots.rs`), answering nothing.
#[cfg(test)]
pub(crate) fn for_snapshot(title: &str) -> TitleWindow {
    TitleWindow { ticket: u64::MAX, title: title.into(), focused: true, answered: true }
}

pub(crate) struct TitleWindow {
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
        let skin = crate::skinned::chrome(ui, &t!("tab-title-title"), crate::icons::RENAME);
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        use native_term_skin::{Choice, Role};
        let choices = [Choice::new(t!("button-ok"), Role::Primary), Choice::new(t!("button-cancel"), Role::Plain)];
        let pressed = crate::skinned::buttons(ui, &skin, "tab-title-buttons", |_| {}, &choices);
        if pressed == Some(1) || escape {
            self.answer(false);
        } else if pressed == Some(0) || enter {
            self.answer(true);
        }
        let frame = crate::skinned::page(&skin);
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
