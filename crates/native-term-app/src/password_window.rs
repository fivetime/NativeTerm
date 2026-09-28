//! The window a password is asked in where none is saved ("Enter Secure
//! Shell Password", as SecureCRT has it): the account, the password,
//! "Save password" (ticked: kept once the login worked), and OK, Cancel
//! (the connection given up) and Skip (asked in the tab). A window of its
//! own, above the others: the terminal that asked is another program's.

use native_term_app::password_ask::{self, PasswordAnswer, Question};
use native_term_app::t;

/// Asks the question `ticket` names (see `password_ask::answer`).
pub fn open(ticket: u64, question: Question) {
    let viewport = egui::ViewportBuilder::default()
        .with_title(t!("password-ask-title"))
        .with_inner_size([460.0, if question.can_save { 250.0 } else { 220.0 }])
        .with_resizable(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_always_on_top()
        .with_active(true);
    let window = PasswordWindow {
        ticket,
        question,
        secret: String::new(),
        save: true,
        focused: false,
        shown: false,
        answered: false,
    };
    crate::window::open_at_pointer(format!("password-{ticket}"), viewport, move |_| Box::new(window));
}

struct PasswordWindow {
    ticket: u64,
    question: Question,
    secret: String,
    save: bool,
    /// The field gets the keyboard when the window opens.
    focused: bool,
    /// It was shown: one made for a question whose window is open already
    /// (asked again to bring it forward) is dropped unseen, and says
    /// nothing.
    shown: bool,
    answered: bool,
}

impl PasswordWindow {
    fn answer(&mut self, answer: PasswordAnswer) {
        if self.answered {
            return;
        }
        self.answered = true;
        password_ask::answer(self.ticket, answer);
        // (the window forgets it at once)
        self.secret.clear();
    }

    fn given(&mut self) {
        let secret = std::mem::take(&mut self.secret);
        let save = self.save && self.question.can_save;
        self.answer(PasswordAnswer::Given { secret, save });
    }
}

impl Drop for PasswordWindow {
    fn drop(&mut self) {
        // closed by its close button, or with the program: given up, as
        // SecureCRT's close button does
        if self.shown {
            self.answer(PasswordAnswer::Cancel);
        }
    }
}

impl crate::window::Ui for PasswordWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        self.shown = true;
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let frame = egui::Frame::NONE.inner_margin(14.0_f32).fill(ui.visuals().panel_fill);
        egui::Panel::bottom("password-buttons").frame(frame).show_separator_line(false).show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button(t!("password-ask-skip")).on_hover_text(t!("password-ask-skip-hint")).clicked() {
                    self.answer(PasswordAnswer::Skip);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t!("button-cancel")).clicked() || escape {
                        self.answer(PasswordAnswer::Cancel);
                    }
                    if ui.button(t!("button-ok")).clicked() || enter {
                        self.given();
                    }
                });
            });
        });
        let frame = frame.inner_margin(egui::Margin { bottom: 0, ..egui::Margin::same(14) });
        egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let q = &self.question;
            let account = format!("{}@{}", q.user, q.host);
            let asks = match &q.label {
                Some(label) if *label != q.host => {
                    t!("password-ask-text-label", account = account.as_str(), label = label.as_str())
                }
                _ => t!("password-ask-text", account = account.as_str()),
            };
            ui.label(asks);
            let error = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
            if q.retry {
                ui.colored_label(error, t!("password-ask-wrong"));
            } else if q.refused {
                ui.colored_label(error, t!("password-ask-refused"));
            }
            egui::Grid::new("password-fields").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(t!("password-ask-user"));
                ui.label(egui::RichText::new(&q.user).monospace());
                ui.end_row();
                ui.label(t!("password-ask-password"));
                let field = ui.add(egui::TextEdit::singleline(&mut self.secret).password(true).desired_width(260.0));
                crate::dialogs::no_ime(&field);
                if !self.focused {
                    self.focused = true;
                    field.request_focus();
                }
                ui.end_row();
            });
            if q.can_save {
                ui.checkbox(&mut self.save, t!("password-ask-save"));
                if self.save {
                    ui.weak(t!("password-ask-save-note"));
                }
            }
        });
    }

    fn wants_close(&self) -> bool {
        self.answered
    }
}
