//! Folders kept to go back to (the design's bookmarks): this computer's
//! for every session, a server's for its host, and some for every host.
//! Kept in `state.db`, a line each (name, a tab, the path; a server's path
//! in hex, being bytes). Ctrl+D keeps or lets go the folder shown, Ctrl+B
//! opens the side's menu of them.

use super::*;

const LOCAL_KEY: &str = "files.bookmarks.local";
/// Every host's: `remote_key("*")`.
const EVERY_HOST: &str = "*";

fn remote_key(alias: &str) -> String {
    format!("files.bookmarks.remote:{alias}")
}

/// A folder kept: what it is called, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Bookmark<P> {
    pub name: String,
    pub path: P,
}

/// The ones read so far (each list once, then kept here and written
/// through).
#[derive(Default)]
pub(super) struct Bookmarks {
    local: Option<Vec<Bookmark<PathBuf>>>,
    remote: HashMap<String, Vec<Bookmark<Vec<u8>>>>,
}

fn clean(name: &str) -> String {
    name.replace(['\t', '\n', '\r'], " ")
}

fn parse<P>(text: &str, path: impl Fn(&str) -> Option<P>) -> Vec<Bookmark<P>> {
    text.lines()
        .filter_map(|line| {
            let (name, p) = line.split_once('\t')?;
            Some(Bookmark { name: name.to_string(), path: path(p)? })
        })
        .collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

impl Bookmarks {
    pub fn local(&mut self, core: Option<&native_term_app::Core>) -> &[Bookmark<PathBuf>] {
        self.local.get_or_insert_with(|| {
            let text = core.and_then(|c| c.setting(LOCAL_KEY)).unwrap_or_default();
            parse(&text, |p| Some(PathBuf::from(p)))
        })
    }

    /// A host's own (`alias`), or every host's (`EVERY_HOST`).
    pub fn remote(&mut self, core: Option<&native_term_app::Core>, alias: &str) -> &[Bookmark<Vec<u8>>] {
        self.remote.entry(alias.to_string()).or_insert_with(|| {
            let text = core.and_then(|c| c.setting(&remote_key(alias))).unwrap_or_default();
            parse(&text, unhex)
        })
    }

    /// Keeps `path` (named `name`) on this computer, or lets it go if it is
    /// kept.
    pub fn toggle_local(&mut self, core: Option<&native_term_app::Core>, name: &str, path: &Path) {
        self.local(core);
        let list = self.local.as_mut().expect("read");
        match list.iter().position(|b| b.path == path) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push(Bookmark { name: clean(name), path: path.to_path_buf() }),
        }
        let text: Vec<String> = list.iter().map(|b| format!("{}\t{}", b.name, b.path.display())).collect();
        if let Some(core) = core {
            core.set_setting(LOCAL_KEY, &text.join("\n"));
        }
    }

    /// Lets `path` go wherever it is kept for `alias` (its own or every
    /// host's); else keeps it as the host's own.
    pub fn toggle_remote(&mut self, core: Option<&native_term_app::Core>, alias: &str, name: &str, path: &[u8]) {
        let mut found = false;
        for list in [alias, EVERY_HOST] {
            self.remote(core, list);
            let kept = self.remote.get_mut(list).expect("read");
            if let Some(i) = kept.iter().position(|b| b.path == path) {
                kept.remove(i);
                found = true;
                self.write_remote(core, list);
            }
        }
        if !found {
            self.remote.get_mut(alias).expect("read").push(Bookmark { name: clean(name), path: path.to_vec() });
            self.write_remote(core, alias);
        }
    }

    /// Lets a host's (or every host's) bookmark go.
    pub fn forget_remote(&mut self, core: Option<&native_term_app::Core>, list: &str, path: &[u8]) {
        self.remote(core, list);
        self.remote.get_mut(list).expect("read").retain(|b| b.path != path);
        self.write_remote(core, list);
    }

    /// This computer's, all of them anew (the manager's OK).
    pub fn set_local(&mut self, core: Option<&native_term_app::Core>, list: Vec<Bookmark<PathBuf>>) {
        let text: Vec<String> = list.iter().map(|b| format!("{}\t{}", clean(&b.name), b.path.display())).collect();
        if let Some(core) = core {
            core.set_setting(LOCAL_KEY, &text.join("\n"));
        }
        self.local = Some(list);
    }

    /// A host's (or every host's) all anew.
    pub fn set_remote(&mut self, core: Option<&native_term_app::Core>, list: &str, kept: Vec<Bookmark<Vec<u8>>>) {
        self.remote
            .insert(list.to_string(), kept.into_iter().map(|b| Bookmark { name: clean(&b.name), ..b }).collect());
        self.write_remote(core, list);
    }

    fn write_remote(&self, core: Option<&native_term_app::Core>, list: &str) {
        let Some(kept) = self.remote.get(list) else { return };
        let text: Vec<String> = kept.iter().map(|b| format!("{}\t{}", b.name, hex(&b.path))).collect();
        if let Some(core) = core {
            core.set_setting(&remote_key(list), &text.join("\n"));
        }
    }
}

/// What the menu was asked for.
pub(super) enum Picked {
    /// Keep (or let go) the folder shown.
    Toggle,
    GoLocal(PathBuf),
    GoRemote(Vec<u8>),
    ForgetLocal(PathBuf),
    /// A server's bookmark let go: from which list, which.
    ForgetRemote(String, Vec<u8>),
    /// The manager asked for.
    Manage,
}

/// The manager's rows as they are edited: a name, where, and (a
/// server's) whether every host has it; gone when let go.
pub(super) struct Managing {
    pub tab: u64,
    pub alias: String,
    pub names: Names,
    pub local: Vec<(String, PathBuf, bool)>,
    /// (name, path, every host, gone)
    pub remote: Vec<(String, Vec<u8>, bool, bool)>,
}

/// The popup's id for a side (Ctrl+B opens it).
pub(super) fn menu_id(remote: bool) -> egui::Id {
    egui::Id::new(("files-bookmarks", remote))
}

impl FilesWindow {
    /// Whether the folder a side shows is kept.
    pub(super) fn kept(&mut self, remote: bool) -> bool {
        let core = self.core().cloned();
        let Some(tab) = self.tabs.get(self.side_index(remote)) else { return false };
        if remote {
            let (alias, path) = (tab.spec.alias.clone(), tab.remote.path.clone());
            [alias.as_str(), EVERY_HOST]
                .iter()
                .any(|l| self.bookmarks.remote(core.as_ref(), l).iter().any(|b| b.path == path))
        } else {
            let Some(path) = tab.local.path.clone() else { return false };
            self.bookmarks.local(core.as_ref()).iter().any(|b| b.path == path)
        }
    }

    /// The side's bookmark button and its menu.
    pub(super) fn bookmark_button(&mut self, ui: &mut egui::Ui, remote: bool) -> Option<Picked> {
        let palette = crate::looks::skin(ui.visuals()).palette;
        let look = files_list::Look::of(ui.visuals());
        let kept = self.kept(remote);
        let glyph = if kept { icons::STAR_FILLED } else { icons::BOOKMARK };
        let mut button = native_term_skin::IconButton::new(glyph.to_string(), t!("files-bookmarks-hint")).small();
        if kept {
            button = button.on(true);
        }
        let response = button.show(ui, &palette);
        if kept {
            // the kept star in the folders' colour
            ui.painter().text(
                response.rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(12.0),
                look.folder,
            );
        }
        let core = self.core().cloned();
        let tab = self.tabs.get(self.side_index(remote))?;
        let (alias, label) = (tab.spec.alias.clone(), tab.spec.label.clone());
        let (local_now, remote_now) = (tab.local.path.clone(), tab.remote.path.clone());
        let names = tab.remote.names;
        let mut picked = None;
        let mono = egui::FontId::monospace(10.5);
        egui::Popup::menu(&response).id(menu_id(remote)).show(|ui| {
            ui.set_min_width(300.0);
            let toggle = if kept { t!("files-bookmark-forget") } else { t!("files-bookmark-keep") };
            let toggle_glyph = if kept { icons::CLEAR } else { icons::BOOKMARK };
            let can = remote || local_now.is_some();
            if ui
                .add_enabled(can, egui::Button::new(format!("{toggle_glyph} {toggle}")).shortcut_text("Ctrl+D"))
                .clicked()
            {
                picked = Some(Picked::Toggle);
            }
            let group = |ui: &mut egui::Ui, title: String| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(title).size(11.0).color(palette.weak));
            };
            let item = |ui: &mut egui::Ui, name: &str, path: String, current: bool| -> (bool, bool) {
                let mut forget = false;
                let clicked = ui
                    .horizontal(|ui| {
                        let text = egui::RichText::new(format!("{} {name}", icons::FOLDER)).strong();
                        let go = ui.add(egui::Button::new(text).selected(current).frame_when_inactive(false));
                        ui.label(egui::RichText::new(&path).font(mono.clone()).color(palette.weak));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            forget = ui
                                .small_button(icons::CLEAR.to_string())
                                .on_hover_text(t!("files-bookmark-forget"))
                                .clicked();
                        });
                        go.clicked()
                    })
                    .inner;
                (clicked, forget)
            };
            if remote {
                for (list, title) in
                    [(alias.as_str(), format!("{label} · {alias}")), (EVERY_HOST, t!("files-bookmarks-every-host"))]
                {
                    group(ui, title);
                    let kept = self.bookmarks.remote(core.as_ref(), list).to_vec();
                    if kept.is_empty() {
                        ui.label(egui::RichText::new(t!("files-bookmarks-none")).size(12.0).color(palette.weak));
                    }
                    for b in kept {
                        let (go, forget) = item(ui, &b.name, names.decode(&b.path), b.path == remote_now);
                        if go {
                            picked = Some(Picked::GoRemote(b.path.clone()));
                            ui.close();
                        }
                        if forget {
                            picked = Some(Picked::ForgetRemote(list.to_string(), b.path.clone()));
                        }
                    }
                }
            } else {
                group(ui, t!("files-bookmarks-local"));
                let kept = self.bookmarks.local(core.as_ref()).to_vec();
                if kept.is_empty() {
                    ui.label(egui::RichText::new(t!("files-bookmarks-none")).size(12.0).color(palette.weak));
                }
                for b in kept {
                    let (go, forget) =
                        item(ui, &b.name, b.path.display().to_string(), Some(&b.path) == local_now.as_ref());
                    if go {
                        picked = Some(Picked::GoLocal(b.path.clone()));
                        ui.close();
                    }
                    if forget {
                        picked = Some(Picked::ForgetLocal(b.path.clone()));
                    }
                }
            }
            ui.separator();
            if ui.button(format!("{} {}", icons::SETTINGS, t!("files-bookmarks-manage"))).clicked() {
                picked = Some(Picked::Manage);
                ui.close();
            }
        });
        picked
    }

    /// Opens the bookmarks' manager for the session shown.
    fn manage_bookmarks(&mut self, remote: bool) {
        let core = self.core().cloned();
        let Some(tab) = self.tabs.get(self.side_index(remote)) else { return };
        let (id, alias, names) = (tab.id, tab.spec.alias.clone(), tab.remote.names);
        let local =
            self.bookmarks.local(core.as_ref()).iter().map(|b| (b.name.clone(), b.path.clone(), false)).collect();
        let mut kept: Vec<(String, Vec<u8>, bool, bool)> = self
            .bookmarks
            .remote(core.as_ref(), &alias)
            .iter()
            .map(|b| (b.name.clone(), b.path.clone(), false, false))
            .collect();
        kept.extend(
            self.bookmarks
                .remote(core.as_ref(), EVERY_HOST)
                .iter()
                .map(|b| (b.name.clone(), b.path.clone(), true, false)),
        );
        self.managing = Some(Managing { tab: id, alias, names, local, remote: kept });
    }

    /// The bookmarks' manager (a window of its own, modal): rename, let go,
    /// a server's for this host or every host.
    pub(super) fn bookmarks_dialog(&mut self, ctx: &egui::Context) {
        let Some(m) = &mut self.managing else { return };
        let mut done = None;
        let mono = egui::FontId::monospace(11.0);
        let closed = modal(ctx, "files-bookmarks", t!("files-bookmarks-manage-title"), crate::icons::BOOKMARK, |ui| {
            ui.set_min_width(560.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                ui.label(egui::RichText::new(t!("files-bookmarks-local")).strong());
                if m.local.iter().all(|(_, _, gone)| *gone) {
                    ui.weak(t!("files-bookmarks-none"));
                }
                for (name, path, gone) in m.local.iter_mut().filter(|(_, _, g)| !*g) {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(name).desired_width(160.0));
                        ui.label(egui::RichText::new(path.display().to_string()).font(mono.clone()).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            *gone = ui
                                .small_button(icons::DELETE.to_string())
                                .on_hover_text(t!("files-bookmark-forget"))
                                .clicked();
                        });
                    });
                }
                ui.add_space(10.0);
                ui.label(egui::RichText::new(t!("files-bookmarks-server", host = m.alias.as_str())).strong());
                if m.remote.iter().all(|r| r.3) {
                    ui.weak(t!("files-bookmarks-none"));
                }
                for (name, path, every, gone) in m.remote.iter_mut().filter(|r| !r.3) {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(name).desired_width(160.0));
                        ui.label(egui::RichText::new(m.names.decode(path)).font(mono.clone()).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            *gone = ui
                                .small_button(icons::DELETE.to_string())
                                .on_hover_text(t!("files-bookmark-forget"))
                                .clicked();
                            ui.checkbox(every, t!("files-bookmarks-every-host"));
                        });
                    });
                }
            });
            done = ok_cancel(ui, t!("button-save"), native_term_skin::Role::Primary);
        });
        if closed && done.is_none() {
            done = Some(false);
        }
        let Some(ok) = done else { return };
        ctx.memory_mut(|mem| mem.stop_text_input());
        let Some(m) = self.managing.take() else { return };
        if !ok {
            return;
        }
        let core = self.core().cloned();
        let local = m.local.into_iter().filter(|l| !l.2).map(|(name, path, _)| Bookmark { name, path }).collect();
        self.bookmarks.set_local(core.as_ref(), local);
        let (mut own, mut every) = (Vec::new(), Vec::new());
        for (name, path, all, gone) in m.remote {
            if !gone {
                if all { &mut every } else { &mut own }.push(Bookmark { name, path });
            }
        }
        self.bookmarks.set_remote(core.as_ref(), &m.alias, own);
        self.bookmarks.set_remote(core.as_ref(), EVERY_HOST, every);
        let _ = m.tab;
    }

    /// Carries out what a side's bookmarks were asked for.
    pub(super) fn bookmark_picked(&mut self, remote: bool, picked: Picked) {
        let core = self.core().cloned();
        let Some(tab) = self.tabs.get(self.side_index(remote)) else { return };
        let (id, alias) = (tab.id, tab.spec.alias.clone());
        match picked {
            Picked::Toggle if remote => {
                let path = tab.remote.path.clone();
                let name = tab.remote.names.decode(last(&path));
                let name = if name.is_empty() { "/".to_string() } else { name };
                self.bookmarks.toggle_remote(core.as_ref(), &alias, &name, &path);
            }
            Picked::Toggle => {
                if let Some(path) = tab.local.path.clone() {
                    let name = path
                        .file_name()
                        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    self.bookmarks.toggle_local(core.as_ref(), &name, &path);
                }
            }
            Picked::GoLocal(path) => self.list_local(id, Some(path)),
            Picked::GoRemote(path) => self.go(id, path),
            Picked::ForgetLocal(path) => {
                let name = String::new();
                // (kept: toggling lets it go)
                if self.bookmarks.local(core.as_ref()).iter().any(|b| b.path == path) {
                    self.bookmarks.toggle_local(core.as_ref(), &name, &path);
                }
            }
            Picked::ForgetRemote(list, path) => self.bookmarks.forget_remote(core.as_ref(), &list, &path),
            Picked::Manage => self.manage_bookmarks(remote),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_read_back() {
        let kept = parse("内核\t2f626f6f74\nbad line\nroot\t2f", unhex);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0], Bookmark { name: "内核".into(), path: b"/boot".to_vec() });
        assert_eq!(kept[1].path, b"/".to_vec());
        assert_eq!(hex(b"/boot"), "2f626f6f74");
        assert_eq!(unhex("2f6"), None, "odd length");
        assert_eq!(clean("a\tb\nc"), "a b c");
    }
}
