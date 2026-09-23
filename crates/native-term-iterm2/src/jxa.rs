//! What iTerm2 is asked and answers: JavaScript for Automation (JXA)
//! scripts run through `osascript`, and their JSON. Pure, so it is
//! tested without an iTerm2.

use std::ffi::OsString;

use native_term_platform::claim::{Pane, WindowTabs};
use native_term_platform::{TabSpec, WindowId};
use serde::Deserialize;

/// A session (a pane) of a tab.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Session {
    /// iTerm2's unique id (what `ITERM_SESSION_ID` ends with).
    pub id: String,
    /// The session's name: what the tab shows for it.
    #[serde(default)]
    pub name: String,
    /// The tab's current session.
    #[serde(default)]
    pub current: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Tab {
    /// The window's current tab.
    #[serde(default)]
    pub current: bool,
    #[serde(default)]
    pub sessions: Vec<Session>,
}

impl Tab {
    /// The tab's name: its current session's (else its first's).
    pub fn name(&self) -> String {
        self.sessions.iter().find(|s| s.current).or(self.sessions.first()).map(|s| s.name.clone()).unwrap_or_default()
    }

    pub fn current_session(&self) -> Option<&Session> {
        self.sessions.iter().find(|s| s.current).or(self.sessions.first())
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    #[serde(default)]
    pub tabs: Vec<Tab>,
}

impl Window {
    pub fn window_id(&self) -> WindowId {
        WindowId(self.id)
    }

    /// The window as the claimer reads one: names in tab order, no
    /// rectangles, the current tab selected, and its sessions as panes.
    pub fn as_window_tabs(&self) -> WindowTabs {
        let selected = self.tabs.iter().position(|t| t.current);
        let panes = selected
            .map(|s| {
                self.tabs[s].sessions.iter().map(|p| Pane { title: p.name.clone(), profile: String::new() }).collect()
            })
            .unwrap_or_default();
        WindowTabs {
            names: self.tabs.iter().map(Tab::name).collect(),
            rects: vec![None; self.tabs.len()],
            selected,
            panes,
        }
    }

    /// The tab at `index`, if it is still called `name`.
    pub fn tab_named(&self, index: usize, name: &str) -> Option<&Tab> {
        self.tabs.get(index).filter(|t| t.name() == name)
    }
}

/// Everything the `list` script reports.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Listing {
    #[serde(default)]
    pub windows: Vec<Window>,
    /// iTerm2 is the frontmost application.
    #[serde(default)]
    pub frontmost: bool,
    /// Its current window, when it has one.
    #[serde(default)]
    pub current_window: Option<u64>,
}

pub fn parse_listing(json: &str) -> Result<Listing, serde_json::Error> {
    serde_json::from_str(json.trim())
}

/// Every window, tab and session, as JSON. Doesn't start iTerm2.
pub fn list_script() -> String {
    r#"(() => {
  const app = Application("com.googlecode.iterm2");
  if (!app.running()) { return JSON.stringify({windows: [], frontmost: false, current_window: null}); }
  const out = [];
  const ws = app.windows();
  for (let i = 0; i < ws.length; i++) {
    const w = ws[i];
    let currentTab = -1;
    try { currentTab = w.currentTab().index(); } catch (e) {}
    const tabs = [];
    const ts = w.tabs();
    for (let j = 0; j < ts.length; j++) {
      const t = ts[j];
      let currentSession = "";
      try { currentSession = t.currentSession().id(); } catch (e) {}
      const sessions = [];
      const ss = t.sessions();
      for (let k = 0; k < ss.length; k++) {
        const s = ss[k];
        const id = s.id();
        // the name a script gives a session is kept as its (own copy of
        // the) profile's name; `name` reads what the tab shows
        let name = "";
        try { name = s.profileName(); } catch (e) { name = s.name(); }
        sessions.push({id: id, name: name, current: id === currentSession});
      }
      tabs.push({current: t.index() === currentTab, sessions: sessions});
    }
    out.push({id: w.id(), tabs: tabs});
  }
  let current = null;
  try { current = app.currentWindow().id(); } catch (e) {}
  return JSON.stringify({windows: out, frontmost: app.frontmost(), current_window: current});
})()"#
        .to_string()
}

/// `text` as a JavaScript string literal.
pub fn js_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// `arg` for the shell iTerm2 runs a command through: single quotes, a
/// `'` inside as `'\''`.
pub fn sh_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The command line a session tab runs: the shim with its arguments.
pub fn shim_command(shim: &str, shim_args: &[OsString], tab: &TabSpec) -> String {
    let mut parts = vec![sh_quote(shim)];
    parts.extend(shim_args.iter().map(|a| sh_quote(&a.to_string_lossy())));
    parts.push("--session".into());
    parts.push(sh_quote(&tab.session));
    if tab.wait {
        parts.push("--wait".into());
    }
    if tab.no_forwards {
        parts.push("--no-forwards".into());
    }
    parts.push(sh_quote(&tab.alias));
    parts.join(" ")
}

/// A tool tab's command line: the shim with `args`.
pub fn tool_command(shim: &str, args: &[String]) -> String {
    std::iter::once(shim).chain(args.iter().map(String::as_str)).map(sh_quote).collect::<Vec<_>>().join(" ")
}

/// The iTerm2 profile NativeTerm's tabs open with (see `dynamic_profile`).
pub const PROFILE: &str = "NativeTerm";

/// NativeTerm's profile, as an iTerm2 dynamic profile: the person's
/// default profile (fonts, colors), with the tab's title only its session
/// name (the label NativeTerm gives it) and programs not allowed to change
/// it — as NativeTerm's Windows Terminal profile does.
pub fn dynamic_profile() -> String {
    serde_json::json!({
        "Profiles": [{
            "Name": PROFILE,
            "Guid": "6e617469-7665-4465-926d-6e6174697665",
            "Dynamic Profile Parent Name": "Default",
            // session name only (job, folder, … are further bits)
            "Title Components": 1,
            "Allow Title Setting": false,
        }]
    })
    .to_string()
}

/// A new window running `command` (with NativeTerm's profile, else the
/// default one), its session named `name`; prints the window's id. Starts
/// iTerm2 if it isn't running.
pub fn new_window_script(command: &str, name: &str) -> String {
    format!(
        r#"(() => {{
  const app = Application("com.googlecode.iterm2");
  app.activate();
  let w;
  try {{ w = app.createWindowWithProfile({profile}, {{command: {command}}}); }}
  catch (e) {{ w = app.createWindowWithDefaultProfile({{command: {command}}}); }}
  w.currentSession().name = {name};
  return String(w.id());
}})()"#,
        command = js_string(command),
        name = js_string(name),
        profile = js_string(PROFILE),
    )
}

/// A new tab in window `window` running `command` (with NativeTerm's
/// profile, else the default one), its session named `name`; prints the tab's index (1-based).
pub fn new_tab_script(window: u64, command: &str, name: &str) -> String {
    format!(
        r#"(() => {{
  const app = Application("com.googlecode.iterm2");
  const w = app.windows.byId({window});
  let t;
  // a window's "create tab with profile" takes the profile as
  // `withProfile` (the form the application's own command has fails)
  try {{ t = w.createTab({{withProfile: {profile}, command: {command}}}); }}
  catch (e) {{ t = w.createTabWithDefaultProfile({{command: {command}}}); }}
  t.currentSession().name = {name};
  return String(t.index());
}})()"#,
        command = js_string(command),
        name = js_string(name),
        profile = js_string(PROFILE),
    )
}

/// Select tab `index` (0-based) of `window` and bring the window forward.
pub fn select_tab_script(window: u64, index: usize) -> String {
    format!(
        r#"(() => {{
  const app = Application("com.googlecode.iterm2");
  const w = app.windows.byId({window});
  w.tabs[{index}].select();
  w.select();
  return "ok";
}})()"#
    )
}

/// Close every session of tab `index` (0-based) of `window`.
pub fn close_tab_script(window: u64, index: usize) -> String {
    format!(
        r#"(() => {{
  const app = Application("com.googlecode.iterm2");
  const w = app.windows.byId({window});
  const ss = w.tabs[{index}].sessions();
  for (let k = ss.length - 1; k >= 0; k--) {{ ss[k].close(); }}
  return "ok";
}})()"#
    )
}

/// Bring `window` forward, and iTerm2 with it.
pub fn activate_script(window: u64) -> String {
    format!(
        r#"(() => {{
  const app = Application("com.googlecode.iterm2");
  app.windows.byId({window}).select();
  app.activate();
  return "ok";
}})()"#
    )
}

/// The current session's screen of `window`'s current tab.
pub fn contents_script(window: u64) -> String {
    format!(
        r#"(() => {{
  const app = Application("com.googlecode.iterm2");
  return app.windows.byId({window}).currentTab().currentSession().contents();
}})()"#
    )
}

/// Type `text` into the current session of tab `index` of `window`
/// (`write` without a newline of its own).
pub fn write_script(window: u64, index: usize, text: &str) -> String {
    format!(
        r#"(() => {{
  const app = Application("com.googlecode.iterm2");
  app.windows.byId({window}).tabs[{index}].currentSession().write({{text: {text}, newline: false}});
  return "ok";
}})()"#,
        text = js_string(text),
    )
}

/// The screen as `contents` printed it: lines, trailing spaces and empty
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

    const LISTING: &str = r#"{"windows":[
      {"id":4711,"tabs":[
        {"current":false,"sessions":[{"id":"A1B2","name":"web01","current":true}]},
        {"current":true,"sessions":[{"id":"C3D4","name":"vim notes.md","current":true},{"id":"E5F6","name":"db01","current":false}]}
      ]},
      {"id":4712,"tabs":[{"current":true,"sessions":[{"id":"9999","name":"zsh","current":true}]}]}
    ],"frontmost":true,"current_window":4712}"#;

    #[test]
    fn a_listing_becomes_windows_and_tabs() {
        let listing = parse_listing(LISTING).unwrap();
        assert_eq!(listing.windows.len(), 2);
        assert!(listing.frontmost);
        assert_eq!(listing.current_window, Some(4712));
        let w = &listing.windows[0];
        assert_eq!(w.window_id(), WindowId(4711));
        let tabs = w.as_window_tabs();
        assert_eq!(tabs.names, ["web01", "vim notes.md"]);
        assert_eq!(tabs.selected, Some(1));
        assert_eq!(tabs.rects, [None, None]);
        assert_eq!(tabs.panes.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), ["vim notes.md", "db01"]);
        assert!(w.tab_named(0, "web01").is_some());
        assert!(w.tab_named(0, "other").is_none());
        assert_eq!(w.tabs[1].current_session().map(|s| s.id.as_str()), Some("C3D4"));
        let none = parse_listing(r#"{"windows":[],"frontmost":false,"current_window":null}"#).unwrap();
        assert!(none.windows.is_empty() && none.current_window.is_none());
    }

    #[test]
    fn commands_are_quoted_for_the_shell_and_for_javascript() {
        let tab = TabSpec {
            terminal_session: "guid".into(),
            label: "web01".into(),
            session: "id-1".into(),
            alias: "it's".into(),
            wait: true,
            no_forwards: false,
            tab_color: None,
        };
        let command = shim_command("/opt/native term/shim", &["--ssh-dir".into(), "/x".into()], &tab);
        assert_eq!(command, r"'/opt/native term/shim' '--ssh-dir' '/x' --session 'id-1' --wait 'it'\''s'");
        assert_eq!(tool_command("/s", &["--install-key".into(), "k".into()]), "'/s' '--install-key' 'k'");
        let script = new_window_script(&command, "web \"quoted\"");
        assert!(script.contains(r#"createWindowWithDefaultProfile({command: "'/opt/native term/shim' "#), "{script}");
        assert!(script.contains(r#"name = "web \"quoted\"""#), "{script}");
        let script = new_tab_script(4711, "x", "n");
        assert!(script.contains("app.windows.byId(4711)") && script.contains("createTabWithDefaultProfile"));
        assert!(select_tab_script(4711, 2).contains("w.tabs[2].select()"));
        assert!(close_tab_script(4711, 0).contains("ss[k].close()"));
        assert!(write_script(4711, 1, "ls\r").contains(r#"write({text: "ls\r", newline: false})"#));
        assert!(list_script().contains("app.running()"));
        assert!(activate_script(1).contains("app.activate()"));
        assert!(contents_script(1).contains("contents()"));
    }

    #[test]
    fn screen_lines_are_trimmed() {
        assert_eq!(screen_lines("a  \nb\n\n", 10), ["a", "b"]);
        assert_eq!(screen_lines("a\nb\nc", 2), ["a", "b"]);
    }
}
