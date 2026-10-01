//! Closing sessions whose tabs have other panes in them.

use native_term_app::t;
use native_term_skin::{Choice, Role};

use super::*;

/// "Really delete?"
/// A batch close that would close tabs holding other panes as well.
pub struct ConfirmCloseMixed {
    pub ids: Vec<String>,
    /// (label, its tab holds other panes)
    sessions: Vec<(String, bool)>,
}

impl ConfirmCloseMixed {
    pub fn new(ids: Vec<String>, sessions: Vec<(String, bool)>) -> ConfirmCloseMixed {
        ConfirmCloseMixed { ids, sessions }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        let mixed: Vec<&str> = self.sessions.iter().filter(|(_, m)| *m).map(|(l, _)| l.as_str()).collect();
        let dialog_title = t!("close-mixed-title");
        let skin = crate::looks::skin(&ctx.global_style().visuals);
        let dialog_shown =
            native_term_skin::Modal::new("dialogs-4", &dialog_title).icon(crate::icons::TABS).show(ctx, &skin, |ui| {
                ui.label(t!("close-mixed-what", count = self.sessions.len(), mixed = mixed.len()));
                ui.weak(t!("close-mixed-hint"));
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for (label, mixed) in &self.sessions {
                        if *mixed {
                            ui.label(format!("{label}  ·  {}", t!("session-split")));
                        } else {
                            ui.weak(label);
                        }
                    }
                });
                let choices = [
                    Choice::new(t!("close-mixed-close", count = self.sessions.len()), Role::Danger),
                    Choice::new(t!("button-cancel"), Role::Plain),
                ];
                match crate::skinned::row(ui, &choices) {
                    Some(0) => outcome = Outcome::Submit(()),
                    Some(_) => outcome = Outcome::Cancel,
                    None => {}
                }
            });
        if dialog_shown.closed {
            open = false;
        }
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}
