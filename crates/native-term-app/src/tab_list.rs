//! All tabs in all Terminal windows, NativeTerm's and the user's own, with
//! their full titles (see `docs/ARCHITECTURE.md`, "Tab switcher"), as a
//! list or as pictures (each tab as it looked when last seen selected).
//! Clicking one only switches to it.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use native_term_app::{fuzzy, t, Core, Preview, Screen, SessionView, State};
use native_term_platform::Snapshot;

use crate::icons;

/// Titles change without a notification: while the list is on screen and
/// NativeTerm has the focus, look again this often.
const RESCAN: Duration = Duration::from_secs(5);
/// How often a tab without a picture is asked what is on its screen.
const SCREEN_EVERY: Duration = Duration::from_secs(5);

/// `state.db` setting: `pictures` for the pictures view.
const VIEW_SETTING: &str = "tabs.view";

/// A picture's size in the pictures view (points).
const CARD: egui::Vec2 = egui::vec2(240.0, 135.0);

#[derive(Default)]
pub struct TabList {
    query: String,
    focus_search: bool,
    last_scan: Option<Instant>,
    /// The view, once read from `state.db`: pictures, or the list.
    pictures: Option<bool>,
    /// Pictures on the GPU, by window and tab: when taken, and the texture.
    textures: HashMap<(isize, usize), (SystemTime, egui::TextureHandle)>,
}

/// One tab, ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub window: isize,
    pub window_number: Option<usize>,
    pub index: usize,
    pub title: String,
    pub selected: bool,
    pub foreground_window: bool,
    /// NativeTerm's session in this tab.
    pub session: Option<(String, State)>,
    /// That session's id, to ask it things (its console screen).
    pub session_id: Option<String>,
}

/// The tabs of `snapshot`, with their sessions, filtered by `query` (best
/// first), or in window and tab order without one.
pub fn entries(
    snapshot: &Snapshot,
    sessions: &[SessionView],
    window_number: impl Fn(isize) -> Option<usize>,
    query: &str,
) -> Vec<Entry> {
    let by_place: HashMap<(isize, usize), &SessionView> = sessions
        .iter()
        .filter(|s| s.state.is_open())
        .filter_map(|s| s.location.as_ref().map(|l| ((l.window, l.tab_index), s)))
        .collect();
    let mut all: Vec<Entry> = snapshot
        .windows
        .iter()
        .flat_map(|w| {
            let by_place = &by_place;
            let number = window_number(w.handle);
            w.tabs.iter().map(move |tab| Entry {
                window: w.handle,
                window_number: number,
                index: tab.index,
                title: tab.name.clone(),
                selected: tab.selected,
                foreground_window: w.foreground,
                session: by_place.get(&(w.handle, tab.index)).map(|s| (s.label.clone(), s.state.clone())),
                session_id: by_place.get(&(w.handle, tab.index)).map(|s| s.id.clone()),
            })
        })
        .collect();
    all.sort_by_key(|e| (e.window_number.unwrap_or(usize::MAX), e.index));
    let query = query.trim();
    if query.is_empty() {
        return all;
    }
    let mut scored: Vec<(i64, Entry)> = all
        .into_iter()
        .filter_map(|e| {
            let label = e.session.as_ref().map(|(l, _)| l.as_str()).unwrap_or("");
            fuzzy::score(query, &[&e.title, label]).map(|s| (s, e))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, e)| e).collect()
}

impl TabList {
    pub fn focus_search(&mut self) {
        self.focus_search = true;
    }

    /// The tab's picture, as a texture.
    fn texture(&mut self, ctx: &egui::Context, core: &Core, e: &Entry) -> Option<(Preview, egui::TextureHandle)> {
        let preview = core.preview(e.window, e.index)?;
        let key = (e.window, e.index);
        match self.textures.get(&key) {
            Some((taken, texture)) if *taken == preview.taken => Some((preview, texture.clone())),
            _ => {
                let image = &preview.image;
                let size = [image.width as usize, image.height as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &image.rgba);
                let name = format!("tab-{}-{}", e.window, e.index);
                let texture = ctx.load_texture(name, pixels, egui::TextureOptions::LINEAR);
                self.textures.insert(key, (preview.taken, texture.clone()));
                Some((preview, texture))
            }
        }
    }

    /// Tabs Terminal has never rendered have no picture; their own shim
    /// can still say what is on them (`Core::ask_screen`). Asked for only
    /// while the pictures are shown, and at most every `SCREEN_EVERY`.
    fn ask_screens(&self, core: &Core, entries: &[Entry]) {
        for e in entries {
            let Some(id) = &e.session_id else { continue };
            if core.preview(e.window, e.index).is_none() {
                core.ask_screen(id, SCREEN_EVERY);
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, core: &Core) {
        core.want_all_tabs(true);
        let pictures = *self.pictures.get_or_insert_with(|| core.setting(VIEW_SETTING).as_deref() == Some("pictures"));
        let now = Instant::now();
        let focused = ui.input(|i| i.viewport().focused.unwrap_or(false));
        if focused && self.last_scan.is_none_or(|t| now - t >= RESCAN) {
            self.last_scan = Some(now);
            core.rescan();
        }
        // in the background, Terminal's notifications repaint it
        if focused {
            ui.ctx().request_repaint_after(RESCAN);
        }

        let mut clear = false;
        let search = ui
            .horizontal(|ui| {
                ui.label(icons::SEARCH.to_string());
                let clear_width = ui.spacing().interact_size.y + ui.spacing().item_spacing.x + 4.0;
                let view_width = 2.0 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.x) + 8.0;
                let width = ui.available_width() - view_width - if self.query.is_empty() { 0.0 } else { clear_width };
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.query).hint_text(t!("tabs-search-hint")).desired_width(width),
                );
                if !self.query.is_empty() && ui.small_button(icons::CLEAR.to_string()).clicked() {
                    clear = true;
                }
                ui.add_space(8.0);
                for (value, glyph, hint) in
                    [(false, icons::LIST, t!("tabs-view-list")), (true, icons::GRID, t!("tabs-view-pictures"))]
                {
                    let button = egui::Button::new(glyph.to_string()).selected(pictures == value);
                    if ui.add(button).on_hover_text(hint).clicked() && pictures != value {
                        self.pictures = Some(value);
                        core.set_setting(VIEW_SETTING, if value { "pictures" } else { "list" });
                    }
                }
                search
            })
            .inner;
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
        }
        if clear || ((search.has_focus() || search.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.query.clear();
        }
        let sessions = core.sessions();
        let list = entries(&core.snapshot(), &sessions, |h| core.window_number(h), &self.query);
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(e) = list.first() {
                core.select_tab(e.window, e.index, &e.title);
            }
        }
        ui.add_space(4.0);
        if list.is_empty() {
            ui.weak(if self.query.trim().is_empty() { t!("tabs-none") } else { t!("tabs-no-match") });
            return;
        }
        let searching = !self.query.trim().is_empty();
        if pictures && focused {
            self.ask_screens(core, &list);
        }
        let row_height = ui.spacing().interact_size.y + 6.0;
        // a picture no tab has any more
        self.textures.retain(|(w, i), _| list.iter().any(|e| e.window == *w && e.index == *i));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // the tabs of one window at a time (all together while searching)
            let mut groups: Vec<Vec<&Entry>> = Vec::new();
            for e in &list {
                match groups.last_mut() {
                    Some(group) if searching || group[0].window == e.window => group.push(e),
                    _ => groups.push(vec![e]),
                }
            }
            for group in groups {
                if !searching {
                    let number = group[0].window_number.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
                    ui.add_space(4.0);
                    ui.weak(t!("tabs-window", number = number, count = group.len()));
                }
                if pictures {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                        for e in group {
                            let picture = self.texture(ui.ctx(), core, e);
                            let screen = picture.is_none().then(|| core.screen(e.session_id.as_ref()?)).flatten();
                            if tab_card(ui, e, searching, picture, screen).clicked() {
                                core.select_tab(e.window, e.index, &e.title);
                            }
                        }
                    });
                } else {
                    for e in group {
                        let response = tab_row(ui, row_height, e, searching);
                        let response = response.on_hover_ui(|ui| match self.texture(ui.ctx(), core, e) {
                            Some((preview, texture)) => {
                                ui.add(egui::Image::new(&texture).max_width(CARD.x * 1.5));
                                ui.weak(taken_text(&preview));
                            }
                            None => {
                                ui.weak(t!("tabs-not-seen"));
                            }
                        });
                        if response.clicked() {
                            core.select_tab(e.window, e.index, &e.title);
                        }
                    }
                }
            }
        });
    }
}

/// "As of 14:05" for a picture.
fn taken_text(preview: &Preview) -> String {
    let unix = preview.taken.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    t!("tabs-seen-at", time = native_term_win::local_time_of_day(unix))
}

/// The name a tab is shown with: NativeTerm's label, then its title.
fn shown_name(e: &Entry) -> String {
    match &e.session {
        Some((label, _)) if *label != e.title => format!("{label} — {}", e.title),
        _ => e.title.clone(),
    }
}

/// A tab's console screen drawn in the card's frame: monospace, sized so
/// that the widest line fits, clipped to the frame. It is not the tab's
/// own font and colors, but it says what is on it — which a tab Terminal
/// has never rendered cannot show in any other way.
fn draw_screen(painter: &egui::Painter, frame: egui::Rect, screen: &Screen, color: egui::Color32) {
    const PAD: f32 = 4.0;
    // egui's monospace is about this wide for its height
    const RATIO: f32 = 0.5;
    let columns = f32::from(screen.columns.max(20));
    let size = ((frame.width() - PAD * 2.0) / (columns * RATIO)).clamp(3.0, 9.0);
    let font = egui::FontId::monospace(size);
    let painter = painter.with_clip_rect(frame.shrink(1.0));
    let mut y = frame.top() + PAD;
    for line in &screen.lines {
        if y > frame.bottom() {
            break;
        }
        if !line.is_empty() {
            let galley = painter.layout_no_wrap(line.clone(), font.clone(), color);
            painter.galley(egui::pos2(frame.left() + PAD, y), galley, color);
        }
        y += size * 1.25;
    }
}

/// One tab in the pictures view: its picture (or what its console says,
/// or a note that it hasn't been seen), then its name and where it is.
fn tab_card(
    ui: &mut egui::Ui,
    e: &Entry,
    searching: bool,
    picture: Option<(Preview, egui::TextureHandle)>,
    screen: Option<Screen>,
) -> egui::Response {
    let body_height = ui.text_style_height(&egui::TextStyle::Body);
    let text_height = body_height + ui.text_style_height(&egui::TextStyle::Small);
    let size = egui::vec2(CARD.x + 12.0, CARD.y + text_height + 20.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let name = shown_name(e);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, e.selected, &name));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let current = e.selected && e.foreground_window;
    let visuals = ui.style().interact_selectable(&response, current);
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, visuals.corner_radius, ui.visuals().faint_bg_color);
    if response.hovered() || current {
        painter.rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    }
    if current {
        let stroke = egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color);
        painter.rect_stroke(rect, visuals.corner_radius, stroke, egui::StrokeKind::Inside);
    }
    let frame = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 6.0), CARD);
    let weak = ui.visuals().weak_text_color();
    let small = egui::TextStyle::Small.resolve(ui.style());
    match &picture {
        Some((_, texture)) => {
            // fitted into the frame, centered
            let s = texture.size_vec2();
            let scale = (CARD.x / s.x).min(CARD.y / s.y);
            let shown = egui::Rect::from_center_size(frame.center(), s * scale);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(texture.id(), shown, uv, egui::Color32::WHITE);
        }
        // no picture: what the tab's own console says is on it
        None => {
            painter.rect_stroke(frame, 2.0, egui::Stroke::new(1.0_f32, weak), egui::StrokeKind::Inside);
            let lines = screen.as_ref().filter(|s| !s.lines.is_empty());
            match lines {
                Some(screen) => draw_screen(&painter, frame, screen, ui.visuals().text_color()),
                None => {
                    let note = painter.layout(t!("tabs-not-seen"), small.clone(), weak, CARD.x - 20.0);
                    painter.galley(frame.center() - note.size() / 2.0, note, weak);
                }
            }
        }
    }
    // the name, with the session's state
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut x = frame.left();
    let y = frame.bottom() + 6.0 + body_height / 2.0;
    if let Some((_, state)) = &e.session {
        painter.circle_filled(egui::pos2(x + 4.0, y), 4.0, state_color(state));
        x += 14.0;
    }
    let text = painter.layout_no_wrap(name, font, visuals.text_color());
    painter.galley(egui::pos2(x, y - text.size().y / 2.0), text, visuals.text_color());
    // where it is, and when it was seen
    let mut place = Vec::new();
    if searching {
        let number = e.window_number.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
        let tab = e.index + 1;
        place.push(t!("session-location", window = number, tab = tab));
    }
    if e.selected {
        place.push(t!("session-selected"));
    }
    if let Some((preview, _)) = &picture {
        place.push(taken_text(preview));
    } else if screen.is_some_and(|s| !s.lines.is_empty()) {
        // not a picture of it: what its own console says is on it
        place.push(t!("tabs-text-preview"));
    }
    let galley = painter.layout_no_wrap(place.join(" · "), small, weak);
    painter.galley(egui::pos2(frame.left(), y + body_height / 2.0 + 2.0), galley, weak);
    response
}

fn state_color(state: &State) -> egui::Color32 {
    match state {
        State::Connected => egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
        State::LoginFailed(_) | State::Unreachable(_) | State::Disconnected(_) | State::Failed(_) => {
            egui::Color32::from_rgb(0xd0, 0x3a, 0x3a)
        }
        _ => egui::Color32::from_rgb(0xd0, 0x9a, 0x1a),
    }
}

fn tab_row(ui: &mut egui::Ui, height: f32, e: &Entry, searching: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let name = shown_name(e);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, e.selected, &name));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.style().interact_selectable(&response, e.selected && e.foreground_window);
    if response.hovered() || (e.selected && e.foreground_window) {
        ui.painter().rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    }
    let painter = ui.painter().with_clip_rect(rect);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let weak = ui.visuals().weak_text_color();
    let mut x = rect.left() + 8.0;
    let y = rect.center().y;
    // what the tab is
    let glyph = if e.session.is_some() { icons::HOST } else { icons::TAB };
    let icon = painter.layout_no_wrap(glyph.to_string(), font.clone(), weak);
    painter.galley(egui::pos2(x, y - icon.size().y / 2.0), icon, weak);
    x += 24.0;
    if let Some((_, state)) = &e.session {
        painter.circle_filled(egui::pos2(x + 3.0, y), 4.0, state_color(state));
        x += 14.0;
    }
    let color = visuals.text_color();
    let text = painter.layout_no_wrap(name, font.clone(), color);
    let text_width = text.size().x;
    painter.galley(egui::pos2(x, y - text.size().y / 2.0), text, color);
    x += text_width + 12.0;
    // where it is
    let mut place = Vec::new();
    if searching {
        let number = e.window_number.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
        let tab = e.index + 1;
        place.push(t!("session-location", window = number, tab = tab));
    }
    if e.selected {
        place.push(t!("session-selected"));
    }
    if let Some((_, state)) = &e.session {
        place.push(state.describe());
    }
    if !place.is_empty() {
        let small = egui::TextStyle::Small.resolve(ui.style());
        let galley = painter.layout_no_wrap(place.join(" · "), small, weak);
        painter.galley(egui::pos2(x, y - galley.size().y / 2.0), galley, weak);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_term_app::Location;
    use native_term_platform::{TabView, WindowView};

    fn tab(index: usize, name: &str, selected: bool) -> TabView {
        TabView { index, name: name.into(), selected, rect: None, claim: None }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            windows: vec![
                WindowView {
                    handle: 20,
                    pid: 1,
                    foreground: false,
                    unresponsive: false,
                    tabs: vec![tab(0, "claude", true)],
                },
                WindowView {
                    handle: 10,
                    pid: 1,
                    foreground: true,
                    unresponsive: false,
                    tabs: vec![tab(0, "pwsh", false), tab(1, "web01", true), tab(2, "vim notes.md", false)],
                },
            ],
            complete: true,
        }
    }

    fn session(label: &str, window: isize, tab_index: usize, state: State) -> SessionView {
        SessionView {
            id: label.into(),
            label: label.into(),
            alias: label.into(),
            state,
            shim_pid: None,
            attempt: 1,
            linked: true,
            location: Some(Location {
                window_number: 1,
                window,
                tab_index,
                title: label.into(),
                selected: false,
                mixed: false,
            }),
            auto_retry: None,
            locked: false,
            renamed_to: None,
            last_position: None,
            quiet_since: None,
            specials: Vec::new(),
        }
    }

    #[test]
    fn all_tabs_in_window_order() {
        let sessions = [session("web01", 10, 1, State::Connected)];
        let number = |h: isize| Some(if h == 10 { 1 } else { 2 });
        let list = entries(&snapshot(), &sessions, number, "");
        let titles: Vec<&str> = list.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["pwsh", "web01", "vim notes.md", "claude"]);
        assert_eq!(list[1].session, Some(("web01".into(), State::Connected)));
        assert!(list[0].session.is_none(), "the user's own tab");
    }

    #[test]
    fn search_ranks_and_filters() {
        let number = |h: isize| Some(if h == 10 { 1 } else { 2 });
        let list = entries(&snapshot(), &[], number, "note");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "vim notes.md");
        assert!(entries(&snapshot(), &[], number, "zzz").is_empty());
    }
}
