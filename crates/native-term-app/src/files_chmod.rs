//! Changing what may be done with files on the server (chmod): a
//! dialog of its own, the owner's, the group's and the others' read,
//! write and run as boxes to tick, and the same as the octal number
//! (each changes the other). It starts from the first file's mode;
//! OK sets the mode on each file chosen (SETSTAT, the mode alone).

use super::*;

/// The dialog's state.
pub(super) struct Chmod {
    tab: u64,
    items: Vec<(Vec<u8>, Attrs)>,
    what: String,
    /// The mode's bits (`0o7777`: with set-user-id, set-group-id and
    /// sticky).
    mode: u32,
    /// The octal number as typed.
    text: String,
}

fn octal(mode: u32) -> String {
    if mode > 0o777 {
        format!("{mode:04o}")
    } else {
        format!("{mode:03o}")
    }
}

/// An octal number as typed (three or four digits), if it is one.
fn parse(text: &str) -> Option<u32> {
    let text = text.trim();
    (matches!(text.len(), 3 | 4) && text.chars().all(|c| ('0'..='7').contains(&c)))
        .then(|| u32::from_str_radix(text, 8).ok())
        .flatten()
}

impl FilesWindow {
    /// Asks how the files chosen on the server may be used.
    pub(super) fn ask_chmod(&mut self, id: u64) {
        let items = self.remote_selection(id);
        let Some((_, first)) = items.first() else { return };
        let mode = first.permissions.unwrap_or(0o644) & 0o7777;
        let names = self.tabs.iter().find(|t| t.id == id).map(|t| t.remote.names).unwrap_or_default();
        let what = describe(&items.iter().map(|(p, _)| names.decode(last(p))).collect::<Vec<_>>());
        self.chmod = Some(Chmod { tab: id, items, what, mode, text: octal(mode) });
    }

    /// The dialog (a window of its own, modal).
    pub(super) fn chmod_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = &mut self.chmod else { return };
        let mut done = None;
        let closed = modal(ctx, "files-chmod", t!("files-chmod-title"), crate::icons::SHIELD, |ui| {
            ui.label(egui::RichText::new(&c.what).strong());
            ui.add_space(8.0);
            let before = c.mode;
            egui::Grid::new("files-chmod-grid").num_columns(4).spacing([18.0, 8.0]).show(ui, |ui| {
                ui.label("");
                for title in [t!("files-chmod-read"), t!("files-chmod-write"), t!("files-chmod-execute")] {
                    ui.label(egui::RichText::new(title).weak());
                }
                ui.end_row();
                for (row, who) in
                    [t!("files-chmod-owner"), t!("files-chmod-group"), t!("files-chmod-others")].iter().enumerate()
                {
                    ui.label(who);
                    for bit in 0..3 {
                        // rwx of owner, group, others: 0o400 down to 0o001
                        let mask = 1u32 << (8 - (row * 3 + bit));
                        let mut on = c.mode & mask != 0;
                        if ui.checkbox(&mut on, "").changed() {
                            c.mode = if on { c.mode | mask } else { c.mode & !mask };
                        }
                    }
                    ui.end_row();
                }
            });
            if c.mode != before {
                c.text = octal(c.mode);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(t!("files-chmod-octal"));
                let field = ui
                    .add(egui::TextEdit::singleline(&mut c.text).desired_width(56.0).font(egui::TextStyle::Monospace));
                if field.changed() {
                    if let Some(mode) = parse(&c.text) {
                        c.mode = mode;
                    }
                }
                let shown = mode_text(0o100000 | c.mode);
                ui.label(egui::RichText::new(shown).monospace().weak());
            });
            let valid = parse(&c.text).is_some();
            if !valid {
                ui.colored_label(RED, t!("files-chmod-bad"));
            }
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && valid;
            done = ok_cancel(ui, t!("button-ok"), native_term_skin::Role::Primary)
                .filter(|ok| !ok || valid)
                .or(enter.then_some(true));
        });
        if closed && done.is_none() {
            done = Some(false);
        }
        let Some(ok) = done else { return };
        ctx.memory_mut(|m| m.stop_text_input());
        let Some(c) = self.chmod.take() else { return };
        if !ok {
            return;
        }
        let Some(sftp) = self.tab(c.tab).and_then(|t| t.remote.sftp.clone()) else { return };
        let mode = c.mode;
        self.log(c.tab, t!("files-chmod-done", what = c.what.as_str(), mode = octal(mode)), false);
        self.spawn(c.tab, move || {
            let failed: Vec<String> = c
                .items
                .iter()
                .filter_map(|(path, _)| {
                    let attrs = Attrs { permissions: Some(mode), ..Attrs::default() };
                    sftp.setstat(path, &attrs).err().map(|e| e.to_string())
                })
                .collect();
            match failed.first() {
                // the list again, with the new modes
                None => What::Refresh,
                Some(e) => What::Notice(e.clone(), true),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_both_ways() {
        assert_eq!(octal(0o755), "755");
        assert_eq!(octal(0o4755), "4755");
        assert_eq!(parse("644"), Some(0o644));
        assert_eq!(parse(" 2775 "), Some(0o2775));
        assert_eq!(parse("78"), None);
        assert_eq!(parse("688"), None);
        assert_eq!(parse("12345"), None);
    }
}
