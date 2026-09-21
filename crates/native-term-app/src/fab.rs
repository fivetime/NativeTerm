//! The floating button (see `docs/ARCHITECTURE.md`, "Floating action
//! button"): shown while the docked main window is hidden. A click opens
//! a small panel: quick connect and host search, the active session's
//! reconnect and clone, a command line for it, the tab list, closing
//! ended sessions, and the main window. Dragging moves it.
//!
//! The window carries its own transparency (`window.rs` shows it with
//! `native_term_win::layered`), so the button is a round, slightly
//! see-through disc rather than a square: the corners are not part of
//! the window at all, and a click there reaches whatever is behind it.

use std::sync::atomic::{AtomicBool, Ordering};

use native_term_app::actions::{close_set, CloseSet, Closing, SessionCommand};
use native_term_app::quick::{self, QuickTarget};
use native_term_app::tab_menu::MenuRequest;
use native_term_app::{fuzzy, t, Core, HostRequest, State};
use native_term_platform::Target;

use crate::icons;
use crate::send_line::SendLine;
use crate::shell::{self, HostEntry};

static EXPANDED: AtomicBool = AtomicBool::new(false);

/// The panel is open (the button stays visible even if the main window
/// comes back).
pub fn expanded() -> bool {
    EXPANDED.load(Ordering::Relaxed)
}

/// Collapsed size, in points: the window, which holds the disc and the
/// room its shadow needs.
pub const BUTTON: f32 = 52.0;
/// What the window keeps around the disc for the shadow.
const MARGIN: f32 = 4.0;
/// How solid the disc is when the pointer is elsewhere.
const RESTING: f32 = 0.86;
const PANEL: egui::Vec2 = egui::vec2(320.0, 400.0);

pub struct Fab {
    core: Option<Core>,
    query: String,
    open: bool,
    /// The collapsed button's rectangle (screen points) while the panel is
    /// open, to go back to.
    collapsed_at: Option<egui::Rect>,
    focus_search: bool,
    was_focused: bool,
    /// The command line for the active session: the sidebar's own, so it
    /// has the same history, targets and audit trail.
    send: SendLine,
    /// The look this window is already showing.
    look: Option<crate::looks::Preset>,
}

impl Fab {
    pub fn new(core: Option<Core>) -> Fab {
        Fab {
            core,
            query: String::new(),
            open: false,
            collapsed_at: None,
            focus_search: false,
            was_focused: false,
            send: SendLine::default(),
            look: None,
        }
    }

    fn expand(&mut self, ctx: &egui::Context) {
        let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else { return };
        // grow up and to the left from the button's bottom right corner,
        // but stay on the screen
        let mut min = outer.max - PANEL;
        if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
            min = min.max(egui::pos2(0.0, 0.0)).min((monitor - PANEL).to_pos2());
        }
        self.collapsed_at = Some(outer);
        self.open = true;
        self.focus_search = true;
        self.was_focused = false;
        EXPANDED.store(true, Ordering::Relaxed);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(PANEL));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(min));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn collapse(&mut self, ctx: &egui::Context) {
        self.open = false;
        self.query.clear();
        EXPANDED.store(false, Ordering::Relaxed);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(BUTTON, BUTTON)));
        if let Some(at) = self.collapsed_at.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(at.min));
        }
    }

    fn button(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("fab"), egui::Sense::click_and_drag());
        let accent = ui.visuals().selection.bg_fill;
        // a disc inside the window, with the margin left for its shadow;
        // solid under the pointer, a little see-through at rest
        let hovered = response.hovered();
        let radius = (rect.width().min(rect.height()) / 2.0 - MARGIN).max(1.0);
        let disc = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(radius * 2.0));
        let fill = if hovered { accent } else { accent.gamma_multiply(RESTING) };
        let painter = ui.painter();
        let shadow = egui::epaint::Shadow {
            offset: [0, 1],
            blur: if hovered { 10 } else { 6 },
            spread: 0,
            color: egui::Color32::from_black_alpha(if hovered { 90 } else { 60 }),
        };
        painter.add(shadow.as_shape(disc, egui::CornerRadius::same(radius.min(255.0) as u8)));
        painter.circle_filled(disc.center(), radius, fill);
        painter.text(
            disc.center(),
            egui::Align2::CENTER_CENTER,
            icons::TABS.to_string(),
            egui::FontId::proportional(radius * 0.84),
            ui.visuals().selection.stroke.color,
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t!("fab-open")));
        let response = response.on_hover_text(t!("fab-open"));
        // egui reports a drag only once the pointer moved a little; the
        // button is still held, so Windows can take over the move
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        } else if response.clicked() {
            self.expand(ui.ctx());
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if focused {
            self.was_focused = true;
        } else if self.was_focused {
            // clicked elsewhere
            self.collapse(&ctx);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.collapse(&ctx);
            return;
        }
        let Some(core) = self.core.clone() else { return };
        let mut close = false;
        let inside = egui::Frame::new().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(icons::with(icons::TABS, "NativeTerm"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(icons::CLEAR.to_string()).on_hover_text(t!("fab-close")).clicked() {
                        close = true;
                    }
                });
            });
            ui.separator();

            // quick connect and host search
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(t!("fab-search-hint"))
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut self.focus_search) {
                search.request_focus();
            }
            let typed = quick::parse(&self.query);
            let hits = matches(&shell::hosts(), &self.query);
            let enter = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if enter {
                if let Some(host) = hits.first() {
                    core.open(&[host.request()], Target::Recent);
                    close = true;
                } else if let Some(target) = &typed {
                    core.open(&[quick_request(target)], Target::Recent);
                    close = true;
                }
            }
            if let Some(target) = &typed {
                if ui.button(icons::with(icons::CONNECT, t!("quick-connect", target = target.label()))).clicked() {
                    core.open(&[quick_request(target)], Target::Recent);
                    close = true;
                }
            }
            for host in hits.iter().take(6) {
                let text = if host.folder.is_empty() {
                    host.label.clone()
                } else {
                    format!("{}   · {}", host.label, host.folder)
                };
                if ui.button(icons::with(icons::HOST, text)).on_hover_text(&host.hostname).clicked() {
                    core.open(&[host.request()], Target::Recent);
                    close = true;
                }
            }
            ui.separator();

            // the session in the Terminal tab the user came from
            match core.active_session() {
                Some(active) => {
                    ui.label(t!("fab-active", label = active.label.as_str()));
                    ui.horizontal(|ui| {
                        let connect =
                            if active.state == State::Waiting { t!("button-connect") } else { t!("button-reconnect") };
                        if ui
                            .add_enabled(SessionCommand::Connect.applies(&active), egui::Button::new(connect))
                            .clicked()
                        {
                            core.run(&active.id, SessionCommand::Connect);
                            close = true;
                        }
                        if ui
                            .add_enabled(SessionCommand::Clone.applies(&active), egui::Button::new(t!("tabmenu-clone")))
                            .clicked()
                        {
                            core.run(&active.id, SessionCommand::Clone);
                            close = true;
                        }
                    });
                }
                None => {
                    ui.weak(t!("fab-no-active"));
                }
            }
            // the sidebar's own command line: ↑ / ↓ for what was sent
            // before, a target to choose, and every send in the audit log
            self.send.show(ui, &core, &no_group_send());
            ui.separator();
            if ui.button(icons::with(icons::TABS, t!("view-tabs"))).clicked() {
                shell::show_tabs();
                close = true;
            }
            let ended = close_set(&core.sessions(), &CloseSet::Ended).len();
            if ui
                .add_enabled(ended > 0, egui::Button::new(icons::with(icons::CLEAR, t!("tabmenu-close-disconnected"))))
                .clicked()
            {
                // like the tab menu: a tab holding other panes is asked about first
                if let Closing::Confirm(ids) = core.close_sessions(&CloseSet::Ended) {
                    crate::shell::ask(MenuRequest::ConfirmClose(ids));
                }
                close = true;
            }
            if ui.button(icons::with(icons::OPEN, t!("fab-show-main"))).clicked() {
                shell::show_main();
                close = true;
            }
        });
        if close {
            self.collapse(&ctx);
            return;
        }
        // fit the window to what is in it, keeping the bottom edge in
        // place (what the panel itself took, not the room it was given)
        let wanted = inside.response.rect.height() + 4.0;
        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            if (outer.height() - wanted).abs() > 2.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(PANEL.x, wanted)));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                    outer.min.x,
                    outer.max.y - wanted,
                )));
            }
        }
    }
}

/// Hosts whose folder is marked "No group send" (published with the
/// hosts on every reload, so the button leaves out the same ones the
/// sidebar does).
fn no_group_send() -> std::collections::HashSet<String> {
    shell::hosts().into_iter().filter(|h| h.no_group_send).map(|h| h.alias).collect()
}

fn quick_request(target: &QuickTarget) -> HostRequest {
    HostRequest::new(target.destination(), target.label())
}

/// Saved hosts matching `query`, best first; none for an empty query.
pub fn matches(hosts: &[HostEntry], query: &str) -> Vec<HostEntry> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i64, &HostEntry)> = hosts
        .iter()
        .filter_map(|h| fuzzy::score(query, &[&h.label, &h.alias, &h.hostname, &h.folder]).map(|s| (s, h)))
        .collect();
    scored.sort_by_key(|(score, h)| (std::cmp::Reverse(*score), h.label.clone()));
    scored.into_iter().map(|(_, h)| h.clone()).collect()
}

impl crate::window::Ui for Fab {
    fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        // the main window keeps the chosen look; this one follows
        if let Some(look) = crate::looks::chosen() {
            if self.look != Some(look) {
                look.apply(ui.ctx());
                self.look = Some(look);
            }
        }
        if self.open {
            // the window is see-through where nothing is painted, so the
            // panel draws its own rounded face
            let visuals = ui.visuals();
            let frame = egui::Frame::new()
                .fill(visuals.window_fill)
                .stroke(visuals.window_stroke)
                .corner_radius(egui::CornerRadius::same(8));
            egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| self.panel(ui));
        } else {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| self.button(ui));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str, label: &str, folder: &str) -> HostEntry {
        HostEntry {
            alias: alias.into(),
            label: label.into(),
            hostname: format!("{alias}.example"),
            folder: folder.into(),
            on_login: None,
            no_group_send: false,
        }
    }

    #[test]
    fn hosts_left_out_of_a_group_send_come_from_the_host_list() {
        let mut quiet = host("db01", "数据库", "生产");
        quiet.no_group_send = true;
        shell::set_hosts(vec![host("web01", "web01", "生产"), quiet]);
        let left_out = no_group_send();
        assert!(left_out.contains("db01") && !left_out.contains("web01"));
        shell::set_hosts(Vec::new());
        assert!(no_group_send().is_empty());
    }

    #[test]
    fn host_search() {
        let hosts = [host("web01", "web01", "生产"), host("db01", "数据库", "生产"), host("lab", "lab", "")];
        assert!(matches(&hosts, "").is_empty());
        assert_eq!(matches(&hosts, "db")[0].alias, "db01");
        assert_eq!(matches(&hosts, "生产").len(), 2, "by folder");
        assert!(matches(&hosts, "zzz").is_empty());
    }
}
