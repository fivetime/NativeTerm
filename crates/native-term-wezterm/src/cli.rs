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

/// Remove the discovery socket a GUI with `pid` left behind (it names
/// them `gui-sock-<pid>` in its runtime folder): the cli tries every
/// socket it finds, and a dead one answers for nobody.
pub fn forget_gui_socket(pid: u32) {
    let name = format!("gui-sock-{pid}");
    let mut dirs = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        dirs.push(std::path::PathBuf::from(runtime).join("wezterm"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".local").join("share").join("wezterm"));
    }
    for dir in dirs {
        let _ = std::fs::remove_file(dir.join(&name));
        // the per-display links (`wayland-<display>-<class>`,
        // `x11-<display>-<class>`) the cli follows first: gone too when
        // they point at the dead socket
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let dead = std::fs::read_link(&path).is_ok_and(|t| t.file_name().is_some_and(|f| f == name.as_str()));
            if dead {
                let _ = std::fs::remove_file(path);
            }
        }
    }
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

-- NativeTerm's look, the same on every system: Windows Terminal's own
-- (its Campbell colours when dark, One Half Light when light), light or
-- dark as the desktop or NativeTerm's theme setting says
"#,
    );
    match look.dark {
        Some(dark) => lua.push_str(&format!("local dark = {dark}\n")),
        None => {
            lua.push_str("local dark = wezterm.gui ~= nil and wezterm.gui.get_appearance():find(\"Dark\") ~= nil\n")
        }
    }
    lua.push_str(LOOK_LUA);
    if let Some(bar) = look.titlebar.as_ref().filter(|_| !cfg!(target_os = "macos")) {
        lua.push_str(&titlebar_colors(bar));
        if let Some(edge) = &bar.edge {
            lua.push_str(&window_edge(edge));
        }
    }
    if cfg!(windows) {
        lua.push_str("frame.font = wezterm.font({ family = \"Segoe UI\" })\nframe.font_size = 10.0\n");
    }
    lua.push_str("config.window_frame = frame\n");
    // the tabs and the window buttons in one strip, as Windows Terminal
    // and Chrome have them
    lua.push_str("config.window_decorations = \"INTEGRATED_BUTTONS|RESIZE\"\n");
    // Chrome's tab strip where the WezTerm draws one (NativeTerm's: its
    // height, tab shape, separators and colour rules)
    lua.push_str("pcall(function()\n  config.tab_strip_style = \"Chrome\"\nend)\n");
    if let Some(layout) = look.button_layout.as_deref().filter(|_| !cfg!(target_os = "macos")) {
        lua.push_str(&title_buttons(layout, &look.button_icons, look.titlebar.as_ref()));
    }
    if !look.fonts.is_empty() {
        // the system's fonts (see native_term_os::fonts::terminal_families);
        // WezTerm's own fallbacks follow
        let list: Vec<String> = look.fonts.iter().map(|f| format!("\"{}\"", lua_escape(f))).collect();
        lua.push_str(&format!("config.font = wezterm.font_with_fallback({{ {} }})\n", list.join(", ")));
    }
    lua.push_str(
        r#"config.font_size = 12.0

-- drawn by the GPU through WebGpu (Metal, Vulkan, DirectX 12) where there
-- is a real one, the integrated one first (it spares the battery); OpenGL
-- where there is none (a virtual machine renders in software). Only the
-- GUI can ask: `wezterm cli` reads this file too.
if wezterm.gui then
  local gpu = nil
  for _, adapter in ipairs(wezterm.gui.enumerate_gpus()) do
    if adapter.device_type == "IntegratedGpu" then
      gpu = adapter
      break
    elseif adapter.device_type == "DiscreteGpu" and gpu == nil then
      gpu = adapter
    end
  end
  if gpu then
    config.front_end = "WebGpu"
    config.webgpu_preferred_adapter = gpu
  end
end

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

/// The colours and the tab strip, for `dark` (defined before it).
const LOOK_LUA: &str = r##"config.color_schemes = {
  ["NativeTerm Dark"] = {
    foreground = "#CCCCCC", background = "#0C0C0C",
    cursor_bg = "#FFFFFF", cursor_fg = "#0C0C0C", cursor_border = "#FFFFFF",
    selection_bg = "#FFFFFF", selection_fg = "#0C0C0C",
    ansi = { "#0C0C0C", "#C50F1F", "#13A10E", "#C19C00", "#0037DA", "#881798", "#3A96DD", "#CCCCCC" },
    brights = { "#767676", "#E74856", "#16C60C", "#F9F1A5", "#3B78FF", "#B4009E", "#61D6D6", "#F2F2F2" },
  },
  ["NativeTerm Light"] = {
    foreground = "#383A42", background = "#FAFAFA",
    cursor_bg = "#4F525D", cursor_fg = "#FAFAFA", cursor_border = "#4F525D",
    selection_bg = "#BFCEFF", selection_fg = "#383A42",
    ansi = { "#383A42", "#E45649", "#50A14F", "#C18301", "#0184BC", "#A626A4", "#0997B3", "#FAFAFA" },
    brights = { "#4F525D", "#DF6C75", "#98C379", "#E4C07A", "#61AFEF", "#C577DD", "#56B5C1", "#FFFFFF" },
  },
}
config.color_scheme = dark and "NativeTerm Dark" or "NativeTerm Light"
-- the tab strip in the window's own light or dark
local strip = dark and { bar = "#202020", active = "#0C0C0C", inactive = "#2B2B2B", fg = "#FFFFFF", dim = "#A0A0A0" }
  or { bar = "#F3F3F3", active = "#FAFAFA", inactive = "#E6E6E6", fg = "#1A1A1A", dim = "#5C5C5C" }
local frame = { active_titlebar_bg = strip.bar, inactive_titlebar_bg = strip.bar }
config.colors = {
  tab_bar = {
    active_tab = { bg_color = strip.active, fg_color = strip.fg },
    inactive_tab = { bg_color = strip.inactive, fg_color = strip.dim },
    inactive_tab_hover = { bg_color = strip.active, fg_color = strip.fg },
    new_tab = { bg_color = strip.bar, fg_color = strip.dim },
    new_tab_hover = { bg_color = strip.active, fg_color = strip.fg },
    inactive_tab_edge = strip.bar,
  },
}
-- in points, so a Retina screen gets as much room as any other
config.window_padding = { left = "6pt", right = "6pt", top = "3pt", bottom = "3pt" }
-- the tab menu (and the command palette) in the same light or dark
config.command_palette_bg_color = dark and "#2C2C2C" or "#F9F9F9"
config.command_palette_fg_color = dark and "#FFFFFF" or "#1A1A1A"
"##;

/// NativeTerm's tab menu: Ctrl+Shift+M, or a right click, runs the shim's
/// `--tab-menu` for the pane, which prints what applies to its session;
/// the choice goes back the same way. NativeTerm's WezTerm
/// (github.com/fivetime/wezterm) pops it up where the mouse is
/// (`PopupMenu`: the heading, separators, dimmed items, icons) for a right
/// click on the tab itself (its `tab-right-click` event), as in Windows
/// Terminal, and leaves the pane's right click alone; another WezTerm has
/// no such event, so there a right click in the pane lists the items that
/// can be chosen in its picker. A tab that is not NativeTerm's gets no
/// menu (the shim prints nothing).
const MENU_LUA: &str = r#"-- NativeTerm's tab menu: right-click a tab, or Ctrl+Shift+M
local shim = "__SHIM__"
local popup = wezterm.has_action ~= nil and wezterm.has_action("PopupMenu")
local function tab_menu(window, pane)
  local ok, out = wezterm.run_child_process({ shim, "--tab-menu", "--pane", tostring(pane:pane_id()) })
  if not ok then
    return
  end
  -- id, text, flags ("d": disabled), icon; "-" for a separator
  local title, lines = "NativeTerm", {}
  for line in out:gmatch("[^\r\n]+") do
    if line == "-" then
      table.insert(lines, { separator = true })
    else
      local id, text, flags, icon = line:match("^(%d+)\t([^\t]*)\t?([^\t]*)\t?(.*)$")
      if id == "0" then
        title = text
      elseif id then
        table.insert(lines, { id = id, label = text, enabled = flags ~= "d", icon = icon ~= "" and icon or nil })
      end
    end
  end
  if #lines == 0 then
    return
  end
  local chosen = wezterm.action_callback(function(_, chosen_pane, id)
    if id then
      wezterm.run_child_process({ shim, "--tab-menu", id, "--pane", tostring(chosen_pane:pane_id()) })
    end
  end)
  if popup then
    table.insert(lines, 1, { label = title, header = true })
    window:perform_action(wezterm.action.PopupMenu({ choices = lines, action = chosen }), pane)
    return
  end
  local choices = {}
  for _, item in ipairs(lines) do
    if item.id and item.enabled then
      table.insert(choices, { id = item.id, label = item.label })
    end
  end
  if #choices == 0 then
    return
  end
  window:perform_action(
    wezterm.action.InputSelector({ title = title, choices = choices, fuzzy = false, action = chosen }),
    pane
  )
end
config.keys = {
  { key = "m", mods = "CTRL|SHIFT", action = wezterm.action_callback(tab_menu) },
}
if popup then
  -- a right click on a tab, as in Windows Terminal; the pane keeps its own
  wezterm.on("tab-right-click", function(window, pane)
    tab_menu(window, pane)
    return false
  end)
else
  config.mouse_bindings = {
    { event = { Down = { streak = 1, button = "Right" } }, mods = "NONE", action = wezterm.action_callback(tab_menu) },
  }
end
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
    /// The font families, in order of fallback (see
    /// `native_term_os::fonts::terminal_families`).
    pub fonts: Vec<String>,
    /// Ctrl+Tab shows the tab navigator (NativeTerm's switcher setting).
    pub switcher: bool,
    /// The desktop's window buttons, GNOME's `button-layout` form
    /// (`close:maximize`; see `native_term_os::appearance`), or `None` for
    /// WezTerm's own (minimize, maximize, close at the right).
    pub button_layout: Option<String>,
    /// The desktop's own icons for them (`close`, `minimize`, `maximize`,
    /// `restore`: SVG files out of its icon theme; see
    /// `native_term_os::icons`).
    pub button_icons: Vec<(&'static str, String)>,
    /// The title bar as the desktop's GTK theme draws it (its buttons,
    /// its header bar's colours; see `native_term_os::titlebar`), where
    /// the desktop is a GTK one.
    pub titlebar: Option<native_term_os::titlebar::Titlebar>,
}

fn hex((r, g, b): native_term_os::titlebar::Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `a` moved `t` of the way to `b`.
fn mix(a: native_term_os::titlebar::Rgb, b: native_term_os::titlebar::Rgb, t: f32) -> native_term_os::titlebar::Rgb {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// The tab strip in the desktop theme's own colours, as Chrome's frame:
/// the header bar's background behind the tabs (focused and not), the
/// title's colour on them. The active tab is what lies beneath it, as
/// Chrome's is its toolbar: the terminal's own background, so it joins
/// the terminal, with the theme's text on it (Chrome's toolbar text); where
/// it would not stand out from the strip, Chrome's style strokes it. The terminal keeps NativeTerm's colours.
fn titlebar_colors(bar: &native_term_os::titlebar::Titlebar) -> String {
    let (frame, title) = (hex(bar.frame), hex(bar.title));
    let hover = hex(mix(bar.frame, bar.title, 0.1));
    format!(
        "-- the tab strip in the desktop theme's own colours (its header bar), as Chrome has it\n\
         frame.active_titlebar_bg = \"{frame}\"\n\
         frame.inactive_titlebar_bg = \"{frame_inactive}\"\n\
         frame.active_titlebar_fg = \"{title}\"\n\
         frame.inactive_titlebar_fg = \"{title_inactive}\"\n\
         local scheme = config.color_schemes[config.color_scheme]\n\
         config.colors.tab_bar = {{\n\
         \x20 active_tab = {{ bg_color = scheme.background, fg_color = \"{text}\" }},\n\
         \x20 inactive_tab = {{ bg_color = \"{frame}\", fg_color = \"{title}\" }},\n\
         \x20 inactive_tab_hover = {{ bg_color = \"{hover}\", fg_color = \"{title}\" }},\n\
         \x20 new_tab = {{ bg_color = \"{frame}\", fg_color = \"{title}\" }},\n\
         \x20 new_tab_hover = {{ bg_color = \"{hover}\", fg_color = \"{title}\" }},\n\
         \x20 inactive_tab_edge = \"{frame}\",\n\
         }}\n",
        frame_inactive = hex(bar.frame_inactive),
        title_inactive = hex(bar.title_inactive),
        text = hex(bar.text),
    )
}

/// The Lua giving the window buttons the desktop's layout and icons, as
/// Chrome does: the buttons it has, where it has them (elementary: close
/// at the left, maximize at the right, no minimize), with its icon
/// theme's own symbols, flat — the same on every desktop, nothing drawn
/// for one in particular. NativeTerm's WezTerm takes all this as it is;
/// any other one refuses the settings it does not know (the config
/// builder raises), and then only lines the buttons up on the close
/// button's side.
fn title_buttons(layout: &str, icons: &[(&str, String)], bar: Option<&native_term_os::titlebar::Titlebar>) -> String {
    let close_left = layout.split_once(':').is_some_and(|(left, _)| left.split(',').any(|b| b == "close"));
    let mut lua = String::from("local desktop_buttons = pcall(function()\n");
    lua.push_str(&format!("  config.integrated_title_button_layout = \"{}\"\n", lua_escape(layout)));
    lua.push_str("  config.integrated_title_button_style = \"Flat\"\n");
    if !icons.is_empty() {
        let entries: Vec<String> =
            icons.iter().map(|(name, path)| format!("{name} = \"{}\"", lua_escape(path))).collect();
        lua.push_str(&format!("  config.integrated_title_button_icons = {{ {} }}\n", entries.join(", ")));
    }
    if let Some(bar) = bar {
        lua.push_str(&button_images(layout, bar));
    }
    lua.push_str("end)\n");
    if close_left {
        lua.push_str("if not desktop_buttons then\n  config.integrated_title_button_alignment = \"Left\"\nend\n");
    }
    lua
}

/// The window's edge as the GTK theme draws it (shadow, border, rounded
/// top corners; see `native_term_os::titlebar::Edge`), which NativeTerm's
/// WezTerm draws around the window itself; set apart, so a WezTerm that
/// knows the buttons but not this keeps the buttons.
fn window_edge(edge: &native_term_os::titlebar::Edge) -> String {
    let [top, right, bottom, left] = edge.thickness;
    format!(
        "-- the window's own edge (shadow, border, round top corners) as the desktop theme draws it\n\
         pcall(function()\n\
         \x20 config.integrated_window_edge = {{ focused = \"{}\", unfocused = \"{}\", top = {top}, right = {right}, \
         bottom = {bottom}, left = {left}, radius = {}, slice = {} }}\n\
         end)\n",
        lua_escape(&edge.focused.to_string_lossy()),
        lua_escape(&edge.unfocused.to_string_lossy()),
        edge.radius,
        edge.slice,
    )
}

/// The GTK theme's own buttons (see `native_term_os::titlebar`), placed
/// as a GTK header bar places them (nav_button_provider_gtk.cc): each
/// with its CSS margins, GTK's spacing between them and next to the tabs,
/// the header bar's padding at the window's edges. `restore` stands where
/// `maximize` does.
fn button_images(layout: &str, bar: &native_term_os::titlebar::Titlebar) -> String {
    let (left, right) = layout.split_once(':').unwrap_or((layout, ""));
    fn side(names: &str) -> Vec<&str> {
        names.split(',').map(str::trim).filter(|n| !n.is_empty()).collect()
    }
    let (left, right) = (side(left), side(right));
    let mut entries = Vec::new();
    for b in &bar.buttons {
        let slot = if b.name == "restore" { "maximize" } else { b.name.as_str() };
        let (ml, mr) = if let Some(i) = left.iter().position(|n| *n == slot) {
            (b.margin_left + if i == 0 { bar.padding_left } else { 0 }, b.margin_right + bar.spacing)
        } else if let Some(i) = right.iter().position(|n| *n == slot) {
            (b.margin_left + bar.spacing, b.margin_right + if i + 1 == right.len() { bar.padding_right } else { 0 })
        } else {
            continue;
        };
        entries.push(format!(
            "    {} = {{ normal = \"{}\", hover = \"{}\", backdrop = \"{}\", width = {}, height = {}, margin_left = {ml}, margin_right = {mr} }},\n",
            b.name,
            lua_escape(&b.normal.to_string_lossy()),
            lua_escape(&b.hover.to_string_lossy()),
            lua_escape(&b.backdrop.to_string_lossy()),
            b.width,
            b.height,
        ));
    }
    if entries.is_empty() {
        return String::new();
    }
    format!("  config.integrated_title_button_images = {{\n{}  }}\n", entries.concat())
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
        assert!(unknown.contains("InputSelector"), "a stock WezTerm's picker");
        assert!(unknown.contains("wezterm.has_action(\"PopupMenu\")"), "NativeTerm's WezTerm pops the menu up");
        assert!(unknown.contains("wezterm.action.PopupMenu({ choices = lines, action = chosen })"));
        assert!(unknown.contains("config.command_palette_bg_color = dark and"));
        assert!(!unknown.contains("ShowTabNavigator"), "Ctrl+Tab stays WezTerm's until asked");
        assert!(unknown.trim_end().ends_with("return config"));
        let with_switcher = default_config(&Look { switcher: true, ..Look::default() }, shim);
        assert!(with_switcher.contains("ShowTabNavigator"));
        let windows = default_config(&Look::default(), Path::new(r"C:\NT\nativeterm-shim.exe"));
        assert!(windows.contains(r#"local shim = "C:\\NT\\nativeterm-shim.exe""#), "backslashes escaped for Lua");
        assert!(unknown.contains("wezterm.config_builder()"));
        assert!(unknown.contains("if wezterm.gui then"), "the renderer is chosen only where the GUI can ask");
        assert!(unknown.contains("config.front_end = \"WebGpu\""));
        assert!(unknown.contains("wezterm.gui.get_appearance()"), "no reading: WezTerm asks the desktop");
        assert!(!unknown.contains("config.font ="), "no fonts known: WezTerm's own");
        assert!(unknown.contains("[\"NativeTerm Dark\"]") && unknown.contains("[\"NativeTerm Light\"]"));
        assert!(unknown.contains("config.window_frame = frame\n"));
        assert!(unknown.contains("config.font_size = 12.0\n"), "Windows Terminal's size");
        let read = default_config(
            &Look {
                dark: Some(true),
                fonts: vec!["Cascadia Mono".into(), "Microsoft YaHei".into()],
                switcher: false,
                button_layout: Some("close:maximize".into()),
                button_icons: vec![
                    ("close", "/usr/share/icons/elementary/actions/symbolic/window-close-symbolic.svg".into()),
                    ("maximize", "/icons/\"odd\".svg".into()),
                ],
                titlebar: None,
            },
            shim,
        );
        assert!(read.contains("local dark = true\n"));
        assert!(!read.contains("get_appearance"));
        // the window buttons in the tab strip, as the desktop has them
        assert!(read.contains("config.window_decorations = \"INTEGRATED_BUTTONS|RESIZE\"\n"));
        assert!(
            read.contains("pcall(function()\n  config.tab_strip_style = \"Chrome\"\nend)\n"),
            "Chrome's tab strip, where the WezTerm knows it"
        );
        let desktop = "local desktop_buttons = pcall(function()\n  config.integrated_title_button_layout = \"close:maximize\"\n  config.integrated_title_button_style = \"Flat\"\n  config.integrated_title_button_icons = { close = \"/usr/share/icons/elementary/actions/symbolic/window-close-symbolic.svg\", maximize = \"/icons/\\\"odd\\\".svg\" }\nend)\nif not desktop_buttons then\n  config.integrated_title_button_alignment = \"Left\"\nend\n";
        assert_eq!(read.contains(desktop), !cfg!(target_os = "macos"));
        assert!(!unknown.contains("integrated_title_button"), "no layout read: WezTerm's own buttons");
        let right = title_buttons(":minimize,maximize,close", &[], None);
        assert!(!right.contains("integrated_title_button_icons"), "no icons found: the drawn symbols");
        assert!(right.contains("config.integrated_title_button_layout = \":minimize,maximize,close\"\n"));
        assert!(!right.contains("alignment"), "WezTerm's own side already");
        assert!(!right.contains("integrated_title_button_images"), "no GTK title bar: no pictures");
        // the menu on a right click on the tab where WezTerm tells of one
        assert!(read.contains("wezterm.on(\"tab-right-click\""));
        assert!(read.contains("config.font = wezterm.font_with_fallback({ \"Cascadia Mono\", \"Microsoft YaHei\" })\n"));
        let light = default_config(
            &Look { dark: Some(false), fonts: vec!["Odd \"Mono\"".into()], switcher: false, ..Look::default() },
            shim,
        );
        assert!(light.contains("local dark = false\n"));
        assert!(light.contains("font_with_fallback({ \"Odd \\\"Mono\\\"\" })"), "quotes escaped");
        // the frame is whole before it is given to the config
        let frame_done = unknown.find("config.window_frame = frame\n").unwrap();
        assert!(!unknown[frame_done..].contains("\nframe."));
    }

    #[test]
    fn the_gtk_title_bar() {
        use native_term_os::titlebar::{Button, Titlebar};
        let button = |name: &str| Button {
            name: name.into(),
            width: 24,
            height: 24,
            margin_left: 1,
            margin_right: 2,
            normal: format!("/c/{name}-normal.png").into(),
            hover: format!("/c/{name}-hover.png").into(),
            backdrop: format!("/c/{name}-backdrop.png").into(),
        };
        let bar = Titlebar {
            frame: (0x30, 0x30, 0x30),
            frame_inactive: (0x28, 0x28, 0x28),
            window: (0x24, 0x24, 0x24),
            text: (0xff, 0xff, 0xff),
            title: (0xee, 0xee, 0xee),
            title_inactive: (0x90, 0x90, 0x90),
            padding_left: 6,
            padding_right: 7,
            spacing: 6,
            buttons: ["close", "minimize", "maximize", "restore"].map(button).to_vec(),
            edge: None,
        };
        // elementary: close at the left end, maximize at the right end
        let lua = button_images("close:maximize", &bar);
        assert!(lua.contains("    close = { normal = \"/c/close-normal.png\", hover = \"/c/close-hover.png\", backdrop = \"/c/close-backdrop.png\", width = 24, height = 24, margin_left = 7, margin_right = 8 },
"),
            "the header bar's padding at the edge, GTK's spacing towards the tabs");
        assert!(
            lua.contains("maximize = { normal = \"/c/maximize-normal.png\"")
                && lua.contains(
                    "margin_left = 7, margin_right = 9 },
    restore"
                )
        );
        assert!(lua.contains("restore = { normal = \"/c/restore-normal.png\""), "restore where maximize stands");
        assert!(!lua.contains("minimize ="), "a button the layout leaves out");
        // GNOME: all three at the right, padding only at the window's edge
        let lua = button_images(":minimize,maximize,close", &bar);
        assert!(lua.contains("minimize = { normal = \"/c/minimize-normal.png\", hover = \"/c/minimize-hover.png\", backdrop = \"/c/minimize-backdrop.png\", width = 24, height = 24, margin_left = 7, margin_right = 2 }"));
        assert!(lua.contains("close = { normal = \"/c/close-normal.png\", hover = \"/c/close-hover.png\", backdrop = \"/c/close-backdrop.png\", width = 24, height = 24, margin_left = 7, margin_right = 9 }"));
        assert_eq!(button_images("close:maximize", &Titlebar { buttons: vec![], ..bar.clone() }), "");
        let edge = native_term_os::titlebar::Edge {
            thickness: [2, 30, 40, 30],
            radius: 8,
            slice: 64,
            focused: "/c/edge-focused.png".into(),
            unfocused: "/c/edge-unfocused.png".into(),
        };
        let lua = window_edge(&edge);
        assert!(lua.starts_with("-- the window's own edge"));
        assert!(lua.contains("pcall(function()
  config.integrated_window_edge = { focused = \"/c/edge-focused.png\", unfocused = \"/c/edge-unfocused.png\", top = 2, right = 30, bottom = 40, left = 30, radius = 8, slice = 64 }
end)
"));
        // the tab strip in the header bar's colours, the active tab the window's
        let colors = titlebar_colors(&bar);
        assert!(colors.contains(
            "frame.active_titlebar_bg = \"#303030\"
"
        ));
        assert!(colors.contains(
            "frame.inactive_titlebar_bg = \"#282828\"
"
        ));
        assert!(
            colors.contains("active_tab = { bg_color = scheme.background, fg_color = \"#ffffff\" },"),
            "the active tab is the terminal beneath it, with the theme's text"
        );
        assert!(colors.contains("inactive_tab = { bg_color = \"#303030\", fg_color = \"#eeeeee\" },"));
        assert!(colors.contains("inactive_tab_hover = { bg_color = \"#434343\""), "a tenth of the way to the title");
        assert!(
            colors.find("local scheme = config.color_schemes[config.color_scheme]\n") < colors.find("active_tab"),
            "the scheme read before its colours are used"
        );
        let whole = default_config(
            &Look { titlebar: Some(bar), button_layout: Some("close:maximize".into()), ..Look::default() },
            Path::new("/opt/nt/nativeterm-shim"),
        );
        assert_eq!(
            whole.contains(
                "config.integrated_title_button_images = {
"
            ),
            !cfg!(target_os = "macos")
        );
        let colours_at = whole.find("frame.active_titlebar_bg = \"#303030\"");
        let frame_given = whole.find(
            "config.window_frame = frame
",
        );
        if !cfg!(target_os = "macos") {
            assert!(colours_at.unwrap() < frame_given.unwrap(), "the frame is whole before it is given");
        }
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
