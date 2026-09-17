//! The session tree panel: search, recent hosts, folders and hosts. Rows
//! are virtualized (only visible rows are laid out), so thousands of
//! hosts cost nothing while scrolling or idle.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;
use native_term_app::fuzzy;
use native_term_app::HostRequest;
use native_term_config::{HostEntry, SessionTree};
use native_term_platform::Target;

/// What the user asked for; carried out by the app.
pub enum TreeAction {
    Open(Vec<HostRequest>, Target),
    NewHost(PathBuf),
    Edit(String),
    Delete(String),
    Move(String, PathBuf),
    NewFolder,
    RenameFolder(PathBuf),
    Reload,
}

#[derive(Default)]
pub struct TreeView {
    pub query: String,
    collapsed: HashSet<PathBuf>,
    selected: Option<String>,
    focus_search: bool,
    /// Search results for (query, tree generation, recent list): scoring
    /// thousands of hosts every frame would be wasted while typing.
    hits: Option<SearchCache>,
}

struct SearchCache {
    query: String,
    generation: u64,
    recent: Vec<String>,
    /// (folder index, host index), best first.
    hits: Vec<(usize, usize)>,
}

enum Row<'a> {
    Heading(&'a str),
    Folder { index: usize, label: String, file: &'a PathBuf, count: usize },
    Host { host: &'a HostEntry, folder: usize, indent: bool },
    Empty(String),
}

/// One full-width, left-aligned, clickable row.
fn draw_row(ui: &mut egui::Ui, height: f32, text: &str, selected: bool, weak: bool, indent: f32) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, text));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() || response.highlighted() {
            ui.painter().rect_filled(rect, visuals.rounding, visuals.weak_bg_fill);
        }
        let color = if weak { ui.visuals().weak_text_color() } else { visuals.text_color() };
        let font = egui::TextStyle::Body.resolve(ui.style());
        let galley = ui.painter().layout(text.to_string(), font, color, f32::INFINITY);
        let pos = egui::pos2(rect.left() + 4.0 + indent, rect.center().y - galley.size().y / 2.0);
        ui.painter().with_clip_rect(rect).galley(pos, galley, color);
    }
    response
}

fn request(host: &HostEntry) -> HostRequest {
    HostRequest { alias: host.alias().to_string(), label: host.label().to_string() }
}

fn folder_title(tree: &SessionTree, index: usize) -> String {
    let folder = tree.folders().nth(index).expect("folder index");
    if folder.name.is_empty() {
        "~/.ssh/config".to_string()
    } else {
        folder.label().to_string()
    }
}

impl TreeView {
    pub fn focus_search(&mut self) {
        self.focus_search = true;
    }

    /// (folder index, host index) of the matches, best first.
    fn search(&mut self, tree: &SessionTree, generation: u64, recent: &[String]) -> Vec<(usize, usize)> {
        let query = self.query.trim().to_string();
        if let Some(c) = &self.hits {
            if c.query == query && c.generation == generation && c.recent == recent {
                return c.hits.clone();
            }
        }
        let mut scored: Vec<(i64, &str, usize, usize)> = Vec::new();
        for (index, folder) in tree.folders().enumerate() {
            for (h, host) in folder.hosts.iter().enumerate() {
                let fields = [
                        host.label(),
                        host.alias(),
                        host.target(),
                        host.user.as_deref().unwrap_or(""),
                        host.nt.get("note").unwrap_or(""),
                        folder.label(),
                    ];
                if let Some(score) = fuzzy::score(&query, &fields) {
                    // recently used hosts rank a little higher
                    let recency = recent.iter().position(|a| a == host.alias()).map_or(0, |p| 20 - p.min(20) as i64);
                    scored.push((score + recency, host.label(), index, h));
                }
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        let hits: Vec<(usize, usize)> = scored.into_iter().map(|(_, _, f, h)| (f, h)).collect();
        self.hits = Some(SearchCache { query, generation, recent: recent.to_vec(), hits: hits.clone() });
        hits
    }

    fn rows<'a>(&mut self, tree: &'a SessionTree, generation: u64, recent: &[String]) -> Vec<Row<'a>> {
        let folders: Vec<_> = tree.folders().collect();
        let mut rows = Vec::new();
        let query = self.query.trim().to_string();
        if !query.is_empty() {
            let hits = self.search(tree, generation, recent);
            if hits.is_empty() {
                rows.push(Row::Empty(format!("No host matches \"{query}\"")));
            }
            rows.extend(hits.into_iter().map(|(f, h)| Row::Host { host: &folders[f].hosts[h], folder: f, indent: false }));
            return rows;
        }
        let recent_hosts: Vec<(usize, &HostEntry)> = recent
            .iter()
            .filter_map(|alias| {
                folders.iter().enumerate().find_map(|(i, f)| f.hosts.iter().find(|h| h.alias() == alias).map(|h| (i, h)))
            })
            .take(5)
            .collect();
        if !recent_hosts.is_empty() {
            rows.push(Row::Heading("Recent"));
            rows.extend(recent_hosts.into_iter().map(|(folder, host)| Row::Host { host, folder, indent: true }));
            rows.push(Row::Heading("All sessions"));
        }
        for (index, folder) in folders.iter().enumerate() {
            if folder.name.is_empty() && folder.hosts.is_empty() {
                continue;
            }
            rows.push(Row::Folder { index, label: folder_title(tree, index), file: &folder.file, count: folder.hosts.len() });
            if !self.collapsed.contains(&folder.file) {
                rows.extend(folder.hosts.iter().map(|host| Row::Host { host, folder: index, indent: true }));
            }
        }
        if tree.hosts().next().is_none() {
            rows.push(Row::Empty("No hosts yet: right-click a folder, or use New folder.".into()));
        }
        rows
    }

    /// `generation` changes whenever `tree` is reloaded.
    pub fn show(&mut self, ui: &mut egui::Ui, tree: &SessionTree, generation: u64, recent: &[String]) -> Vec<TreeAction> {
        let mut actions = Vec::new();
        ui.horizontal(|ui| {
            ui.heading("Sessions");
            if ui.small_button("⟳").on_hover_text("Reload ~/.ssh").clicked() {
                actions.push(TreeAction::Reload);
            }
            if ui.small_button("＋ Folder").clicked() {
                actions.push(TreeAction::NewFolder);
            }
        });
        let mut clear = false;
        let search = ui
            .horizontal(|ui| {
                let width = ui.available_width() - if self.query.is_empty() { 0.0 } else { 28.0 };
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search name, host, user, note…  (Ctrl+F)")
                        .desired_width(width),
                );
                if !self.query.is_empty() && ui.small_button("×").on_hover_text("Clear the search (Esc)").clicked() {
                    clear = true;
                }
                search
            })
            .inner;
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
        }
        // Esc also takes the focus away in the same frame
        if clear || ((search.has_focus() || search.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.query.clear();
        }
        let rows = self.rows(tree, generation, recent);
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(Row::Host { host, .. }) = rows.iter().find(|r| matches!(r, Row::Host { .. })) {
                actions.push(TreeAction::Open(vec![request(host)], Target::Recent));
            }
        }
        ui.separator();

        let folder_files: Vec<(String, PathBuf)> =
            (0..tree.folders().count()).map(|i| (folder_title(tree, i), tree.folders().nth(i).unwrap().file.clone())).collect();
        let row_height = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_height, rows.len(), |ui, range| {
            for row in &rows[range] {
                match row {
                    Row::Heading(text) => {
                        draw_row(ui, row_height, text, false, true, 0.0);
                    }
                    Row::Empty(text) => {
                        draw_row(ui, row_height, text, false, true, 0.0);
                    }
                    Row::Folder { index, label, file, count } => {
                        let open = !self.collapsed.contains(*file);
                        let arrow = if open { "▼" } else { "▶" };
                        let response = draw_row(ui, row_height, &format!("{arrow} {label}  ({count})"), false, false, 0.0);
                        if response.clicked() {
                            if open {
                                self.collapsed.insert((*file).clone());
                            } else {
                                self.collapsed.remove(*file);
                            }
                        }
                        let hosts: Vec<HostRequest> =
                            tree.folders().nth(*index).map(|f| f.hosts.iter().map(request).collect()).unwrap_or_default();
                        let is_main = tree.folders().nth(*index).is_some_and(|f| f.name.is_empty());
                        response.context_menu(|ui| {
                            if ui.add_enabled(!hosts.is_empty(), egui::Button::new("Connect All")).clicked() {
                                actions.push(TreeAction::Open(hosts.clone(), Target::Recent));
                                ui.close_menu();
                            }
                            if ui.add_enabled(!hosts.is_empty(), egui::Button::new("Connect All in New Window")).clicked() {
                                actions.push(TreeAction::Open(hosts.clone(), Target::NewWindow));
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("New Host…").clicked() {
                                actions.push(TreeAction::NewHost((*file).clone()));
                                ui.close_menu();
                            }
                            if ui.add_enabled(!is_main, egui::Button::new("Rename Folder…")).clicked() {
                                actions.push(TreeAction::RenameFolder((*file).clone()));
                                ui.close_menu();
                            }
                        });
                    }
                    Row::Host { host, folder, indent } => {
                        let alias = host.alias();
                        let selected = self.selected.as_deref() == Some(alias);
                        let mut text = host.label().to_string();
                        if !self.query.trim().is_empty() {
                            text.push_str(&format!("   · {}", folder_title(tree, *folder)));
                        }
                        let pad = if *indent { 18.0 } else { 0.0 };
                        let response = draw_row(ui, row_height, &text, selected, false, pad).on_hover_text(hover(host));
                        if response.clicked() {
                            self.selected = Some(alias.to_string());
                        }
                        if response.double_clicked() {
                            actions.push(TreeAction::Open(vec![request(host)], Target::Recent));
                        }
                        response.context_menu(|ui| {
                            if ui.button("Connect").clicked() {
                                actions.push(TreeAction::Open(vec![request(host)], Target::Recent));
                                ui.close_menu();
                            }
                            if ui.button("Connect in New Window").clicked() {
                                actions.push(TreeAction::Open(vec![request(host)], Target::NewWindow));
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Edit…").clicked() {
                                actions.push(TreeAction::Edit(alias.to_string()));
                                ui.close_menu();
                            }
                            ui.menu_button("Move to", |ui| {
                                for (title, file) in &folder_files {
                                    if *file != host.file && ui.button(title).clicked() {
                                        actions.push(TreeAction::Move(alias.to_string(), file.clone()));
                                        ui.close_menu();
                                    }
                                }
                            });
                            if ui.button("Delete…").clicked() {
                                actions.push(TreeAction::Delete(alias.to_string()));
                                ui.close_menu();
                            }
                        });
                    }
                }
            }
        });
        actions
    }
}

fn hover(host: &HostEntry) -> String {
    let mut text = format!(
        "{}{}{}",
        host.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
        host.target(),
        host.port.map(|p| format!(":{p}")).unwrap_or_default()
    );
    if let Some(jump) = &host.proxy_jump {
        text.push_str(&format!("\nvia {jump}"));
    }
    if let Some(note) = host.nt.get("note") {
        text.push_str(&format!("\n{note}"));
    }
    text.push_str(&format!("\nalias {}", host.alias()));
    text
}
