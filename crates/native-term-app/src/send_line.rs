//! The command line pinned to the bottom of the sidebar: type a command,
//! Enter sends it to the active session (the NativeTerm tab last in
//! front) or to every logged-in session. Sending to more than one asks
//! once more (Enter again, or Esc); locked sessions and folders marked
//! "No group send" are left out of "all". ↑ / ↓ go through this run's
//! earlier lines. Every send lands in the audit log (`Core::send_text`).

use std::collections::HashSet;

use native_term_app::{t, Core, SessionView, State};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Target {
    #[default]
    Active,
    AllConnected,
}

#[derive(Default)]
pub struct SendLine {
    text: String,
    target: Target,
    history: Vec<String>,
    /// Where ↑ / ↓ are in `history` (None: the line being typed).
    browsing: Option<usize>,
    /// Sending to several, waiting for the second Enter: (ids, text).
    confirm: Option<(Vec<String>, String)>,
    result: Option<(bool, String)>,
}

const HISTORY: usize = 100;

impl SendLine {
    /// `no_group_send`: aliases whose folder is marked "No group send".
    pub fn show(&mut self, ui: &mut egui::Ui, core: &Core, no_group_send: &HashSet<String>) {
        let sessions = core.sessions();
        let active = core.active_session().filter(|s| s.state == State::Connected && s.linked);
        let all: Vec<&SessionView> = sessions.iter().filter(|s| s.state == State::Connected && s.linked).collect();
        let chosen: Vec<&SessionView> =
            all.iter().copied().filter(|s| !s.locked && !no_group_send.contains(&s.alias)).collect();
        let left_out = all.len() - chosen.len();

        ui.horizontal(|ui| {
            ui.label(t!("send-line-to"));
            let active_text = match &active {
                Some(s) => t!("send-line-active", label = s.label.as_str()),
                None => t!("send-line-no-active"),
            };
            let all_text = t!("send-line-all", count = chosen.len());
            let current = match self.target {
                Target::Active => active_text.clone(),
                Target::AllConnected => all_text.clone(),
            };
            egui::ComboBox::from_id_salt("send-line-target").selected_text(current).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.target, Target::Active, active_text);
                ui.selectable_value(&mut self.target, Target::AllConnected, all_text);
            });
            if self.target == Target::AllConnected && left_out > 0 {
                ui.weak(t!("send-line-left-out", count = left_out)).on_hover_text(t!("send-line-left-out-hint"));
            }
        });

        let id = egui::Id::new("send-line-text");
        let focused = ui.memory(|m| m.has_focus(id));
        if focused && self.confirm.is_none() {
            let (up, down) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                )
            });
            self.browse(up, down);
        }
        let targets: Vec<String> = match self.target {
            Target::Active => active.iter().map(|s| s.id.clone()).collect(),
            Target::AllConnected => chosen.iter().map(|s| s.id.clone()).collect(),
        };
        let edit = ui.add_enabled(
            self.confirm.is_none(),
            egui::TextEdit::singleline(&mut self.text)
                .id(id)
                .hint_text(t!("send-line-hint"))
                .desired_width(f32::INFINITY),
        );
        if edit.changed() {
            self.browsing = None;
            self.result = None;
        }
        let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if let Some((ids, text)) = self.confirm.clone() {
            let names: Vec<String> = sessions.iter().filter(|s| ids.contains(&s.id)).map(|s| s.label.clone()).collect();
            ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), t!("send-line-confirm", count = ids.len()))
                .on_hover_text(names.join("\n"));
            let (again, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            let mut send = again;
            let mut cancel = escape;
            ui.horizontal(|ui| {
                send |= ui.button(t!("send-line-send")).clicked();
                cancel |= ui.button(t!("button-cancel")).clicked();
            });
            if send {
                self.confirm = None;
                self.send(core, &ids, &text);
                ui.memory_mut(|m| m.request_focus(id));
            } else if cancel {
                self.confirm = None;
                ui.memory_mut(|m| m.request_focus(id));
            }
        } else if entered && !self.text.trim().is_empty() {
            match targets.len() {
                0 => self.result = Some((false, t!("send-line-nobody"))),
                1 => {
                    let text = std::mem::take(&mut self.text);
                    self.send(core, &targets, &text);
                }
                _ => self.confirm = Some((targets, std::mem::take(&mut self.text))),
            }
            ui.memory_mut(|m| m.request_focus(id));
        }
        if let Some((ok, text)) = &self.result {
            let color = if *ok { ui.visuals().weak_text_color() } else { egui::Color32::from_rgb(0xd0, 0x3a, 0x3a) };
            ui.colored_label(color, text);
        }
    }

    fn send(&mut self, core: &Core, ids: &[String], text: &str) {
        let report = core.send_text(ids, text, true);
        let mut summary = t!("send-line-sent", count = report.sent.len());
        if !report.skipped.is_empty() {
            summary.push_str(&format!(" · {}", t!("send-line-skipped", names = report.skipped.join(", "))));
        }
        if !report.failed.is_empty() {
            summary.push_str(&format!(" · {}", t!("send-line-failed", names = report.failed.join(", "))));
        }
        self.result = Some((report.failed.is_empty() && !report.sent.is_empty(), summary));
        if self.history.last().map(String::as_str) != Some(text) {
            self.history.push(text.to_string());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        self.browsing = None;
    }

    /// ↑: an earlier line; ↓: a later one, then an empty line again.
    fn browse(&mut self, up: bool, down: bool) {
        if self.history.is_empty() || !(up || down) {
            return;
        }
        let last = self.history.len() - 1;
        self.browsing = match (self.browsing, up) {
            (None, true) => Some(last),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i < last => Some(i + 1),
            (_, false) => None,
        };
        self.text = self.browsing.map(|i| self.history[i].clone()).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_goes_back_and_forth() {
        let mut line = SendLine { history: vec!["uptime".into(), "df -h".into()], ..SendLine::default() };
        line.browse(true, false);
        assert_eq!(line.text, "df -h");
        line.browse(true, false);
        assert_eq!(line.text, "uptime");
        line.browse(true, false);
        assert_eq!(line.text, "uptime", "stays at the oldest");
        line.browse(false, true);
        assert_eq!(line.text, "df -h");
        line.browse(false, true);
        assert_eq!(line.text, "", "back to a new line");
    }
}
