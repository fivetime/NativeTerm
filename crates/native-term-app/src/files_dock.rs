//! The files window's frame round its sides, as the design has it: the
//! rail between the sides (`.rail`: send either way, sync), the panel
//! below them (`.dock`: the transfers, the session's log, its errors,
//! one at a time), the status line at the very bottom (`.statusbar`), and
//! the session shown in the title bar's middle.

use super::*;
use native_term_skin::{IconButton, Segment};

/// What the panel below the sides shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Dock {
    #[default]
    Queue,
    Log,
    Errors,
}

/// Where a transfer goes from and to, as the queue shows it.
fn route(job: &Job) -> Option<(String, String)> {
    let parent_text = |p: &Path| p.parent().map_or_else(|| p.display().to_string(), |d| d.display().to_string());
    match job.work.as_ref()? {
        Work::Upload { names, files, into } => Some((parent_text(files.first()?), names.decode(into))),
        Work::Download { names, items, folder } => {
            Some((names.decode(&native_term_sftp::parent(&items.first()?.0)), folder.display().to_string()))
        }
        Work::UploadPairs { names, pairs } => {
            let (from, to) = pairs.first()?;
            Some((parent_text(from), names.decode(&native_term_sftp::parent(to))))
        }
        Work::DownloadPairs { names, pairs } => {
            let (from, _, to) = pairs.first()?;
            Some((names.decode(&native_term_sftp::parent(from)), parent_text(to)))
        }
    }
}

impl FilesWindow {
    /// The session shown, in the title bar's middle: its state's dot, its
    /// name, its alias.
    pub(super) fn title_middle(&self, ui: &mut egui::Ui, bar: egui::Rect) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let palette = crate::looks::skin(ui.visuals()).palette;
        let tones = crate::looks::tones(ui.visuals());
        let dot = match (tab.remote.up(), &tab.remote.failed) {
            (_, Some(_)) => tones.danger,
            (true, None) => tones.alive,
            (false, None) => palette.weak,
        };
        let name = ui.painter().layout_no_wrap(
            tab.spec.label.clone(),
            native_term_skin::font(ui.ctx(), 12.0, native_term_skin::Weight::Semibold),
            palette.text,
        );
        let alias = (tab.spec.label != tab.spec.alias)
            .then(|| ui.painter().layout_no_wrap(tab.spec.alias.clone(), egui::FontId::monospace(11.0), palette.weak));
        let width = 7.0 + 8.0 + name.size().x + alias.as_ref().map_or(0.0, |a| 8.0 + a.size().x);
        let mut x = bar.center().x - width / 2.0;
        let y = bar.center().y;
        let painter = ui.painter();
        painter.circle_filled(egui::pos2(x + 3.5, y), 3.5, dot);
        x += 7.0 + 8.0;
        let w = name.size().x;
        painter.galley(egui::pos2(x, y - name.size().y / 2.0), name, palette.text);
        x += w + 8.0;
        if let Some(alias) = alias {
            painter.galley(egui::pos2(x, y - alias.size().y / 2.0), alias, palette.weak);
        }
    }

    /// The rail between the sides: upload, download, sync.
    pub(super) fn rail(&mut self, ui: &mut egui::Ui) {
        let palette = crate::looks::skin(ui.visuals()).palette;
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, palette.bar);
        ui.painter().vline(rect.left() + 0.5, rect.y_range(), egui::Stroke::new(1.0, palette.line));
        ui.painter().vline(rect.right() - 0.5, rect.y_range(), egui::Stroke::new(1.0, palette.line));
        let Some(tab) = self.tabs.get(self.active) else { return };
        let id = tab.id;
        let connected = tab.remote.up();
        let (local_chosen, remote_chosen) = (!tab.local.selected.is_empty(), !tab.remote.selected.is_empty());
        let local_folder = tab.local.path.is_some();
        let (mut upload, mut download, mut sync) = (false, false, false);
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space((ui.available_height() / 2.0 - 70.0).max(8.0));
            let arrow = icons::ARROW_RIGHT.to_string();
            let look = files_list::Look::of(ui.visuals());
            let upload_button = IconButton::new(&arrow, t!("files-upload-hint")).round(look.local);
            upload = upload_button.enabled(connected && local_chosen).show(ui, &palette).clicked();
            let arrow = icons::ARROW_LEFT.to_string();
            let download_button = IconButton::new(&arrow, t!("files-download-hint")).round(look.remote);
            download = download_button.enabled(connected && remote_chosen).show(ui, &palette).clicked();
            let (line, _) = ui.allocate_exact_size(egui::vec2(16.0, 1.0), egui::Sense::hover());
            ui.painter().hline(line.x_range(), line.center().y, egui::Stroke::new(1.0, palette.line));
            let glyph = icons::SYNC.to_string();
            let sync_button = IconButton::new(&glyph, t!("files-sync-hint")).round(look.local);
            sync = sync_button.enabled(connected && local_folder).show(ui, &palette).clicked();
        });
        if upload {
            let files = self.local_selection(id);
            self.upload(id, files);
        }
        if download {
            let items = self.remote_selection(id);
            self.download(id, items);
        }
        if sync {
            self.open_sync(id);
        }
    }

    /// The panel below the sides: the transfers, the session's log or its
    /// errors.
    pub(super) fn dock(&mut self, ui: &mut egui::Ui) {
        let palette = crate::looks::skin(ui.visuals()).palette;
        let rect = ui.max_rect();
        ui.painter().hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, palette.line));
        let errors = self.tabs.get(self.active).map_or(0, |t| t.log.iter().filter(|(_, _, e)| *e).count());
        let active = self.jobs.iter().filter(|j| matches!(j.state, JobState::Running | JobState::Paused)).count();
        let queue_label = t!("files-queue");
        let log_label = t!("files-dock-log");
        let errors_label = t!("files-dock-errors");
        let (queue_glyph, log_glyph, error_glyph) =
            (icons::SYNC.to_string(), icons::DOCUMENT.to_string(), icons::ERROR.to_string());
        let segments = [
            Segment { glyph: &queue_glyph, label: &queue_label, count: Some(active + self.edits(None)) },
            Segment { glyph: &log_glyph, label: &log_label, count: None },
            Segment { glyph: &error_glyph, label: &errors_label, count: Some(errors) },
        ];
        let chosen = match self.dock {
            Dock::Queue => 0,
            Dock::Log => 1,
            Dock::Errors => 2,
        };
        // the head: the switch, then what the queue's buttons do
        let (head, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
        ui.painter().hline(head.x_range(), head.bottom() - 0.5, egui::Stroke::new(1.0, palette.line));
        let mut bar = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(head.shrink2(egui::vec2(8.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if let Some(i) = native_term_skin::segmented(&mut bar, &palette, &segments, chosen) {
            self.dock = [Dock::Queue, Dock::Log, Dock::Errors][i];
        }
        if self.dock == Dock::Queue {
            bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.queue_tools(ui, &palette));
        }
        match self.dock {
            Dock::Queue => self.queue(ui, &palette),
            Dock::Log => self.log_lines(ui, &palette, false),
            Dock::Errors => self.log_lines(ui, &palette, true),
        }
    }

    /// The queue's buttons (right to left): how many at once, clear the
    /// finished, resume, pause, the speed.
    fn queue_tools(&mut self, ui: &mut egui::Ui, palette: &native_term_skin::Palette) {
        ui.spacing_mut().item_spacing.x = 6.0;
        let mut at_once = self.at_once();
        let widget = egui::DragValue::new(&mut at_once).range(1..=AT_ONCE_MAX).speed(0.05);
        if ui.add(widget).on_hover_text(t!("files-at-once-hint")).changed() {
            self.set_at_once(at_once);
        }
        ui.label(egui::RichText::new(t!("files-at-once")).size(12.0).color(palette.weak));
        ui.add_space(8.0);
        let ghost = |text: String| {
            egui::Button::new(egui::RichText::new(text).size(12.0))
                .frame_when_inactive(false)
                .min_size(egui::vec2(0.0, 30.0))
        };
        // finished: done, cancelled or failed (paused ones stay)
        let finished = self.jobs.iter().any(|j| !matches!(j.state, JobState::Running | JobState::Paused));
        if ui.add_enabled(finished, ghost(t!("files-queue-clear"))).clicked() {
            self.jobs.retain(|j| matches!(j.state, JobState::Running | JobState::Paused));
        }
        let paused = self.jobs.iter().filter(|j| matches!(j.state, JobState::Paused)).count();
        if ui.add_enabled(paused > 0, ghost(format!("{} {}", icons::PLAY, t!("files-queue-resume-all")))).clicked() {
            let ids: Vec<u64> =
                self.jobs.iter().filter(|j| matches!(j.state, JobState::Paused)).map(|j| j.id).collect();
            for id in ids {
                self.run_job(id);
            }
        }
        let pausable = self.jobs.iter().any(|j| matches!(j.state, JobState::Running) && j.work.is_some());
        if ui.add_enabled(pausable, ghost(format!("{} {}", icons::PAUSE, t!("files-queue-pause-all")))).clicked() {
            for job in self.jobs.iter().filter(|j| matches!(j.state, JobState::Running) && j.work.is_some()) {
                job.progress.pause.store(true, Ordering::Relaxed);
            }
        }
        let speed: f64 = self
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Running) && j.kind != Kind::Delete)
            .map(Job::speed)
            .sum();
        if speed > 0.0 {
            let text = format!("{} {}/s", icons::ACTIVITY, size_text(speed as u64));
            ui.label(egui::RichText::new(text).font(egui::FontId::monospace(11.0)).color(palette.weak));
        }
    }

    /// The transfers, one to a row, and the files being edited.
    fn queue(&mut self, ui: &mut egui::Ui, palette: &native_term_skin::Palette) {
        let look = files_list::Look::of(ui.visuals());
        let tones = crate::looks::tones(ui.visuals());
        let mut actions = Vec::new();
        egui::ScrollArea::vertical().id_salt("files-queue").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if self.jobs.is_empty() && self.edits(None) == 0 {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new(t!("files-queue-empty")).size(12.0).color(palette.weak));
                });
            }
            for job in &self.jobs {
                if let Some(action) = job_row(ui, job, palette, &look, &tones) {
                    actions.push((job.id, action));
                }
            }
            for tab in &mut self.tabs {
                let mut stop = None;
                for (i, edit) in tab.edits.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.set_min_height(36.0);
                        ui.add_space(16.0);
                        ui.label(egui::RichText::new(icons::EDIT.to_string()).size(14.0).color(look.code));
                        ui.label(egui::RichText::new(&edit.name).size(13.0));
                        ui.label(egui::RichText::new(format!("[{}]", tab.spec.label)).size(11.0).color(palette.weak));
                        ui.label(
                            egui::RichText::new(edit.status.lock().unwrap_or_else(|e| e.into_inner()).as_str())
                                .size(11.0)
                                .color(palette.weak),
                        );
                        if edit.conflict.load(Ordering::Relaxed)
                            && ui.small_button(egui::RichText::new(t!("files-edit-overwrite")).color(RED)).clicked()
                        {
                            edit.overwrite.store(true, Ordering::Relaxed);
                        }
                        if ui
                            .small_button(t!("files-edit-stop"))
                            .on_hover_text(edit.local.display().to_string())
                            .clicked()
                        {
                            stop = Some(i);
                        }
                    });
                }
                if let Some(i) = stop {
                    let edit = tab.edits.remove(i);
                    edit.stop.store(true, Ordering::Relaxed);
                }
            }
        });
        for (id, action) in actions {
            self.job_action(id, action);
        }
    }

    /// The session's log, or only its errors.
    fn log_lines(&mut self, ui: &mut egui::Ui, palette: &native_term_skin::Palette, errors: bool) {
        let tones = crate::looks::tones(ui.visuals());
        let Some(tab) = self.tabs.get(self.active) else { return };
        let lines: Vec<&(String, String, bool)> = tab.log.iter().filter(|(_, _, e)| !errors || *e).collect();
        if errors && lines.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(icons::ACCEPT.to_string()).size(20.0).color(tones.alive));
                ui.label(egui::RichText::new(t!("files-dock-no-errors")).size(12.0).color(palette.weak));
            });
            return;
        }
        let mono = egui::FontId::monospace(11.5);
        egui::ScrollArea::vertical()
            .id_salt(if errors { "files-errors" } else { "files-log" })
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(10.0);
                for (time, text, error) in lines {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(egui::RichText::new(time).font(mono.clone()).color(palette.weak.gamma_multiply(0.8)));
                        ui.add_space(6.0);
                        let color = if *error { tones.danger } else { palette.weak };
                        ui.add(egui::Label::new(egui::RichText::new(text).font(mono.clone()).color(color)).wrap());
                    });
                }
            });
    }

    /// The status line at the window's foot: the session's state, its
    /// name, the protocol, how names are encoded.
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let palette = crate::looks::skin(ui.visuals()).palette;
        let tones = crate::looks::tones(ui.visuals());
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, palette.page);
        ui.painter().hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, palette.line));
        let Some(tab) = self.tabs.get(self.active) else { return };
        let small = egui::FontId::proportional(11.0);
        let mono = egui::FontId::monospace(11.0);
        let (dot, state) = match (tab.remote.up(), &tab.remote.failed) {
            (_, Some(_)) => (tones.danger, t!("files-status-failed")),
            (true, None) => (tones.alive, t!("files-status-connected")),
            (false, None) => (palette.weak, t!("files-status-connecting")),
        };
        let names = tab.remote.names;
        let alias = tab.spec.alias.clone();
        let mut line = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(16.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        line.spacing_mut().item_spacing.x = 6.0;
        let (d, _) = line.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        line.painter().circle_filled(d.center(), 3.5, dot);
        line.label(egui::RichText::new(state).font(small.clone()).color(palette.weak));
        line.add_space(10.0);
        line.label(egui::RichText::new(icons::NETWORK.to_string()).size(12.0).color(palette.weak));
        line.label(egui::RichText::new(alias).font(mono.clone()).color(palette.weak));
        // the latency, from the requests the window makes anyway
        if let Some(latency) = tab.remote.sftp.as_ref().and_then(|s| s.latency()) {
            line.add_space(10.0);
            line.label(egui::RichText::new(icons::SPEED.to_string()).size(12.0).color(palette.weak));
            let ms = latency.as_secs_f64() * 1000.0;
            let text = if ms < 10.0 { format!("{ms:.1} ms") } else { format!("{ms:.0} ms") };
            line.label(egui::RichText::new(t!("files-latency")).font(small.clone()).color(palette.weak));
            line.label(egui::RichText::new(text).font(mono.clone()).color(palette.weak))
                .on_hover_text(t!("files-latency-hint"));
        }
        let mut chosen = None;
        line.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let shown =
                if matches!(names, Names::Auto { .. }) { "UTF-8".to_string() } else { names.label().to_string() };
            let encoding = ui
                .add(
                    egui::Label::new(egui::RichText::new(shown).font(mono.clone()).color(palette.weak))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text(t!("files-encoding-hint"));
            egui::Popup::menu(&encoding).show(|ui| {
                for label in ENCODINGS {
                    if let Some(choice) = Names::from_label(label) {
                        let text = if label == "auto" { t!("files-encoding-auto") } else { label.to_string() };
                        if ui.radio(names == choice, text).clicked() {
                            chosen = Some(choice);
                            ui.close();
                        }
                    }
                }
            });
            ui.label(egui::RichText::new("SFTP v3").font(mono).color(palette.weak));
        });
        if let Some(choice) = chosen {
            let id = self.tabs[self.active].id;
            self.remote_asked(id, files_pane::Asked::Names(choice), ui.ctx());
        }
    }
}

/// A transfer's row (`.job`): its kind's icon, its name and direction,
/// where from and to, its size, its progress, its state, its buttons.
fn job_row(
    ui: &mut egui::Ui,
    job: &Job,
    palette: &native_term_skin::Palette,
    look: &files_list::Look,
    tones: &crate::looks::Tones,
) -> Option<JobAction> {
    let mut action = None;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, palette.card);
    }
    let done = job.progress.done.load(Ordering::Relaxed);
    let total = job.progress.total.load(Ordering::Relaxed);
    let fraction = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
    let rate = job.speed();
    // the columns: icon 32, name, route, size 72, progress, state 84, buttons 56
    let pad = 16.0;
    let gap = 12.0;
    let inner = rect.shrink2(egui::vec2(pad, 0.0));
    let fixed = 32.0 + 72.0 + 84.0 + 56.0 + gap * 6.0;
    let flexible = (inner.width() - fixed).max(300.0);
    let name_w = (flexible * 1.3 / 4.3).max(150.0);
    let progress_w = (flexible * 1.0 / 4.3).max(140.0);
    let route_w = (flexible - name_w - progress_w).max(0.0);
    let mut x = inner.left();
    let mut column = |w: f32| {
        let r = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w, rect.bottom()));
        x += w + gap;
        r
    };
    let (icon_r, name_r, route_r, size_r, progress_r, state_r, buttons_r) =
        (column(32.0), column(name_w), column(route_w), column(72.0), column(progress_w), column(84.0), column(56.0));
    let painter = ui.painter_at(rect);
    let y = rect.center().y;
    // the kind's icon on a tile
    let kind = files_list::kind(&job.title, false, false);
    let tint = match kind {
        files_list::Kind::Code => look.code,
        files_list::Kind::Doc => look.doc,
        files_list::Kind::Binary => look.binary,
        _ => palette.weak,
    };
    let tile = egui::Rect::from_center_size(icon_r.center(), egui::vec2(32.0, 32.0));
    painter.rect_filled(tile, 8.0, if response.hovered() { palette.page } else { palette.card });
    let glyph = if job.kind == Kind::Delete { icons::DELETE } else { icons::DOCUMENT };
    painter.text(tile.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(14.0), tint);
    // name, and which way it goes
    let name_font = native_term_skin::font(ui.ctx(), 13.0, native_term_skin::Weight::Medium);
    let mut job_name = egui::text::LayoutJob::simple_singleline(job.title.clone(), name_font, palette.text);
    job_name.wrap = egui::text::TextWrapping::truncate_at_width(name_r.width());
    let g = painter.layout_job(job_name);
    painter.galley(egui::pos2(name_r.left(), y - 15.0), g, palette.text);
    // (the title says what it does; under it, which way and where)
    let way_glyph = match job.kind {
        Kind::Upload => icons::UPLOAD,
        Kind::Download => icons::DOWNLOAD,
        Kind::Delete => icons::DELETE,
    };
    painter.text(
        egui::pos2(name_r.left(), y + 9.0),
        egui::Align2::LEFT_CENTER,
        format!("{way_glyph} {}", job.host),
        egui::FontId::proportional(11.0),
        palette.weak,
    );
    // where from and to
    if let Some((from, to)) = route(job) {
        let text = format!("{from}  →  {to}");
        let mut r = egui::text::LayoutJob::simple_singleline(text, egui::FontId::monospace(11.0), palette.weak);
        r.wrap = egui::text::TextWrapping::truncate_at_width(route_r.width());
        let g = painter.layout_job(r);
        painter.galley(egui::pos2(route_r.left(), y - g.size().y / 2.0), g, palette.weak);
    }
    // its size
    if job.kind != Kind::Delete && total > 0 {
        painter.text(
            egui::pos2(size_r.right(), y),
            egui::Align2::RIGHT_CENTER,
            size_text(total),
            egui::FontId::monospace(11.0),
            palette.weak,
        );
    }
    // the progress: a track, what has gone, what is left
    let (state_fill, state_text, bar) = match &job.state {
        JobState::Running => (native_term_skin::thin(look.local, 41), look.local_ink, look.local),
        JobState::Paused => (tones.busy.fill, tones.busy.text, tones.busy.text),
        JobState::Done => (tones.good.fill, tones.good.text, look.remote),
        JobState::Failed(_) => (tones.bad.fill, tones.bad.text, tones.danger),
        JobState::Cancelled => (palette.card, palette.weak, palette.weak),
    };
    if job.kind != Kind::Delete {
        let track =
            egui::Rect::from_min_size(egui::pos2(progress_r.left(), y - 9.0), egui::vec2(progress_r.width(), 6.0));
        painter.rect_filled(track, 3.0, if response.hovered() { palette.line } else { palette.card });
        let filled = egui::Rect::from_min_size(track.min, egui::vec2(track.width() * fraction, track.height()));
        painter.rect_filled(filled, 3.0, bar);
        let small = egui::FontId::monospace(10.5);
        painter.text(
            egui::pos2(progress_r.left(), y + 8.0),
            egui::Align2::LEFT_CENTER,
            format!("{} / {}", size_text(done), size_text(total)),
            small.clone(),
            palette.weak,
        );
        let right = match &job.state {
            JobState::Running if rate > 0.0 && total > done => {
                t!("files-job-left", time = clock_text(((total - done) as f64 / rate).ceil() as u64))
            }
            JobState::Running => format!("{}/s", size_text(rate as u64)),
            JobState::Paused => t!("files-job-paused-short"),
            JobState::Done => {
                let secs = job.finished.unwrap_or_else(Instant::now).duration_since(job.started).as_secs_f64();
                format!("{secs:.1} s")
            }
            JobState::Failed(_) | JobState::Cancelled => String::new(),
        };
        painter.text(egui::pos2(progress_r.right(), y + 8.0), egui::Align2::RIGHT_CENTER, right, small, palette.weak);
    }
    // its state, as a badge
    let label = match &job.state {
        JobState::Running if job.kind == Kind::Delete => t!("files-delete"),
        JobState::Running => format!("{}%", (fraction * 100.0) as u32),
        JobState::Paused => t!("files-job-paused-short"),
        JobState::Done => t!("files-job-done"),
        JobState::Failed(_) => t!("files-job-failed"),
        JobState::Cancelled => t!("files-job-cancelled-short"),
    };
    let mut badge_ui =
        ui.new_child(egui::UiBuilder::new().max_rect(state_r).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let badge = native_term_skin::badge(&mut badge_ui, &label, state_fill, state_text);
    if let JobState::Failed(e) = &job.state {
        badge.on_hover_text(e);
    }
    // its buttons
    let mut buttons = ui
        .new_child(egui::UiBuilder::new().max_rect(buttons_r).layout(egui::Layout::right_to_left(egui::Align::Center)));
    buttons.spacing_mut().item_spacing.x = 2.0;
    let clear = icons::CLEAR.to_string();
    let (play, pause) = (icons::PLAY.to_string(), icons::PAUSE.to_string());
    match &job.state {
        JobState::Running | JobState::Paused => {
            if IconButton::new(&clear, t!("button-cancel")).small().danger().show(&mut buttons, palette).clicked() {
                action = Some(JobAction::Cancel);
            }
            if job.work.is_some() {
                let (glyph, hint, asked) = if matches!(job.state, JobState::Running) {
                    (&pause, t!("files-job-pause"), JobAction::Pause)
                } else {
                    (&play, t!("files-job-resume"), JobAction::Resume)
                };
                if IconButton::new(glyph, &hint).small().show(&mut buttons, palette).clicked() {
                    action = Some(asked);
                }
            }
        }
        JobState::Done | JobState::Failed(_) | JobState::Cancelled => {
            if IconButton::new(&clear, t!("files-job-remove")).small().show(&mut buttons, palette).clicked() {
                action = Some(JobAction::Remove);
            }
            if matches!(job.state, JobState::Failed(_))
                && job.work.is_some()
                && IconButton::new(&play, t!("files-job-resume-hint")).small().show(&mut buttons, palette).clicked()
            {
                action = Some(JobAction::Resume);
            }
        }
    }
    action
}
