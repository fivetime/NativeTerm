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
    /// The pane is the active one of its tab (not: its tab the active one
    /// of its window — `list` doesn't say that; `list-clients` says which
    /// pane has the focus).
    #[serde(default)]
    pub is_active: bool,
}

/// One client as `wezterm cli list-clients --format json` prints it (the
/// GUI is one).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ListedClient {
    #[serde(default)]
    pub focused_pane_id: Option<u64>,
    /// Since the client's last input.
    #[serde(default)]
    pub idle_time: Option<Seconds>,
}

/// A duration as the client list prints it.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Seconds {
    #[serde(default)]
    pub secs: u64,
    #[serde(default)]
    pub nanos: u32,
}

/// A client's focus: the pane, and how long ago the person last gave
/// that client input. The GUI reports a focus only from its own events,
/// so a tab NativeTerm activated shows there only once the window has
/// had the focus; until the person acts, what NativeTerm chose is what
/// the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub pane: u64,
    pub since_input: std::time::Duration,
}

/// The panes the clients have the focus on (the GUI's, normally one).
pub fn parse_focus(json: &str) -> Result<Vec<Focus>, serde_json::Error> {
    let clients: Vec<ListedClient> = serde_json::from_str(json)?;
    Ok(clients
        .into_iter()
        .filter_map(|c| {
            let idle = c.idle_time.unwrap_or_default();
            Some(Focus { pane: c.focused_pane_id?, since_input: std::time::Duration::new(idle.secs, idle.nanos) })
        })
        .collect())
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

    pub fn has_pane(&self, pane_id: u64) -> bool {
        self.panes.iter().any(|p| p.pane_id == pane_id)
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

    /// The index of the tab holding `pane_id`.
    pub fn tab_of_pane(&self, pane_id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.has_pane(pane_id))
    }

    /// The index of the tab with id `tab_id`.
    pub fn tab_index(&self, tab_id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.tab_id == tab_id)
    }

    /// The window as the claimer reads a window: names in strip order, no
    /// rectangles, tab `selected` selected, and its panes.
    pub fn as_window_tabs(&self, selected: Option<usize>) -> WindowTabs {
        let selected = selected.filter(|s| *s < self.tabs.len());
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

pub fn list_clients_args() -> Vec<OsString> {
    ["cli", "--no-auto-start", "list-clients", "--format", "json"].iter().map(OsString::from).collect()
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
/// (A configuration goes in `WEZTERM_CONFIG_FILE`, never `--config-file`:
/// a GUI started with the flag publishes no discovery socket, and no
/// `wezterm cli` finds it.)
pub fn start_args(program: &[OsString]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["start".into(), "--".into()];
    args.extend(program.iter().cloned());
    args
}

/// Whether the person has a WezTerm configuration of their own
/// (`WEZTERM_CONFIG_FILE`, `~/.wezterm.lua`, `~/.config/wezterm/`): then
/// NativeTerm leaves the look to it.
pub fn user_config_exists() -> bool {
    if std::env::var_os("WEZTERM_CONFIG_FILE").is_some_and(|f| !f.is_empty()) {
        return true;
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from);
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    home.as_ref().is_some_and(|h| h.join(".wezterm.lua").is_file())
        || config_home.is_some_and(|c| c.join("wezterm").join("wezterm.lua").is_file())
}

/// The configuration NativeTerm's WezTerm windows use when the person has
/// none: the desktop's light or dark, no questions when a tab is closed
/// from outside, the tab bar always there.
pub fn default_config(look: &Look, shim: &Path) -> String {
    let mut lua = String::from(
        r#"-- Written by NativeTerm for the WezTerm windows it opens, and used only
-- while you have no WezTerm configuration of your own (~/.wezterm.lua or
-- ~/.config/wezterm/wezterm.lua): make one and this file is ignored.
-- NativeTerm rewrites it when the desktop's look or its theme setting
-- changes, and WezTerm reloads it on its own.
local wezterm = require("wezterm")
local config = wezterm.config_builder()

-- the desktop's look, as NativeTerm read it
"#,
    );
    let scheme = |dark: bool| if dark { "Builtin Tango Dark" } else { "Builtin Tango Light" };
    match look.dark {
        Some(dark) => lua.push_str(&format!("config.color_scheme = \"{}\"\n", scheme(dark))),
        None => lua.push_str(&format!(
            "config.color_scheme = wezterm.gui.get_appearance():find(\"Dark\") and \"{}\" or \"{}\"\n",
            scheme(true),
            scheme(false)
        )),
    }
    if let Some((r, g, b)) = look.accent {
        // the selected tab in the accent colour, its title in black or white
        let light = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000 > 140;
        let on_accent = if light { "#000000" } else { "#ffffff" };
        lua.push_str(&format!(
            "config.colors = {{ tab_bar = {{ active_tab = {{ bg_color = \"#{r:02x}{g:02x}{b:02x}\", fg_color = \"{on_accent}\" }} }} }}\n"
        ));
    }
    if let Some(font) = &look.font {
        // the desktop's monospace font first; WezTerm's own fallbacks follow
        lua.push_str(&format!("config.font = wezterm.font_with_fallback({{ \"{}\" }})\n", lua_escape(font)));
    }
    lua.push_str(
        r#"config.font_size = 11.0

-- NativeTerm closes tabs itself; the tab strip is where it looks
config.window_close_confirmation = "NeverPrompt"
config.hide_tab_bar_if_only_one_tab = false
config.use_fancy_tab_bar = true

"#,
    );
    lua.push_str(&MENU_LUA.replace("__SHIM__", &lua_escape(&shim.display().to_string())));
    if look.switcher {
        lua.push_str(SWITCHER_LUA);
    }
    lua.push_str("\nreturn config\n");
    lua
}

/// NativeTerm's tab menu, through WezTerm's own picker: a right click in
/// a tab (or Ctrl+Shift+M) runs the shim's `--tab-menu` for the pane,
/// which prints what applies to its session; the choice goes back the
/// same way. A tab that is not NativeTerm's gets no menu (the shim
/// prints nothing), and a right click while a program on the other side
/// takes the mouse goes to that program, as WezTerm always does.
const MENU_LUA: &str = r#"-- NativeTerm's tab menu: right-click in a tab, or Ctrl+Shift+M
local shim = "__SHIM__"
local function tab_menu(window, pane)
  local ok, out = wezterm.run_child_process({ shim, "--tab-menu", "--pane", tostring(pane:pane_id()) })
  if not ok then
    return
  end
  local title, choices = "NativeTerm", {}
  for line in out:gmatch("[^\r\n]+") do
    local id, text = line:match("^(%d+)\t(.*)$")
    if id == "0" then
      title = text
    elseif id then
      table.insert(choices, { id = id, label = text })
    end
  end
  if #choices == 0 then
    return
  end
  window:perform_action(
    wezterm.action.InputSelector({
      title = title,
      choices = choices,
      fuzzy = false,
      action = wezterm.action_callback(function(_, chosen_pane, id)
        if id then
          wezterm.run_child_process({ shim, "--tab-menu", id, "--pane", tostring(chosen_pane:pane_id()) })
        end
      end),
    }),
    pane
  )
end
config.keys = {
  { key = "m", mods = "CTRL|SHIFT", action = wezterm.action_callback(tab_menu) },
}
config.mouse_bindings = {
  { event = { Down = { streak = 1, button = "Right" } }, mods = "NONE", action = wezterm.action_callback(tab_menu) },
}
"#;

/// Ctrl+Tab shows WezTerm's tab navigator (its list of tabs), as
/// NativeTerm's grid does on Windows; only when the person turned the
/// switcher on, since it takes WezTerm's own next-tab key.
const SWITCHER_LUA: &str = r#"-- Ctrl+Tab: the tab navigator (NativeTerm's "tab switcher" setting)
table.insert(config.keys, { key = "Tab", mods = "CTRL", action = wezterm.action.ShowTabNavigator })
"#;

/// How the windows NativeTerm opens should look: what it read from the
/// desktop, overridden by its own theme setting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    /// Dark or light; `None` lets WezTerm ask the desktop itself.
    pub dark: Option<bool>,
    /// The desktop's accent colour, for the selected tab.
    pub accent: Option<(u8, u8, u8)>,
    /// The desktop's monospace font family.
    pub font: Option<String>,
    /// Ctrl+Tab shows the tab navigator (NativeTerm's switcher setting).
    pub switcher: bool,
}

/// `text` inside a Lua double-quoted string.
fn lua_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
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
        let tabs = windows[0].as_window_tabs(Some(1));
        assert_eq!(tabs.names, ["web01", "vim notes.md"], "a set title, else the pane's");
        assert_eq!(tabs.selected, Some(1));
        assert_eq!(tabs.rects, [None, None]);
        assert_eq!(tabs.panes.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), ["vim notes.md", "db01"]);
        assert_eq!(windows[0].as_window_tabs(Some(9)).selected, None, "a tab that is gone");
        assert!(windows[0].as_window_tabs(None).panes.is_empty());
        assert_eq!(windows[0].tabs[1].active_pane(), Some(1));
        assert_eq!(windows[0].tab_of_pane(3), Some(1));
        assert_eq!(windows[0].tab_index(1), Some(1));
        assert_eq!(windows[1].tab_of_pane(3), None);
        let clients = r#"[{"username":"x","hostname":"h","pid":1,"workspace":"default","focused_pane_id":7,"idle_time":{"secs":9,"nanos":500000000}}]"#;
        assert_eq!(
            parse_focus(clients).unwrap(),
            [Focus { pane: 7, since_input: std::time::Duration::from_millis(9500) }]
        );
        assert!(parse_focus("[]").unwrap().is_empty());
        let no_idle = r#"[{"focused_pane_id":3}]"#;
        assert_eq!(parse_focus(no_idle).unwrap()[0].since_input, std::time::Duration::ZERO);
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
        let shim = Path::new("/opt/nt/nativeterm-shim");
        let unknown = default_config(&Look::default(), shim);
        assert!(unknown.contains("local shim = \"/opt/nt/nativeterm-shim\"\n"));
        assert!(unknown.contains("\"--tab-menu\""));
        assert!(unknown.contains("InputSelector"));
        assert!(!unknown.contains("ShowTabNavigator"), "Ctrl+Tab stays WezTerm's until asked");
        assert!(unknown.trim_end().ends_with("return config"));
        let with_switcher = default_config(&Look { switcher: true, ..Look::default() }, shim);
        assert!(with_switcher.contains("ShowTabNavigator"));
        let windows = default_config(&Look::default(), Path::new(r"C:\NT\nativeterm-shim.exe"));
        assert!(windows.contains(r#"local shim = "C:\\NT\\nativeterm-shim.exe""#), "backslashes escaped for Lua");
        assert!(unknown.contains("wezterm.config_builder()"));
        assert!(unknown.contains("wezterm.gui.get_appearance()"), "no reading: WezTerm asks the desktop");
        assert!(!unknown.contains("config.font ="));
        assert!(!unknown.contains("config.colors"));
        let read = default_config(
            &Look {
                dark: Some(true),
                accent: Some((0x1f, 0x6e, 0xe7)),
                font: Some("Noto Mono".into()),
                switcher: false,
            },
            shim,
        );
        assert!(read.contains("config.color_scheme = \"Builtin Tango Dark\"\n"));
        assert!(!read.contains("get_appearance"));
        assert!(read.contains("active_tab = { bg_color = \"#1f6ee7\", fg_color = \"#ffffff\" }"));
        assert!(read.contains("config.font = wezterm.font_with_fallback({ \"Noto Mono\" })\n"));
        let light = default_config(
            &Look {
                dark: Some(false),
                accent: Some((0xff, 0xc6, 0x00)),
                font: Some("Odd \"Mono\"".into()),
                switcher: false,
            },
            shim,
        );
        assert!(light.contains("\"Builtin Tango Light\""));
        assert!(light.contains("fg_color = \"#000000\""), "black on a light accent");
        assert!(light.contains("font_with_fallback({ \"Odd \\\"Mono\\\"\" })"), "quotes escaped");
    }

    #[test]
    fn the_other_commands() {
        assert_eq!(strings(&list_args()), ["cli", "--no-auto-start", "list", "--format", "json"]);
        assert_eq!(strings(&list_clients_args())[2], "list-clients");
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
