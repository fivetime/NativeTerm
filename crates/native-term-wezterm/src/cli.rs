//! What `wezterm cli` says and is told: its `list --format json` output as
//! windows and tabs, and the argument lists of the commands the backend
//! runs. Pure, so it is tested without a WezTerm.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use native_term_platform::claim::{Pane, WindowTabs};
use native_term_platform::{TabSpec, WindowId};
use serde::Deserialize;

/// One pane as `wezterm cli list --format json` prints it (the fields
/// the backend reads; the rest are ignored).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ListedPane {
    pub window_id: u64,
    pub tab_id: u64,
    pub pane_id: u64,
    /// The pane's own title (what the program in it set).
    #[serde(default)]
    pub title: String,
    /// The tab's title when one was set (`set-tab-title`); empty otherwise.
    #[serde(default)]
    pub tab_title: String,
    /// The pane is the active one of its tab, and its tab the active one
    /// of its window.
    #[serde(default)]
    pub is_active: bool,
}

/// One tab: its id, its panes in order, and which is active.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedTab {
    pub tab_id: u64,
    pub panes: Vec<ListedPane>,
}

impl ListedTab {
    /// What the tab is called: its own title if one was set, else its
    /// active (or first) pane's.
    pub fn name(&self) -> String {
        let pane = self.panes.iter().find(|p| p.is_active).or(self.panes.first());
        match pane {
            Some(p) if !p.tab_title.is_empty() => p.tab_title.clone(),
            Some(p) => p.title.clone(),
            None => String::new(),
        }
    }

    /// The pane the tab shows (the active one, else the first).
    pub fn active_pane(&self) -> Option<u64> {
        self.panes.iter().find(|p| p.is_active).or(self.panes.first()).map(|p| p.pane_id)
    }

    pub fn is_active(&self) -> bool {
        self.panes.iter().any(|p| p.is_active)
    }
}

/// One window: its tabs in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedWindow {
    pub window_id: u64,
    pub tabs: Vec<ListedTab>,
}

impl ListedWindow {
    pub fn id(&self) -> WindowId {
        WindowId(self.window_id)
    }

    /// The window as the claimer reads a window: names in strip order, no
    /// rectangles, the active tab selected, and its panes.
    pub fn as_window_tabs(&self) -> WindowTabs {
        let selected = self.tabs.iter().position(ListedTab::is_active);
        let panes = selected
            .map(|s| {
                self.tabs[s].panes.iter().map(|p| Pane { title: p.title.clone(), profile: String::new() }).collect()
            })
            .unwrap_or_default();
        WindowTabs {
            names: self.tabs.iter().map(ListedTab::name).collect(),
            rects: vec![None; self.tabs.len()],
            selected,
            panes,
        }
    }

    /// The tab at `index`, if it is still called `name`.
    pub fn tab_named(&self, index: usize, name: &str) -> Option<&ListedTab> {
        self.tabs.get(index).filter(|t| t.name() == name)
    }
}

/// `list --format json`, as windows in the order WezTerm lists them.
pub fn parse_list(json: &str) -> Result<Vec<ListedWindow>, serde_json::Error> {
    let panes: Vec<ListedPane> = serde_json::from_str(json)?;
    Ok(group(panes))
}

fn group(panes: Vec<ListedPane>) -> Vec<ListedWindow> {
    let mut windows: Vec<ListedWindow> = Vec::new();
    let mut tabs: BTreeMap<(u64, u64), usize> = BTreeMap::new();
    for pane in panes {
        let w = match windows.iter().position(|w| w.window_id == pane.window_id) {
            Some(w) => w,
            None => {
                windows.push(ListedWindow { window_id: pane.window_id, tabs: Vec::new() });
                windows.len() - 1
            }
        };
        let t = match tabs.get(&(pane.window_id, pane.tab_id)) {
            Some(&t) => t,
            None => {
                windows[w].tabs.push(ListedTab { tab_id: pane.tab_id, panes: Vec::new() });
                let t = windows[w].tabs.len() - 1;
                tabs.insert((pane.window_id, pane.tab_id), t);
                t
            }
        };
        windows[w].tabs[t].panes.push(pane);
    }
    windows
}

/// The pane id `spawn` prints.
pub fn parse_spawned(stdout: &str) -> Option<u64> {
    stdout.trim().parse().ok()
}

/// The program a session tab runs: the shim with its arguments (see the
/// Windows profile's command line for the same list).
pub fn shim_program(shim: &Path, shim_args: &[OsString], tab: &TabSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![shim.into()];
    args.extend(shim_args.iter().cloned());
    args.extend(["--session".into(), tab.session.clone().into()]);
    if tab.wait {
        args.push("--wait".into());
    }
    if tab.no_forwards {
        args.push("--no-forwards".into());
    }
    args.push(tab.alias.clone().into());
    args
}

/// Where a spawn goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Into {
    NewWindow,
    Window(u64),
}

/// `cli spawn …` for `program`.
pub fn spawn_args(into: Into, program: &[OsString]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["cli".into(), "--no-auto-start".into(), "spawn".into()];
    match into {
        Into::NewWindow => args.push("--new-window".into()),
        Into::Window(id) => args.extend(["--window-id".into(), id.to_string().into()]),
    }
    args.push("--".into());
    args.extend(program.iter().cloned());
    args
}

pub fn list_args() -> Vec<OsString> {
    ["cli", "--no-auto-start", "list", "--format", "json"].iter().map(OsString::from).collect()
}

pub fn set_tab_title_args(pane_id: u64, title: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        ["cli", "--no-auto-start", "set-tab-title", "--pane-id"].iter().map(OsString::from).collect();
    args.extend([pane_id.to_string().into(), "--".into(), title.into()]);
    args
}

pub fn activate_tab_args(tab_id: u64) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        ["cli", "--no-auto-start", "activate-tab", "--tab-id"].iter().map(OsString::from).collect();
    args.push(tab_id.to_string().into());
    args
}

pub fn activate_pane_args(pane_id: u64) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        ["cli", "--no-auto-start", "activate-pane", "--pane-id"].iter().map(OsString::from).collect();
    args.push(pane_id.to_string().into());
    args
}

pub fn kill_pane_args(pane_id: u64) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        ["cli", "--no-auto-start", "kill-pane", "--pane-id"].iter().map(OsString::from).collect();
    args.push(pane_id.to_string().into());
    args
}

pub fn get_text_args(pane_id: u64) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        ["cli", "--no-auto-start", "get-text", "--pane-id"].iter().map(OsString::from).collect();
    args.push(pane_id.to_string().into());
    args
}

/// `send-text --no-paste`: the text goes in as typed (a bracketed paste
/// would wrap it, and a shell then waits for Enter inside the paste).
pub fn send_text_args(pane_id: u64, text: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        ["cli", "--no-auto-start", "send-text", "--no-paste", "--pane-id"].iter().map(OsString::from).collect();
    args.extend([pane_id.to_string().into(), "--".into(), text.into()]);
    args
}

/// `wezterm-gui start -- program`: the first window, when no WezTerm runs.
pub fn start_args(program: &[OsString]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["start".into(), "--".into()];
    args.extend(program.iter().cloned());
    args
}

/// The screen as `get-text` printed it: lines, trailing spaces and empty
/// lines at the end gone, at most `max_lines`.
pub fn screen_lines(text: &str, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(|l| l.trim_end().to_string()).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.truncate(max_lines);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
      {"window_id":0,"tab_id":0,"pane_id":0,"workspace":"default","size":{"rows":24,"cols":80,"pixel_width":640,"pixel_height":480,"dpi":96},"title":"bash","cwd":"file://box/home/x","cursor_x":0,"cursor_y":1,"cursor_shape":"Default","cursor_visibility":"Visible","left_col":0,"top_row":0,"tab_title":"web01","window_title":"wezterm","is_active":false,"is_zoomed":false,"tty_name":"/dev/pts/1"},
      {"window_id":0,"tab_id":1,"pane_id":1,"workspace":"default","size":{"rows":24,"cols":80,"pixel_width":640,"pixel_height":480,"dpi":96},"title":"vim notes.md","cwd":"file://box/home/x","cursor_x":0,"cursor_y":1,"cursor_shape":"Default","cursor_visibility":"Visible","left_col":0,"top_row":0,"tab_title":"","window_title":"wezterm","is_active":true,"is_zoomed":false,"tty_name":"/dev/pts/2"},
      {"window_id":0,"tab_id":1,"pane_id":3,"workspace":"default","size":{"rows":24,"cols":40,"pixel_width":320,"pixel_height":480,"dpi":96},"title":"db01","cwd":"file://box/home/x","cursor_x":0,"cursor_y":1,"cursor_shape":"Default","cursor_visibility":"Visible","left_col":40,"top_row":0,"tab_title":"","window_title":"wezterm","is_active":false,"is_zoomed":false,"tty_name":"/dev/pts/4"},
      {"window_id":2,"tab_id":5,"pane_id":7,"workspace":"default","size":{"rows":24,"cols":80,"pixel_width":640,"pixel_height":480,"dpi":96},"title":"zsh","cwd":"file://box/home/x","cursor_x":0,"cursor_y":1,"cursor_shape":"Default","cursor_visibility":"Visible","left_col":0,"top_row":0,"tab_title":"","window_title":"wezterm","is_active":true,"is_zoomed":false,"tty_name":"/dev/pts/5"}
    ]"#;

    #[test]
    fn panes_become_windows_and_tabs_in_order() {
        let windows = parse_list(LIST).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id(), WindowId(0));
        assert_eq!(windows[0].tabs.len(), 2);
        assert_eq!(windows[0].tabs[1].panes.len(), 2, "a split tab keeps both panes");
        assert_eq!(windows[1].tabs[0].tab_id, 5);
        let tabs = windows[0].as_window_tabs();
        assert_eq!(tabs.names, ["web01", "vim notes.md"], "a set title, else the pane's");
        assert_eq!(tabs.selected, Some(1));
        assert_eq!(tabs.rects, [None, None]);
        assert_eq!(tabs.panes.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), ["vim notes.md", "db01"]);
        assert_eq!(windows[0].tabs[1].active_pane(), Some(1));
        assert!(windows[0].tab_named(0, "web01").is_some());
        assert!(windows[0].tab_named(0, "moved").is_none());
        assert!(windows[0].tab_named(9, "web01").is_none());
    }

    #[test]
    fn unknown_fields_and_an_empty_list_are_fine() {
        assert!(parse_list("[]").unwrap().is_empty());
        let one = r#"[{"window_id":1,"tab_id":2,"pane_id":3,"new_field":{"a":1}}]"#;
        let windows = parse_list(one).unwrap();
        assert_eq!(windows[0].tabs[0].name(), "", "no titles at all");
        assert!(parse_list("not json").is_err());
        assert_eq!(parse_spawned(" 12\n"), Some(12));
        assert_eq!(parse_spawned("error: no"), None);
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_shim_gets_the_same_arguments_as_on_windows() {
        let tab = TabSpec {
            terminal_session: "guid".into(),
            label: "web01".into(),
            session: "id-1".into(),
            alias: "web01".into(),
            wait: true,
            no_forwards: true,
            tab_color: None,
        };
        let program = shim_program(Path::new("/opt/nt/nativeterm-shim"), &["--ssh-dir".into(), "/x".into()], &tab);
        assert_eq!(
            strings(&program),
            ["/opt/nt/nativeterm-shim", "--ssh-dir", "/x", "--session", "id-1", "--wait", "--no-forwards", "web01"]
        );
        assert_eq!(
            strings(&spawn_args(Into::Window(4), &program))[..6],
            ["cli", "--no-auto-start", "spawn", "--window-id", "4", "--"]
        );
        assert_eq!(strings(&spawn_args(Into::NewWindow, &program))[3], "--new-window");
        assert_eq!(strings(&start_args(&program))[..2], ["start", "--"]);
    }

    #[test]
    fn the_other_commands() {
        assert_eq!(strings(&list_args()), ["cli", "--no-auto-start", "list", "--format", "json"]);
        assert_eq!(strings(&set_tab_title_args(7, "-dash"))[4..], ["7", "--", "-dash"]);
        assert_eq!(strings(&activate_tab_args(5))[2..], ["activate-tab", "--tab-id", "5"]);
        assert_eq!(strings(&kill_pane_args(9))[2..], ["kill-pane", "--pane-id", "9"]);
        assert_eq!(strings(&get_text_args(9))[2..], ["get-text", "--pane-id", "9"]);
        assert_eq!(
            strings(&send_text_args(9, "ls\r"))[2..],
            ["send-text", "--no-paste", "--pane-id", "9", "--", "ls\r"]
        );
        assert_eq!(strings(&activate_pane_args(1))[2..], ["activate-pane", "--pane-id", "1"]);
    }

    #[test]
    fn screen_lines_are_trimmed() {
        assert_eq!(screen_lines("a  \nb\n\n   \n", 10), ["a", "b"]);
        assert_eq!(screen_lines("a\nb\nc\n", 2), ["a", "b"]);
        assert!(screen_lines("\n\n", 5).is_empty());
    }
}
