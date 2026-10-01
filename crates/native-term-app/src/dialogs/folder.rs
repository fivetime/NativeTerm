//! A new folder, or a folder renamed.

use std::path::PathBuf;

use native_term_app::t;
use native_term_skin::{Choice, Role};

use super::*;

/// A folder name (new folder, or rename).
pub struct FolderDialog {
    pub title: String,
    /// `Some(file)` when renaming.
    pub file: Option<PathBuf>,
    name: String,
    pub error: Option<String>,
}

impl FolderDialog {
    pub fn new_folder() -> FolderDialog {
        FolderDialog { title: t!("folder-new-title"), file: None, name: String::new(), error: None }
    }

    pub fn rename(file: PathBuf, current: &str) -> FolderDialog {
        FolderDialog {
            title: t!("folder-rename-title", name = current),
            file: Some(file),
            name: current.to_string(),
            error: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<String> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        let dialog_title = self.title.clone();
        let skin = crate::looks::skin(&ctx.global_style().visuals);
        let dialog_shown = native_term_skin::Modal::new("dialogs-2", &dialog_title).icon(crate::icons::FOLDER).show(
            ctx,
            &skin,
            |ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.name).hint_text(t!("folder-name-hint")).desired_width(260.0),
                );
                if self.name.is_empty() {
                    edit.request_focus();
                }
                if let Some(error) = &self.error {
                    ui.colored_label(crate::looks::skin(ui.visuals()).palette.danger, error);
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let named = !self.name.trim().is_empty();
                let choices = [
                    Choice::new(t!("button-save"), Role::Primary).enabled(named),
                    Choice::new(t!("button-cancel"), Role::Plain),
                ];
                let pressed = crate::skinned::row(ui, &choices);
                if pressed == Some(0) || (enter && named) {
                    outcome = Outcome::Submit(self.name.trim().to_string());
                } else if pressed == Some(1) {
                    outcome = Outcome::Cancel;
                }
            },
        );
        if dialog_shown.closed {
            open = false;
        }
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}
