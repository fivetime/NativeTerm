//! The window "Paste as Quotation" asks in, as SecureCRT's: the quotation
//! characters, whether the text goes between them, what a line then looks
//! like, and whether to ask again. A window of its own, small and above
//! the others: the terminal that asked is another program's window, which
//! this one cannot be a dialog of.

use native_term_app::quotation::{self, Quotation};
use native_term_app::{t, Core};

/// What stands for the pasted text in the sample.
const PLACEHOLDER: &str = "<text>";

/// Asks for the question `ticket` names (see `quotation::answer`).
pub fn open(ticket: u64, core: Core) {
    let viewport = egui::ViewportBuilder::default()
        .with_title(t!("quote-title"))
        .with_inner_size([460.0, 240.0])
        .with_resizable(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_always_on_top();
    // where the pointer is: in the terminal's window, on the menu's item
    crate::window::open_at_pointer("paste-quotation", viewport, move |_| Box::new(QuotationWindow::new(ticket, core)));
}

struct QuotationWindow {
    ticket: u64,
    core: Core,
    quotation: Quotation,
    no_prompt: bool,
    /// The field gets the keyboard when the window opens.
    focused: bool,
    answered: bool,
}

impl QuotationWindow {
    fn new(ticket: u64, core: Core) -> QuotationWindow {
        let quotation = Quotation::saved(&core);
        QuotationWindow { ticket, core, quotation, no_prompt: false, focused: false, answered: false }
    }

    fn answer(&mut self, paste: bool) {
        if self.answered {
            return;
        }
        self.answered = true;
        if !paste {
            quotation::answer(self.ticket, None);
            return;
        }
        let quotation = self.quotation.clone().cleaned();
        quotation.save(&self.core);
        if self.no_prompt {
            quotation::set_prompts(&self.core, false);
        }
        quotation::answer(self.ticket, Some(quotation));
    }
}

impl Drop for QuotationWindow {
    fn drop(&mut self) {
        // closed by its close button, or with the program: no paste
        self.answer(false);
    }
}

impl crate::window::Ui for QuotationWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let frame = egui::Frame::NONE.inner_margin(14.0_f32).fill(ui.visuals().panel_fill);
        // the buttons first, at the bottom: there whatever room the window
        // system leaves the rest (its title bar is taken from the window's
        // height on some)
        egui::Panel::bottom("quote-buttons").frame(frame).show_separator_line(false).show_inside(ui, |ui| {
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
            ui.horizontal(|ui| {
                ui.label(t!("quote-chars"));
                let edit = ui.add(egui::TextEdit::singleline(&mut self.quotation.chars).desired_width(110.0));
                if !self.focused {
                    self.focused = true;
                    edit.request_focus();
                }
            });
            ui.checkbox(&mut self.quotation.between, t!("quote-between"));
            ui.horizontal(|ui| {
                ui.label(t!("quote-sample"));
                ui.monospace(self.quotation.clone().cleaned().sample(PLACEHOLDER));
            });
            ui.label(t!("quote-note", placeholder = PLACEHOLDER));
            ui.checkbox(&mut self.no_prompt, t!("quote-no-prompt"));
        });
    }

    fn wants_close(&self) -> bool {
        self.answered
    }
}
