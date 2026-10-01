//! The window a host key is decided in (see `host_key_ask`): for a new
//! host its fingerprint, "Accept & Save", "Accept Once", "Cancel"; for a
//! changed key a warning with both fingerprints, "Remove the Old Key and
//! Connect" and "Cancel" (the default: Enter and Esc cancel there). A
//! window of its own, above the others: the terminal that asked is
//! another program's.

use native_term_app::host_key_ask::{self, HostKeyAnswer, Question};
use native_term_app::t;

/// Asks the question `ticket` names.
pub fn open(ticket: u64, question: Question) {
    let changed = question.old.is_some();
    let title = if changed { t!("hostkey-changed-title") } else { t!("hostkey-new-title") };
    let viewport = crate::skinned::viewport(&title, 560.0, if changed { 350.0 } else { 245.0 })
        .with_resizable(false)
        .with_always_on_top()
        .with_active(true);
    let window = HostKeyWindow { ticket, question, shown: false, answered: false };
    crate::window::open_at_pointer(format!("hostkey-{ticket}"), viewport, move |_| Box::new(window));
}

/// The window for a picture of it (`snapshots.rs`), answering nothing.
#[cfg(test)]
pub(crate) fn for_snapshot(question: Question) -> HostKeyWindow {
    HostKeyWindow { ticket: u64::MAX, question, shown: false, answered: true }
}

pub(crate) struct HostKeyWindow {
    ticket: u64,
    question: Question,
    /// It was shown (one made again only to bring it forward is dropped
    /// unseen and says nothing).
    shown: bool,
    answered: bool,
}

impl HostKeyWindow {
    fn answer(&mut self, answer: HostKeyAnswer) {
        if !self.answered {
            self.answered = true;
            host_key_ask::answer(self.ticket, answer);
        }
    }
}

impl Drop for HostKeyWindow {
    fn drop(&mut self) {
        // closed: not trusted
        if self.shown {
            self.answer(HostKeyAnswer::Cancel);
        }
    }
}

/// A fingerprint in a field that can be selected and copied.
fn fingerprint(ui: &mut egui::Ui, text: &str) {
    let mut shown = text.to_string();
    ui.add(egui::TextEdit::singleline(&mut shown).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
}

impl crate::window::Ui for HostKeyWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        self.shown = true;
        use native_term_skin::{Choice, Notice, Role};
        let changed = self.question.old.is_some();
        let title = if changed { t!("hostkey-changed-title") } else { t!("hostkey-new-title") };
        let skin = crate::skinned::chrome(ui, &title, crate::icons::LOCK);
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        // a changed key is never trusted by a key press: Enter cancels
        let choices = if changed {
            vec![
                Choice::new(t!("hostkey-replace"), Role::Danger),
                Choice::new(t!("button-cancel"), Role::Plain).default(),
            ]
        } else {
            vec![
                Choice::new(t!("hostkey-save"), Role::Primary).hint(t!("hostkey-save-hint")),
                Choice::new(t!("hostkey-once"), Role::Plain).hint(t!("hostkey-once-hint")),
                Choice::new(t!("button-cancel"), Role::Plain),
            ]
        };
        let pressed = crate::skinned::buttons(ui, &skin, "hostkey-buttons", |_| {}, &choices);
        let cancel = choices.len() - 1;
        if pressed == Some(cancel) || escape || (enter && changed) {
            self.answer(HostKeyAnswer::Cancel);
        } else if pressed == Some(0) || enter {
            self.answer(HostKeyAnswer::Save);
        } else if pressed == Some(1) {
            self.answer(HostKeyAnswer::Once);
        }
        let notice = if changed { Notice::Error } else { Notice::Question };
        egui::CentralPanel::default().frame(crate::skinned::page(&skin)).show_inside(ui, |ui| {
            native_term_skin::body(ui, Some(notice), |ui| self.text(ui, &skin));
        });
    }

    fn wants_close(&self) -> bool {
        self.answered
    }
}

impl HostKeyWindow {
    /// What is said: the host, the key's fingerprint, for a changed one
    /// both and where the old one is.
    fn text(&self, ui: &mut egui::Ui, skin: &native_term_skin::Skin) {
        {
            ui.spacing_mut().item_spacing.y = 8.0;
            let q = &self.question;
            let host = match &q.label {
                Some(label) if *label != q.host => format!("{label} ({})", q.host),
                _ => q.host.clone(),
            };
            let shown_ip = if q.ip.is_empty() || q.ip == q.host { String::new() } else { format!(" [{}]", q.ip) };
            match &q.old {
                None => {
                    ui.label(t!("hostkey-new-text", host = format!("{host}{shown_ip}")));
                    ui.label(t!("hostkey-new-fingerprint", key_type = q.key_type.as_str()));
                    fingerprint(ui, &q.fingerprint);
                    ui.weak(t!("hostkey-new-note"));
                }
                Some(old) => {
                    let red = skin.palette.danger;
                    ui.colored_label(red, egui::RichText::new(t!("hostkey-changed-warning")).strong());
                    ui.label(t!("hostkey-changed-text", host = format!("{host}{shown_ip}")));
                    ui.label(t!("hostkey-changed-old", key_type = old.key_type.as_str()));
                    fingerprint(ui, &old.fingerprint);
                    ui.label(t!("hostkey-changed-new", key_type = q.key_type.as_str()));
                    fingerprint(ui, &q.fingerprint);
                    let place = format!("{}:{}", old.file, old.line);
                    ui.weak(t!("hostkey-changed-note", place = place.as_str()));
                }
            }
        }
    }
}
