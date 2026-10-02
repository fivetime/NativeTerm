//! The session tree: a search field, the tags, folders and hosts (or the
//! hosts used lately, as a list). Folder labels like `生产 / 控制节点` (an
//! imported SecureCRT tree) are shown as nested folders. Rows are
//! virtualized (only visible rows are laid out), so thousands of hosts
//! cost nothing while scrolling or idle. Hosts can be selected together
//! (their checkboxes, Ctrl+click, Shift+click) and opened or given the
//! key as a group; what is selected is what the properties at the
//! window's right are about (`properties.rs`).
//!
//! The rows are the design's (`layout.rs`): a checkbox, the sign that
//! opens a folder, a picture, the name, and at the row's end its marks
//! and where the host is.

use std::collections::HashMap;
use std::path::PathBuf;

use native_term_app::quick::{self, QuickTarget};
use native_term_app::{fuzzy, t, HostRequest, State};
use native_term_config::plink::Protocol;
use native_term_config::{Folder, HostEntry, SessionTree};
use native_term_platform::Target;

use crate::icons;
use crate::layout;
use crate::looks::{Tint, Tones};

/// What the user asked for; carried out by the app.
pub enum TreeAction {
    Open(Vec<HostRequest>, Target),
    NewHost(PathBuf),
    /// A non-SSH session in this folder file.
    NewPlink(PathBuf),
    Edit(String),
    Options(String),
    Delete(String),
    ForgetKey(String),
    Favorite(String, bool),
    /// (alias, label) of each host.
    InstallKey(Vec<(String, String)>),
    Move(String, PathBuf),
    NewFolder,
    RenameFolder(PathBuf),
    FolderOptions(PathBuf),
    /// The folder's persistent-session default (`tmux`, `screen`, none).
    FolderPersistent(PathBuf, Option<String>),
    /// The folder's credential set (`None`: no set).
    FolderCredential(PathBuf, Option<String>),
    /// The credential sets dialog.
    CredentialSets,
    /// NativeTerm's tmux / screen sessions on this host.
    ServerSessions(String),
    /// The same for the folder's hosts kept on the server: its name, and
    /// those hosts.
    FolderServerSessions(String, Vec<HostRequest>),
    /// The host's files (SFTP).
    Files(String),
    /// Keep the folder out of sends to several sessions (or not).
    FolderNoGroupSend(PathBuf, bool),
    /// The folder's tab color / color scheme default (`None`: none).
    FolderTabColor(PathBuf, Option<String>),
    FolderColorScheme(PathBuf, Option<String>),
    Reload,
    /// A tag called something else, or deleted: everywhere it is.
    RenameTag(String),
    DeleteTag(String),
    /// The rows' checkboxes shown or not, from now on.
    Checks(bool),
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
            State::LoginFailed(_) | State::Unreachable(_) | State::Disconnected(_) | State::Failed(_) => {
                Some(Activity::Failed)
            }
            State::Ended(_) | State::Gone | State::Closed => None,
        }
    }

    /// What it is called, and the colour it is marked with.
    pub fn mark(self, tones: &Tones) -> (String, Tint) {
        match self {
            Activity::Connected => (t!("props-state-connected"), tones.good),
            Activity::Busy => (t!("props-state-busy"), tones.busy),
            Activity::Failed => (t!("props-state-failed"), tones.bad),
        }
    }
}

/// Which hosts are shown: the chips above the tree. They are the tags
/// the hosts were given (`registry::Note`), and before them what is said
/// of a host without being written: a favorite, connected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Favorites,
    /// Logged in, in a session that is open.
    Connected,
    /// Those that have this tag.
    Tag(String),
}

impl Filter {
    fn label(&self) -> String {
        match self {
            Filter::All => t!("filter-all"),
            Filter::Favorites => icons::with(icons::STAR_FILLED, t!("tree-favorites")),
            Filter::Connected => icons::with(icons::CONNECT, t!("props-state-connected")),
            Filter::Tag(tag) => tag.clone(),
        }
    }

    fn takes(&self, host: &HostEntry, activity: &HashMap<String, Activity>, written: Written) -> bool {
        match self {
            Filter::All => true,
            Filter::Favorites => host.favorite(),
            Filter::Connected => activity.get(host.alias()) == Some(&Activity::Connected),
            Filter::Tag(tag) => written.of(host).is_some_and(|note| note.has_tag(tag)),
        }
    }
}

/// What the chips offer: the favorites where there are some, and the
/// tags there are with how many of the tree's hosts have each, those
/// most hosts have first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Chips {
    favorites: bool,
    tags: Vec<(String, usize)>,
}

impl Chips {
    fn of(tree: &SessionTree, written: Written, known: &[String]) -> Chips {
        let mut tags: Vec<(String, usize)> = known.iter().map(|tag| (tag.clone(), 0)).collect();
        let mut favorites = false;
        for (_, host) in tree.hosts() {
            favorites |= host.favorite();
            for tag in written.of(host).map(|note| note.tags.as_slice()).unwrap_or_default() {
                match tags.iter_mut().find(|(known, _)| known.eq_ignore_ascii_case(tag)) {
                    Some((_, count)) => *count += 1,
                    None => tags.push((tag.clone(), 1)),
                }
            }
        }
        tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        Chips { favorites, tags }
    }

    fn filters(&self) -> Vec<Filter> {
        let mut filters = vec![Filter::All];
        if self.favorites {
            filters.push(Filter::Favorites);
        }
        filters.push(Filter::Connected);
        filters.extend(self.tags.iter().map(|(tag, _)| Filter::Tag(tag.clone())));
        filters
    }
}

/// Which of the hosts the view is of: all of them in their folders, or
/// those used lately, the latest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Tree,
    Recent,
}

/// What the view shows, as the app has it on this frame. `generation`
/// changes whenever `tree` is reloaded; `activity` has the state of each
/// alias with open sessions.
#[derive(Clone, Copy)]
pub struct Shown<'a> {
    pub tree: &'a SessionTree,
    pub generation: u64,
    pub recent: &'a [String],
    pub activity: &'a HashMap<String, Activity>,
    pub written: Written<'a>,
    /// The tags there are (the database's), whether a host has them or
    /// not.
    pub tags: &'a [String],
    /// What the hosts' servers said they are.
    pub servers: &'a native_term_app::server::Servers,
    pub scope: Scope,
}

/// What is chosen, for the properties at the window's right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    Nothing,
    /// A host, by its alias.
    Host(String),
    Hosts(Vec<String>),
    /// A folder, by its path in the tree (the main config's is empty).
    Folder(String),
}

/// A folder as the properties show it.
pub struct FolderView {
    pub name: String,
    /// Its file, if it has one (a group of folders has none), and
    /// whether that is the main config.
    pub file: Option<PathBuf>,
    pub main: bool,
    /// The folder of the file, in `SessionTree::folders()`.
    pub index: Option<usize>,
    /// The folders under it, itself not counted.
    pub folders: usize,
    /// The hosts in it and under it.
    pub hosts: Vec<HostRequest>,
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
    /// The chip that is on.
    pub filter: Filter,
    /// Folder paths the user opened or closed (the default depends on how
    /// many folders there are).
    toggled: HashMap<String, bool>,
    /// Selected aliases, in the order they were selected.
    selected: Vec<String>,
    /// Where a Shift+click range starts: the last plain or Ctrl click.
    anchor: Option<String>,
    /// The folder clicked last, by its path, while no host was clicked
    /// since: what the properties are about then.
    focus: Option<String>,
    focus_search: bool,
    /// Search results for (query, tree generation, recent list): scoring
    /// thousands of hosts every frame would be wasted while typing.
    hits: Option<SearchCache>,
    /// The folder hierarchy of a tree generation.
    nodes: (u64, Vec<Node>),
    /// Pinyin forms of labels and folder names, per tree generation.
    pinyin: (u64, HashMap<String, (String, String)>),
    /// What the chips offer, per tree and per notes as they are.
    chips: (Option<(u64, u64)>, Chips),
    /// The chips are all shown, in as many lines as they take (one line
    /// of them otherwise).
    chips_open: bool,
    /// The systems' pictures, as made for the rows so far.
    logos: crate::logos::Logos,
    /// How many folders the tree has (files), as the rows count them for
    /// which are open at first.
    folder_count: usize,
    /// The rows have their checkboxes (off: hosts are chosen by clicks,
    /// Ctrl and Shift with them).
    pub checks: bool,
}

struct SearchCache {
    query: String,
    /// Bumped when a note or its tags change.
    notes: u64,
    generation: u64,
    recent: Vec<String>,
    /// (folder index, host index), best first.
    hits: Vec<(usize, usize)>,
}

enum Row<'a> {
    Quick(QuickTarget),
    Folder { depth: usize, name: String, path: String, folder: Option<usize>, count: usize, open: bool, check: Check },
    Host { host: &'a HostEntry, folder: usize, depth: usize },
    Empty(String),
}

/// A row's checkbox: nothing of what it stands for is selected, some of
/// it, all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Check {
    Off,
    Partly,
    On,
}

impl Check {
    fn of(selected: usize, all: usize) -> Check {
        match selected {
            0 => Check::Off,
            n if n >= all => Check::On,
            _ => Check::Partly,
        }
    }
}

/// A row, as the design has it: around the list `p-3`, in the row
/// `level * 20 + 10` before a checkbox (`w-3.5`), the sign that opens a
/// folder (`w-3.5` with `p-0.5` around it), a picture (`w-4`), the name;
/// `gap-2` between them.
const LIST_PAD: f32 = 12.0;
const ROW_PAD: f32 = 10.0;
const LEVEL: f32 = 20.0;
const CHECK: f32 = 14.0;
const CARET: f32 = 18.0;
const PICTURE: f32 = 16.0;
const GAP: f32 = 8.0;
/// Between two rows (`space-y-0.5`).
const ROW_GAP: f32 = 2.0;
/// The name keeps this much of a row before a mark, and before the
/// host's address, is left out.
const NAME_BEFORE_MARK: f32 = 120.0;
const NAME_BEFORE_ADDRESS: f32 = 200.0;

/// Hosts being dragged in the tree, by alias. A drag of one host that
/// is part of the selection takes the whole selection.
#[derive(Clone, Debug)]
struct Dragged(Vec<String>);

#[derive(Default)]
struct RowLook {
    /// How deep in the tree the row is.
    level: usize,
    /// Its checkbox (a row that is no host and no folder has none).
    check: Option<Check>,
    /// The sign that opens and closes a folder; a host has a dot there.
    chevron: Option<char>,
    picture: Option<(char, Kind)>,
    /// In the picture's place: the system the host's server is of, and
    /// what it is drawn with (`logos.rs`).
    logo: Option<(egui::TextureId, egui::Color32)>,
    /// Said after the name, weakly.
    after: Option<String>,
    /// A favorite.
    star: bool,
    /// The host's tab color, as a bar at the row's start.
    stripe: Option<egui::Color32>,
    /// At the row's end: its marks, and last, in the fixed font, where
    /// the host is.
    marks: Vec<(String, Option<Tint>)>,
    address: Option<String>,
    selected: bool,
    weak: bool,
    /// A host row: it can be dragged into another folder.
    draggable: bool,
    /// A folder row: hosts dropped on it move there.
    accepts_drop: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    Host,
    /// What leads somewhere (a typed target).
    Link,
}

/// A row as it was drawn: what happened to it, and whether a click on it
/// was on its checkbox.
struct Drawn {
    response: egui::Response,
    on_check: bool,
}

fn paint_check(painter: &egui::Painter, tones: &Tones, rect: egui::Rect, check: Check, near: bool) {
    if check == Check::Off {
        painter.rect_filled(rect, 4.0, tones.card);
        let line = if near { tones.near } else { tones.line };
        painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside);
        return;
    }
    painter.rect_filled(rect, 4.0, if near { tones.primary_near } else { tones.primary });
    let sign = egui::Stroke::new(1.6_f32, tones.on_primary);
    let at = |x: f32, y: f32| rect.min + egui::vec2(rect.width() * x, rect.height() * y);
    if check == Check::On {
        painter.line_segment([at(0.22, 0.52), at(0.42, 0.72)], sign);
        painter.line_segment([at(0.42, 0.72), at(0.78, 0.30)], sign);
    } else {
        painter.line_segment([at(0.25, 0.5), at(0.75, 0.5)], sign);
    }
}

fn draw_row(ui: &mut egui::Ui, tones: &Tones, height: f32, text: &str, look: RowLook) -> Drawn {
    let width = ui.available_width();
    // a host can be dragged into another folder; a click is still a click
    // (egui only calls it a drag once the pointer has moved)
    let sense = if look.draggable { egui::Sense::click_and_drag() } else { egui::Sense::click() };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), sense);
    let drop_here =
        look.accepts_drop && response.contains_pointer() && egui::DragAndDrop::has_payload_of_type::<Dragged>(ui.ctx());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, look.selected, text));
    let within = rect.shrink2(egui::vec2(LIST_PAD, 0.0));
    let mut x = within.left() + ROW_PAD + look.level as f32 * LEVEL;
    let check_at = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - CHECK / 2.0), egui::vec2(CHECK, CHECK));
    // (a little around it counts: it is small)
    let on = |at: Option<egui::Pos2>| at.is_some_and(|at| check_at.expand(5.0).contains(at));
    let on_check = look.check.is_some() && response.clicked() && on(response.interact_pointer_pos());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect);
        let middle = rect.center().y;
        let near = response.hovered() || response.highlighted();
        if look.selected {
            painter.rect_filled(within, 8.0, tones.chosen.fill);
            let line = egui::Stroke::new(1.0_f32, tones.chosen.line);
            painter.rect_stroke(within, 8.0, line, egui::StrokeKind::Inside);
        } else if near {
            painter.rect_filled(within, 8.0, tones.raised);
        }
        if drop_here {
            // where the dragged hosts would land
            painter.rect_filled(within, 8.0, layout::thin(tones.accent, 0x40));
            let line = egui::Stroke::new(1.0_f32, tones.accent);
            painter.rect_stroke(within, 8.0, line, egui::StrokeKind::Inside);
        }
        // a line down each level the row is under, from under that
        // level's checkbox (its chevron, where there are none)
        let first = if look.check.is_some() { CHECK } else { CARET };
        for level in 0..look.level {
            let under = (within.left() + ROW_PAD + level as f32 * LEVEL + first / 2.0).round() + 0.5;
            let whole = egui::Rangef::new(rect.top() - ROW_GAP, rect.bottom());
            ui.painter().vline(under, whole, egui::Stroke::new(1.0_f32, tones.guide));
        }
        if let Some(stripe) = look.stripe {
            let bar = egui::Rect::from_min_size(
                egui::pos2(within.left() + 2.0, rect.top() + 5.0),
                egui::vec2(3.0, rect.height() - 10.0),
            );
            painter.rect_filled(bar, 1.5, stripe);
        }
        if let Some(check) = look.check {
            paint_check(&painter, tones, check_at, check, near && on(ui.ctx().pointer_hover_pos()));
            x += CHECK + GAP;
        }
        // a folder's chevron; a host's dot beside the checkboxes (without
        // them, nothing: the picture is where the host begins)
        if look.check.is_some() || look.chevron.is_some() {
            let sign = egui::pos2(x + CARET / 2.0, middle);
            let (glyph, size, color) = match look.chevron {
                Some(chevron) => (chevron, 12.0, if near { tones.text } else { tones.weak }),
                None => ('•', 13.0, tones.weak),
            };
            painter.text(sign, egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(size), color);
            x += CARET + GAP;
        }
        if let Some((glyph, kind)) = look.picture {
            let tint = match kind {
                Kind::Folder => tones.folder,
                Kind::Host => tones.host,
                Kind::Link => tones.accent,
            };
            let at = egui::pos2(x + PICTURE / 2.0, middle);
            match look.logo {
                Some((logo, tint)) => {
                    crate::logos::paint(&painter, logo, crate::logos::place(ui.ctx(), at, PICTURE), tint);
                }
                None => {
                    painter.text(at, egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(PICTURE), tint);
                }
            }
            x += PICTURE + GAP;
        }
        // the row's end, from the right: what does not fit is left out
        let mut end = within.right() - ROW_PAD;
        if let Some(address) = &look.address {
            let galley = painter.layout_no_wrap(address.clone(), egui::FontId::monospace(layout::TINY), tones.weak);
            let start = end - galley.size().x;
            if start - GAP - x >= NAME_BEFORE_ADDRESS {
                painter.galley(egui::pos2(start, middle - galley.size().y / 2.0), galley, tones.weak);
                end = start - GAP;
            }
        }
        for (mark, tint) in look.marks.iter().rev() {
            let size = layout::badge_size(&painter, mark);
            let start = end - size.x;
            if start - GAP - x >= NAME_BEFORE_MARK {
                let at = egui::Rect::from_min_size(egui::pos2(start, middle - size.y / 2.0), size);
                layout::paint_badge(&painter, tones, at, mark, *tint);
                end = start - GAP;
            }
        }
        let color = match (look.weak, look.selected) {
            (true, _) => tones.weak,
            (false, true) => tones.chosen.text,
            (false, false) => tones.text,
        };
        let font = egui::TextStyle::Body.resolve(ui.style());
        let name = layout::elided(&painter, text, font.clone(), color, end - x);
        let name_width = name.size().x;
        painter.galley(egui::pos2(x, middle - name.size().y / 2.0), name, color);
        x += name_width;
        if look.star {
            let star =
                painter.layout_no_wrap(icons::STAR_FILLED.to_string(), egui::FontId::proportional(12.0), tones.folder);
            if x + 6.0 + star.size().x <= end {
                let width = star.size().x;
                painter.galley(egui::pos2(x + 6.0, middle - star.size().y / 2.0), star, tones.folder);
                x += 6.0 + width;
            }
        }
        if let Some(after) = look.after.as_deref().filter(|_| end - x - GAP > 24.0) {
            let after = layout::elided(&painter, after, font, tones.weak, end - x - GAP);
            painter.galley(egui::pos2(x + GAP, middle - after.size().y / 2.0), after, tones.weak);
        }
    }
    Drawn { response, on_check }
}

/// What opening `host` means; the login command falls back to its
/// folder's default.
pub fn request(tree: &SessionTree, host: &HostEntry) -> HostRequest {
    let on_login = tree.find(host.alias()).and_then(|(folder, h)| folder.nt(h, "onlogin")).map(str::to_string);
    HostRequest { on_login, ..HostRequest::new(host.alias(), host.label()) }
}

/// The aliases from the host row with alias `anchor` nearest `to` to the
/// row `to`, in row order, each once (a host can be listed twice: under
/// recent and in its folder). Just `to`'s alias without such a row.
fn span(host_rows: &[(usize, &str)], anchor: &str, to: usize) -> Vec<String> {
    let Some(&(_, target)) = host_rows.iter().find(|(row, _)| *row == to) else { return Vec::new() };
    let start =
        host_rows.iter().filter(|(_, alias)| *alias == anchor).map(|(row, _)| *row).min_by_key(|row| row.abs_diff(to));
    let Some(start) = start else { return vec![target.to_string()] };
    let (low, high) = (start.min(to), start.max(to));
    let mut aliases: Vec<String> = Vec::new();
    for (_, alias) in host_rows.iter().filter(|(row, _)| (low..=high).contains(row)) {
        if !aliases.iter().any(|a| a == alias) {
            aliases.push(alias.to_string());
        }
    }
    aliases
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

    /// What is chosen: the folder clicked last, or the hosts selected
    /// (those that are still in `tree`).
    pub fn chosen(&self, tree: &SessionTree) -> Chosen {
        if let Some(path) = &self.focus {
            return Chosen::Folder(path.clone());
        }
        let mut hosts: Vec<String> = self.selected.iter().filter(|a| tree.find(a).is_some()).cloned().collect();
        match hosts.len() {
            0 => Chosen::Nothing,
            1 => Chosen::Host(hosts.remove(0)),
            _ => Chosen::Hosts(hosts),
        }
    }

    /// How many hosts are selected.
    pub fn selected(&self) -> usize {
        self.selected.len()
    }

    /// Nothing is chosen any more.
    pub fn choose_nothing(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.focus = None;
    }

    /// A view whose rows have their checkboxes, or not.
    pub fn with_checks(checks: bool) -> TreeView {
        TreeView { checks, ..TreeView::default() }
    }

    /// Whether a folder is open (what one button opens or closes all by).
    fn any_open(&self) -> bool {
        fn each(view: &TreeView, nodes: &[Node], depth: usize, count: usize) -> bool {
            nodes
                .iter()
                .any(|node| view.is_open(&node.path, depth, count) || each(view, &node.children, depth + 1, count))
        }
        // (the main configuration's folder, and the others, as the rows have them)
        self.toggled.get("").copied().unwrap_or(true) || each(self, &self.nodes.1, 0, self.folder_count)
    }

    /// Every folder open, or every folder closed.
    pub fn open_all(&mut self, open: bool) {
        fn each(nodes: &[Node], open: bool, toggled: &mut HashMap<String, bool>) {
            for node in nodes {
                toggled.insert(node.path.clone(), open);
                each(&node.children, open, toggled);
            }
        }
        self.toggled.insert(String::new(), open);
        each(&self.nodes.1, open, &mut self.toggled);
    }

    /// The folder at `path` (the main config's is empty), as the
    /// properties show it.
    pub fn folder(&self, tree: &SessionTree, path: &str) -> Option<FolderView> {
        let folders: Vec<&Folder> = tree.folders().collect();
        if path.is_empty() {
            let (index, main) = folders.iter().enumerate().find(|(_, f)| f.name.is_empty())?;
            return Some(FolderView {
                name: folder_title(main),
                file: Some(main.file.clone()),
                main: true,
                index: Some(index),
                folders: 0,
                hosts: main.hosts.iter().map(|h| request(tree, h)).collect(),
            });
        }
        let node = find(&self.nodes.1, path)?;
        let mut under = Vec::new();
        folders_under(node, &mut under);
        let own = node.folder.and_then(|i| folders.get(i));
        Some(FolderView {
            name: node.name.clone(),
            file: own.map(|f| f.file.clone()),
            main: false,
            index: node.folder,
            folders: under.len().saturating_sub(usize::from(node.folder.is_some())),
            hosts: self.hosts_under(tree, path, node.folder),
        })
    }

    fn update_chips(&mut self, shown: &Shown<'_>) {
        let key = (shown.generation, shown.written.generation);
        if self.chips.0 != Some(key) {
            self.chips = (Some(key), Chips::of(shown.tree, shown.written, shown.tags));
        }
        // a filter that is not offered any more is off
        if !self.chips.1.filters().contains(&self.filter) {
            self.filter = Filter::All;
        }
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
        self.folder_count = tree.folders().count();
    }

    fn update_pinyin(&mut self, tree: &SessionTree, generation: u64) {
        if self.pinyin.0 == generation && !self.pinyin.1.is_empty() {
            return;
        }
        let mut map = HashMap::new();
        for folder in tree.folders() {
            for text in std::iter::once(folder.label()).chain(folder.hosts.iter().map(|h| h.label())) {
                if !map.contains_key(text) {
                    if let Some(forms) = native_term_config::alias::pinyin_forms(text) {
                        map.insert(text.to_string(), forms);
                    }
                }
            }
        }
        self.pinyin = (generation, map);
    }

    /// (folder index, host index) of the matches, best first.
    fn search(
        &mut self,
        tree: &SessionTree,
        generation: u64,
        recent: &[String],
        written: Written,
    ) -> Vec<(usize, usize)> {
        let query = self.query.trim().to_string();
        if let Some(c) = &self.hits {
            if c.query == query && c.generation == generation && c.recent == recent && c.notes == written.generation {
                return c.hits.clone();
            }
        }
        self.update_pinyin(tree, generation);
        let pinyin = &self.pinyin.1;
        let forms = |text: &str| pinyin.get(text).map(|(f, i)| (f.as_str(), i.as_str())).unwrap_or(("", ""));
        let mut scored: Vec<(i64, &str, usize, usize)> = Vec::new();
        for (index, folder) in tree.folders().enumerate() {
            let (folder_full, folder_initials) = forms(folder.label());
            for (h, host) in folder.hosts.iter().enumerate() {
                let (full, initials) = forms(host.label());
                let note = written.of(host);
                let tags = note.map(native_term_app::registry::Note::tag_line).unwrap_or_default();
                let fields = [
                    host.label(),
                    host.alias(),
                    host.target(),
                    host.user.as_deref().unwrap_or(""),
                    host.nt.get("note").unwrap_or(""),
                    host.plink.as_ref().and_then(|session| session.note.as_deref()).unwrap_or(""),
                    note.map(|n| n.text.as_str()).unwrap_or(""),
                    &tags,
                    folder.label(),
                    full,
                    initials,
                    folder_full,
                    folder_initials,
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
        self.hits = Some(SearchCache {
            query,
            notes: written.generation,
            generation,
            recent: recent.to_vec(),
            hits: hits.clone(),
        });
        hits
    }

    fn is_open(&self, path: &str, depth: usize, folder_count: usize) -> bool {
        // few folders: everything open; many: only the top level, or nothing
        self.toggled.get(path).copied().unwrap_or(folder_count <= 20 || (depth == 0 && folder_count <= 60))
    }

    fn rows<'a>(&mut self, shown: &Shown<'a>) -> Vec<Row<'a>> {
        let tree = shown.tree;
        let folders: Vec<&Folder> = tree.folders().collect();
        let mut rows = Vec::new();
        let query = self.query.trim().to_string();
        let filter = self.filter.clone();
        let lately = |host: &HostEntry| shown.recent.iter().any(|a| a == host.alias());
        let takes = |host: &HostEntry| filter.takes(host, shown.activity, shown.written);
        if !query.is_empty() {
            let hits = self.search(tree, shown.generation, shown.recent, shown.written);
            let hits: Vec<&HostEntry> = hits
                .into_iter()
                .map(|(f, h)| &folders[f].hosts[h])
                .filter(|host| takes(host) && (shown.scope == Scope::Tree || lately(host)))
                .collect();
            if let Some(target) = quick::parse(&query) {
                rows.push(Row::Quick(target));
            } else if hits.is_empty() {
                rows.push(Row::Empty(t!("tree-no-match", query = query.as_str())));
            }
            let folder_of = |host: &HostEntry| folders.iter().position(|f| f.file == host.file).unwrap_or(0);
            rows.extend(hits.into_iter().map(|host| Row::Host { host, folder: folder_of(host), depth: 0 }));
            return rows;
        }
        if shown.scope == Scope::Recent {
            let hosts = shown.recent.iter().filter_map(|alias| {
                folders
                    .iter()
                    .enumerate()
                    .find_map(|(i, f)| f.hosts.iter().find(|h| h.alias() == alias).map(|h| (i, h)))
            });
            rows.extend(hosts.filter(|(_, h)| takes(h)).map(|(folder, host)| Row::Host { host, folder, depth: 0 }));
            if rows.is_empty() {
                rows.push(Row::Empty(if filter == Filter::All { t!("recent-empty") } else { t!("filter-empty") }));
            }
            return rows;
        }
        let selected: std::collections::HashSet<&str> = self.selected.iter().map(String::as_str).collect();
        let look = Look { filter: &filter, activity: shown.activity, written: shown.written, selected: &selected };
        // the main config's own hosts first, then the folder hierarchy
        if let Some((index, main)) = folders.iter().enumerate().find(|(_, f)| f.name.is_empty()) {
            let hosts: Vec<&HostEntry> = main.hosts.iter().filter(|h| takes(h)).collect();
            if !hosts.is_empty() {
                // the main config is open unless the user closed it
                let open = self.toggled.get("").copied().unwrap_or(true);
                let checked = hosts.iter().filter(|h| selected.contains(h.alias())).count();
                rows.push(Row::Folder {
                    depth: 0,
                    name: folder_title(main),
                    path: String::new(),
                    folder: Some(index),
                    count: hosts.len(),
                    open,
                    check: Check::of(checked, hosts.len()),
                });
                if open {
                    rows.extend(hosts.into_iter().map(|host| Row::Host { host, folder: index, depth: 1 }));
                }
            }
        }
        for node in &self.nodes.1 {
            self.node_rows(node, 0, &folders, &look, &mut rows);
        }
        if tree.hosts().next().is_none() {
            rows.push(Row::Empty(t!("tree-empty")));
        } else if rows.is_empty() {
            rows.push(Row::Empty(t!("filter-empty")));
        }
        rows
    }

    fn node_rows<'a>(&self, node: &Node, depth: usize, folders: &[&'a Folder], look: &Look, rows: &mut Vec<Row<'a>>) {
        let mut under = Vec::new();
        folders_under(node, &mut under);
        let takes = |host: &&HostEntry| look.filter.takes(host, look.activity, look.written);
        let hosts = || under.iter().flat_map(|&i| folders[i].hosts.iter()).filter(takes);
        let count = hosts().count();
        // a folder with nothing the filter passes is not shown
        if count == 0 && *look.filter != Filter::All {
            return;
        }
        let checked =
            if look.selected.is_empty() { 0 } else { hosts().filter(|h| look.selected.contains(h.alias())).count() };
        let open = self.is_open(&node.path, depth, folders.len());
        rows.push(Row::Folder {
            depth,
            name: node.name.clone(),
            path: node.path.clone(),
            folder: node.folder,
            count,
            open,
            check: Check::of(checked, count),
        });
        if !open {
            return;
        }
        for child in &node.children {
            self.node_rows(child, depth + 1, folders, look, rows);
        }
        if let Some(index) = node.folder {
            let hosts = folders[index].hosts.iter().filter(takes);
            rows.extend(hosts.map(|host| Row::Host { host, folder: index, depth: depth + 1 }));
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
        indices
            .into_iter()
            .filter_map(|i| folders.get(i))
            .flat_map(|f| f.hosts.iter().map(|h| request(tree, h)))
            .collect()
    }

    /// The search field and the chips, in a bar of their own above the
    /// rows (`p-4 space-y-3`).
    fn search_bar(
        &mut self,
        ui: &mut egui::Ui,
        tones: &Tones,
        actions: &mut Vec<TreeAction>,
    ) -> (egui::Response, bool) {
        let frame = layout::bar(tones, 16);
        egui::Panel::top("tree-search")
            .frame(frame)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                let search = layout::search_field(
                    ui,
                    tones,
                    &mut self.query,
                    &native_term_app::shortcuts::hint(
                        t!("tree-search-hint"),
                        native_term_app::shortcuts::Command::SearchHosts,
                    ),
                );
                self.chips_row(ui, tones, actions);
                search
            })
            .inner
    }

    /// The two switches at `rect`: the checkboxes, and every folder opened
    /// or closed. The second does what the tree asks for as it is: while
    /// a folder is open it closes them all, else it opens them all.
    fn switches(&mut self, ui: &mut egui::Ui, tones: &Tones, rect: egui::Rect, actions: &mut Vec<TreeAction>) {
        layout::switches_frame(ui.painter(), tones, rect);
        let side = egui::Vec2::splat(layout::SWITCH);
        let first = egui::Rect::from_min_size(rect.left_center() + egui::vec2(2.0, -layout::SWITCH / 2.0), side);
        let second = egui::Rect::from_min_size(
            rect.right_center() - egui::vec2(2.0 + layout::SWITCH, layout::SWITCH / 2.0),
            side,
        );
        let checks = layout::Switch::Checks { on: self.checks };
        let hint = if self.checks { t!("tree-checks-off") } else { t!("tree-checks-on") };
        if layout::switch(ui, tones, first, checks, &hint).clicked() {
            self.checks = !self.checks;
            actions.push(TreeAction::Checks(self.checks));
        }
        let open = self.any_open();
        let (what, hint) = if open {
            (layout::Switch::CloseAll, t!("header-collapse"))
        } else {
            (layout::Switch::OpenAll, t!("header-expand"))
        };
        if layout::switch(ui, tones, second, what, &hint).clicked() {
            self.open_all(!open);
        }
    }

    /// The chips: one line of them, and where they are more than a line
    /// holds what opens the row (two chevrons at its end): then all of
    /// them, in as many lines as they take (`CHIP_LINES` at most: the
    /// others are scrolled to, the rows under them staying in sight).
    /// In the one line the chip that is on is among those shown. A
    /// tag's chip has a menu: the tag called something else, or deleted.
    fn chips_row(&mut self, ui: &mut egui::Ui, tones: &Tones, actions: &mut Vec<TreeAction>) {
        const BETWEEN: f32 = 6.0;
        const CHIP_LINES: usize = 6;
        let filters = self.chips.1.filters();
        let caption = t!("filter-label");
        // (the two switches at the row's end: the chips have what they leave)
        let switches = egui::Rect::from_min_size(
            egui::pos2(ui.max_rect().right() - layout::SWITCHES, ui.cursor().min.y),
            egui::vec2(layout::SWITCHES, layout::CHIP),
        );
        self.switches(ui, tones, switches, actions);
        let full = ui.available_width() - layout::SWITCHES - BETWEEN;
        let widths: Vec<f32> = filters.iter().map(|f| layout::chip_width(ui, &f.label())).collect();
        let before = layout::caption_width(ui, tones, &caption);
        let folds = before + widths.iter().map(|w| BETWEEN + w).sum::<f32>() > full;
        let room = if folds { full - layout::CHIP - BETWEEN } else { full };
        let shown = match (folds, self.chips_open) {
            (true, false) => {
                in_one_line(&widths, filters.iter().position(|f| *f == self.filter), room - before, BETWEEN)
            }
            _ => (0..filters.len()).collect(),
        };
        let open = folds && self.chips_open;
        let mut chosen = None;
        let on = &self.filter;
        let mut chips = |ui: &mut egui::Ui| {
            let lines = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(open);
            ui.allocate_ui_with_layout(egui::vec2(room, layout::CHIP), lines, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(BETWEEN, BETWEEN);
                ui.set_min_height(layout::CHIP);
                layout::caption(ui, tones, &caption);
                for filter in shown.iter().map(|i| &filters[*i]) {
                    let chip = layout::chip(ui, tones, &filter.label(), on == filter);
                    if chip.clicked() {
                        chosen = Some(filter.clone());
                    }
                    let Filter::Tag(tag) = filter else { continue };
                    chip.context_menu(|ui| {
                        if ui.button(icons::with(icons::RENAME, t!("tag-rename"))).clicked() {
                            actions.push(TreeAction::RenameTag(tag.clone()));
                            ui.close();
                        }
                        if ui.button(icons::with(icons::DELETE, t!("tag-delete"))).clicked() {
                            actions.push(TreeAction::DeleteTag(tag.clone()));
                            ui.close();
                        }
                    });
                }
            });
        };
        let first = ui.cursor().min;
        if open && layout::chip_lines(&widths, before, room, BETWEEN) > CHIP_LINES {
            // (as high as that whatever the bar was before: the bar is as
            // high as what is in it, and what scrolls takes what there is)
            let most = CHIP_LINES as f32 * layout::CHIP + (CHIP_LINES - 1) as f32 * BETWEEN;
            ui.allocate_ui(egui::vec2(room, most), |ui| {
                egui::ScrollArea::vertical().id_salt("tree-chips").auto_shrink(false).max_height(most).show(ui, chips);
            });
        } else {
            chips(ui);
        }
        if folds {
            // (at the first line's end, however many lines there are)
            let place =
                egui::Rect::from_min_size(first + egui::vec2(room + BETWEEN, 0.0), egui::Vec2::splat(layout::CHIP));
            let mut end = ui.new_child(egui::UiBuilder::new().max_rect(place));
            let hidden = filters.len() - shown.len();
            let hint = if open { t!("tags-fold") } else { t!("tags-unfold", count = hidden) };
            if layout::fold_button(&mut end, tones, open, &hint).clicked() {
                self.chips_open = !self.chips_open;
            }
        }
        if let Some(filter) = chosen {
            self.filter = filter;
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shown: &Shown<'_>) -> Vec<TreeAction> {
        let tones = crate::looks::tones(ui.visuals());
        let (tree, activity, written) = (shown.tree, shown.activity, shown.written);
        let mut actions = Vec::new();
        // (the pictures made so far: the view's again when the rows are drawn)
        let mut logos = std::mem::take(&mut self.logos);
        self.update_nodes(tree, shown.generation);
        self.update_chips(shown);
        let (search, clear) = self.search_bar(ui, &tones, &mut actions);
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
        }
        // Esc also takes the focus away in the same frame
        if clear || ((search.has_focus() || search.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.query.clear();
        }
        let rows = self.rows(shown);
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            // the best saved host; a typed target only if nothing matches
            let host =
                rows.iter().find_map(|r| if let Row::Host { host, .. } = r { Some(request(tree, host)) } else { None });
            let typed = rows.iter().find_map(|r| if let Row::Quick(q) = r { Some(quick_request(q)) } else { None });
            if let Some(request) = host.or(typed) {
                actions.push(TreeAction::Open(vec![request], Target::Recent));
            }
        }
        // (the list's `p-3`, less what is between two rows)
        ui.add_space(LIST_PAD - ROW_GAP);
        ui.spacing_mut().item_spacing.y = ROW_GAP;

        let folders: Vec<&Folder> = tree.folders().collect();
        let folder_files: Vec<(String, PathBuf)> = folders.iter().map(|f| (folder_title(f), f.file.clone())).collect();
        let row_height = ui.spacing().interact_size.y + 4.0;
        let searching = !self.query.trim().is_empty();
        let listed = searching || shown.scope == Scope::Recent;
        let mut toggle = None;
        // a folder's checkbox: its path, its folder, and what its hosts become
        let mut check_folder: Option<(String, Option<usize>, bool)> = None;
        let host_rows: Vec<(usize, &str)> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| if let Row::Host { host, .. } = r { Some((i, host.alias())) } else { None })
            .collect();
        let modifiers = ui.input(|i| i.modifiers);
        let mut click = None;
        // how many hosts were just picked up (for the label at the pointer)
        let mut dragging: Option<usize> = None;
        // the selected hosts that still exist, in selection order
        let chosen: Vec<&HostEntry> = self.selected.iter().filter_map(|a| tree.find(a).map(|(_, h)| h)).collect();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_height, rows.len(), |ui, range| {
            let first = range.start;
            for (offset, row) in rows[range].iter().enumerate() {
                let index = first + offset;
                match row {
                    Row::Quick(target) => {
                        let look = RowLook { picture: Some((icons::CONNECT, Kind::Link)), ..RowLook::default() };
                        let text = t!("quick-connect", target = target.label());
                        let response = draw_row(ui, &tones, row_height, &text, look).response;
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
                        draw_row(ui, &tones, row_height, text, RowLook { weak: true, ..RowLook::default() });
                    }
                    Row::Folder { depth, name, path, folder, count, open, check } => {
                        let chevron = if *open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
                        let icon = if *open { icons::FOLDER_OPEN } else { icons::FOLDER };
                        // a folder with a file of its own can take hosts;
                        // a grouping node (no file) can't
                        let file = folder.and_then(|i| folders.get(i)).map(|f| f.file.clone());
                        let look = RowLook {
                            level: *depth,
                            check: self.checks.then_some(*check),
                            chevron: Some(chevron),
                            picture: Some((icon, Kind::Folder)),
                            after: Some(count.to_string()),
                            selected: self.focus.as_deref() == Some(path.as_str()),
                            accepts_drop: file.is_some(),
                            ..RowLook::default()
                        };
                        let Drawn { response, on_check } = draw_row(ui, &tones, row_height, name, look);
                        if let (Some(file), Some(dropped)) = (&file, response.dnd_release_payload::<Dragged>()) {
                            // the ones that are somewhere else; a host
                            // dropped on its own folder changes nothing
                            for alias in dropped.0.iter() {
                                if tree.find(alias).is_some_and(|(_, h)| h.file != *file) {
                                    actions.push(TreeAction::Move(alias.clone(), file.clone()));
                                }
                            }
                        }
                        if on_check {
                            // (some of them selected: all of them are)
                            check_folder = Some((path.clone(), *folder, *check != Check::On));
                        } else if response.clicked() {
                            // what the properties are about; a double
                            // click (Explorer's way to open) reports two
                            // clicks: the second would close it again
                            self.focus = Some(path.clone());
                            if !response.double_clicked() {
                                toggle = Some((path.clone(), !*open));
                            }
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
                                actions.push(TreeAction::Open(hosts.clone(), Target::NewWindow));
                                ui.close();
                            }
                            if ui
                                .add_enabled(!hosts.is_empty(), egui::Button::new(t!("menu-install-key-all")))
                                .clicked()
                            {
                                let list = hosts.iter().map(|h| (h.alias.clone(), h.label.clone())).collect();
                                actions.push(TreeAction::InstallKey(list));
                                ui.close();
                            }
                            // its hosts kept on the server (tmux / screen)
                            let kept: Vec<HostRequest> = hosts
                                .iter()
                                .filter(|r| {
                                    tree.find(&r.alias).is_some_and(|(f, h)| {
                                        h.plink.is_none() && native_term_config::persistent::for_host(f, h).is_some()
                                    })
                                })
                                .cloned()
                                .collect();
                            if ui
                                .add_enabled(!kept.is_empty(), egui::Button::new(t!("menu-server-sessions")))
                                .on_hover_text(t!("menu-folder-server-sessions-hint"))
                                .on_disabled_hover_text(t!("menu-folder-server-sessions-none"))
                                .clicked()
                            {
                                actions.push(TreeAction::FolderServerSessions(name.clone(), kept));
                                ui.close();
                            }
                            if let Some((file, is_main)) = &own {
                                ui.separator();
                                if ui.button(t!("menu-new-host")).clicked() {
                                    actions.push(TreeAction::NewHost(file.clone()));
                                    ui.close();
                                }
                                if ui.button(t!("menu-new-plink")).clicked() {
                                    actions.push(TreeAction::NewPlink(file.clone()));
                                    ui.close();
                                }
                                if ui.add_enabled(!is_main, egui::Button::new(t!("menu-rename-folder"))).clicked() {
                                    actions.push(TreeAction::RenameFolder(file.clone()));
                                    ui.close();
                                }
                                if ui.add_enabled(!is_main, egui::Button::new(t!("menu-folder-options"))).clicked() {
                                    actions.push(TreeAction::FolderOptions(file.clone()));
                                    ui.close();
                                }
                                if !is_main {
                                    use native_term_config::persistent;
                                    let current = folder
                                        .and_then(|i| folders.get(i))
                                        .and_then(|f| f.defaults.get(persistent::KEY))
                                        .filter(|v| persistent::parse(v).is_some())
                                        .map(|v| v.to_ascii_lowercase());
                                    ui.menu_button(t!("menu-folder-persistent"), |ui| {
                                        let choices = [
                                            (None, t!("persistent-off")),
                                            (Some("tmux"), "tmux".into()),
                                            (Some(persistent::TMUX_LOG), t!("persistent-tmux-log")),
                                            (Some("screen"), "screen".into()),
                                        ];
                                        for (value, text) in choices {
                                            let value = value.map(str::to_string);
                                            if ui.radio(current == value, text).clicked() {
                                                actions.push(TreeAction::FolderPersistent(file.clone(), value));
                                                ui.close();
                                            }
                                        }
                                    })
                                    .response
                                    .on_hover_text(t!("field-persistent-hint"));
                                    let set = folder
                                        .and_then(|i| folders.get(i))
                                        .and_then(|f| f.defaults.get(native_term_config::password::KEY))
                                        .map(str::to_string);
                                    ui.menu_button(t!("menu-folder-credential"), |ui| {
                                        let sets = crate::credential_sets::names();
                                        for value in std::iter::once(None).chain(sets.into_iter().map(Some)) {
                                            let text = value.clone().unwrap_or_else(|| t!("credential-folder-none"));
                                            if ui.radio(set == value, text).clicked() {
                                                actions.push(TreeAction::FolderCredential(file.clone(), value));
                                                ui.close();
                                            }
                                        }
                                        ui.separator();
                                        if ui.button(t!("cred-sets-button")).clicked() {
                                            actions.push(TreeAction::CredentialSets);
                                            ui.close();
                                        }
                                    })
                                    .response
                                    .on_hover_text(t!("field-credential-hint"));
                                    let defaults = folder.and_then(|i| folders.get(i)).map(|f| &f.defaults);
                                    let color = defaults
                                        .and_then(|d| d.get(native_term_config::appearance::TAB_COLOR))
                                        .map(str::to_string);
                                    ui.menu_button(t!("menu-folder-tab-color"), |ui| {
                                        let presets = native_term_config::appearance::PRESETS
                                            .iter()
                                            .map(|(n, _)| Some(n.to_string()));
                                        for value in std::iter::once(None).chain(presets) {
                                            let text: egui::WidgetText = match &value {
                                                None => t!("look-none").into(),
                                                Some(v) => crate::dialogs::color_text(v).into(),
                                            };
                                            if ui.radio(color == value, text).clicked() {
                                                actions.push(TreeAction::FolderTabColor(file.clone(), value));
                                                ui.close();
                                            }
                                        }
                                    });
                                    let scheme = defaults
                                        .and_then(|d| d.get(native_term_config::appearance::COLOR_SCHEME))
                                        .map(str::to_string);
                                    ui.menu_button(t!("menu-folder-color-scheme"), |ui| {
                                        let names = native_term_config::appearance::SCHEMES
                                            .iter()
                                            .map(|s| Some(s.name.to_string()));
                                        for value in std::iter::once(None).chain(names) {
                                            let text = value.clone().unwrap_or_else(|| t!("look-terminal-default"));
                                            if ui.radio(scheme == value, text).clicked() {
                                                actions.push(TreeAction::FolderColorScheme(file.clone(), value));
                                                ui.close();
                                            }
                                        }
                                    });
                                    let excluded =
                                        folder.and_then(|i| folders.get(i)).is_some_and(|f| f.no_group_send());
                                    let mut on = excluded;
                                    let toggle = ui
                                        .checkbox(&mut on, t!("menu-folder-no-group-send"))
                                        .on_hover_text(t!("menu-folder-no-group-send-hint"));
                                    if toggle.changed() {
                                        actions.push(TreeAction::FolderNoGroupSend(file.clone(), on));
                                        ui.close();
                                    }
                                }
                            }
                        });
                    }
                    Row::Host { host, folder, depth } => {
                        let alias = host.alias();
                        let selected = self.selected.iter().any(|a| a == alias);
                        let plink = host.plink.as_ref();
                        let icon = match plink.map(|p| p.protocol) {
                            None => icons::HOST,
                            Some(Protocol::Serial) => icons::SERIAL,
                            Some(_) => icons::NETWORK,
                        };
                        // (an SSH host whose system is known: the person
                        // said it, or its server did)
                        let named = native_term_config::system::for_host(folders[*folder], host);
                        let os = native_term_app::server::system(named, shown.servers.get(alias))
                            .filter(|_| plink.is_none())
                            .map(|(os, _)| os);
                        let logo = os.and_then(|os| {
                            let picture = logos.picture(ui.ctx(), os, PICTURE)?;
                            Some((picture, crate::logos::tint(os, &tones, ui.visuals().dark_mode)))
                        });
                        let stripe = native_term_config::appearance::for_host(folders[*folder], host)
                            .tab_color
                            .and_then(|hex| egui::Color32::from_hex(&hex).ok());
                        // how its sessions are doing, and the first of
                        // what it was tagged with
                        let mut marks: Vec<(String, Option<Tint>)> = Vec::new();
                        if let Some((text, tint)) = activity.get(alias).map(|a| a.mark(&tones)) {
                            marks.push((text, Some(tint)));
                        }
                        if let Some(tag) = written.of(host).and_then(|note| note.tags.first()) {
                            marks.push((tag.clone(), None));
                        }
                        let address = match plink {
                            Some(session) => session.target(),
                            None => address(host),
                        };
                        let look = RowLook {
                            level: *depth,
                            check: self.checks.then_some(if selected { Check::On } else { Check::Off }),
                            picture: Some((icon, Kind::Host)),
                            logo,
                            // (in a list the host's folder is said too)
                            after: listed.then(|| folder_title(folders[*folder])),
                            star: host.favorite(),
                            stripe,
                            marks,
                            address: Some(address),
                            selected,
                            draggable: true,
                            ..RowLook::default()
                        };
                        // the tooltip's text only while it shows, not for every row on every frame
                        let Drawn { response, on_check } = draw_row(ui, &tones, row_height, host.label(), look);
                        let response = response.on_hover_ui(|ui| {
                            ui.label(hover(folders[*folder], host, written));
                        });
                        if let Some(how) = host_click(self.checks, on_check, response.clicked(), modifiers) {
                            click = Some((index, alias.to_string(), how));
                        }
                        // dragging one of several selected hosts takes them all
                        if response.drag_started() {
                            let dragged = if selected && chosen.len() > 1 {
                                chosen.iter().map(|h| h.alias().to_string()).collect()
                            } else {
                                vec![alias.to_string()]
                            };
                            dragging = Some(dragged.len());
                            egui::DragAndDrop::set_payload(ui.ctx(), Dragged(dragged));
                        }
                        if response.double_clicked() && !modifiers.ctrl && !modifiers.shift {
                            actions.push(TreeAction::Open(vec![request(tree, host)], Target::Recent));
                        }
                        // right-clicking outside the selection selects that host alone
                        if response.secondary_clicked() && !selected {
                            self.selected = vec![alias.to_string()];
                            self.anchor = Some(alias.to_string());
                            self.focus = None;
                        }
                        let group = selected && chosen.len() > 1;
                        response.context_menu(|ui| {
                            if group {
                                let requests: Vec<HostRequest> = chosen.iter().map(|h| request(tree, h)).collect();
                                let count = requests.len();
                                if ui.button(t!("menu-connect-selected", count = count)).clicked() {
                                    actions.push(TreeAction::Open(requests.clone(), Target::Recent));
                                    ui.close();
                                }
                                if ui.button(t!("menu-connect-selected-new-window", count = count)).clicked() {
                                    actions.push(TreeAction::Open(requests, Target::NewWindow));
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button(t!("menu-install-key-selected", count = count)).clicked() {
                                    let list =
                                        chosen.iter().map(|h| (h.alias().to_string(), h.label().to_string())).collect();
                                    actions.push(TreeAction::InstallKey(list));
                                    ui.close();
                                }
                                if ui.button(t!("menu-clear-selection")).clicked() {
                                    self.selected.clear();
                                    ui.close();
                                }
                                return;
                            }
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
                            let ssh = host.plink.is_none();
                            if ssh && ui.button(t!("menu-options")).clicked() {
                                actions.push(TreeAction::Options(alias.to_string()));
                                ui.close();
                            }
                            if ssh
                                && ui
                                    .button(t!("menu-server-sessions"))
                                    .on_hover_text(t!("menu-server-sessions-hint"))
                                    .clicked()
                            {
                                actions.push(TreeAction::ServerSessions(alias.to_string()));
                                ui.close();
                            }
                            if ssh && ui.button(t!("menu-files")).on_hover_text(t!("menu-files-hint")).clicked() {
                                actions.push(TreeAction::Files(alias.to_string()));
                                ui.close();
                            }
                            // the same thing dragging does, for a long list
                            ui.menu_button(t!("menu-move-to"), |ui| {
                                egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                                    for (title, file) in &folder_files {
                                        if *file != host.file && ui.button(title).clicked() {
                                            actions.push(TreeAction::Move(alias.to_string(), file.clone()));
                                            ui.close();
                                        }
                                    }
                                });
                            })
                            .response
                            .on_hover_text(t!("tree-drag-hint"));
                            let (label, on) = if host.favorite() {
                                (t!("menu-unfavorite"), false)
                            } else {
                                (t!("menu-favorite"), true)
                            };
                            if ui.button(label).clicked() {
                                actions.push(TreeAction::Favorite(alias.to_string(), on));
                                ui.close();
                            }
                            if ssh && ui.button(t!("menu-install-key")).clicked() {
                                actions
                                    .push(TreeAction::InstallKey(vec![(alias.to_string(), host.label().to_string())]));
                                ui.close();
                            }
                            if ssh && ui.button(t!("menu-forget-key")).clicked() {
                                actions.push(TreeAction::ForgetKey(alias.to_string()));
                                ui.close();
                            }
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
        if let Some((path, folder, on)) = check_folder {
            // the hosts under it the filter passes: those are what its
            // checkbox stood for
            let filter = self.filter.clone();
            let under: Vec<String> = self
                .hosts_under(tree, &path, folder)
                .into_iter()
                .map(|r| r.alias)
                .filter(|a| tree.find(a).is_some_and(|(_, h)| filter.takes(h, activity, written)))
                .collect();
            self.selected.retain(|a| !under.contains(a));
            if on {
                self.selected.extend(under);
            }
            self.focus = None;
        }
        // what is being carried, next to the pointer
        let carried = dragging.or_else(|| egui::DragAndDrop::payload::<Dragged>(ui.ctx()).map(|d| d.0.len()));
        if let Some(count) = carried {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(at) = ui.ctx().pointer_interact_pos() {
                let text = t!("tree-dragging", count = count);
                let painter = ui
                    .ctx()
                    .layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("nativeterm-tree-drag")));
                let font = egui::TextStyle::Body.resolve(ui.style());
                let color = ui.visuals().strong_text_color();
                let galley = painter.layout_no_wrap(text, font, color);
                let at = at + egui::vec2(14.0, 8.0);
                let around = egui::Rect::from_min_size(at, galley.size()).expand(4.0);
                painter.rect_filled(around, 4.0, ui.visuals().window_fill);
                painter.rect_stroke(around, 4.0, ui.visuals().window_stroke, egui::StrokeKind::Inside);
                painter.galley(at, galley, color);
            }
        }
        if let Some((index, alias, modifiers)) = click {
            self.click(&host_rows, index, alias, modifiers);
            self.focus = None;
        }
        self.logos = logos;
        actions
    }

    /// A left click on a host row: alone, Ctrl toggles, Shift selects the
    /// range from the anchor (Ctrl+Shift adds the range).
    fn click(&mut self, host_rows: &[(usize, &str)], index: usize, alias: String, modifiers: egui::Modifiers) {
        match (modifiers.ctrl, modifiers.shift) {
            (_, true) if self.anchor.is_some() => {
                let range = span(host_rows, self.anchor.as_deref().unwrap_or_default(), index);
                if !modifiers.ctrl {
                    self.selected.clear();
                }
                for a in range {
                    if !self.selected.contains(&a) {
                        self.selected.push(a);
                    }
                }
            }
            (true, _) => {
                match self.selected.iter().position(|a| *a == alias) {
                    Some(at) => {
                        self.selected.remove(at);
                    }
                    None => self.selected.push(alias.clone()),
                }
                self.anchor = Some(alias);
            }
            _ => {
                self.selected = vec![alias.clone()];
                self.anchor = Some(alias);
            }
        }
    }
}

/// What a click on a host's row does, as `TreeView::click` takes it: its
/// checkbox, or the whole row while the checkboxes are shown, is a Ctrl
/// with a click (the host joins what is chosen, or leaves it); Shift
/// still takes a range. Elsewhere a click is what it was.
fn host_click(checks: bool, on_check: bool, clicked: bool, modifiers: egui::Modifiers) -> Option<egui::Modifiers> {
    if !(clicked || on_check) {
        return None;
    }
    Some(if (on_check || checks) && !modifiers.shift { egui::Modifiers { ctrl: true, ..modifiers } } else { modifiers })
}

/// The chips that are shown in one line `room` wide (each as wide as
/// `widths` says, `between` before each), by their places: from the
/// first on as many as fit, the one that is on (`on`) among them, in the
/// place of the last that would have fitted if it would not have.
fn in_one_line(widths: &[f32], on: Option<usize>, room: f32, between: f32) -> Vec<usize> {
    let mut shown = Vec::new();
    let mut used = 0.0;
    for (i, width) in widths.iter().enumerate() {
        if used + between + width > room {
            break;
        }
        used += between + width;
        shown.push(i);
    }
    if let Some(on) = on.filter(|on| !shown.contains(on)) {
        // (the first chip is "All": it stays)
        while shown.len() > 1 && used + between + widths[on] > room {
            used -= between + widths[shown.pop().unwrap_or_default()];
        }
        shown.push(on);
    }
    shown
}

/// What passes the filter, and what is selected, while the rows are made.
struct Look<'a> {
    filter: &'a Filter,
    written: Written<'a>,
    activity: &'a HashMap<String, Activity>,
    selected: &'a std::collections::HashSet<&'a str>,
}

/// Where an SSH host is: the user, the host, the port if it is not 22.
pub fn address(host: &HostEntry) -> String {
    format!(
        "{}{}{}",
        host.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
        host.target(),
        host.port.map(|p| format!(":{p}")).unwrap_or_default()
    )
}

/// What was written about the hosts, by `NativeTermId` (`notes.rs`).
pub type Notes = std::collections::BTreeMap<String, native_term_app::registry::Note>;

/// The notes, with a number that changes whenever one of them does: the
/// search keeps its results until something it looked at changed.
#[derive(Clone, Copy)]
pub struct Written<'a> {
    pub notes: &'a Notes,
    pub generation: u64,
}

impl Written<'_> {
    /// What was written about this host, if anything.
    pub fn of(&self, host: &HostEntry) -> Option<&native_term_app::registry::Note> {
        self.notes.get(host.id()?)
    }
}

fn hover(folder: &Folder, host: &HostEntry, written: Written) -> String {
    if let Some(session) = &host.plink {
        let mut text = format!("{} · {}", crate::plink_dialog::protocol_text(session.protocol), session.target());
        if let Some(charset) = &session.charset {
            text.push_str(&format!(" · {charset}"));
        }
        if let Some(note) = &session.note {
            text.push_str(&format!("\n{note}"));
        }
        text.push_str(&format!("\n{}", t!("host-alias", alias = host.alias())));
        return text;
    }
    let mut text = address(host);
    if let Some(jump) = &host.proxy_jump {
        text.push_str(&format!("\n{}", t!("host-via", jump = jump.as_str())));
    }
    if let Some(note) = host.nt.get("note") {
        text.push_str(&format!("\n{note}"));
    }
    if let Some(note) = written.of(host) {
        if !note.tags.is_empty() {
            text.push_str(&format!("\n{}", t!("host-tags", tags = note.tag_line())));
        }
        if !note.text.trim().is_empty() {
            text.push_str(&format!("\n{}", note.text.trim()));
        }
    }
    let look = native_term_config::appearance::for_host(folder, host);
    if look.tab_color.is_some() || look.color_scheme.is_some() {
        let color = look.tab_color.unwrap_or_else(|| t!("look-none"));
        let scheme = look.color_scheme.unwrap_or_else(|| t!("look-terminal-default"));
        text.push_str(&format!("\n{}", t!("host-look", color = color.as_str(), scheme = scheme.as_str())));
    }
    if let Some(p) = native_term_config::persistent::for_host(folder, host) {
        text.push_str(&format!("\n{}", t!("host-persistent", program = p.name())));
        if native_term_config::persistent::logged_for_host(folder, host) {
            text.push_str(&format!(", {}", t!("host-persistent-log")));
        }
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
    fn pinyin_search() {
        let dir = tempfile::tempdir().unwrap();
        let config = "Host node1\n  HostName 10.0.0.1\n  NativeTermLabel 控制节点\n\nHost web\n  HostName 10.0.0.2\n";
        std::fs::write(dir.path().join("config"), config).unwrap();
        let tree = SessionTree::load_with(dir.path(), dir.path());
        let mut view = TreeView::default();
        let notes = Notes::new();
        let written = Written { notes: &notes, generation: 0 };
        for query in ["kzjd", "kongzhi", "控制", "kz jd"] {
            view.query = query.into();
            let hits = view.search(&tree, 1, &[], written);
            let labels: Vec<&str> =
                hits.iter().map(|(f, h)| tree.folders().nth(*f).unwrap().hosts[*h].label()).collect();
            assert_eq!(labels, ["控制节点"], "{query}");
        }
        view.query = "web".into();
        assert_eq!(view.search(&tree, 1, &[], written).len(), 1);
    }

    #[test]
    fn shift_click_ranges() {
        // rows: 0 heading, 1 recent b, 2 heading, 3 folder, 4 a, 5 b, 6 c, 7 d
        let rows = [(1, "b"), (4, "a"), (5, "b"), (6, "c"), (7, "d")];
        assert_eq!(span(&rows, "a", 6), ["a", "b", "c"]);
        assert_eq!(span(&rows, "d", 4), ["a", "b", "c", "d"], "upwards, in row order");
        // the anchor's nearest row: b under recent (1) is farther from 7 than b at 5
        assert_eq!(span(&rows, "b", 7), ["b", "c", "d"]);
        assert_eq!(span(&rows, "b", 4), ["a", "b"], "b at 5 is nearer to row 4 than recent b at 1");
        assert_eq!(span(&rows, "gone", 6), ["c"]);
        assert!(span(&rows, "a", 3).is_empty(), "not a host row");
    }

    #[test]
    fn clicks_select() {
        let rows = [(0, "a"), (1, "b"), (2, "c"), (3, "d")];
        let none = egui::Modifiers::NONE;
        let ctrl = egui::Modifiers { ctrl: true, ..none };
        let shift = egui::Modifiers { shift: true, ..none };
        let mut view = TreeView::default();
        view.click(&rows, 1, "b".into(), none);
        assert_eq!(view.selected, ["b"]);
        view.click(&rows, 3, "d".into(), shift);
        assert_eq!(view.selected, ["b", "c", "d"]);
        view.click(&rows, 2, "c".into(), ctrl);
        assert_eq!(view.selected, ["b", "d"]);
        view.click(&rows, 0, "a".into(), shift);
        assert_eq!(view.selected, ["a", "b", "c"], "the anchor moved to c");
        view.click(&rows, 3, "d".into(), none);
        assert_eq!(view.selected, ["d"]);
    }

    /// A tree of three folders: SSH hosts, one of them a favorite, a
    /// Telnet session and a serial line.
    fn tree_of_kinds(dir: &std::path::Path) -> SessionTree {
        let main = "Include config.d/*.conf\n\nHost web\n  HostName 10.0.0.2\n  NativeTermFavorite yes\n";
        std::fs::write(dir.join("config"), main).unwrap();
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        let lab = "Host __nativeterm_folder__\n  NativeTermLabel Lab\n\n\
                   Host db1\n  HostName 10.0.1.1\n  NativeTermId id-db1\n\n\
                   Host db2\n  HostName 10.0.1.2\n  NativeTermId id-db2\n  NativeTermNote the replica\n";
        std::fs::write(dir.join("config.d/lab.conf"), lab).unwrap();
        SessionTree::load_with(dir, dir)
    }

    fn names_of(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Folder { name, count, check, .. } => format!("[{name} {count} {check:?}]"),
                Row::Host { host, .. } => host.label().to_string(),
                Row::Quick(_) => "quick".into(),
                Row::Empty(_) => "empty".into(),
            })
            .collect()
    }

    fn noted(tags: &[(&str, &[&str])]) -> Notes {
        let note = |tags: &[&str]| native_term_app::registry::Note {
            text: String::new(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            updated_at: 1,
        };
        tags.iter().map(|(id, tags)| (id.to_string(), note(tags))).collect()
    }

    #[test]
    fn the_filters_and_the_checkboxes() {
        let dir = tempfile::tempdir().unwrap();
        let tree = tree_of_kinds(dir.path());
        let notes = noted(&[("id-db1", &["prod", "ceph"]), ("id-db2", &["Prod"]), ("id-gone", &["old"])]);
        let known: Vec<String> = ["ceph", "old", "prod", "unused"].iter().map(|t| t.to_string()).collect();
        let nothing = HashMap::new();
        let servers = Default::default();
        let recent = vec!["db2".to_string(), "web".to_string()];
        let mut view = TreeView::default();
        view.update_nodes(&tree, 1);
        let shown = |activity, scope| Shown {
            tree: &tree,
            generation: 1,
            recent: &recent,
            activity,
            written: Written { notes: &notes, generation: 0 },
            tags: &known,
            servers: &servers,
            scope,
        };
        view.update_chips(&shown(&nothing, Scope::Tree));
        // the tags there are, those most of the tree's hosts have first;
        // one in other letters is the same one
        let tag = |t: &str| Filter::Tag(t.to_string());
        assert_eq!(
            view.chips.1.filters(),
            [Filter::All, Filter::Favorites, Filter::Connected, tag("prod"), tag("ceph"), tag("old"), tag("unused")]
        );
        assert_eq!(view.chips.1.tags[0], ("prod".to_string(), 2));
        let all = names_of(&view.rows(&shown(&nothing, Scope::Tree)));
        assert_eq!(all, ["[~/.ssh/config 1 Off]", "web", "[Lab 2 Off]", "db1", "db2"]);
        // what is selected shows on its folder
        view.selected = vec!["db1".into()];
        let rows = names_of(&view.rows(&shown(&nothing, Scope::Tree)));
        assert_eq!(rows[2], "[Lab 2 Partly]");
        view.selected.push("db2".into());
        let rows = names_of(&view.rows(&shown(&nothing, Scope::Tree)));
        assert_eq!(rows[2], "[Lab 2 On]");
        assert_eq!(view.chosen(&tree), Chosen::Hosts(vec!["db1".into(), "db2".into()]));
        // a folder with nothing the filter passes is not shown
        view.filter = Filter::Favorites;
        let rows = names_of(&view.rows(&shown(&nothing, Scope::Tree)));
        assert_eq!(rows, ["[~/.ssh/config 1 Off]", "web"]);
        view.filter = tag("ceph");
        assert_eq!(names_of(&view.rows(&shown(&nothing, Scope::Tree))), ["[Lab 1 On]", "db1"]);
        view.filter = tag("PROD");
        assert_eq!(names_of(&view.rows(&shown(&nothing, Scope::Tree))), ["[Lab 2 On]", "db1", "db2"]);
        view.filter = tag("unused");
        assert_eq!(names_of(&view.rows(&shown(&nothing, Scope::Tree))), ["empty"]);
        view.filter = Filter::Connected;
        assert_eq!(names_of(&view.rows(&shown(&nothing, Scope::Tree))), ["empty"]);
        let activity = HashMap::from([("db2".to_string(), Activity::Connected), ("db1".to_string(), Activity::Failed)]);
        assert_eq!(names_of(&view.rows(&shown(&activity, Scope::Tree))), ["[Lab 1 On]", "db2"]);
        // the hosts used lately, the latest first, the filter over them
        view.filter = Filter::All;
        assert_eq!(names_of(&view.rows(&shown(&activity, Scope::Recent))), ["db2", "web"]);
        view.filter = tag("prod");
        assert_eq!(names_of(&view.rows(&shown(&activity, Scope::Recent))), ["db2"]);
        // a tag that is no more is no filter any more
        view.filter = tag("deleted");
        view.update_chips(&shown(&activity, Scope::Tree));
        assert_eq!(view.filter, Filter::All);
    }

    #[test]
    fn a_host_is_found_by_its_tags_and_what_was_written_about_it() {
        let dir = tempfile::tempdir().unwrap();
        let tree = tree_of_kinds(dir.path());
        let mut notes = noted(&[("id-db1", &["存储", "ceph"])]);
        notes.get_mut("id-db1").unwrap().text = "rack 12, the second from the top".into();
        let written = Written { notes: &notes, generation: 0 };
        let mut view = TreeView::default();
        let mut found = |query: &str| {
            view.query = query.into();
            let hits = view.search(&tree, 1, &[], written);
            hits.iter().map(|(f, h)| tree.folders().nth(*f).unwrap().hosts[*h].label().to_string()).collect::<Vec<_>>()
        };
        assert_eq!(found("ceph"), ["db1"], "a tag");
        assert_eq!(found("存储"), ["db1"], "a tag");
        assert_eq!(found("rack 12"), ["db1"], "what was written about it");
        assert_eq!(found("replica"), ["db2"], "its note in the configuration");
        assert!(found("nowhere").is_empty());
    }

    #[test]
    fn one_button_opens_or_closes_every_folder_as_the_tree_is() {
        let dir = tempfile::tempdir().unwrap();
        let tree = tree_of_kinds(dir.path());
        let mut view = TreeView::default();
        view.update_nodes(&tree, 1);
        // few folders: open at first, so the button closes them
        assert!(view.any_open());
        view.open_all(false);
        assert!(!view.any_open());
        // one opened by hand: the button closes again
        view.toggled.insert("Lab".into(), true);
        assert!(view.any_open());
        view.open_all(true);
        assert!(view.any_open() && view.is_open("Lab", 0, 2));
    }

    #[test]
    fn a_checkbox_adds_the_host_and_so_does_its_row_while_they_are_shown() {
        let dir = tempfile::tempdir().unwrap();
        let tree = tree_of_kinds(dir.path());
        let rows = [(0, "web"), (1, "db1"), (2, "db2")];
        let plain = egui::Modifiers::NONE;
        let shift = egui::Modifiers { shift: true, ..plain };
        let mut view = TreeView::default();
        let click = |view: &mut TreeView, index: usize, checks, on_check, modifiers| {
            if let Some(how) = host_click(checks, on_check, true, modifiers) {
                view.click(&rows, index, rows[index].1.to_string(), how);
            }
        };
        // the checkboxes of two hosts, one after the other: both
        click(&mut view, 1, false, true, plain);
        click(&mut view, 2, false, true, plain);
        assert_eq!(view.selected, ["db1", "db2"]);
        // a row's click where the checkboxes are not shown: that one alone
        click(&mut view, 0, false, false, plain);
        assert_eq!(view.selected, ["web"]);
        // while they are shown, the row is its checkbox: in, and out again
        click(&mut view, 1, true, false, plain);
        assert_eq!(view.selected, ["web", "db1"]);
        click(&mut view, 0, true, false, plain);
        assert_eq!(view.selected, ["db1"]);
        // Shift takes a range, as ever: from the row clicked last to this one
        click(&mut view, 2, true, false, shift);
        assert_eq!(view.selected, ["web", "db1", "db2"]);
        assert_eq!(view.chosen(&tree), Chosen::Hosts(vec!["web".into(), "db1".into(), "db2".into()]));
        assert_eq!(host_click(true, false, false, plain), None, "no click, nothing");
    }

    #[test]
    fn the_chips_of_one_line() {
        let widths = [40.0, 60.0, 60.0, 80.0, 50.0];
        // all of them, where there is room
        assert_eq!(in_one_line(&widths, None, 400.0, 6.0), [0, 1, 2, 3, 4]);
        // as many as fit, from the first
        assert_eq!(in_one_line(&widths, None, 180.0, 6.0), [0, 1, 2]);
        assert_eq!(in_one_line(&widths, Some(1), 180.0, 6.0), [0, 1, 2]);
        // the one that is on is among them
        assert_eq!(in_one_line(&widths, Some(4), 180.0, 6.0), [0, 1, 4]);
        assert_eq!(in_one_line(&widths, Some(3), 180.0, 6.0), [0, 3]);
        // "All" stays, whatever the room
        assert_eq!(in_one_line(&widths, Some(3), 60.0, 6.0), [0, 3]);
        // and the lines all of them take
        assert_eq!(layout::chip_lines(&widths, 30.0, 400.0, 6.0), 1);
        assert_eq!(layout::chip_lines(&widths, 30.0, 180.0, 6.0), 3);
        assert_eq!(layout::chip_lines(&widths, 30.0, 60.0, 6.0), 6, "one in each, after what is said before them");
    }

    #[test]
    fn what_is_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let tree = tree_of_kinds(dir.path());
        let mut view = TreeView::default();
        view.update_nodes(&tree, 1);
        assert_eq!(view.chosen(&tree), Chosen::Nothing);
        view.selected = vec!["web".into(), "gone".into()];
        assert_eq!(view.chosen(&tree), Chosen::Host("web".into()), "a host that is no more is not chosen");
        view.focus = Some("Lab".into());
        assert_eq!(view.chosen(&tree), Chosen::Folder("Lab".into()), "the folder clicked last");
        let lab = view.folder(&tree, "Lab").unwrap();
        assert_eq!((lab.name.as_str(), lab.hosts.len(), lab.main, lab.folders), ("Lab", 2, false, 0));
        assert!(lab.file.unwrap().ends_with("lab.conf"));
        let main = view.folder(&tree, "").unwrap();
        assert!(main.main);
        assert_eq!(main.hosts.len(), 1);
        assert!(view.folder(&tree, "Nowhere").is_none());
        // all closed, all open
        view.open_all(false);
        assert!(!view.is_open("Lab", 0, 2) && !view.is_open("", 0, 2));
        view.open_all(true);
        assert!(view.is_open("Lab", 0, 2));
        view.choose_nothing();
        assert_eq!(view.chosen(&tree), Chosen::Nothing);
    }

    #[test]
    fn a_checkbox_says_how_much_is_selected() {
        assert_eq!(Check::of(0, 3), Check::Off);
        assert_eq!(Check::of(2, 3), Check::Partly);
        assert_eq!(Check::of(3, 3), Check::On);
        assert_eq!(Check::of(0, 0), Check::Off, "an empty folder");
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
