//! Deleting a host from the ssh configuration.

use native_term_app::t;
use native_term_skin::{Choice, Role};

use super::*;

pub struct ConfirmDelete {
    pub alias: String,
    label: String,
    pub error: Option<String>,
}

impl ConfirmDelete {
    pub fn new(alias: &str, label: &str) -> ConfirmDelete {
        ConfirmDelete { alias: alias.to_string(), label: label.to_string(), error: None }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        use native_term_skin::{Message, Notice};
        let skin = crate::looks::skin(&ctx.global_style().visuals);
        let title = t!("delete-title");
        let shown = Message::new("delete-host", &title)
            .icon(crate::icons::DELETE)
            .notice(Notice::Warning)
            .choice(Choice::new(t!("button-delete"), Role::Danger))
            .choice(Choice::new(t!("button-cancel"), Role::Plain))
            .show(
                ctx,
                &skin,
                |ui| {
                    ui.label(t!("delete-question", label = self.label.as_str(), alias = self.alias.as_str()));
                    ui.weak(t!("delete-backup-note"));
                    if let Some(error) = &self.error {
                        ui.colored_label(skin.palette.danger, error);
                    }
                },
                |_| {},
            );
        match shown.pressed {
            Some(0) => Outcome::Submit(()),
            Some(_) => Outcome::Cancel,
            None if shown.closed => Outcome::Cancel,
            None => Outcome::Open,
        }
    }
}
