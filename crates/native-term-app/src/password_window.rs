//! The window a password is asked in where none is saved ("Enter Secure
//! Shell Password", as SecureCRT has it): the account, the password,
//! "Save password" (ticked: kept once the login worked), and OK, Cancel
//! (the connection given up) and Skip (asked in the tab). A window of its
//! own, above the others: the terminal that asked is another program's.

use native_term_app::password_ask::{self, PasswordAnswer, Question};
use native_term_app::t;

/// Asks the question `ticket` names (see `password_ask::answer`).
pub fn open(ticket: u64, question: Question) {
    let height = if question.can_save { 280.0 } else { 240.0 };
    let viewport = crate::skinned::viewport(&t!("password-ask-title"), 460.0, height)
        .with_resizable(false)
        .with_always_on_top()
        .with_active(true);
    let window = PasswordWindow {
        ticket,
        user: question.user.clone(),
        question,
        secret: String::new(),
        save: true,
        focused: false,
        shown: false,
        answered: false,
    };
    crate::window::open_at_pointer(format!("password-{ticket}"), viewport, move |_| Box::new(window));
}

/// The window for a picture of it (`snapshots.rs`), answering nothing.
#[cfg(test)]
pub(crate) fn for_snapshot(question: Question) -> PasswordWindow {
    PasswordWindow {
        ticket: u64::MAX,
        user: question.user.clone(),
        question,
        secret: "secret".into(),
        save: true,
        focused: true,
        shown: false,
        answered: true,
    }
}

pub(crate) struct PasswordWindow {
    ticket: u64,
    question: Question,
    /// The user name, as the person may change it.
    user: String,
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
        let user = self.user.trim();
        let user = (user != self.question.user).then(|| user.to_string());
        self.answer(PasswordAnswer::Given { secret, save, user });
    }

    /// A user name ssh can be given: one word.
    fn user_is_valid(&self) -> bool {
        let user = self.user.trim();
        !user.is_empty() && !user.chars().any(|c| c.is_whitespace() || c.is_control())
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
        // (its close button gives up, as the system's did: see `Drop`)
        let skin = crate::skinned::chrome(ui, &t!("password-ask-title"), crate::icons::KEY);
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let frame = egui::Frame::NONE.inner_margin(14.0_f32).fill(skin.palette.page);
        egui::Panel::bottom("password-buttons").frame(frame).show_separator_line(false).show(ui, |ui| {
            use native_term_skin::{Choice, Role};
            let valid = self.user_is_valid();
            let choices = [
                Choice::new(t!("button-ok"), Role::Primary).enabled(valid),
                Choice::new(t!("button-cancel"), Role::Plain),
            ];
            let mut skip = false;
            let pressed = native_term_skin::footer(
                ui,
                &skin,
                |ui| {
                    let button = native_term_skin::button(ui, &skin, &t!("password-ask-skip"), Role::Plain, true);
                    skip = button.on_hover_text(t!("password-ask-skip-hint")).clicked();
                },
                &choices,
            );
            if skip {
                self.answer(PasswordAnswer::Skip);
            } else if pressed == Some(1) || escape {
                self.answer(PasswordAnswer::Cancel);
            } else if pressed == Some(0) || (enter && valid) {
                // (Enter in the field too)
                self.given();
            }
        });
        let frame = frame.inner_margin(egui::Margin { bottom: 0, ..egui::Margin::same(14) });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let changed = self.user.trim() != self.question.user;
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
                let user = ui.add(egui::TextEdit::singleline(&mut self.user).desired_width(260.0));
                user.on_hover_text(t!("password-ask-user-hint"));
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
            if changed {
                let note =
                    if self.save && q.can_save { t!("password-ask-user-kept") } else { t!("password-ask-user-once") };
                ui.weak(note);
            }
        });
    }

    fn wants_close(&self) -> bool {
        self.answered
    }
}
