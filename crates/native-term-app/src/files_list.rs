//! A side's file list in the files window, on the skin's `ListView`:
//! the design's columns (name, size, then type and date here, mode and
//! date on the server), the side's colour for what is chosen (the
//! accent here, green on the server), an icon tinted by the kind of file,
//! sorting by a column (folders first, as Explorer has them), the keys,
//! a menu, renaming in place, dragging out and dropping in.

use super::*;
use native_term_skin::{Cell, Column, GridView, Item, ListView, Selection, Sort, Width};

/// How a side shows its files: in columns, or only their names across
/// and down (for a folder of many).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Details,
    Names,
}

/// The switch between the views (at a side's status line's end): the
/// view asked for.
pub(super) fn view_switch(ui: &mut egui::Ui, palette: &native_term_skin::Palette, view: View) -> Option<View> {
    let mut asked = None;
    // (right to left: the last first)
    for (this, glyph, hint) in
        [(View::Names, icons::GRID, t!("files-view-names")), (View::Details, icons::LIST, t!("files-view-details"))]
    {
        let on = view == this;
        let glyph = glyph.to_string();
        let button = native_term_skin::IconButton::new(&glyph, &hint).on(on);
        if button.small().show(ui, palette).clicked() && !on {
            asked = Some(this);
        }
    }
    asked
}

/// The design's colours for a side and for kinds of files (its tokens
/// `--accent`, `--remote`, `--file-*`), dark and light.
pub(super) struct Look {
    pub local: egui::Color32,
    pub remote: egui::Color32,
    /// A chosen name on the local side (`--accent-ink`).
    pub local_ink: egui::Color32,
    pub folder: egui::Color32,
    pub code: egui::Color32,
    pub doc: egui::Color32,
    pub binary: egui::Color32,
}

impl Look {
    pub fn of(visuals: &egui::Visuals) -> Look {
        let tones = crate::looks::tones(visuals);
        let rgb = |c: u32| egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
        if visuals.dark_mode {
            Look {
                local: tones.accent,
                remote: rgb(0x34d399),
                local_ink: rgb(0xa9c7ff),
                folder: rgb(0xf5b544),
                code: rgb(0x34d399),
                doc: rgb(0x60a5fa),
                binary: rgb(0xc084fc),
            }
        } else {
            Look {
                local: tones.accent,
                remote: rgb(0x047857),
                local_ink: rgb(0x1d4ed8),
                folder: rgb(0xd97706),
                code: rgb(0x059669),
                doc: rgb(0x2563eb),
                binary: rgb(0x9333ea),
            }
        }
    }

    /// A side's colour.
    pub fn side(&self, remote: bool) -> egui::Color32 {
        if remote {
            self.remote
        } else {
            self.local
        }
    }
}

/// What kind of file a name says (for its icon's tint and its type).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Folder,
    Link,
    Code,
    Doc,
    Binary,
    Plain,
}

/// The extension of `name`, lowercase (none for a name starting with a dot).
fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_lowercase())
}

pub(super) fn kind(name: &str, dir: bool, link: bool) -> Kind {
    if dir {
        return Kind::Folder;
    }
    if link {
        return Kind::Link;
    }
    let lower = name.to_lowercase();
    // a kernel and its initial file system have no telling extension
    if lower.starts_with("vmlinuz") || lower.starts_with("initrd") || lower.starts_with("initramfs") {
        return Kind::Binary;
    }
    match extension(name).as_deref() {
        Some(
            "sh" | "bash" | "zsh" | "fish" | "ps1" | "py" | "rs" | "go" | "js" | "ts" | "jsx" | "tsx" | "c" | "h"
            | "cc" | "cpp" | "hpp" | "java" | "kt" | "rb" | "php" | "pl" | "lua" | "swift" | "cs" | "sql",
        ) => Kind::Code,
        Some(
            "md" | "txt" | "rst" | "pdf" | "doc" | "docx" | "odt" | "rtf" | "log" | "csv" | "html" | "htm" | "xls"
            | "xlsx" | "ppt" | "pptx",
        ) => Kind::Doc,
        Some(
            "exe" | "dll" | "so" | "bin" | "img" | "iso" | "qcow2" | "vmdk" | "gz" | "tgz" | "xz" | "bz2" | "zst"
            | "zip" | "7z" | "rar" | "tar" | "deb" | "rpm" | "apk" | "msi" | "o" | "a" | "jar" | "war",
        ) => Kind::Binary,
        _ => Kind::Plain,
    }
}

/// The local side's type column: the folder, a name for well-known
/// extensions, else the extension itself.
pub(super) fn type_text(name: &str, dir: bool) -> String {
    if dir {
        return t!("files-type-folder");
    }
    let Some(ext) = extension(name) else { return t!("files-type-file") };
    let known = match ext.as_str() {
        "md" => "Markdown",
        "sh" | "bash" | "zsh" => "Shell",
        "ps1" => "PowerShell",
        "py" => "Python",
        "rs" => "Rust",
        "go" => "Go",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "html" | "htm" => "HTML",
        "txt" => "Text",
        "log" => "Log",
        "pdf" => "PDF",
        "zip" => "ZIP",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "Image",
        _ => return ext.to_uppercase(),
    };
    known.to_string()
}

/// The date of a modified time (`YYYY-MM-DD`), all of it as a hint.
fn date(unix: u64) -> (String, String) {
    let full = native_term_os::time::local_date_time(unix);
    (full.chars().take(10).collect(), full)
}

/// The columns of a side.
/// The columns of a side that fit `width`, and which of all four each
/// is (name, size, mode or type, modified): a narrow list keeps the name
/// readable, so the mode (or type) goes first, then the date, then the
/// size.
pub(super) fn columns(remote: bool, width: f32) -> (Vec<Column>, Vec<usize>) {
    let all = [
        Column::new(t!("files-col-name"), Width::Rest(140.0)),
        Column::new(t!("files-col-size"), Width::Fixed(84.0)).at_end(),
        if remote {
            Column::new(t!("files-col-mode"), Width::Fixed(100.0))
        } else {
            Column::new(t!("files-col-type"), Width::Fixed(96.0))
        },
        Column::new(t!("files-col-modified"), Width::Fixed(104.0)),
    ];
    let wide = |c: &Column| match c.width {
        Width::Rest(least) | Width::Fixed(least) => least,
    };
    let mut kept: Vec<usize> = (0..all.len()).collect();
    for drop in [2, 3, 1] {
        if kept.iter().map(|&i| wide(&all[i])).sum::<f32>() <= width {
            break;
        }
        kept.retain(|&i| i != drop);
    }
    (kept.iter().map(|&i| all[i].clone()).collect(), kept)
}

/// Folders first, then by the column sorted by (names as people read
/// them: case left aside).
pub(super) fn order(lines: &[Line], sort: Sort, remote: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (&lines[a], &lines[b]);
        let folders = y.dir.cmp(&x.dir);
        let by = match sort.column {
            1 => x.size.cmp(&y.size),
            2 if remote => x.mode.cmp(&y.mode),
            2 => x.kind_text.to_lowercase().cmp(&y.kind_text.to_lowercase()),
            3 => x.modified.cmp(&y.modified),
            _ => std::cmp::Ordering::Equal,
        };
        let by = by.then_with(|| x.name.to_lowercase().cmp(&y.name.to_lowercase()));
        folders.then(if sort.descending { by.reverse() } else { by })
    });
    order
}

/// Which list: its id, the server's side or not, whether the keys go to it.
pub(super) struct Side<'a> {
    pub salt: &'a str,
    pub remote: bool,
    pub keyboard: bool,
}

/// A file list, in columns or names only: what is chosen, opened,
/// renamed, dragged and dropped; `lines` in the order shown.
pub(super) fn list(
    ui: &mut egui::Ui,
    side: Side<'_>,
    lines: &[Line],
    selected: &mut Selection<Vec<u8>>,
    renaming: &mut Option<(Vec<u8>, String)>,
    sort: &mut Sort,
    view: View,
) -> Listed {
    let Side { salt, remote, keyboard } = side;
    let palette = crate::looks::skin(ui.visuals()).palette;
    let look = Look::of(ui.visuals());
    let mono = egui::FontId::monospace(11.0);
    let name_font = egui::FontId::proportional(13.0);
    let chosen_font = native_term_skin::font(ui.ctx(), 13.0, native_term_skin::Weight::Medium);
    let accent = look.side(remote);
    let ink = if remote { look.remote } else { look.local_ink };

    let area = ui.available_rect_before_wrap();
    let response = ui.interact(area, ui.id().with((salt, "drop")), egui::Sense::hover());
    let dropped = response.dnd_release_payload::<Dragged>();
    let hovered_drop = response.dnd_hover_payload::<Dragged>().is_some_and(|d| d.from_remote != remote);
    let mut out = Listed {
        response,
        clicked: false,
        open: None,
        action: None,
        renamed: None,
        drag: false,
        drag_row: None,
        dropped,
    };

    let (columns, kept) = columns(remote, ui.available_width());
    // the sort is by one of all four; the view knows those it shows
    let shown_sort = kept.iter().position(|&i| i == sort.column).map(|column| Sort { column, ..*sort });
    let keys: Vec<Vec<u8>> = lines.iter().map(|l| l.key.clone()).collect();
    let many = selected.len() > 1;
    let mut renamed = None;
    // a name with its icon, tinted by kind (in a row or an item)
    let name_cell = |ui: &mut egui::Ui,
                     line: &Line,
                     chosen: bool,
                     renaming: &mut Option<(Vec<u8>, String)>,
                     renamed: &mut Option<(Vec<u8>, String)>| {
        let tint = match line.kind {
            Kind::Folder => look.folder,
            Kind::Code => look.code,
            Kind::Doc => look.doc,
            Kind::Binary => look.binary,
            Kind::Link | Kind::Plain => palette.weak,
        };
        ui.label(egui::RichText::new(line.glyph.to_string()).size(14.0).color(tint));
        ui.add_space(4.0);
        if renaming.as_ref().is_some_and(|(k, _)| *k == line.key) {
            rename_field(ui, salt, line, renaming, renamed);
        } else {
            let (font, color) = if chosen { (chosen_font.clone(), ink) } else { (name_font.clone(), palette.text) };
            ui.add(
                egui::Label::new(egui::RichText::new(&line.name).font(font).color(color)).truncate().selectable(false),
            );
        }
    };
    let mut each = |i: usize, is_chosen: bool, response: &egui::Response| {
        let line = &lines[i];
        response
            .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_chosen, &line.name));
        if response.clicked() || response.secondary_clicked() {
            out.clicked = true;
        }
        if response.drag_started() {
            out.drag = true;
            out.response = response.clone();
            out.drag_row = Some(i);
        }
        response.context_menu(|ui| menu(ui, i, line, remote, many, &mut out));
    };
    let keyboard = keyboard && renaming.is_none();
    let shown = if view == View::Names {
        GridView::new(salt, &palette).accent(accent).keyboard(keyboard).show(
            ui,
            &keys,
            selected,
            |i| lines[i].name.clone(),
            |ui, Item { index, chosen }| name_cell(ui, &lines[index], chosen, renaming, &mut renamed),
            &mut each,
        )
    } else {
        ListView::new(salt, &columns, &palette).accent(accent).sorted(shown_sort).keyboard(keyboard).show(
            ui,
            &keys,
            selected,
            |i| lines[i].name.clone(),
            |ui, Cell { row, column, chosen }| {
                let line = &lines[row];
                match kept[column] {
                    0 => name_cell(ui, line, chosen, renaming, &mut renamed),
                    1 => {
                        if let Some(size) = line.size {
                            ui.label(egui::RichText::new(size_text(size)).font(mono.clone()).color(palette.weak));
                        } else {
                            ui.label(
                                egui::RichText::new("—").font(mono.clone()).color(palette.weak.gamma_multiply(0.6)),
                            );
                        }
                    }
                    2 if remote => {
                        if let Some(mode) = &line.mode {
                            ui.label(egui::RichText::new(mode).font(mono.clone()).color(palette.weak));
                        }
                    }
                    2 => {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&line.kind_text).size(12.0).color(palette.weak))
                                .truncate(),
                        );
                    }
                    _ => {
                        if let Some(m) = line.modified {
                            let (day, full) = date(m);
                            ui.label(egui::RichText::new(day).font(mono.clone()).color(palette.weak))
                                .on_hover_text(full);
                        }
                    }
                }
            },
            &mut each,
        )
    };
    if hovered_drop {
        ui.painter().rect_stroke(area, 4.0, egui::Stroke::new(2.0_f32, accent), egui::StrokeKind::Inside);
    }
    if let Some(s) = shown.sort {
        *sort = Sort { column: kept[s.column], ..s };
    }
    if shown.open.is_some() {
        out.open = shown.open;
    }
    out.renamed = renamed;
    out.clicked |= shown.changed;
    // the rows' own places, for what acts on them
    out.open = out.open.map(|i| lines[i].index);
    out.action = out.action.map(|(i, action)| (lines[i].index, action));
    // a row dragged that wasn't chosen: it alone is what goes
    if let Some(i) = out.drag_row.filter(|&i| !selected.contains(&keys[i])) {
        selected.only(keys[i].clone(), i);
    }
    out
}

fn rename_field(
    ui: &mut egui::Ui,
    salt: &str,
    line: &Line,
    renaming: &mut Option<(Vec<u8>, String)>,
    renamed: &mut Option<(Vec<u8>, String)>,
) {
    let (_, text) = renaming.as_mut().expect("renaming");
    let edit = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
    // the focus once, when renaming starts (taken every frame, the field
    // would never give it up)
    let started = egui::Id::new((salt, "renaming"));
    let fresh = ui.data_mut(|d| {
        let fresh = d.get_temp::<Vec<u8>>(started).as_ref() != Some(&line.key);
        d.insert_temp(started, line.key.clone());
        fresh
    });
    if fresh {
        edit.request_focus();
    } else if edit.lost_focus() {
        // Enter or a click elsewhere renames; Esc doesn't
        let (key, text) = renaming.take().expect("renaming");
        ui.data_mut(|d| d.remove::<Vec<u8>>(started));
        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if !cancelled && !text.trim().is_empty() && text != line.name {
            *renamed = Some((key, text));
        }
    }
}

/// A row's menu.
fn menu(ui: &mut egui::Ui, i: usize, line: &Line, remote: bool, many: bool, out: &mut Listed) {
    let mut item = |ui: &mut egui::Ui, text: String, action: Action| {
        if ui.button(text).clicked() {
            out.action = Some((i, action));
            ui.close();
        }
    };
    item(ui, format!("{} {}", icons::OPEN, t!("files-open")), Action::Open);
    if remote {
        item(ui, format!("{} {}", icons::DOWNLOAD, t!("files-download-to-local")), Action::Transfer);
        if !many && !line.dir {
            item(ui, format!("{} {}", icons::EDIT, t!("files-edit")), Action::Edit);
        }
    } else {
        item(ui, format!("{} {}", icons::UPLOAD, t!("files-upload-to-remote")), Action::Transfer);
    }
    ui.separator();
    if !many {
        item(ui, format!("{} {}", icons::RENAME, t!("files-rename")), Action::Rename);
    }
    item(ui, t!("files-copy-path"), Action::CopyPath);
    ui.separator();
    if ui.button(egui::RichText::new(format!("{} {}", icons::DELETE, t!("files-delete"))).color(RED)).clicked() {
        out.action = Some((i, Action::Delete));
        ui.close();
    }
}
