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
    let viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([560.0, if changed { 350.0 } else { 245.0 }])
        .with_resizable(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_always_on_top()
        .with_active(true);
    let window = HostKeyWindow { ticket, question, shown: false, answered: false };
    crate::window::open_at_pointer(format!("hostkey-{ticket}"), viewport, move |_| Box::new(window));
}

struct HostKeyWindow {
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
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        let changed = self.question.old.is_some();
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let frame = egui::Frame::NONE.inner_margin(14.0_f32).fill(ui.visuals().panel_fill);
        egui::Panel::bottom("hostkey-buttons").frame(frame).show_separator_line(false).show_inside(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // a changed key is never trusted by a key press
                if ui.button(t!("button-cancel")).clicked() || escape || (enter && changed) {
                    self.answer(HostKeyAnswer::Cancel);
                }
                if changed {
                    let red = egui::Color32::from_rgb(0xc0, 0x39, 0x2b);
                    let replace = egui::Button::new(egui::RichText::new(t!("hostkey-replace")).color(red));
                    if ui.add(replace).clicked() {
                        self.answer(HostKeyAnswer::Save);
                    }
                } else {
                    if ui.button(t!("hostkey-once")).on_hover_text(t!("hostkey-once-hint")).clicked() {
                        self.answer(HostKeyAnswer::Once);
                    }
                    if ui.button(t!("hostkey-save")).on_hover_text(t!("hostkey-save-hint")).clicked() || enter {
                        self.answer(HostKeyAnswer::Save);
                    }
                }
            });
        });
        let frame = frame.inner_margin(egui::Margin { bottom: 0, ..egui::Margin::same(14) });
        egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| {
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
                    let red = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
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
        });
    }

    fn wants_close(&self) -> bool {
        self.answered
    }
}
