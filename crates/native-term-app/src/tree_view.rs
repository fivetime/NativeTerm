//! The session tree panel: search, recent hosts, folders and hosts. Folder
//! labels like `生产 / 控制节点` (an imported SecureCRT tree) are shown as
//! nested folders. Rows are virtualized (only visible rows are laid out),
//! so thousands of hosts cost nothing while scrolling or idle.

use std::collections::HashMap;
use std::path::PathBuf;

use native_term_app::quick::{self, QuickTarget};
use native_term_app::{fuzzy, t, HostRequest, State};
use native_term_config::{Folder, HostEntry, SessionTree};
use native_term_platform::Target;

use crate::icons;

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

/// How a host's sessions are doing, for the dot next to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    Failed,
    Busy,
    Connected,
}

impl Activity {
    pub fn of(state: &State) -> Option<Activity> {
        match state {
            State::Connected => Some(Activity::Connected),
            State::Opening | State::Detached | State::Waiting | State::Connecting => Some(Activity::Busy),
            State::LoginFailed(_) | State::Disconnected(_) | State::Failed(_) => Some(Activity::Failed),
            State::Ended(_) | State::Gone | State::Closed => None,
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Activity::Connected => egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
            Activity::Busy => egui::Color32::from_rgb(0xd0, 0x9a, 0x1a),
            Activity::Failed => egui::Color32::from_rgb(0xd0, 0x3a, 0x3a),
        }
    }
}

/// A node of the folder hierarchy: a folder file, or a group that only
/// exists because deeper folders name it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    /// `生产 / 控制节点`; the key for opening and closing.
    pub path: String,
    /// Index into `SessionTree::folders()` if a file has this label.
    pub folder: Option<usize>,
    pub children: Vec<Node>,
}

pub const SEPARATOR: &str = " / ";

/// Nest folder labels at " / ". `labels` are (folder index, label).
pub fn hierarchy(labels: &[(usize, String)]) -> Vec<Node> {
    let mut roots: Vec<Node> = Vec::new();
    for (index, label) in labels {
        let parts: Vec<&str> = label.split(SEPARATOR).map(str::trim).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let mut level = &mut roots;
        for (depth, part) in parts.iter().enumerate() {
            let at = match level.iter().position(|n| n.name == *part) {
                Some(at) => at,
                None => {
                    let path = parts[..=depth].join(SEPARATOR);
                    level.push(Node { name: part.to_string(), path, folder: None, children: Vec::new() });
                    level.len() - 1
                }
            };
            if depth + 1 == parts.len() && level[at].folder.is_none() {
                level[at].folder = Some(*index);
            }
            level = &mut level[at].children;
        }
    }
    sort(&mut roots);
    roots
}

fn sort(nodes: &mut [Node]) {
    nodes.sort_by_key(|n| n.name.to_lowercase());
    for n in nodes {
        sort(&mut n.children);
    }
}

/// Folder indices under a node, the node's own first.
fn folders_under(node: &Node, out: &mut Vec<usize>) {
    out.extend(node.folder);
    for child in &node.children {
        folders_under(child, out);
    }
}

fn find<'n>(nodes: &'n [Node], path: &str) -> Option<&'n Node> {
    nodes.iter().find_map(|n| if n.path == path { Some(n) } else { find(&n.children, path) })
}

#[derive(Default)]
pub struct TreeView {
    pub query: String,
    /// Folder paths the user opened or closed (the default depends on how
    /// many folders there are).
    toggled: HashMap<String, bool>,
    selected: Option<String>,
    focus_search: bool,
    /// Search results for (query, tree generation, recent list): scoring
    /// thousands of hosts every frame would be wasted while typing.
    hits: Option<SearchCache>,
    /// The folder hierarchy of a tree generation.
    nodes: (u64, Vec<Node>),
}

struct SearchCache {
    query: String,
    generation: u64,
    recent: Vec<String>,
    /// (folder index, host index), best first.
    hits: Vec<(usize, usize)>,
}

enum Row<'a> {
    Heading(String),
    Quick(QuickTarget),
    Folder { depth: usize, name: String, path: String, folder: Option<usize>, count: usize, open: bool },
    Host { host: &'a HostEntry, folder: usize, depth: usize },
    Empty(String),
}

const INDENT: f32 = 16.0;

/// One full-width, left-aligned, clickable row: indentation, an optional
/// leading glyph, the text, and an optional status dot after it.
struct RowLook {
    icon: Option<char>,
    dot: Option<egui::Color32>,
    selected: bool,
    weak: bool,
    indent: f32,
}

fn draw_row(ui: &mut egui::Ui, height: f32, text: &str, look: RowLook) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, look.selected, text));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, look.selected);
        if look.selected || response.hovered() || response.highlighted() {
            ui.painter().rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }
        let weak = ui.visuals().weak_text_color();
        let color = if look.weak { weak } else { visuals.text_color() };
        let font = egui::TextStyle::Body.resolve(ui.style());
        let painter = ui.painter().with_clip_rect(rect);
        let mut x = rect.left() + 6.0 + look.indent;
        if let Some(glyph) = look.icon {
            let galley = painter.layout_no_wrap(glyph.to_string(), font.clone(), weak);
            painter.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, weak);
            x += 22.0;
        }
        let galley = painter.layout_no_wrap(text.to_string(), font, color);
        let text_width = galley.size().x;
        painter.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
        if let Some(dot) = look.dot {
            let center = egui::pos2((x + text_width + 10.0).min(rect.right() - 8.0), rect.center().y);
            painter.circle_filled(center, 4.0, dot);
        }
    }
    response
}

/// What opening `host` means; the login command falls back to its
/// folder's default.
fn request(tree: &SessionTree, host: &HostEntry) -> HostRequest {
    let on_login = tree.find(host.alias()).and_then(|(folder, h)| folder.nt(h, "onlogin")).map(str::to_string);
    HostRequest { on_login, ..HostRequest::new(host.alias(), host.label()) }
}

fn quick_request(target: &QuickTarget) -> HostRequest {
    HostRequest::new(target.destination(), target.label())
}

fn folder_title(folder: &Folder) -> String {
    if folder.name.is_empty() {
        t!("tree-main-config")
    } else {
        folder.label().to_string()
    }
}

impl TreeView {
    pub fn focus_search(&mut self) {
        self.focus_search = true;
    }

    fn update_nodes(&mut self, tree: &SessionTree, generation: u64) {
        if self.nodes.0 == generation && !self.nodes.1.is_empty() {
            return;
        }
        let labels: Vec<(usize, String)> = tree
            .folders()
            .enumerate()
            .filter(|(_, f)| !f.name.is_empty())
            .map(|(i, f)| (i, f.label().to_string()))
            .collect();
        self.nodes = (generation, hierarchy(&labels));
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

    fn is_open(&self, path: &str, depth: usize, folder_count: usize) -> bool {
        // few folders: everything open; many: only the top level, or nothing
        self.toggled.get(path).copied().unwrap_or(folder_count <= 20 || (depth == 0 && folder_count <= 60))
    }

    fn rows<'a>(&mut self, tree: &'a SessionTree, generation: u64, recent: &[String]) -> Vec<Row<'a>> {
        let folders: Vec<&Folder> = tree.folders().collect();
        let mut rows = Vec::new();
        let query = self.query.trim().to_string();
        if !query.is_empty() {
            let hits = self.search(tree, generation, recent);
            if let Some(target) = quick::parse(&query) {
                rows.push(Row::Quick(target));
            } else if hits.is_empty() {
                rows.push(Row::Empty(t!("tree-no-match", query = query.as_str())));
            }
            rows.extend(hits.into_iter().map(|(f, h)| Row::Host { host: &folders[f].hosts[h], folder: f, depth: 0 }));
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
            rows.push(Row::Heading(t!("tree-recent")));
            rows.extend(recent_hosts.into_iter().map(|(folder, host)| Row::Host { host, folder, depth: 1 }));
            rows.push(Row::Heading(t!("tree-all")));
        }
        // the main config's own hosts first, then the folder hierarchy
        if let Some((index, main)) = folders.iter().enumerate().find(|(_, f)| f.name.is_empty()) {
            if !main.hosts.is_empty() {
                // the main config is open unless the user closed it
                let open = self.toggled.get("").copied().unwrap_or(true);
                rows.push(Row::Folder {
                    depth: 0,
                    name: folder_title(main),
                    path: String::new(),
                    folder: Some(index),
                    count: main.hosts.len(),
                    open,
                });
                if open {
                    rows.extend(main.hosts.iter().map(|host| Row::Host { host, folder: index, depth: 1 }));
                }
            }
        }
        for node in &self.nodes.1 {
            self.node_rows(node, 0, &folders, &mut rows);
        }
        if tree.hosts().next().is_none() {
            rows.push(Row::Empty(t!("tree-empty")));
        }
        rows
    }

    fn node_rows<'a>(&self, node: &Node, depth: usize, folders: &[&'a Folder], rows: &mut Vec<Row<'a>>) {
        let mut under = Vec::new();
        folders_under(node, &mut under);
        let count = under.iter().map(|&i| folders[i].hosts.len()).sum();
        let open = self.is_open(&node.path, depth, folders.len());
        rows.push(Row::Folder { depth, name: node.name.clone(), path: node.path.clone(), folder: node.folder, count, open });
        if !open {
            return;
        }
        for child in &node.children {
            self.node_rows(child, depth + 1, folders, rows);
        }
        if let Some(index) = node.folder {
            rows.extend(folders[index].hosts.iter().map(|host| Row::Host { host, folder: index, depth: depth + 1 }));
        }
    }

    /// Hosts in a folder row's folder and every folder below it.
    fn hosts_under(&self, tree: &SessionTree, path: &str, folder: Option<usize>) -> Vec<HostRequest> {
        let mut indices = Vec::new();
        match find(&self.nodes.1, path) {
            Some(node) if !path.is_empty() => folders_under(node, &mut indices),
            _ => indices.extend(folder),
        }
        let folders: Vec<&Folder> = tree.folders().collect();
        indices.into_iter().filter_map(|i| folders.get(i)).flat_map(|f| f.hosts.iter().map(|h| request(tree, h))).collect()
    }

    /// `generation` changes whenever `tree` is reloaded; `activity` has the
    /// state of each alias with open sessions.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tree: &SessionTree,
        generation: u64,
        recent: &[String],
        activity: &HashMap<String, Activity>,
    ) -> Vec<TreeAction> {
        let mut actions = Vec::new();
        ui.horizontal(|ui| {
            ui.heading(t!("tree-heading"));
            if ui.small_button(icons::REFRESH.to_string()).on_hover_text(t!("tree-reload-hint")).clicked() {
                actions.push(TreeAction::Reload);
            }
            if ui.small_button(icons::with(icons::ADD, t!("tree-new-folder"))).clicked() {
                actions.push(TreeAction::NewFolder);
            }
        });
        let mut clear = false;
        let search = ui
            .horizontal(|ui| {
                ui.label(icons::SEARCH.to_string());
                // the clear button and the spacing before it: a field that is
                // too wide makes the resizable panel grow on every frame
                let clear_width = ui.spacing().interact_size.y + ui.spacing().item_spacing.x + 4.0;
                let width = ui.available_width() - if self.query.is_empty() { 0.0 } else { clear_width };
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.query).hint_text(t!("tree-search-hint")).desired_width(width),
                );
                if !self.query.is_empty()
                    && ui.small_button(icons::CLEAR.to_string()).on_hover_text(t!("tree-search-clear")).clicked()
                {
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
        self.update_nodes(tree, generation);
        let rows = self.rows(tree, generation, recent);
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            // the best saved host; a typed target only if nothing matches
            let host = rows.iter().find_map(|r| if let Row::Host { host, .. } = r { Some(request(tree, host)) } else { None });
            let typed = rows.iter().find_map(|r| if let Row::Quick(q) = r { Some(quick_request(q)) } else { None });
            if let Some(request) = host.or(typed) {
                actions.push(TreeAction::Open(vec![request], Target::Recent));
            }
        }
        ui.separator();

        let folders: Vec<&Folder> = tree.folders().collect();
        let folder_files: Vec<(String, PathBuf)> = folders.iter().map(|f| (folder_title(f), f.file.clone())).collect();
        let row_height = ui.spacing().interact_size.y + 4.0;
        let searching = !self.query.trim().is_empty();
        let mut toggle = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_height, rows.len(), |ui, range| {
            for row in &rows[range] {
                match row {
                    Row::Heading(text) => {
                        let look = RowLook { icon: None, dot: None, selected: false, weak: true, indent: 0.0 };
                        draw_row(ui, row_height, text, look);
                    }
                    Row::Quick(target) => {
                        let look = RowLook { icon: Some(icons::CONNECT), dot: None, selected: false, weak: false, indent: 0.0 };
                        let text = t!("quick-connect", target = target.label());
                        let response = draw_row(ui, row_height, &text, look);
                        if response.clicked() {
                            actions.push(TreeAction::Open(vec![quick_request(target)], Target::Recent));
                        }
                        response.context_menu(|ui| {
                            if ui.button(t!("menu-connect-new-window")).clicked() {
                                actions.push(TreeAction::Open(vec![quick_request(target)], Target::NewWindow));
                                ui.close();
                            }
                        });
                    }
                    Row::Empty(text) => {
                        let look = RowLook { icon: None, dot: None, selected: false, weak: true, indent: 0.0 };
                        draw_row(ui, row_height, text, look);
                    }
                    Row::Folder { depth, name, path, folder, count, open } => {
                        let chevron = if *open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
                        let icon = if *open { icons::FOLDER_OPEN } else { icons::FOLDER };
                        let text = format!("{icon}  {name}  ({count})");
                        let look = RowLook {
                            icon: Some(chevron),
                            dot: None,
                            selected: false,
                            weak: false,
                            indent: *depth as f32 * INDENT,
                        };
                        let response = draw_row(ui, row_height, &text, look);
                        if response.clicked() {
                            toggle = Some((path.clone(), !*open));
                        }
                        let own = folder.and_then(|i| folders.get(i)).map(|f| (f.file.clone(), f.name.is_empty()));
                        response.context_menu(|ui| {
                            let hosts = self.hosts_under(tree, path, *folder);
                            if ui.add_enabled(!hosts.is_empty(), egui::Button::new(t!("menu-connect-all"))).clicked() {
                                actions.push(TreeAction::Open(hosts.clone(), Target::Recent));
                                ui.close();
                            }
                            if ui
                                .add_enabled(!hosts.is_empty(), egui::Button::new(t!("menu-connect-all-new-window")))
                                .clicked()
                            {
                                actions.push(TreeAction::Open(hosts, Target::NewWindow));
                                ui.close();
                            }
                            if let Some((file, is_main)) = &own {
                                ui.separator();
                                if ui.button(t!("menu-new-host")).clicked() {
                                    actions.push(TreeAction::NewHost(file.clone()));
                                    ui.close();
                                }
                                if ui.add_enabled(!is_main, egui::Button::new(t!("menu-rename-folder"))).clicked() {
                                    actions.push(TreeAction::RenameFolder(file.clone()));
                                    ui.close();
                                }
                            }
                        });
                    }
                    Row::Host { host, folder, depth } => {
                        let alias = host.alias();
                        let selected = self.selected.as_deref() == Some(alias);
                        let mut text = host.label().to_string();
                        if searching {
                            text.push_str(&format!("   · {}", folder_title(folders[*folder])));
                        }
                        let look = RowLook {
                            icon: Some(icons::HOST),
                            dot: activity.get(alias).map(|a| a.color()),
                            selected,
                            weak: false,
                            indent: *depth as f32 * INDENT,
                        };
                        let response = draw_row(ui, row_height, &text, look).on_hover_text(hover(host));
                        if response.clicked() {
                            self.selected = Some(alias.to_string());
                        }
                        if response.double_clicked() {
                            actions.push(TreeAction::Open(vec![request(tree, host)], Target::Recent));
                        }
                        response.context_menu(|ui| {
                            if ui.button(t!("menu-connect")).clicked() {
                                actions.push(TreeAction::Open(vec![request(tree, host)], Target::Recent));
                                ui.close();
                            }
                            if ui.button(t!("menu-connect-new-window")).clicked() {
                                actions.push(TreeAction::Open(vec![request(tree, host)], Target::NewWindow));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(t!("menu-edit")).clicked() {
                                actions.push(TreeAction::Edit(alias.to_string()));
                                ui.close();
                            }
                            ui.menu_button(t!("menu-move-to"), |ui| {
                                egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                                    for (title, file) in &folder_files {
                                        if *file != host.file && ui.button(title).clicked() {
                                            actions.push(TreeAction::Move(alias.to_string(), file.clone()));
                                            ui.close();
                                        }
                                    }
                                });
                            });
                            if ui.button(t!("menu-delete")).clicked() {
                                actions.push(TreeAction::Delete(alias.to_string()));
                                ui.close();
                            }
                        });
                    }
                }
            }
        });
        if let Some((path, open)) = toggle {
            self.toggled.insert(path, open);
        }
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
        text.push_str(&format!("\n{}", t!("host-via", jump = jump.as_str())));
    }
    if let Some(note) = host.nt.get("note") {
        text.push_str(&format!("\n{note}"));
    }
    text.push_str(&format!("\n{}", t!("host-alias", alias = host.alias())));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(nodes: &[Node]) -> Vec<String> {
        nodes.iter().map(|n| format!("{}{}", n.name, if n.folder.is_some() { "*" } else { "" })).collect()
    }

    #[test]
    fn labels_nest_at_the_separator() {
        let labels: Vec<(usize, String)> = [
            (1, "生产 / 项目2"),
            (2, "生产 / 项目1"),
            (3, "Lab"),
            (4, "生产"),
            (5, "客户A / 北京 / 机房1"),
            (6, "a/b without spaces"),
        ]
        .iter()
        .map(|(i, l)| (*i, l.to_string()))
        .collect();
        let nodes = hierarchy(&labels);
        assert_eq!(names(&nodes), ["a/b without spaces*", "Lab*", "客户A", "生产*"]);
        let prod = find(&nodes, "生产").unwrap();
        assert_eq!(prod.folder, Some(4), "a file with the group's own label");
        assert_eq!(names(&prod.children), ["项目1*", "项目2*"]);
        assert_eq!(prod.children[0].path, "生产 / 项目1");
        assert_eq!(find(&nodes, "客户A / 北京 / 机房1").unwrap().folder, Some(5));
        assert!(find(&nodes, "客户A / 北京").unwrap().folder.is_none(), "a group only");
        let mut under = Vec::new();
        folders_under(prod, &mut under);
        assert_eq!(under, [4, 2, 1]);
    }

    #[test]
    fn activity() {
        assert_eq!(Activity::of(&State::Connected), Some(Activity::Connected));
        assert_eq!(Activity::of(&State::Waiting), Some(Activity::Busy));
        assert_eq!(Activity::of(&State::LoginFailed(255)), Some(Activity::Failed));
        assert_eq!(Activity::of(&State::Closed), None);
        assert!(Activity::Connected > Activity::Failed, "the best state wins for the dot");
    }
}
