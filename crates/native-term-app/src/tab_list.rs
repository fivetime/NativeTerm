//! All tabs in all Terminal windows, NativeTerm's and the user's own, with
//! their full titles (see `docs/ARCHITECTURE.md`, "Tab switcher").
//! Clicking one only switches to it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use native_term_app::{fuzzy, t, Core, SessionView, State};
use native_term_platform::Snapshot;

use crate::icons;

/// Titles change without a notification: while the list is on screen and
/// NativeTerm has the focus, look again this often.
const RESCAN: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct TabList {
    query: String,
    focus_search: bool,
    last_scan: Option<Instant>,
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

    pub fn show(&mut self, ui: &mut egui::Ui, core: &Core) {
        core.want_all_tabs(true);
        let now = Instant::now();
        let focused = ui.input(|i| i.viewport().focused.unwrap_or(false));
        if focused && self.last_scan.is_none_or(|t| now - t >= RESCAN) {
            self.last_scan = Some(now);
            core.rescan();
        }
        ui.ctx().request_repaint_after(RESCAN);

        let mut clear = false;
        let search = ui
            .horizontal(|ui| {
                ui.label(icons::SEARCH.to_string());
                let clear_width = ui.spacing().interact_size.y + ui.spacing().item_spacing.x + 4.0;
                let width = ui.available_width() - if self.query.is_empty() { 0.0 } else { clear_width };
                let search =
                    ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(t!("tabs-search-hint")).desired_width(width));
                if !self.query.is_empty() && ui.small_button(icons::CLEAR.to_string()).clicked() {
                    clear = true;
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
        let row_height = ui.spacing().interact_size.y + 6.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut last_window = None;
            for e in &list {
                if !searching && last_window != Some(e.window) {
                    last_window = Some(e.window);
                    let count = list.iter().filter(|x| x.window == e.window).count();
                    let number = e.window_number.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
                    ui.add_space(4.0);
                    ui.weak(t!("tabs-window", number = number, count = count));
                }
                if tab_row(ui, row_height, e, searching).clicked() {
                    core.select_tab(e.window, e.index, &e.title);
                }
            }
        });
    }
}

fn state_color(state: &State) -> egui::Color32 {
    match state {
        State::Connected => egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
        State::LoginFailed(_) | State::Disconnected(_) | State::Failed(_) => egui::Color32::from_rgb(0xd0, 0x3a, 0x3a),
        _ => egui::Color32::from_rgb(0xd0, 0x9a, 0x1a),
    }
}

fn tab_row(ui: &mut egui::Ui, height: f32, e: &Entry, searching: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let name = match &e.session {
        Some((label, _)) if *label != e.title => format!("{label} — {}", e.title),
        _ => e.title.clone(),
    };
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
                WindowView { handle: 20, pid: 1, foreground: false, unresponsive: false, tabs: vec![tab(0, "claude", true)] },
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
