//! The session log's settings, laid out as SecureCRT's Session Options →
//! Terminal → Log File page: the file, the options, the custom texts and
//! the substitutions. Shared by the session options (an SSH host, or a
//! folder's default) and the non-SSH session dialog. A session either
//! uses its folder's settings or has a whole set of its own (see
//! `native_term_config::session_log`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use native_term_app::t;
use native_term_config::session_log::{self, LogSettings, MAX_TRACE};

pub struct LogPage {
    /// A session's page (a folder's has no "use the folder's").
    session: bool,
    /// The session uses its folder's settings.
    inherit: bool,
    own: LogSettings,
    folder: LogSettings,
    /// The default file, in full (shown when none is named).
    default_file: String,
    /// The desktop's file dialog, answered on a thread of its own.
    picked: Option<Arc<Mutex<Option<Option<PathBuf>>>>>,
}

impl LogPage {
    /// A session's page: its own set if it has one, else its folder's.
    pub fn for_session(own: Option<LogSettings>, folder: Option<LogSettings>, data_dir: &Path) -> LogPage {
        let folder = folder.unwrap_or_default();
        LogPage {
            session: true,
            inherit: own.is_none(),
            own: own.unwrap_or_else(|| folder.clone()),
            folder,
            default_file: session_log::default_file(data_dir).display().to_string(),
            picked: None,
        }
    }

    /// A folder's default set.
    pub fn for_folder(folder: Option<LogSettings>, data_dir: &Path) -> LogPage {
        LogPage {
            session: false,
            inherit: false,
            own: folder.unwrap_or_default(),
            folder: LogSettings::default(),
            default_file: session_log::default_file(data_dir).display().to_string(),
            picked: None,
        }
    }

    /// What to write: a session's own set, or `None` for its folder's; a
    /// folder's set, or `None` when nothing in it differs from nothing set.
    pub fn result(&self) -> Option<LogSettings> {
        match (self.session, self.inherit) {
            (true, true) => None,
            (true, false) => Some(self.own.clone()),
            (false, _) => (self.own != LogSettings::default()).then(|| self.own.clone()),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.take_picked();
        if self.session {
            if ui.checkbox(&mut self.inherit, t!("log-use-folder")).changed() && !self.inherit {
                // starting from what the folder has
                self.own = self.folder.clone();
            }
            ui.add_space(4.0);
        }
        let editable = !self.inherit;
        let shown = if self.inherit { self.folder.clone() } else { self.own.clone() };
        let mut s = shown;
        ui.add_enabled_ui(editable, |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(t!("log-file-name"));
                ui.horizontal(|ui| {
                    let field = egui::TextEdit::singleline(&mut s.file).hint_text(&self.default_file);
                    ui.add(field.desired_width(ui.available_width() - 40.0));
                    if ui.button("…").on_hover_text(t!("log-pick-title")).clicked() && self.picked.is_none() {
                        self.pick(&s.file);
                    }
                });
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(t!("log-options"));
                ui.columns(2, |columns| {
                    let left = &mut columns[0];
                    left.checkbox(&mut s.prompt, t!("log-prompt"));
                    left.checkbox(&mut s.start, t!("log-start"));
                    left.checkbox(&mut s.raw, t!("log-raw"));
                    left.checkbox(&mut s.midnight, t!("log-midnight"));
                    let right = &mut columns[1];
                    right.radio_value(&mut s.append, false, t!("log-overwrite"));
                    right.radio_value(&mut s.append, true, t!("log-append"));
                    right.checkbox(&mut s.timestamp, t!("log-timestamp"));
                    right.horizontal(|ui| {
                        ui.label(t!("log-trace"));
                        ui.add(egui::DragValue::new(&mut s.trace).range(0..=MAX_TRACE));
                    });
                });
                ui.weak(t!("log-trace-note"));
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(t!("log-custom"));
                egui::Grid::new("log-custom").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    for (label, text) in [
                        (t!("log-upon-connect"), &mut s.upon_connect),
                        (t!("log-upon-disconnect"), &mut s.upon_disconnect),
                        (t!("log-each-line"), &mut s.each_line),
                    ] {
                        ui.label(label);
                        ui.add(egui::TextEdit::singleline(text).desired_width(360.0));
                        ui.end_row();
                    }
                });
                let custom = s.has_custom();
                if !custom {
                    s.only_custom = false;
                }
                ui.add_enabled(custom, egui::Checkbox::new(&mut s.only_custom, t!("log-only-custom")));
            });
        });
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(t!("log-substitutions"));
            for line in [t!("log-subst-names"), t!("log-subst-date"), t!("log-subst-time"), t!("log-subst-other")] {
                ui.monospace(line);
            }
            ui.weak(t!("log-default-note", file = self.default_file.as_str()));
        });
        if editable {
            self.own = s;
        }
    }

    /// The desktop's save dialog, on a thread of its own (it waits for
    /// the person; the window keeps drawing).
    fn pick(&mut self, current: &str) {
        let slot = Arc::new(Mutex::new(None));
        self.picked = Some(Arc::clone(&slot));
        let start = match current.trim() {
            "" => PathBuf::from(&self.default_file),
            file => PathBuf::from(file),
        };
        let title = t!("log-pick-title");
        std::thread::spawn(move || {
            let chosen = native_term_os::picker::pick_save(&title, &start);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(chosen);
        });
    }

    fn take_picked(&mut self) {
        let Some(slot) = &self.picked else { return };
        let answer = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(answer) = answer {
            if let (Some(path), false) = (answer, self.inherit) {
                self.own.file = path.display().to_string();
            }
            self.picked = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_writes_its_own_set_or_none() {
        let folder = LogSettings { start: true, ..LogSettings::default() };
        let page = LogPage::for_session(None, Some(folder.clone()), Path::new("/d"));
        assert_eq!(page.result(), None, "inherits: nothing of its own is written");
        let mut page = LogPage::for_session(None, Some(folder.clone()), Path::new("/d"));
        page.inherit = false;
        page.own.raw = true;
        assert_eq!(page.result(), Some(LogSettings { raw: true, ..folder }));
        // a folder with nothing set writes nothing
        let page = LogPage::for_folder(None, Path::new("/d"));
        assert_eq!(page.result(), None);
    }
}
