//! The files window's Synchronize dialog: what comparing a local folder
//! with a server's folder found, what each direction would do with it,
//! and which of those to do.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use native_term_app::t;
use native_term_sftp::sync::{Act, Found, Meta, Mode};
use native_term_sftp::wire::Attrs;

use crate::files_window::{size_text, GREEN, RED};
use crate::icons;

pub enum Scan {
    Running { seen: Arc<AtomicUsize>, stop: Arc<AtomicBool> },
    Done(Vec<Found>),
    Failed(String),
}

pub struct SyncDialog {
    pub tab: u64,
    pub host: String,
    pub local: PathBuf,
    pub remote: Vec<u8>,
    pub remote_text: String,
    pub scan: Scan,
    mode: Mode,
    delete_extra: bool,
    show_all: bool,
    /// Rows unticked (indexes into what was found).
    off: HashSet<usize>,
    /// Deleting asked about once more before it starts.
    confirming: bool,
}

/// What to do, as the dialog's Start asks.
#[derive(Default)]
pub struct Plan {
    pub uploads: Vec<(PathBuf, Vec<u8>)>,
    pub downloads: Vec<(Vec<u8>, Attrs, PathBuf)>,
    pub delete_remote: Vec<(Vec<u8>, Attrs)>,
    pub delete_local: Vec<PathBuf>,
}

/// What the dialog asks the window for.
pub enum Asked {
    Rescan,
    Start(Plan),
    Close,
}

impl SyncDialog {
    pub fn new(tab: u64, host: String, local: PathBuf, remote: Vec<u8>, remote_text: String, scan: Scan) -> Self {
        SyncDialog {
            tab,
            host,
            local,
            remote,
            remote_text,
            scan,
            mode: Mode::Both,
            delete_extra: false,
            show_all: false,
            off: HashSet::new(),
            confirming: false,
        }
    }

    /// Stops a comparison still running (the dialog closes).
    pub fn stop(&self) {
        if let Scan::Running { stop, .. } = &self.scan {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn act(&self, f: &Found) -> Act {
        f.act(self.mode, self.delete_extra && self.mode != Mode::Both)
    }

    /// The ticked rows' plan.
    fn plan(&self, found: &[Found]) -> Plan {
        let mut plan = Plan::default();
        for (_, f) in found.iter().enumerate().filter(|(i, _)| !self.off.contains(i)) {
            match (self.act(f), &f.attrs) {
                (Act::Upload, _) => plan.uploads.push((f.local.clone(), f.remote.clone())),
                (Act::Download, Some(a)) => plan.downloads.push((f.remote.clone(), a.clone(), f.local.clone())),
                (Act::DeleteRemote, Some(a)) => plan.delete_remote.push((f.remote.clone(), a.clone())),
                (Act::DeleteLocal, _) => plan.delete_local.push(f.local.clone()),
                _ => {}
            }
        }
        plan
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<Asked> {
        let mut asked = None;
        let mut open = true;
        egui::Window::new(t!("files-sync-title", host = self.host.as_str()))
            .id(egui::Id::new("files-sync"))
            .collapsible(false)
            .resizable(true)
            .default_size([900.0, 560.0])
            .open(&mut open)
            .show(ctx, |ui| asked = self.body(ui));
        if !open {
            self.stop();
            return Some(Asked::Close);
        }
        asked
    }

    fn body(&mut self, ui: &mut egui::Ui) -> Option<Asked> {
        let mut asked = None;
        egui::Grid::new("files-sync-where").num_columns(2).show(ui, |ui| {
            ui.weak(t!("files-sync-local"));
            ui.label(self.local.display().to_string());
            ui.end_row();
            ui.weak(t!("files-sync-remote"));
            ui.label(self.remote_text.as_str());
            ui.end_row();
        });
        ui.separator();
        let before = (self.mode, self.delete_extra);
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.mode, Mode::Both, t!("files-sync-both")).on_hover_text(t!("files-sync-both-hint"));
            ui.radio_value(&mut self.mode, Mode::Upload, format!("{} {}", icons::UPLOAD, t!("files-sync-upload")))
                .on_hover_text(t!("files-sync-upload-hint"));
            ui.radio_value(
                &mut self.mode,
                Mode::Download,
                format!("{} {}", icons::DOWNLOAD, t!("files-sync-download")),
            )
            .on_hover_text(t!("files-sync-download-hint"));
        });
        ui.horizontal(|ui| {
            let one_way = self.mode != Mode::Both;
            ui.add_enabled(one_way, egui::Checkbox::new(&mut self.delete_extra, t!("files-sync-delete-extra")))
                .on_hover_text(t!("files-sync-delete-extra-hint"));
            ui.checkbox(&mut self.show_all, t!("files-sync-show-all"));
        });
        if before != (self.mode, self.delete_extra) {
            self.confirming = false;
        }
        ui.separator();
        match &self.scan {
            Scan::Running { seen, stop } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t!("files-sync-comparing", count = seen.load(Ordering::Relaxed)));
                    if ui.button(t!("button-cancel")).clicked() {
                        stop.store(true, Ordering::Relaxed);
                    }
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                return None;
            }
            Scan::Failed(e) => {
                ui.colored_label(RED, e);
                if ui.button(t!("files-sync-rescan")).clicked() {
                    asked = Some(Asked::Rescan);
                }
                return asked;
            }
            Scan::Done(_) => {}
        }
        // drawn from outside `scan` (the rows' ticks change `self`)
        let Scan::Done(found) = std::mem::replace(&mut self.scan, Scan::Done(Vec::new())) else { return None };
        let asked = self.results(ui, &found);
        self.scan = Scan::Done(found);
        asked
    }

    fn results(&mut self, ui: &mut egui::Ui, found: &[Found]) -> Option<Asked> {
        let mut asked = None;
        // the rows shown: what something is done with (all with "show all")
        let rows: Vec<(usize, Act)> = found
            .iter()
            .enumerate()
            .map(|(i, f)| (i, self.act(f)))
            .filter(|(_, a)| self.show_all || !matches!(a, Act::Same | Act::Skip))
            .collect();
        let plan = self.plan(found);
        // what the ticked copies weigh: files' sizes, and how many folders
        // are copied whole (their size isn't known before)
        let (mut up, mut down) = ((0, 0), (0, 0));
        for (_, f) in found.iter().enumerate().filter(|(i, _)| !self.off.contains(i)) {
            let (side, meta) = match self.act(f) {
                Act::Upload => (&mut up, f.here.as_ref()),
                Act::Download => (&mut down, f.there.as_ref()),
                _ => continue,
            };
            match meta {
                Some(m) if m.dir => side.1 += 1,
                Some(m) => side.0 += m.size,
                None => {}
            }
        }
        let deletes = plan.delete_remote.len() + plan.delete_local.len();
        let conflicts = found.iter().filter(|f| self.act(f) == Act::Conflict).count();
        if rows.is_empty() {
            ui.label(t!("files-sync-alike"));
        }
        let footer = 80.0;
        let columns = Columns::for_width(ui.available_width());
        if !rows.is_empty() {
            columns.row(
                ui,
                None,
                [
                    egui::RichText::new(t!("files-sync-col-act")).weak(),
                    egui::RichText::new(t!("files-sync-col-path")).weak(),
                    egui::RichText::new(t!("files-sync-local")).weak(),
                    egui::RichText::new(t!("files-sync-remote")).weak(),
                ],
            );
        }
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - footer).max(120.0))
            .auto_shrink([false, true])
            .show_rows(ui, ROW, rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (n, &(i, act)) in rows[range.clone()].iter().enumerate() {
                    let f = &found[i];
                    let doable = matches!(act, Act::Upload | Act::Download | Act::DeleteLocal | Act::DeleteRemote);
                    let mut on = doable && !self.off.contains(&i);
                    let (text, color) = act_text(act);
                    let color = color.unwrap_or(ui.visuals().text_color());
                    let striped = (range.start + n) % 2 == 1;
                    let changed = columns.row(
                        ui,
                        Some((&mut on, doable, striped)),
                        [
                            egui::RichText::new(text).color(color),
                            egui::RichText::new(f.rel.as_str()),
                            egui::RichText::new(meta_text(f.here.as_ref())).weak(),
                            egui::RichText::new(meta_text(f.there.as_ref())).weak(),
                        ],
                    );
                    if changed {
                        if on {
                            self.off.remove(&i);
                        } else {
                            self.off.insert(i);
                        }
                        self.confirming = false;
                    }
                }
            });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(t!("files-sync-summary-up", count = plan.uploads.len(), size = weight(up)));
            ui.label(t!("files-sync-summary-down", count = plan.downloads.len(), size = weight(down)));
            if deletes > 0 {
                ui.colored_label(RED, t!("files-sync-summary-delete", count = deletes));
            }
            if conflicts > 0 {
                ui.colored_label(RED, t!("files-sync-summary-conflicts", count = conflicts))
                    .on_hover_text(t!("files-sync-conflict-hint"));
            }
        });
        ui.horizontal(|ui| {
            let work = plan.uploads.len() + plan.downloads.len() + deletes > 0;
            if self.confirming {
                ui.colored_label(RED, t!("files-sync-confirm-delete", count = deletes));
                if ui.button(egui::RichText::new(t!("files-sync-start-delete")).color(RED)).clicked() {
                    asked = Some(Asked::Start(self.plan(found)));
                }
            } else if ui.add_enabled(work, egui::Button::new(t!("files-sync-start"))).clicked() {
                if deletes > 0 {
                    self.confirming = true;
                } else {
                    asked = Some(Asked::Start(self.plan(found)));
                }
            }
            if ui.button(format!("{} {}", icons::REFRESH, t!("files-sync-rescan"))).clicked() {
                asked = Some(Asked::Rescan);
            }
            if ui.button(t!("button-close")).clicked() {
                asked = Some(Asked::Close);
            }
        });
        asked
    }
}

const ROW: f32 = 22.0;

/// The rows' columns: a tick, what's done, the path (what's left), and
/// each side's size and time.
struct Columns {
    tick: f32,
    act: f32,
    path: f32,
    side: f32,
}

impl Columns {
    fn for_width(width: f32) -> Columns {
        let (tick, act, side) = (28.0, 130.0, 230.0);
        Columns { tick, act, side, path: (width - tick - act - 2.0 * side).max(120.0) }
    }

    /// One row (`tick`: its box, whether it can be ticked, and shading);
    /// whether the box changed.
    fn row(&self, ui: &mut egui::Ui, tick: Option<(&mut bool, bool, bool)>, cells: [egui::RichText; 4]) -> bool {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::hover());
        let mut changed = false;
        if let Some((_, _, true)) = &tick {
            ui.painter().rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
        }
        let mut x = rect.left();
        let mut cell = |ui: &mut egui::Ui, width: f32, add: &mut dyn FnMut(&mut egui::Ui)| {
            let at = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(width - 6.0, ROW));
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.set_clip_rect(at.intersect(ui.clip_rect()));
                    add(ui)
                },
            );
            x += width;
        };
        match tick {
            Some((on, doable, _)) => cell(ui, self.tick, &mut |ui| {
                changed = ui.add_enabled(doable, egui::Checkbox::without_text(on)).changed();
            }),
            None => cell(ui, self.tick, &mut |_| {}),
        }
        let [act, path, here, there] = cells;
        cell(ui, self.act, &mut |ui| {
            ui.label(act.clone());
        });
        cell(ui, self.path, &mut |ui| {
            ui.add(egui::Label::new(path.clone()).truncate());
        });
        cell(ui, self.side, &mut |ui| {
            ui.add(egui::Label::new(here.clone()).truncate());
        });
        cell(ui, self.side, &mut |ui| {
            ui.add(egui::Label::new(there.clone()).truncate());
        });
        changed
    }
}

/// Files' size, and the folders copied whole (not weighed before).
fn weight((bytes, folders): (u64, usize)) -> String {
    if folders == 0 {
        size_text(bytes)
    } else if bytes == 0 {
        t!("files-sync-folders", count = folders)
    } else {
        t!("files-sync-size-folders", size = size_text(bytes), count = folders)
    }
}

fn act_text(act: Act) -> (String, Option<egui::Color32>) {
    match act {
        Act::Upload => (format!("{} {}", icons::UPLOAD, t!("files-sync-act-upload")), Some(GREEN)),
        Act::Download => (format!("{} {}", icons::DOWNLOAD, t!("files-sync-act-download")), Some(GREEN)),
        Act::DeleteLocal => (format!("{} {}", icons::DELETE, t!("files-sync-act-delete-local")), Some(RED)),
        Act::DeleteRemote => (format!("{} {}", icons::DELETE, t!("files-sync-act-delete-remote")), Some(RED)),
        Act::Same => (t!("files-sync-act-same"), None),
        Act::Skip => (t!("files-sync-act-skip"), None),
        Act::Conflict => (t!("files-sync-act-conflict"), Some(RED)),
    }
}

/// One side's size and time, or that it has none.
fn meta_text(meta: Option<&Meta>) -> String {
    match meta {
        None => "—".to_string(),
        Some(m) if m.dir => t!("files-sync-folder"),
        Some(m) => {
            let when = m.modified.map(native_term_os::time::local_date_time).unwrap_or_default();
            format!("{}  {when}", size_text(m.size))
        }
    }
}
