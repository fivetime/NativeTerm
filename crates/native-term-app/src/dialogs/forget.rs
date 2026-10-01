//! Forgetting the saved passwords of a host's accounts.

use native_term_app::t;
use native_term_skin::{Choice, Role};

use super::*;

/// "Forget the host keys of …?"
pub struct ConfirmForget {
    pub alias: String,
    names: Vec<String>,
    pub error: Option<String>,
}

impl ConfirmForget {
    pub fn new(alias: &str, names: Vec<String>) -> ConfirmForget {
        ConfirmForget { alias: alias.to_string(), names, error: None }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        let dialog_title = t!("forget-title");
        let skin = crate::looks::skin(&ctx.global_style().visuals);
        let dialog_shown =
            native_term_skin::Modal::new("dialogs-3", &dialog_title).icon(crate::icons::LOCK).show(ctx, &skin, |ui| {
                ui.label(t!("forget-question", alias = self.alias.as_str()));
                for name in &self.names {
                    ui.monospace(format!("  {name}"));
                }
                ui.weak(t!("forget-note"));
                if let Some(error) = &self.error {
                    ui.colored_label(crate::looks::skin(ui.visuals()).palette.danger, error);
                }
                let choices =
                    [Choice::new(t!("forget-button"), Role::Danger), Choice::new(t!("button-cancel"), Role::Plain)];
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
