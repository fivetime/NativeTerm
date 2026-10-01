//! The main window's dialogs for editing sessions, a file each. Each is
//! a modal window of its own (see `app_host`) and returns what the person
//! decided (`Outcome`); the app does it and shows an error back in the
//! dialog, which then stays open.

use native_term_app::t;

pub enum Outcome<T> {
    Open,
    Cancel,
    Submit(T),
}

/// The tags there are (the database's), for the dialogs that give a
/// host its tags: the main window says them when they change.
static KNOWN_TAGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn set_known_tags(tags: &[String]) {
    *KNOWN_TAGS.lock().unwrap_or_else(|e| e.into_inner()) = tags.to_vec();
}

/// A host's tags: typed, separated by commas, or chosen among the tags
/// there are (under the field: a click puts one in, another takes it
/// out again). Where they are many, what is typed of one narrows them
/// to those that have it in them, and the one chosen takes its place.
pub fn tags_field(ui: &mut egui::Ui, tags: &mut String) {
    use native_term_app::registry::Note;
    const WIDTH: f32 = 280.0;
    ui.vertical(|ui| {
        let field =
            ui.add(egui::TextEdit::singleline(tags).hint_text(t!("field-tags-hint-short")).desired_width(WIDTH));
        let known = KNOWN_TAGS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if known.is_empty() {
            return;
        }
        const BETWEEN: f32 = 4.0;
        const LINES: usize = 2;
        let tones = crate::looks::tones(ui.visuals());
        let has = Note::tags_from(tags);
        let offered = Note::offered(tags, &known);
        let mut chosen = None;
        // (two lines of them, the others scrolled to or typed the beginning
        // of; as high as that however many there are, so that nothing
        // under them moves while a tag is typed)
        let most = LINES as f32 * crate::layout::CHIP + (LINES - 1) as f32 * BETWEEN;
        ui.allocate_ui(egui::vec2(WIDTH, most), |ui| {
            egui::ScrollArea::vertical().id_salt("known-tags").auto_shrink(false).max_height(most).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(BETWEEN, BETWEEN);
                    for tag in offered {
                        let on = has.iter().any(|t| t.eq_ignore_ascii_case(tag));
                        let chip = crate::layout::chip(ui, &tones, tag, on);
                        if chip.on_hover_text(t!("field-tags-choose")).clicked() {
                            chosen = Some((tag.clone(), !on));
                        }
                    }
                });
            });
        });
        if let Some((tag, on)) = chosen {
            *tags = if on { Note::line_choosing(tags, &tag) } else { Note::line_with(tags, &tag, false) };
            // (typing goes on after it)
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field.id) {
                let end = egui::text::CCursor::new(tags.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ui.ctx(), field.id);
            }
            field.request_focus();
        }
    });
}

mod close_mixed;
mod delete;
mod drop;
mod folder;
mod forget;
mod host;
mod tag;

pub use close_mixed::ConfirmCloseMixed;
pub use delete::ConfirmDelete;
pub use drop::{DropChoice, DropDialog};
pub use folder::FolderDialog;
pub use forget::ConfirmForget;
pub use host::HostDialog;
pub use tag::TagDialog;

/// A preset's or a hex color's swatch and name.
pub fn color_text(value: &str) -> egui::RichText {
    use native_term_config::appearance::{tab_color, PRESETS};
    let name = PRESETS.iter().find(|(n, _)| n.eq_ignore_ascii_case(value)).map(|(n, _)| match *n {
        "red" => t!("color-red"),
        "orange" => t!("color-orange"),
        "yellow" => t!("color-yellow"),
        "green" => t!("color-green"),
        "blue" => t!("color-blue"),
        _ => t!("color-purple"),
    });
    let swatch = tab_color(value).and_then(|hex| egui::Color32::from_hex(&hex).ok()).unwrap_or(egui::Color32::GRAY);
    egui::RichText::new(format!("■ {}", name.unwrap_or_else(|| value.to_string()))).color(swatch)
}

/// A password field keeps the input method off while it has the focus, as
/// Windows' own password boxes do: an IME in Chinese mode would otherwise
/// turn the typed letters into candidates (seen with Sogou pinyin).
/// eframe allows the IME exactly when the frame's output asks for it.
pub fn no_ime(field: &egui::Response) {
    if field.has_focus() {
        field.ctx.output_mut(|o| o.ime = None);
    }
}
