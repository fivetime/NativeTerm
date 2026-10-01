//! A side of the files window as the design has it (`.pane`): the
//! sessions' strip at its top, a bar of what to do (up, refresh, the
//! path's crumbs, a filter, send to the other side, a new folder,
//! delete, more), and a line at its foot (counts, the view's switch).
//! The tree and the list between them are `tree` and `files_list`.

use super::*;
use native_term_skin::{IconButton, PathBar, StripTab, TabStrip};

/// What a side's bar asked for.
pub(super) enum Asked {
    Up,
    Refresh,
    /// The path's folder at this crumb.
    Crumb(usize),
    /// A path typed.
    Typed(String),
    /// To the other side: upload or download what is chosen.
    Send,
    NewFolder,
    Delete,
    Edit,
    Sync,
    CopyPath,
    Names(Names),
}

/// A local path's crumbs: its root (`C:`, `/`) and each folder in it;
/// the computer's name for the drives.
pub(super) fn local_crumbs(path: Option<&Path>, computer: &str) -> Vec<(String, Option<PathBuf>)> {
    let Some(path) = path else { return vec![(computer.to_string(), None)] };
    let mut crumbs = Vec::new();
    let mut at = PathBuf::new();
    for part in path.components() {
        at.push(part.as_os_str());
        let name = match part {
            std::path::Component::Prefix(p) => p.as_os_str().to_string_lossy().into_owned(),
            std::path::Component::RootDir if cfg!(windows) => continue,
            std::path::Component::RootDir => "/".to_string(),
            other => other.as_os_str().to_string_lossy().into_owned(),
        };
        crumbs.push((name, Some(at.clone())));
    }
    // on Windows the drive's crumb is the drive with its root (`C:\`)
    for (_, p) in &mut crumbs {
        if let Some(path) = p {
            if cfg!(windows) && path.as_os_str().len() == 2 {
                path.push(std::path::MAIN_SEPARATOR_STR);
            }
        }
    }
    crumbs
}

/// A server's path's crumbs: `/` and each folder, with the path to it.
pub(super) fn remote_crumbs(path: &[u8], names: Names) -> Vec<(String, Vec<u8>)> {
    let mut crumbs = vec![("/".to_string(), b"/".to_vec())];
    let mut at = Vec::new();
    for part in path.split(|b| *b == b'/').filter(|p| !p.is_empty()) {
        at.push(b'/');
        at.extend_from_slice(part);
        crumbs.push((names.decode(part), at.clone()));
    }
    crumbs
}

impl FilesWindow {
    /// The sessions' strip of a side: numbered alike on both sides, the
    /// chosen one in the side's colour.
    pub(super) fn strip(&mut self, ui: &mut egui::Ui, remote: bool) {
        let palette = crate::looks::skin(ui.visuals()).palette;
        let look = files_list::Look::of(ui.visuals());
        let tones = crate::looks::tones(ui.visuals());
        let tabs: Vec<StripTab> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                if remote {
                    let dot = match (tab.remote.up(), &tab.remote.failed) {
                        (_, Some(_)) => tones.danger,
                        (true, None) => tones.alive,
                        (false, None) => palette.weak,
                    };
                    StripTab {
                        number: Some(i + 1),
                        glyph: Some(icons::SERVER.to_string()),
                        label: tab.spec.label.clone(),
                        sub: (tab.spec.label != tab.spec.alias).then(|| tab.spec.alias.clone()),
                        dot: Some(dot),
                        hint: Some(tab.spec.alias.clone()),
                        closable: true,
                        marked: false,
                    }
                } else {
                    let folder =
                        tab.local.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
                    let shown =
                        tab.local.path.as_ref().map_or_else(|| self.computer.clone(), |p| p.display().to_string());
                    StripTab {
                        number: Some(i + 1),
                        glyph: Some(icons::LAPTOP.to_string()),
                        label: folder.unwrap_or_else(|| self.computer.clone()),
                        sub: None,
                        dot: None,
                        hint: Some(format!("{} · {shown}", self.computer)),
                        closable: true,
                        marked: false,
                    }
                }
            })
            .collect();
        let accent = look.side(remote);
        let salt = if remote { "remote-strip" } else { "local-strip" };
        let shown = TabStrip::new(salt, &palette).accent(accent).show(
            ui,
            &tabs,
            Some(self.active),
            &t!("files-close-tab"),
            |_| {},
        );
        if let Some(i) = shown.clicked {
            self.active = i;
            self.remote_focus = remote;
        }
        if let Some(i) = shown.closed {
            let id = self.tabs[i].id;
            if self.running(Some(id)) + self.edits(Some(id)) > 0 {
                self.confirm = Some(Confirm::CloseTab(id));
            } else {
                self.close_tab(id);
            }
        }
    }

    /// A side's bar: what it asked for.
    pub(super) fn head(&mut self, ui: &mut egui::Ui, remote: bool) -> Vec<Asked> {
        let mut asked = Vec::new();
        let palette = crate::looks::skin(ui.visuals()).palette;
        let look = files_list::Look::of(ui.visuals());
        let Some(tab) = self.tabs.get(self.active) else { return asked };
        let connected = tab.remote.up();
        let (enabled, chosen, at_top) = if remote {
            (connected, !tab.remote.selected.is_empty(), tab.remote.path == b"/")
        } else {
            (true, !tab.local.selected.is_empty(), tab.local.path.is_none())
        };
        let one_file = remote
            && tab.remote.selected.len() == 1
            && tab.remote.rows.iter().any(|r| !r.dir && tab.remote.selected.contains(&r.entry.name));
        let (crumbs, full): (Vec<String>, String) = if remote {
            (
                remote_crumbs(&tab.remote.path, tab.remote.names).into_iter().map(|(n, _)| n).collect(),
                tab.remote.path_text.clone(),
            )
        } else {
            (
                local_crumbs(tab.local.path.as_deref(), &self.computer).into_iter().map(|(n, _)| n).collect(),
                tab.local.path_text.clone(),
            )
        };
        let names = tab.remote.names;
        let mut filter = if remote { tab.remote.filter.clone() } else { tab.local.filter.clone() };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, palette.line));
        let mut bar = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut bar;
        ui.spacing_mut().item_spacing.x = 2.0;
        if IconButton::new(icons::UP.to_string(), t!("files-up"))
            .small()
            .enabled(enabled && !at_top)
            .show(ui, &palette)
            .clicked()
        {
            asked.push(Asked::Up);
        }
        if IconButton::new(icons::REFRESH.to_string(), t!("files-refresh"))
            .small()
            .enabled(enabled)
            .show(ui, &palette)
            .clicked()
        {
            asked.push(Asked::Refresh);
        }
        ui.add_space(4.0);
        // what is at the end: the path takes what is left, the filter
        // gives some of its room first
        let filter_width = (ui.available_width() * 0.2).clamp(90.0, 140.0);
        let ends = filter_width + 8.0 + 9.0 + 8.0 + 76.0 + 26.0 * if remote { 4.0 } else { 3.0 } + 12.0;
        let width = (ui.available_width() - ends).max(120.0);
        let edit = ui.input(|i| i.key_pressed(egui::Key::F4))
            && self.remote_focus == remote
            && !ui.ctx().egui_wants_keyboard_input();
        let salt = if remote { "remote-path" } else { "local-path" };
        let path = PathBar::new(salt, &palette).show(ui, &crumbs, &full, width, edit && enabled);
        if let Some(i) = path.crumb {
            asked.push(Asked::Crumb(i));
        }
        if let Some(text) = path.typed {
            asked.push(Asked::Typed(text));
        }
        ui.add_space(4.0);
        native_term_skin::filter_field(
            ui,
            &palette,
            &mut filter,
            &icons::SEARCH.to_string(),
            &t!("files-filter"),
            filter_width,
        );
        ui.add_space(4.0);
        let (sep, _) = ui.allocate_exact_size(egui::vec2(9.0, 16.0), egui::Sense::hover());
        ui.painter().vline(sep.center().x, sep.y_range(), egui::Stroke::new(1.0, palette.line));
        // send to the other side, in the side's colour
        let (glyph, text, hint) = if remote {
            (icons::DOWNLOAD, t!("files-download-to-local"), t!("files-download-hint"))
        } else {
            (icons::UPLOAD, t!("files-upload-to-remote"), t!("files-upload-hint"))
        };
        let side = look.side(remote);
        let send_on = chosen && connected && enabled;
        let label = egui::RichText::new(format!("{glyph} {text}"))
            .font(native_term_skin::font(ui.ctx(), 12.0, native_term_skin::Weight::Medium))
            .color(if send_on { side } else { palette.weak });
        let send = egui::Button::new(label)
            .fill(native_term_skin::thin(side, if send_on { 40 } else { 16 }))
            .stroke(egui::Stroke::NONE)
            .corner_radius(8.0)
            .min_size(egui::vec2(0.0, 26.0));
        if ui.add_enabled(send_on, send).on_hover_text(hint).clicked() {
            asked.push(Asked::Send);
        }
        ui.add_space(4.0);
        if IconButton::new(icons::NEW_FOLDER.to_string(), t!("files-new-folder"))
            .small()
            .enabled(enabled && !at_top || remote && enabled)
            .show(ui, &palette)
            .clicked()
        {
            asked.push(Asked::NewFolder);
        }
        if remote
            && IconButton::new(icons::EDIT.to_string(), t!("files-edit-hint"))
                .small()
                .enabled(one_file)
                .show(ui, &palette)
                .clicked()
        {
            asked.push(Asked::Edit);
        }
        if IconButton::new(icons::DELETE.to_string(), t!("files-delete"))
            .small()
            .danger()
            .enabled(enabled && chosen)
            .show(ui, &palette)
            .clicked()
        {
            asked.push(Asked::Delete);
        }
        let more = IconButton::new(icons::MORE.to_string(), t!("files-more")).small().show(ui, &palette);
        egui::Popup::menu(&more).show(|ui| {
            if ui.add_enabled(chosen, egui::Button::new(t!("files-copy-path"))).clicked() {
                asked.push(Asked::CopyPath);
            }
            if remote {
                let local_folder = self.tabs.get(self.active).is_some_and(|t| t.local.path.is_some());
                if ui
                    .add_enabled(
                        connected && local_folder,
                        egui::Button::new(format!("{} {}", icons::SYNC, t!("files-sync"))),
                    )
                    .on_hover_text(t!("files-sync-hint"))
                    .clicked()
                {
                    asked.push(Asked::Sync);
                }
                ui.separator();
                ui.menu_button(t!("files-encoding"), |ui| {
                    for label in ENCODINGS {
                        if let Some(choice) = Names::from_label(label) {
                            let text = if label == "auto" { t!("files-encoding-auto") } else { label.to_string() };
                            if ui.radio(names == choice, text).clicked() {
                                asked.push(Asked::Names(choice));
                                ui.close();
                            }
                        }
                    }
                });
            }
        });
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if remote {
                tab.remote.filter = filter;
            } else {
                tab.local.filter = filter;
            }
        }
        asked
    }

    /// A side's foot: its counts, what is chosen, the view's switch.
    pub(super) fn foot(&mut self, ui: &mut egui::Ui, remote: bool) {
        let palette = crate::looks::skin(ui.visuals()).palette;
        let Some(tab) = self.tabs.get(self.active) else { return };
        let (folders, files, count, bytes, view) = if remote {
            let r = &tab.remote;
            let chosen = r.rows.iter().filter(|x| r.selected.contains(&x.entry.name));
            let (n, b) =
                chosen.fold((0, 0), |(n, b), x| (n + 1, b + if x.dir { 0 } else { x.entry.attrs.size.unwrap_or(0) }));
            let folders = r.rows.iter().filter(|x| x.dir).count();
            (folders, r.rows.len() - folders, n, b, r.view)
        } else {
            let l = &tab.local;
            let chosen = l.rows.iter().filter(|x| l.selected.contains(&local_key(x)));
            let (n, b) = chosen.fold((0, 0), |(n, b), x| (n + 1, b + x.size.unwrap_or(0)));
            let folders = l.rows.iter().filter(|x| x.dir).count();
            (folders, l.rows.len() - folders, n, b, l.view)
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, palette.bar);
        ui.painter().hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, palette.line));
        let mut line = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let small = egui::FontId::proportional(11.0);
        line.label(
            egui::RichText::new(t!("files-status-items", folders = folders, files = files))
                .font(small.clone())
                .color(palette.weak),
        );
        if count > 0 {
            line.label(egui::RichText::new("·").font(small.clone()).color(palette.weak));
            line.label(
                egui::RichText::new(t!("files-status-selected", count = count, size = size_text(bytes)))
                    .font(small)
                    .color(palette.weak),
            );
        }
        let mut asked = None;
        line.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            asked = files_list::view_switch(ui, &palette, view);
        });
        if let (Some(view), Some(tab)) = (asked, self.tabs.get_mut(self.active)) {
            if remote {
                tab.remote.view = view;
            } else {
                tab.local.view = view;
            }
        }
    }
}
