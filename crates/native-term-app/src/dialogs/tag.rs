//! Renaming or deleting a tag on every host that has it.

use native_term_app::t;
use native_term_skin::{Choice, Role};

use super::*;

/// A tag called something else, or deleted: everywhere it is.
pub struct TagDialog {
    pub tag: String,
    /// What it is called from now on; `None`: it is deleted.
    name: Option<String>,
    /// How many hosts have it.
    hosts: usize,
    focused: bool,
    pub error: Option<String>,
}

impl TagDialog {
    pub fn rename(tag: &str, hosts: usize) -> TagDialog {
        TagDialog { tag: tag.to_string(), name: Some(tag.to_string()), hosts, focused: false, error: None }
    }

    pub fn delete(tag: &str, hosts: usize) -> TagDialog {
        TagDialog { tag: tag.to_string(), name: None, hosts, focused: false, error: None }
    }

    /// `Submit(Some(name))`: called that; `Submit(None)`: deleted.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<Option<String>> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        let title = match self.name {
            Some(_) => t!("tag-rename-title", tag = self.tag.as_str()),
            None => t!("tag-delete-title", tag = self.tag.as_str()),
        };
        let dialog_title = title;
        let skin = crate::looks::skin(&ctx.global_style().visuals);
        let dialog_shown =
            native_term_skin::Modal::new("dialogs-0", &dialog_title).icon(crate::icons::EDIT).show(ctx, &skin, |ui| {
                ui.set_max_width(320.0);
                let mut entered = false;
                match &mut self.name {
                    Some(name) => {
                        let edit = ui
                            .add(egui::TextEdit::singleline(name).hint_text(t!("tag-name-hint")).desired_width(300.0));
                        if !std::mem::replace(&mut self.focused, true) {
                            edit.request_focus();
                        }
                        entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.weak(t!("tag-rename-text", count = self.hosts));
                    }
                    None => {
                        ui.label(t!("tag-delete-text", count = self.hosts));
                    }
                }
                if let Some(error) = &self.error {
                    ui.colored_label(crate::looks::skin(ui.visuals()).palette.danger, error);
                }
                // (one name: a comma would make two tags of it)
                let named = self.name.as_deref().map(native_term_app::registry::Note::tags_from);
                let fine = named.as_ref().is_none_or(|tags| tags.len() == 1);
                let first = match self.name {
                    Some(_) => Choice::new(t!("button-save"), Role::Primary),
                    None => Choice::new(t!("button-delete"), Role::Danger),
                };
                let pressed =
                    crate::skinned::row(ui, &[first.enabled(fine), Choice::new(t!("button-cancel"), Role::Plain)]);
                if pressed == Some(0) || (entered && fine) {
                    outcome = Outcome::Submit(named.and_then(|mut tags| tags.pop()));
                } else if pressed == Some(1) {
                    outcome = Outcome::Cancel;
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
