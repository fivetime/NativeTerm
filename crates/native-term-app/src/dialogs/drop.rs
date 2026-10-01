//! Files dropped into a tab: uploaded, or typed as their names.

use std::path::PathBuf;

use native_term_app::t;
use native_term_skin::{Choice, Role};

use super::*;

/// What to do with files dropped into a tab: Terminal pasted their names
/// and the client held the text back.
pub struct DropDialog {
    pub alias: String,
    pub session: String,
    pub label: String,
    pub paths: Vec<PathBuf>,
    /// The text Terminal pasted, sent to the session when that is chosen.
    pub text: String,
    /// The answer is remembered and this dialog skipped from then on.
    pub remember: bool,
    /// The session's files are on a server we can reach over SFTP.
    pub can_upload: bool,
}

/// What the person chose for dropped files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropChoice {
    Upload,
    Text,
}

impl DropDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<(DropChoice, bool)> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        // a folder's own entry says nothing about what is in it, which is
        // counted when the transfer is planned
        let files = self.paths.iter().filter(|p| p.is_file());
        let total: u64 = files.filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
        let folders = self.paths.iter().any(|p| p.is_dir());
        let dialog_title = t!("drop-title");
        let skin = crate::looks::skin(&ctx.global_style().visuals);
        let dialog_shown = native_term_skin::Modal::new("dialogs-5", &dialog_title).icon(crate::icons::UPLOAD).show(
            ctx,
            &skin,
            |ui| {
                ui.label(t!("drop-what", count = self.paths.len(), label = self.label.as_str()));
                egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                    for path in &self.paths {
                        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().to_string();
                        if path.is_dir() {
                            ui.label(format!("{name}  ·  {}", t!("drop-folder")));
                        } else {
                            ui.label(name);
                        }
                    }
                });
                let size = crate::files_window::size_text(total);
                if folders {
                    ui.weak(t!("drop-size-folders", size = size));
                } else if total > 0 {
                    ui.weak(t!("drop-size", size = size));
                }
                ui.weak(t!("drop-where"));
                ui.checkbox(&mut self.remember, t!("drop-remember"));
                let choices = [
                    Choice::new(t!("drop-upload"), Role::Primary).enabled(self.can_upload),
                    Choice::new(t!("drop-text"), Role::Plain),
                    Choice::new(t!("button-cancel"), Role::Plain),
                ];
                match crate::skinned::row(ui, &choices) {
                    Some(0) => outcome = Outcome::Submit((DropChoice::Upload, self.remember)),
                    Some(1) => outcome = Outcome::Submit((DropChoice::Text, self.remember)),
                    Some(_) => outcome = Outcome::Cancel,
                    None => {}
                }
                if !self.can_upload {
                    ui.weak(t!("drop-no-sftp"));
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
