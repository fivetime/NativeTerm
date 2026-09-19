//! A host's files over SFTP (`native_term_sftp`), in a window of its own
//! (`window::open`, one per host): browse, upload (button or dropped from
//! Explorer), download, new folder, rename, delete, and edit: a file is
//! downloaded, opened with its program, and uploaded again whenever it is
//! saved. Everything that touches the network or the disk runs on a
//! thread; the window only shows what comes back.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use native_term_app::t;
use native_term_sftp::transfer::{self, Progress};
use native_term_sftp::wire::Attrs;
use native_term_sftp::{Entry, Names, Session};

use crate::icons;

const RED: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);
const ROW: f32 = 22.0;

/// The encodings offered for file names (any `encoding_rs` label can be
/// set in the config).
const ENCODINGS: [&str; 14] = [
    "auto", "UTF-8", "GBK", "gb18030", "Big5", "Shift_JIS", "EUC-JP", "EUC-KR", "windows-1250", "windows-1251", "windows-1252",
    "KOI8-R", "ISO-8859-2", "windows-1256",
];

/// What opening a host's files needs.
pub struct Spec {
    pub alias: String,
    pub label: String,
    pub ssh: PathBuf,
    /// `-F` for a folder other than `~/.ssh`.
    pub config: Option<PathBuf>,
    pub names: Names,
    /// `nativeterm-shim.exe`: ssh's askpass helper.
    pub shim: PathBuf,
}

/// A question from ssh (password, passphrase, new host key, code), for
/// the window to ask.
struct Question {
    prompt: String,
    /// Typed text isn't shown (all but yes/no questions).
    secret: bool,
    reply: Sender<Option<String>>,
}

/// Opens the host's files window (or shows the open one).
pub fn open(spec: Spec) {
    let title = t!("files-title", label = spec.label.as_str());
    let viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([900.0, 620.0])
        .with_min_inner_size([520.0, 320.0])
        .with_drag_and_drop(true);
    let key = format!("files:{}", spec.alias);
    crate::window::open(key, viewport, move |ctx| Box::new(FilesWindow::new(ctx, spec)));
}

/// A directory entry as shown.
#[derive(Clone)]
struct Row {
    entry: Entry,
    name: String,
    /// A folder, or a link to one.
    dir: bool,
}

enum Event {
    Connected(Arc<Session>, Vec<u8>),
    Failed(String),
    Listed { path: Vec<u8>, result: Result<Vec<Row>, String> },
    JobDone { id: u64, result: Result<(), native_term_sftp::Error> },
    /// Files picked for uploading, or a folder to download into.
    PickedUpload(Vec<PathBuf>),
    PickedFolder(PathBuf, Vec<(Vec<u8>, Attrs)>),
    Notice(String, bool),
    Refresh,
    Ask(Question),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Upload,
    Download,
    Delete,
}

enum JobState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

struct Job {
    id: u64,
    kind: Kind,
    title: String,
    progress: Arc<Progress>,
    state: JobState,
    started: Instant,
    /// A download's folder (to show it).
    folder: Option<PathBuf>,
}

/// A file being edited: uploaded again whenever its local copy changes.
struct Edit {
    name: String,
    local: PathBuf,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    /// Set when the file on the server changed meanwhile: an upload waits
    /// for "Overwrite".
    conflict: Arc<AtomicBool>,
    overwrite: Arc<AtomicBool>,
}

struct FilesWindow {
    spec: Spec,
    ctx: egui::Context,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    sftp: Option<Arc<Session>>,
    failed: Option<String>,
    path: Vec<u8>,
    path_text: String,
    rows: Vec<Row>,
    listing: bool,
    list_error: Option<String>,
    selected: HashSet<Vec<u8>>,
    anchor: Option<usize>,
    names: Names,
    jobs: Vec<Job>,
    next_job: u64,
    edits: Vec<Edit>,
    renaming: Option<(Vec<u8>, String)>,
    new_folder: Option<String>,
    confirm_delete: Option<Vec<(Vec<u8>, Attrs)>>,
    confirm_close: bool,
    close: bool,
    notice: Option<(String, bool)>,
    edit_dir: PathBuf,
    /// ssh's question being asked, and the answer being typed.
    question: Option<(Question, String)>,
}

impl FilesWindow {
    fn new(ctx: &egui::Context, spec: Spec) -> FilesWindow {
        let (tx, rx) = mpsc::channel();
        let edit_dir = std::env::temp_dir().join("NativeTerm-edit").join(transfer::local_name(&Names::default(), spec.alias.as_bytes()));
        let names = spec.names;
        let mut window = FilesWindow {
            spec,
            ctx: ctx.clone(),
            tx,
            rx,
            sftp: None,
            failed: None,
            path: Vec::new(),
            path_text: String::new(),
            rows: Vec::new(),
            listing: false,
            list_error: None,
            selected: HashSet::new(),
            anchor: None,
            names,
            jobs: Vec::new(),
            next_job: 1,
            edits: Vec::new(),
            renaming: None,
            new_folder: None,
            confirm_delete: None,
            confirm_close: false,
            close: false,
            notice: None,
            edit_dir,
            question: None,
        };
        window.connect();
        window
    }

    /// Runs `work` on a thread; its event comes back with a repaint.
    fn spawn(&self, work: impl FnOnce() -> Event + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    /// Connects in the background. ssh's questions come here through the
    /// shim as askpass helper and a private pipe: the account's saved
    /// password (Credential Manager) answers its own password prompt, as in
    /// a terminal tab; everything else is asked in this window.
    fn connect(&mut self) {
        self.failed = None;
        self.sftp = None;
        let (ssh, config, alias, shim) = (self.spec.ssh.clone(), self.spec.config.clone(), self.spec.alias.clone(), self.spec.shim.clone());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.spawn(move || {
            let effective = native_term_config::effective::effective_with(&ssh, config.as_deref(), &alias).unwrap_or_default();
            let target = native_term_config::password::target(&effective);
            let saved = target.as_ref().is_some_and(|t| {
                native_term_win::credentials::read(&t.name).ok().flatten().is_some_and(|s| s.comment != native_term_config::password::REFUSED)
            });
            let ssh_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let served = Arc::new(AtomicBool::new(false));
            let askpass = serve_questions(target.clone(), Arc::clone(&ssh_pid), Arc::clone(&served), tx, ctx).ok().filter(|_| shim.exists()).map(|pipe| {
                native_term_sftp::Askpass {
                    program: shim.clone(),
                    // unanswered = cancelled: ssh's console here is hidden
                    env: vec![("NATIVETERM_ASKPASS".to_string(), pipe), ("NATIVETERM_ASKPASS_NO_CONSOLE".to_string(), "1".to_string())],
                    password_prompts: saved.then_some(1),
                }
            });
            let connected = Session::connect(&ssh, config.as_deref(), &alias, askpass.as_ref(), |pid| ssh_pid.store(pid, Ordering::SeqCst));
            match connected.and_then(|sftp| sftp.realpath(b".").map(|home| (sftp, home))) {
                Ok((sftp, home)) => Event::Connected(Arc::new(sftp), home),
                Err(e) => {
                    let text = e.to_string();
                    // the saved password was given and refused: marked, not tried again
                    if served.load(Ordering::SeqCst) && text.contains("Permission denied") {
                        if let Some(t) = &target {
                            if let Ok(Some(mut s)) = native_term_win::credentials::read(&t.name) {
                                s.comment = native_term_config::password::REFUSED.to_string();
                                let _ = native_term_win::credentials::write(&t.name, &s);
                            }
                        }
                    }
                    Event::Failed(text)
                }
            }
        });
    }

    fn list(&mut self, path: Vec<u8>) {
        let Some(sftp) = self.sftp.clone() else { return };
        self.listing = true;
        let names = self.names;
        self.spawn(move || {
            let result = sftp.read_dir(&path).map_err(|e| e.to_string()).map(|entries| {
                let mut rows: Vec<Row> = entries
                    .into_iter()
                    .map(|entry| {
                        // a link to a folder opens like one
                        let dir = entry.attrs.is_dir()
                            || (entry.attrs.is_symlink()
                                && sftp.stat(&native_term_sftp::join(&path, &entry.name)).is_ok_and(|a| a.is_dir()));
                        Row { name: names.decode(&entry.name), dir, entry }
                    })
                    .collect();
                rows.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                rows
            });
            Event::Listed { path, result }
        });
    }

    fn go(&mut self, path: Vec<u8>) {
        self.selected.clear();
        self.anchor = None;
        self.renaming = None;
        self.list(path);
    }

    fn refresh(&mut self) {
        let path = self.path.clone();
        self.list(path);
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Connected(sftp, home) => {
                self.sftp = Some(sftp);
                self.go(home);
            }
            Event::Failed(e) => self.failed = Some(e),
            Event::Listed { path, result } => {
                self.listing = false;
                match result {
                    Ok(rows) => {
                        self.path_text = self.names.decode(&path);
                        self.path = path;
                        self.rows = rows;
                        self.list_error = None;
                        let names: HashSet<&Vec<u8>> = self.rows.iter().map(|r| &r.entry.name).collect();
                        self.selected.retain(|n| names.contains(n));
                    }
                    Err(e) => {
                        // the connection went: say so, offer to reconnect
                        if let Some(why) = self.sftp.as_ref().and_then(|s| s.closed()) {
                            self.failed = Some(why);
                            self.sftp = None;
                        } else {
                            self.list_error = Some(e);
                        }
                    }
                }
            }
            Event::JobDone { id, result } => {
                let mut refresh = false;
                if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                    job.state = match result {
                        Ok(()) => JobState::Done,
                        Err(native_term_sftp::Error::Cancelled) => JobState::Cancelled,
                        Err(e) => JobState::Failed(e.to_string()),
                    };
                    refresh = job.kind != Kind::Download;
                }
                if refresh {
                    self.refresh();
                }
            }
            Event::PickedUpload(files) => self.upload(files),
            Event::PickedFolder(folder, items) => self.download(items, folder),
            Event::Notice(text, error) => self.notice = Some((text, error)),
            Event::Refresh => self.refresh(),
            Event::Ask(question) => {
                if let Some((old, _)) = self.question.take() {
                    let _ = old.reply.send(None);
                }
                self.question = Some((question, String::new()));
            }
        }
    }

    fn selection(&self) -> Vec<(Vec<u8>, Attrs)> {
        self.rows
            .iter()
            .filter(|r| self.selected.contains(&r.entry.name))
            .map(|r| {
                let mut attrs = r.entry.attrs.clone();
                if r.dir && attrs.is_symlink() {
                    // a link to a folder is downloaded as that folder
                    attrs.permissions = Some(0o040755);
                }
                (native_term_sftp::join(&self.path, &r.entry.name), attrs)
            })
            .collect()
    }

    fn add_job(&mut self, kind: Kind, title: String, folder: Option<PathBuf>) -> (u64, Arc<Progress>) {
        let id = self.next_job;
        self.next_job += 1;
        let progress = Arc::new(Progress::default());
        self.jobs.push(Job { id, kind, title, progress: Arc::clone(&progress), state: JobState::Running, started: Instant::now(), folder });
        (id, progress)
    }

    fn upload(&mut self, files: Vec<PathBuf>) {
        let Some(sftp) = self.sftp.clone() else { return };
        if files.is_empty() {
            return;
        }
        let title = describe(&files.iter().map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect::<Vec<_>>());
        let (id, progress) = self.add_job(Kind::Upload, t!("files-job-upload", what = title), None);
        let (names, into) = (self.names, self.path.clone());
        self.spawn(move || {
            let result = transfer::plan_upload(&names, &files, &into, &progress).and_then(|items| transfer::upload(&sftp, &names, &items, &progress));
            Event::JobDone { id, result }
        });
    }

    fn download(&mut self, items: Vec<(Vec<u8>, Attrs)>, folder: PathBuf) {
        let Some(sftp) = self.sftp.clone() else { return };
        if items.is_empty() {
            return;
        }
        let title = describe(&items.iter().map(|(p, _)| self.names.decode(last(p))).collect::<Vec<_>>());
        let (id, progress) = self.add_job(Kind::Download, t!("files-job-download", what = title), Some(folder.clone()));
        let names = self.names;
        self.spawn(move || {
            let mut result = Ok(());
            let mut plan = Vec::new();
            for (path, attrs) in &items {
                match transfer::plan_download(&sftp, &names, path, attrs, &folder, &progress) {
                    Ok(p) => plan.extend(p),
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            let result = result.and_then(|()| transfer::download(&sftp, &names, &plan, &progress));
            Event::JobDone { id, result }
        });
    }

    fn delete(&mut self, items: Vec<(Vec<u8>, Attrs)>) {
        let Some(sftp) = self.sftp.clone() else { return };
        let title = describe(&items.iter().map(|(p, _)| self.names.decode(last(p))).collect::<Vec<_>>());
        let (id, _) = self.add_job(Kind::Delete, t!("files-job-delete", what = title), None);
        self.spawn(move || {
            let result = items.iter().try_for_each(|(path, attrs)| transfer::remove(&sftp, path, attrs));
            Event::JobDone { id, result }
        });
    }

    fn rename(&mut self, old: Vec<u8>, new: String) {
        let Some(sftp) = self.sftp.clone() else { return };
        let Some(new_name) = self.names.encode(new.trim()).filter(|n| !n.is_empty() && !n.contains(&b'/')) else {
            self.notice = Some((t!("files-bad-name", name = new.as_str()), true));
            return;
        };
        let (from, to) = (native_term_sftp::join(&self.path, &old), native_term_sftp::join(&self.path, &new_name));
        self.spawn(move || match sftp.rename(&from, &to, false) {
            Ok(()) => Event::Refresh,
            Err(e) => Event::Notice(e.to_string(), true),
        });
    }

    fn mkdir(&mut self, name: String) {
        let Some(sftp) = self.sftp.clone() else { return };
        let Some(bytes) = self.names.encode(name.trim()).filter(|n| !n.is_empty() && !n.contains(&b'/')) else {
            self.notice = Some((t!("files-bad-name", name = name.as_str()), true));
            return;
        };
        let path = native_term_sftp::join(&self.path, &bytes);
        self.spawn(move || match sftp.mkdir(&path) {
            Ok(()) => Event::Refresh,
            Err(e) => Event::Notice(e.to_string(), true),
        });
    }

    /// Downloads a file, opens it with its program, and uploads it again
    /// whenever it's saved.
    fn edit(&mut self, row: &Row) {
        let Some(sftp) = self.sftp.clone() else { return };
        let remote = native_term_sftp::join(&self.path, &row.entry.name);
        let folder = self.edit_dir.join(format!("{:x}", unique()));
        let local = folder.join(transfer::local_name(&self.names, &row.entry.name));
        let status = Arc::new(Mutex::new(t!("files-edit-opening")));
        let (stop, conflict, overwrite) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        self.edits.push(Edit {
            name: row.name.clone(),
            local: local.clone(),
            status: Arc::clone(&status),
            stop: Arc::clone(&stop),
            conflict: Arc::clone(&conflict),
            overwrite: Arc::clone(&overwrite),
        });
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let set = |text: String| {
                *status.lock().unwrap_or_else(|e| e.into_inner()) = text;
                ctx.request_repaint();
            };
            let started = std::fs::create_dir_all(&folder)
                .map_err(native_term_sftp::Error::from)
                .and_then(|()| sftp.stat(&remote))
                .and_then(|attrs| sftp.download(&remote, &local, &mut |_| true).map(|_| attrs));
            let attrs = match started {
                Ok(a) => a,
                Err(e) => return set(t!("files-edit-failed", error = e.to_string())),
            };
            if let Err(e) = native_term_win::shell::open_file(&local) {
                return set(t!("files-edit-failed", error = e.to_string()));
            }
            set(t!("files-edit-watching"));
            watch_and_upload(&sftp, &remote, &local, attrs, &stop, &conflict, &overwrite, &set);
        });
    }

    /// The selection into the Downloads folder (`NATIVETERM_DOWNLOADS` for
    /// tests).
    fn download_here(&mut self) {
        let folder = std::env::var_os("NATIVETERM_DOWNLOADS").map(PathBuf::from).or_else(native_term_win::shell::downloads_folder);
        match folder {
            Some(folder) => self.download(self.selection(), folder),
            None => self.notice = Some((t!("files-no-downloads"), true)),
        }
    }

    fn pick_upload(&self) {
        let (tx, ctx, title) = (self.tx.clone(), self.ctx.clone(), t!("files-pick-upload"));
        let owner = native_term_win::shell::foreground_window();
        std::thread::spawn(move || {
            if let Some(files) = native_term_win::shell::pick_files(owner, &title) {
                let _ = tx.send(Event::PickedUpload(files));
                ctx.request_repaint();
            }
        });
    }

    fn pick_download(&self, items: Vec<(Vec<u8>, Attrs)>) {
        let (tx, ctx, title) = (self.tx.clone(), self.ctx.clone(), t!("files-pick-folder"));
        let owner = native_term_win::shell::foreground_window();
        std::thread::spawn(move || {
            if let Some(folder) = native_term_win::shell::pick_folder(owner, &title) {
                let _ = tx.send(Event::PickedFolder(folder, items));
                ctx.request_repaint();
            }
        });
    }

    fn running(&self) -> usize {
        self.jobs.iter().filter(|j| matches!(j.state, JobState::Running)).count()
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let connected = self.sftp.is_some();
            let up = ui.add_enabled(connected && self.path != b"/", egui::Button::new(icon(icons::UP))).on_hover_text(t!("files-up"));
            up.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, connected, t!("files-up")));
            if up.clicked() {
                let parent = native_term_sftp::parent(&self.path);
                self.go(parent);
            }
            let refresh = ui.add_enabled(connected, egui::Button::new(icon(icons::REFRESH))).on_hover_text(t!("files-refresh"));
            refresh.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, connected, t!("files-refresh")));
            if refresh.clicked() {
                self.refresh();
            }
            let path = ui.add_enabled(connected, egui::TextEdit::singleline(&mut self.path_text).desired_width((ui.available_width() - 600.0).max(160.0)));
            if path.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                match self.names.encode(self.path_text.trim()) {
                    Some(p) if !p.is_empty() => self.go(p),
                    _ => self.notice = Some((t!("files-bad-name", name = self.path_text.as_str()), true)),
                }
            }
            if ui.add_enabled(connected, egui::Button::new(format!("{} {}", icons::UPLOAD, t!("files-upload")))).clicked() {
                self.pick_upload();
            }
            let download = egui::Button::new(format!("{} {}", icons::DOWNLOAD, t!("files-download")));
            if ui.add_enabled(connected && !self.selected.is_empty(), download).on_hover_text(t!("files-download-hint")).clicked() {
                self.download_here();
            }
            let file = self.rows.iter().find(|r| self.selected.len() == 1 && self.selected.contains(&r.entry.name) && !r.dir).cloned();
            let edit = egui::Button::new(format!("{} {}", icons::EDIT, t!("files-edit")));
            if ui.add_enabled(connected && file.is_some(), edit).on_hover_text(t!("files-edit-hint")).clicked() {
                if let Some(row) = file {
                    self.edit(&row);
                }
            }
            let delete = egui::Button::new(format!("{} {}", icons::DELETE, t!("files-delete")));
            if ui.add_enabled(connected && !self.selected.is_empty(), delete).clicked() {
                self.confirm_delete = Some(self.selection());
            }
            if ui.add_enabled(connected, egui::Button::new(format!("{} {}", icons::NEW_FOLDER, t!("files-new-folder")))).clicked() {
                self.new_folder = Some(String::new());
            }
            let mut names = self.names;
            let shown = if matches!(names, Names::Auto { .. }) { t!("files-encoding-auto") } else { names.label().to_string() };
            egui::ComboBox::from_id_salt("files-encoding").selected_text(shown).width(110.0).show_ui(ui, |ui| {
                for label in ENCODINGS {
                    if let Some(choice) = Names::from_label(label) {
                        let text = if label == "auto" { t!("files-encoding-auto") } else { label.to_string() };
                        ui.selectable_value(&mut names, choice, text);
                    }
                }
            })
            .response
            .on_hover_text(t!("files-encoding-hint"));
            if names != self.names {
                self.names = names;
                for row in &mut self.rows {
                    row.name = names.decode(&row.entry.name);
                }
                self.path_text = names.decode(&self.path);
            }
        });
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        let visuals = ui.visuals().clone();
        let mut open = None;
        let mut menu_action = None;
        let (size_w, date_w, mode_w) = (90.0, 140.0, 116.0);
        // header, placed like the rows' columns
        {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
            let (font, color, y) = (egui::TextStyle::Body.resolve(ui.style()), visuals.weak_text_color(), rect.center().y);
            let name_right = rect.right() - size_w - date_w - mode_w;
            let painter = ui.painter_at(rect);
            painter.text(egui::pos2(rect.left() + 28.0, y), egui::Align2::LEFT_CENTER, t!("files-col-name"), font.clone(), color);
            painter.text(egui::pos2(name_right + size_w - 8.0, y), egui::Align2::RIGHT_CENTER, t!("files-col-size"), font.clone(), color);
            painter.text(egui::pos2(name_right + size_w + 8.0, y), egui::Align2::LEFT_CENTER, t!("files-col-modified"), font.clone(), color);
            painter.text(egui::pos2(name_right + size_w + date_w + 4.0, y), egui::Align2::LEFT_CENTER, t!("files-col-mode"), font, color);
        }
        ui.separator();
        let rows = self.rows.len();
        let modifiers = ui.input(|i| i.modifiers);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, rows, |ui, range| {
            for i in range {
                let row = self.rows[i].clone();
                let selected = self.selected.contains(&row.entry.name);
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
                // for screen readers and UI automation: a selectable item named after the file
                response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &row.name));
                if selected {
                    ui.painter().rect_filled(rect, 3.0, visuals.selection.bg_fill);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
                }
                let text_color = if selected { visuals.selection.stroke.color } else { visuals.text_color() };
                let font = egui::TextStyle::Body.resolve(ui.style());
                let glyph = if row.dir { icons::FOLDER } else if row.entry.attrs.is_symlink() { icons::LINK } else { icons::DOCUMENT };
                let y = rect.center().y;
                let painter = ui.painter_at(rect);
                painter.text(egui::pos2(rect.left() + 6.0, y), egui::Align2::LEFT_CENTER, glyph, font.clone(), text_color);
                let name_right = rect.right() - size_w - date_w - mode_w;
                let name_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 28.0, rect.top()), egui::pos2(name_right - 8.0, rect.bottom()));
                if self.renaming.as_ref().is_some_and(|(n, _)| *n == row.entry.name) {
                    let (_, text) = self.renaming.as_mut().expect("renaming");
                    let edit = ui.put(name_rect, egui::TextEdit::singleline(text));
                    edit.request_focus();
                    if edit.lost_focus() {
                        let (old, text) = self.renaming.take().expect("renaming");
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) && text != row.name {
                            self.rename(old, text);
                        }
                    }
                } else {
                    ui.painter_at(name_rect).text(
                        egui::pos2(name_rect.left(), y),
                        egui::Align2::LEFT_CENTER,
                        &row.name,
                        font.clone(),
                        text_color,
                    );
                }
                let size = if row.dir { String::new() } else { row.entry.attrs.size.map(size_text).unwrap_or_default() };
                painter.text(egui::pos2(name_right + size_w - 8.0, y), egui::Align2::RIGHT_CENTER, size, font.clone(), text_color);
                let date = row.entry.attrs.mtime().map(|m| native_term_win::local_date_time(m as u64)).unwrap_or_default();
                painter.text(egui::pos2(name_right + size_w + 8.0, y), egui::Align2::LEFT_CENTER, date, font.clone(), text_color);
                let mode = row.entry.attrs.permissions.map(mode_text).unwrap_or_default();
                let mono = egui::TextStyle::Monospace.resolve(ui.style());
                painter.text(egui::pos2(name_right + size_w + date_w + 4.0, y), egui::Align2::LEFT_CENTER, mode, mono, text_color);

                if response.clicked() {
                    self.click(i, modifiers);
                }
                if response.double_clicked() {
                    open = Some(row.clone());
                }
                if response.secondary_clicked() && !selected {
                    self.selected = std::iter::once(row.entry.name.clone()).collect();
                    self.anchor = Some(i);
                }
                response.context_menu(|ui| {
                    let many = self.selected.len() > 1;
                    if ui.button(format!("{} {}", icons::DOWNLOAD, t!("files-download"))).clicked() {
                        menu_action = Some(MenuAction::Download);
                        ui.close();
                    }
                    if ui.button(t!("files-download-to")).clicked() {
                        menu_action = Some(MenuAction::DownloadTo);
                        ui.close();
                    }
                    if !row.dir && !many && ui.button(format!("{} {}", icons::EDIT, t!("files-edit"))).clicked() {
                        menu_action = Some(MenuAction::Edit(row.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if !many && ui.button(format!("{} {}", icons::RENAME, t!("files-rename"))).clicked() {
                        menu_action = Some(MenuAction::Rename(row.clone()));
                        ui.close();
                    }
                    if ui.button(t!("files-copy-path")).clicked() {
                        let paths: Vec<String> = self.selection().iter().map(|(p, _)| self.names.decode(p)).collect();
                        ui.ctx().copy_text(paths.join("\n"));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(egui::RichText::new(format!("{} {}", icons::DELETE, t!("files-delete"))).color(RED)).clicked() {
                        menu_action = Some(MenuAction::Delete);
                        ui.close();
                    }
                });
            }
        });
        if let Some(row) = open {
            if row.dir {
                self.go(native_term_sftp::join(&self.path, &row.entry.name));
            } else {
                self.edit(&row);
            }
        }
        match menu_action {
            Some(MenuAction::Download) => self.download_here(),
            Some(MenuAction::DownloadTo) => self.pick_download(self.selection()),
            Some(MenuAction::Edit(row)) => self.edit(&row),
            Some(MenuAction::Rename(row)) => self.renaming = Some((row.entry.name.clone(), row.name.clone())),
            Some(MenuAction::Delete) => self.confirm_delete = Some(self.selection()),
            None => {}
        }
    }

    fn click(&mut self, i: usize, modifiers: egui::Modifiers) {
        let name = self.rows[i].entry.name.clone();
        if modifiers.shift {
            let from = self.anchor.unwrap_or(i);
            let (a, b) = (from.min(i), from.max(i));
            if !modifiers.command {
                self.selected.clear();
            }
            self.selected.extend(self.rows[a..=b].iter().map(|r| r.entry.name.clone()));
        } else if modifiers.command {
            if !self.selected.remove(&name) {
                self.selected.insert(name);
            }
            self.anchor = Some(i);
        } else {
            self.selected = std::iter::once(name).collect();
            self.anchor = Some(i);
        }
    }

    /// Transfers and edited files, at the bottom.
    fn activity(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        for job in &self.jobs {
            ui.horizontal(|ui| {
                let done = job.progress.done.load(Ordering::Relaxed);
                let total = job.progress.total.load(Ordering::Relaxed);
                match &job.state {
                    JobState::Running => {
                        let secs = job.started.elapsed().as_secs_f64().max(0.001);
                        let speed = format!("{}/s", size_text((done as f64 / secs) as u64));
                        let fraction = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        let text = if job.kind == Kind::Delete { job.title.clone() } else { format!("{}  {} / {}  {speed}", job.title, size_text(done), size_text(total)) };
                        ui.add(egui::ProgressBar::new(fraction).desired_width(ui.available_width() - 90.0).text(text));
                        if ui.small_button(t!("button-cancel")).clicked() {
                            job.progress.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    JobState::Done => {
                        ui.colored_label(GREEN, format!("{} {}", icons::ACCEPT, job.title));
                        if let Some(folder) = &job.folder {
                            if ui.small_button(t!("files-open-folder")).clicked() {
                                let _ = std::process::Command::new("explorer.exe").arg(folder).spawn();
                            }
                        }
                        if ui.small_button(icon(icons::CLEAR)).clicked() {
                            remove = Some(job.id);
                        }
                    }
                    JobState::Failed(e) => {
                        ui.colored_label(RED, format!("{}: {e}", job.title));
                        if ui.small_button(icon(icons::CLEAR)).clicked() {
                            remove = Some(job.id);
                        }
                    }
                    JobState::Cancelled => {
                        ui.weak(t!("files-job-cancelled", what = job.title.as_str()));
                        if ui.small_button(icon(icons::CLEAR)).clicked() {
                            remove = Some(job.id);
                        }
                    }
                }
            });
        }
        if let Some(id) = remove {
            self.jobs.retain(|j| j.id != id);
        }
        let mut stop = None;
        for (i, edit) in self.edits.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{} {}", icons::EDIT, edit.name));
                ui.weak(edit.status.lock().unwrap_or_else(|e| e.into_inner()).as_str());
                if edit.conflict.load(Ordering::Relaxed) && ui.small_button(egui::RichText::new(t!("files-edit-overwrite")).color(RED)).clicked() {
                    edit.overwrite.store(true, Ordering::Relaxed);
                }
                if ui.small_button(t!("files-edit-stop")).on_hover_text(edit.local.display().to_string()).clicked() {
                    stop = Some(i);
                }
            });
        }
        if let Some(i) = stop {
            let edit = self.edits.remove(i);
            edit.stop.store(true, Ordering::Relaxed);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some((question, typed)) = &mut self.question {
            let mut done = None;
            modal(ctx, "files-question", t!("files-question-title", host = self.spec.alias.as_str()), |ui| {
                ui.label(question.prompt.trim());
                let edit = ui.add(egui::TextEdit::singleline(typed).password(question.secret).desired_width(320.0));
                edit.request_focus();
                // no IME in a password field (it would compose the typing)
                if question.secret {
                    crate::dialogs::no_ime(&edit);
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("button-ok")).clicked() || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        done = Some(true);
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(ok) = done {
                if let Some((question, typed)) = self.question.take() {
                    let _ = question.reply.send(ok.then_some(typed));
                }
            }
        }
        if let Some(name) = &mut self.new_folder {
            let mut done = None;
            modal(ctx, "files-new-folder", t!("files-new-folder"), |ui| {
                let edit = ui.add(egui::TextEdit::singleline(name).hint_text(t!("files-new-folder-hint")));
                edit.request_focus();
                ui.horizontal(|ui| {
                    if ui.button(t!("button-create")).clicked() || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        done = Some(true);
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        done = Some(false);
                    }
                });
            });
            match done {
                Some(true) => {
                    let name = self.new_folder.take().unwrap_or_default();
                    if !name.trim().is_empty() {
                        self.mkdir(name);
                    }
                }
                Some(false) => self.new_folder = None,
                None => {}
            }
        }
        if let Some(items) = &self.confirm_delete {
            let what = describe(&items.iter().map(|(p, _)| self.names.decode(last(p))).collect::<Vec<_>>());
            let folders = items.iter().any(|(_, a)| a.is_dir() && !a.is_symlink());
            let mut done = None;
            modal(ctx, "files-delete", t!("files-delete"), |ui| {
                ui.label(t!("files-delete-confirm", what = what));
                if folders {
                    ui.colored_label(RED, t!("files-delete-folders"));
                }
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new(t!("files-delete-now")).color(RED)).clicked() {
                        done = Some(true);
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(yes) = done {
                let items = self.confirm_delete.take().unwrap_or_default();
                if yes {
                    self.delete(items);
                }
            }
        }
        if self.confirm_close {
            let mut done = None;
            let (running, edits) = (self.running(), self.edits.len());
            modal(ctx, "files-close", t!("files-close-title"), |ui| {
                if running > 0 {
                    ui.label(t!("files-close-running", count = running));
                }
                if edits > 0 {
                    ui.label(t!("files-close-edits", count = edits));
                }
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new(t!("files-close-now")).color(RED)).clicked() {
                        done = Some(true);
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        done = Some(false);
                    }
                });
            });
            match done {
                Some(true) => self.close = true,
                Some(false) => self.confirm_close = false,
                None => {}
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.sftp.is_none() {
            return;
        }
        let (f5, back, delete, f2, enter) = ctx.input(|i| {
            let k = |key| i.key_pressed(key);
            (k(egui::Key::F5), k(egui::Key::Backspace), k(egui::Key::Delete), k(egui::Key::F2), k(egui::Key::Enter))
        });
        if enter && self.selected.len() == 1 {
            if let Some(row) = self.rows.iter().find(|r| self.selected.contains(&r.entry.name)).cloned() {
                if row.dir {
                    self.go(native_term_sftp::join(&self.path, &row.entry.name));
                } else {
                    self.edit(&row);
                }
            }
        }
        if f5 {
            self.refresh();
        }
        if back && self.path != b"/" {
            let parent = native_term_sftp::parent(&self.path);
            self.go(parent);
        }
        if delete && !self.selected.is_empty() {
            self.confirm_delete = Some(self.selection());
        }
        if f2 && self.selected.len() == 1 {
            if let Some(row) = self.rows.iter().find(|r| self.selected.contains(&r.entry.name)) {
                self.renaming = Some((row.entry.name.clone(), row.name.clone()));
            }
        }
    }
}

enum MenuAction {
    Download,
    DownloadTo,
    Edit(Row),
    Rename(Row),
    Delete,
}

impl crate::window::Ui for FilesWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        while let Ok(event) = self.rx.try_recv() {
            self.handle(event);
        }
        let ctx = ui.ctx().clone();
        // files dropped from Explorer: uploaded here
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() && self.sftp.is_some() {
            self.upload(dropped);
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        self.keys(&ctx);
        egui::Panel::top("files-toolbar").show_inside(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        if !self.jobs.is_empty() || !self.edits.is_empty() {
            egui::Panel::bottom("files-activity").resizable(true).max_size(240.0).show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.activity(ui));
            });
        }
        if let Some((text, error)) = &self.notice {
            let mut dismiss = false;
            egui::Panel::bottom("files-notice").show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    if *error {
                        ui.colored_label(RED, text);
                    } else {
                        ui.label(text);
                    }
                    dismiss = ui.small_button(icon(icons::CLEAR)).clicked();
                });
            });
            if dismiss {
                self.notice = None;
            }
        }
        egui::CentralPanel::default().show_inside(ui, |ui| match (&self.sftp, &self.failed) {
            (_, Some(error)) => {
                ui.colored_label(RED, error);
                ui.weak(t!("files-batch"));
                if ui.button(t!("files-reconnect")).clicked() {
                    self.connect();
                }
            }
            (None, None) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t!("files-connecting", host = self.spec.alias.as_str()));
                });
            }
            (Some(_), None) => {
                if let Some(e) = &self.list_error {
                    ui.colored_label(RED, e);
                }
                if self.listing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(t!("files-listing"));
                    });
                }
                if hovering {
                    ui.colored_label(GREEN, t!("files-drop-here", path = self.path_text.as_str()));
                }
                self.table(ui);
            }
        });
        self.dialogs(&ctx);
        // progress bars move by themselves
        if self.running() > 0 {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if !self.edits.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn close_requested(&mut self) -> bool {
        if self.running() == 0 && self.edits.is_empty() {
            return true;
        }
        self.confirm_close = true;
        false
    }

    fn wants_close(&self) -> bool {
        self.close
    }
}

impl Drop for FilesWindow {
    fn drop(&mut self) {
        if let Some((question, _)) = self.question.take() {
            let _ = question.reply.send(None);
        }
        for job in &self.jobs {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
        for edit in &self.edits {
            edit.stop.store(true, Ordering::Relaxed);
        }
        if let Some(sftp) = &self.sftp {
            sftp.disconnect();
        }
    }
}

/// Serves ssh's askpass questions on a private pipe (only this user may
/// open it; the name is random) and returns its name. Only the helper
/// started by our ssh (its parent) is answered: the account's own
/// password prompt from Credential Manager when saved, everything else by
/// asking in the window. Cancelling a question cancels the rest of this
/// connection's too (ssh would ask for the password again otherwise).
fn serve_questions(
    target: Option<native_term_config::password::Target>,
    ssh_pid: Arc<std::sync::atomic::AtomicU32>,
    served: Arc<AtomicBool>,
    tx: Sender<Event>,
    ctx: egui::Context,
) -> std::io::Result<String> {
    use native_term_session::pipe;
    let name = format!(r"\\.\pipe\NativeTerm-askpass-{}-{:016x}", std::process::id(), unique());
    let mut listener = pipe::PipeListener::bind(&name)?;
    std::thread::spawn(move || {
        let mut cancelled = false;
        while let Ok(conn) = listener.accept() {
            let Ok(Some(prompt)) = conn.recv::<String>(Duration::from_secs(5)) else { continue };
            let helper = conn.client_pid().unwrap_or(0);
            let ours = native_term_win::parent_pid(helper).is_some_and(|p| p != 0 && p == ssh_pid.load(Ordering::SeqCst));
            if !ours {
                let _ = conn.send(&None::<String>);
                continue;
            }
            let saved = target
                .as_ref()
                .filter(|t| t.answers(&prompt))
                .and_then(|t| native_term_win::credentials::read(&t.name).ok().flatten())
                .filter(|s| s.comment != native_term_config::password::REFUSED);
            let answer = match saved {
                Some(s) => {
                    served.store(true, Ordering::SeqCst);
                    Some(s.secret)
                }
                None if cancelled => None,
                None => {
                    let (reply, answer) = mpsc::channel();
                    let secret = !prompt.contains("(yes/no");
                    if tx.send(Event::Ask(Question { prompt: prompt.clone(), secret, reply })).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                    let typed = answer.recv_timeout(Duration::from_secs(290)).ok().flatten();
                    cancelled = typed.is_none();
                    typed
                }
            };
            let _ = conn.send(&answer);
        }
    });
    Ok(name)
}

/// Watches an edited file's local copy and uploads it when it changes
/// (and has stopped changing: an editor may save in steps). It goes to a
/// temporary name next to the file first and then replaces it, so the
/// file on the server is never half written; where that isn't allowed
/// (no write access to the folder), the file is written in place. The
/// original's permissions are kept. If the server's file changed since it
/// was opened, nothing is uploaded until "Overwrite".
#[allow(clippy::too_many_arguments)]
fn watch_and_upload(
    sftp: &Session,
    remote: &[u8],
    local: &Path,
    mut attrs: Attrs,
    stop: &AtomicBool,
    conflict: &AtomicBool,
    overwrite: &AtomicBool,
    set: &dyn Fn(String),
) {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut seen = modified(local);
    let mut pending: Option<SystemTime> = None;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(700));
        if sftp.closed().is_some() {
            return set(t!("files-edit-disconnected"));
        }
        let now = modified(local);
        if now != seen {
            // changed: wait until it stays the same for one more look
            seen = now;
            pending = now;
            continue;
        }
        let forced = overwrite.swap(false, Ordering::Relaxed);
        if pending.is_none() && !forced {
            continue;
        }
        // changed on the server meanwhile?
        let server = sftp.stat(remote).ok();
        if !forced && server.as_ref().and_then(|a| a.mtime()) != attrs.mtime() {
            conflict.store(true, Ordering::Relaxed);
            set(t!("files-edit-conflict"));
            continue;
        }
        conflict.store(false, Ordering::Relaxed);
        set(t!("files-edit-uploading"));
        match upload_in_place(sftp, remote, local, &attrs) {
            Ok(()) => {
                pending = None;
                if let Ok(a) = sftp.stat(remote) {
                    attrs = Attrs { permissions: attrs.permissions, ..a };
                }
                let time = native_term_win::local_time_of_day(unix_now());
                set(t!("files-edit-uploaded", time = time));
            }
            Err(e) => set(t!("files-edit-failed", error = e.to_string())),
        }
    }
}

fn upload_in_place(sftp: &Session, remote: &[u8], local: &Path, attrs: &Attrs) -> native_term_sftp::Result<()> {
    let name = last(remote);
    let mut temp_name = b".".to_vec();
    temp_name.extend_from_slice(name);
    temp_name.extend_from_slice(format!(".nt-{:x}", unique()).as_bytes());
    let temp = native_term_sftp::join(&native_term_sftp::parent(remote), &temp_name);
    let replaced = sftp
        .upload(local, &temp, attrs.permissions, &mut |_| true)
        .and_then(|_| {
            if let Some(p) = attrs.permissions {
                let _ = sftp.setstat(&temp, &Attrs { permissions: Some(p & 0o7777), ..Default::default() });
            }
            sftp.rename(&temp, remote, true)
        });
    match replaced {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = sftp.remove(&temp);
            sftp.upload(local, remote, None, &mut |_| true).map(|_| ())
        }
    }
}

fn modal(ctx: &egui::Context, id: &str, title: String, body: impl FnOnce(&mut egui::Ui)) {
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, body);
}

/// An icon alone (the icon font is a fallback of the text font).
fn icon(glyph: char) -> egui::RichText {
    egui::RichText::new(glyph.to_string())
}

/// A path's last part.
fn last(path: &[u8]) -> &[u8] {
    path.rsplit(|&c| c == b'/').find(|p| !p.is_empty()).unwrap_or(path)
}

/// "a.txt" or "a.txt and 3 more".
fn describe(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => t!("files-and-more", first = first.as_str(), count = rest.len()),
    }
}

pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// `drwxr-xr-x`, as `ls -l` shows it.
pub fn mode_text(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o020000 => 'c',
        0o060000 => 'b',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    };
    let mut out = String::from(kind);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        out.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    out
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A number unlikely to repeat (for temporary names).
fn unique() -> u64 {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    nanos ^ (u64::from(std::process::id()) << 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(mode_text(0o040755), "drwxr-xr-x");
        assert_eq!(mode_text(0o100600), "-rw-------");
        assert_eq!(mode_text(0o120777), "lrwxrwxrwx");
        assert_eq!(last(b"/home/a/b.txt"), b"b.txt");
        assert_eq!(last(b"/home/a/"), b"a");
    }

    /// An edited file is uploaded when saved, by replacing (the server's
    /// file is never half written), keeping its permissions; a change on
    /// the server meanwhile holds the upload until "Overwrite".
    #[test]
    fn edits_go_back_to_the_server() {
        let server = Path::new(r"C:\Windows\System32\OpenSSH\sftp-server.exe");
        if !server.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(server);
        command.arg("-d").arg(dir.path());
        let sftp = Arc::new(Session::spawn(command).unwrap());
        let remote_dir = format!("/{}", dir.path().display().to_string().replace('\\', "/")).into_bytes();
        std::fs::write(dir.path().join("conf.txt"), b"v1").unwrap();
        let remote = native_term_sftp::join(&remote_dir, b"conf.txt");
        let local = dir.path().join("local-copy.txt");
        let attrs = sftp.stat(&remote).unwrap();
        sftp.download(&remote, &local, &mut |_| true).unwrap();

        let (stop, conflict, overwrite) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let status = Arc::new(Mutex::new(String::new()));
        let (s, r, l, st, c, o, stat) = (Arc::clone(&sftp), remote.clone(), local.clone(), Arc::clone(&stop), Arc::clone(&conflict), Arc::clone(&overwrite), Arc::clone(&status));
        let watcher = std::thread::spawn(move || {
            watch_and_upload(&s, &r, &l, attrs, &st, &c, &o, &|text| *stat.lock().unwrap() = text);
        });
        let wait_for = |what: &dyn Fn() -> bool| {
            let end = Instant::now() + Duration::from_secs(15);
            while !what() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(100));
            }
            what()
        };
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&local, b"v2 saved in the editor").unwrap();
        assert!(wait_for(&|| std::fs::read(dir.path().join("conf.txt")).unwrap() == b"v2 saved in the editor"), "uploaded");
        // no temporary file left next to it
        let names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.contains(".nt-")), "{names:?}");

        // someone changes it on the server; our next save waits
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(dir.path().join("conf.txt"), b"changed on the server").unwrap();
        std::fs::write(&local, b"v3").unwrap();
        assert!(wait_for(&|| conflict.load(Ordering::Relaxed)), "conflict seen");
        assert_eq!(std::fs::read(dir.path().join("conf.txt")).unwrap(), b"changed on the server");
        overwrite.store(true, Ordering::Relaxed);
        assert!(wait_for(&|| std::fs::read(dir.path().join("conf.txt")).unwrap() == b"v3"), "overwritten when asked");
        stop.store(true, Ordering::Relaxed);
        watcher.join().unwrap();
    }
}
