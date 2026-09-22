//! `settings.toml` in the data directory: what the user chose, in a file
//! they can read, copy and sync.
//!
//! Everything is shared between machines except the few things that only
//! make sense on the machine they were made on (`MACHINE_KEYS`): where a
//! window was, which Windows Terminal to drive, where the session folder
//! is. Those go under `[machine."<name>"]`, so a data directory carried
//! between computers doesn't put one machine's layout on another.
//!
//! ```toml
//! language = "zh-CN"
//! theme = "dark"
//! "tabs.hover" = "on"
//!
//! [machine."SIMON-PC"]
//! window = "0,0,960,640"
//! "terminal.install" = 'C:\Program Files\WindowsApps\…'
//! ```
//!
//! What is *not* here: the open-session registry, usage counts, long
//! notes and each host's last folders in the files window. Those are
//! records rather than choices, they change constantly, and they live in
//! `state.db`.
//!
//! The file is written whole, through a temporary file and a rename, so
//! it is never half written — and comments in it are lost, like any file
//! a program rewrites. A file that cannot be parsed is **never**
//! overwritten: NativeTerm says so and keeps the session's changes in
//! memory only.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Settings that belong to one machine, not to the data directory.
pub const MACHINE_KEYS: &[&str] = &[
    "window",               // where NativeTerm's window was
    "fab",                  // where the floating button was
    "dock_pinned",          // pinned to this machine's taskbar area
    "terminal.install",     // which Windows Terminal, by folder
    "folders_dir",          // where the session folder is on this computer
    "first_run_done",       // the start checks are about this computer
    "agent_hint_dismissed", // ssh-agent is a service of this machine
];

/// Keys that stay in `state.db`: one per host, written as the user walks
/// around in the files window (`files.remote:<alias>`).
const NOT_SETTINGS: &[&str] = &["files.remote", "files.local"];

pub const FILE: &str = "settings.toml";

#[derive(Default)]
struct Values {
    shared: BTreeMap<String, String>,
    /// Machine name → its own settings (every machine's, so another one's
    /// are kept when this one writes).
    machines: BTreeMap<String, BTreeMap<String, String>>,
}

pub struct Settings {
    path: PathBuf,
    machine: String,
    values: Mutex<Values>,
    /// Why the file could not be read; then it is never written.
    problem: Option<String>,
}

/// This computer's name, as the section is called.
#[must_use]
pub fn machine_name() -> String {
    Some(native_term_os::host::name()).filter(|n| !n.is_empty()).unwrap_or_else(|| "this-computer".to_string())
}

/// Whether `key` is kept per machine.
#[must_use]
pub fn is_machine_key(key: &str) -> bool {
    MACHINE_KEYS.contains(&key)
}

impl Settings {
    /// Read the file, or start an empty one. Never fails: settings are
    /// worth a notice, not a failed start.
    pub fn open(data_dir: &Path) -> Settings {
        Settings::at(&data_dir.join(FILE))
    }

    fn at(path: &Path) -> Settings {
        let (values, problem) = match std::fs::read_to_string(path) {
            Ok(text) => match parse(&text) {
                Ok(values) => (values, None),
                Err(e) => (Values::default(), Some(e)),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Values::default(), None),
            Err(e) => (Values::default(), Some(e.to_string())),
        };
        Settings { path: path.to_path_buf(), machine: machine_name(), values: Mutex::new(values), problem }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What is wrong with the file, if anything (then nothing is written).
    #[must_use]
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    fn values(&self) -> std::sync::MutexGuard<'_, Values> {
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        let values = self.values();
        if is_machine_key(key) {
            return values.machines.get(&self.machine)?.get(key).cloned();
        }
        values.shared.get(key).cloned()
    }

    /// Set (or, with an empty value, clear) a setting and write the file.
    pub fn set(&self, key: &str, value: &str) -> io::Result<()> {
        {
            let mut values = self.values();
            let machine = self.machine.clone();
            let table = match is_machine_key(key) {
                true => values.machines.entry(machine).or_default(),
                false => &mut values.shared,
            };
            match value.is_empty() {
                true => drop(table.remove(key)),
                false => drop(table.insert(key.to_string(), value.to_string())),
            }
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        if let Some(problem) = &self.problem {
            // the user's file is not ours to throw away
            return Err(io::Error::new(io::ErrorKind::InvalidData, problem.clone()));
        }
        let text = render(&self.values());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }

    /// Take the settings NativeTerm used to keep in `state.db` into the
    /// file, once (an older data directory). Records that belong in the
    /// database are left there. Returns how many were taken.
    pub fn take_over(&self, rows: Vec<(String, String)>) -> io::Result<usize> {
        if self.path.exists() || self.problem.is_some() {
            return Ok(0);
        }
        let mut taken = 0;
        {
            let mut values = self.values();
            for (key, value) in rows {
                if value.is_empty() || NOT_SETTINGS.iter().any(|p| key.starts_with(p)) {
                    continue;
                }
                let table = match is_machine_key(&key) {
                    true => values.machines.entry(self.machine.clone()).or_default(),
                    false => &mut values.shared,
                };
                table.insert(key, value);
                taken += 1;
            }
        }
        if taken == 0 {
            return Ok(0);
        }
        self.save()?;
        Ok(taken)
    }
}

fn parse(text: &str) -> Result<Values, String> {
    // a file saved by a Windows editor may start with a byte order mark
    let table: toml::Table =
        text.trim_start_matches(crate::data_lock::BOM).parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut values = Values::default();
    for (key, value) in table {
        if key == "machine" {
            let Some(machines) = value.as_table() else {
                return Err("machine is not a table of machines".to_string());
            };
            for (name, table) in machines {
                let Some(table) = table.as_table() else { continue };
                values.machines.insert(name.clone(), strings(table));
            }
            continue;
        }
        if let Some(text) = as_string(&value) {
            values.shared.insert(key, text);
        }
    }
    Ok(values)
}

/// Settings are text; a number or a yes/no someone typed by hand is read
/// as the text it looks like, rather than thrown away.
fn as_string(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(text) => Some(text.clone()),
        toml::Value::Integer(n) => Some(n.to_string()),
        toml::Value::Float(n) => Some(n.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn strings(table: &toml::Table) -> BTreeMap<String, String> {
    table.iter().filter_map(|(k, v)| Some((k.clone(), as_string(v)?))).collect()
}

fn render(values: &Values) -> String {
    let mut table = toml::Table::new();
    for (key, value) in &values.shared {
        table.insert(key.clone(), toml::Value::String(value.clone()));
    }
    let machines: toml::Table = values
        .machines
        .iter()
        .filter(|(_, own)| !own.is_empty())
        .map(|(name, own)| {
            let own: toml::Table =
                own.iter().map(|(k, v)| (k.clone(), toml::Value::String(v.clone()))).collect::<toml::Table>();
            (name.clone(), toml::Value::Table(own))
        })
        .collect();
    if !machines.is_empty() {
        table.insert("machine".to_string(), toml::Value::Table(machines));
    }
    let body = toml::to_string_pretty(&table).unwrap_or_default();
    format!("# NativeTerm settings. Everything here is shared between machines\n# except the [machine.\"…\"] sections.\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(dir: &Path) -> Settings {
        Settings::at(&dir.join(FILE))
    }

    #[test]
    fn a_setting_is_read_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let first = settings(dir.path());
        first.set("language", "zh-CN").unwrap();
        first.set("tabs.hover", "off").unwrap();
        first.set("window", "0,0,960,640").unwrap();
        let text = std::fs::read_to_string(first.path()).unwrap();
        assert!(text.contains("language = \"zh-CN\""), "{text}");
        assert!(text.contains("\"tabs.hover\" = \"off\""), "a dotted key is one key: {text}");
        let section = format!("[machine.{}]", machine_name());
        let quoted = format!("[machine.{:?}]", machine_name());
        assert!(text.contains(&section) || text.contains(&quoted), "{text}");

        let again = settings(dir.path());
        assert_eq!(again.get("language").as_deref(), Some("zh-CN"));
        assert_eq!(again.get("tabs.hover").as_deref(), Some("off"));
        assert_eq!(again.get("window").as_deref(), Some("0,0,960,640"));
        assert_eq!(again.get("theme"), None);
        // an empty value clears it
        again.set("language", "").unwrap();
        assert_eq!(settings(dir.path()).get("language"), None);
    }

    #[test]
    fn another_machines_settings_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        std::fs::write(
            &file,
            "language = \"en\"\n\n[machine.\"OTHER-PC\"]\nwindow = \"1,2,3,4\"\n\"terminal.install\" = \"D:\\\\wt\"\n",
        )
        .unwrap();
        let settings = Settings::at(&file);
        assert_eq!(settings.get("window"), None, "that window belongs to another machine");
        settings.set("window", "9,9,9,9").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("[machine.OTHER-PC]") || text.contains("[machine.\"OTHER-PC\"]"), "{text}");
        assert!(text.contains("window = \"1,2,3,4\""), "{text}");
        assert!(text.contains("window = \"9,9,9,9\""), "{text}");
        assert_eq!(Settings::at(&file).get("language").as_deref(), Some("en"));
    }

    #[test]
    fn a_file_we_cannot_read_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        std::fs::write(&file, "language = \n[[oops\n").unwrap();
        let settings = Settings::at(&file);
        assert!(settings.problem().is_some());
        assert!(settings.set("theme", "dark").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "language = \n[[oops\n");
        // the change still holds for this run
        assert_eq!(settings.get("theme").as_deref(), Some("dark"));
    }

    #[test]
    fn the_old_database_settings_are_taken_over_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = settings(dir.path());
        let rows = vec![
            ("language".to_string(), "zh-CN".to_string()),
            ("window".to_string(), "1,2,3,4".to_string()),
            ("files.remote:web01".to_string(), "2f686f6d65".to_string()),
            ("files.local:web01".to_string(), r"C:\Downloads".to_string()),
            ("theme".to_string(), String::new()),
        ];
        assert_eq!(store.take_over(rows.clone()).unwrap(), 2, "a host's last folder stays in state.db");
        assert_eq!(store.get("window").as_deref(), Some("1,2,3,4"));
        assert_eq!(store.get("files.remote:web01"), None);
        assert_eq!(store.get("files.local:web01"), None, "the files window's own memory, not a setting");
        // the file is there now: a second start takes nothing
        assert_eq!(settings(dir.path()).take_over(rows).unwrap(), 0);
    }

    #[test]
    fn a_file_saved_by_an_editor_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        std::fs::write(&file, "\u{feff}language = \"en\"\r\n").unwrap();
        let settings = Settings::at(&file);
        assert_eq!(settings.problem(), None);
        assert_eq!(settings.get("language").as_deref(), Some("en"));
    }

    #[test]
    fn a_hand_written_file_is_read_as_it_looks() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        std::fs::write(&file, "\"files.at_once\" = 5\nfirst_run_done = true\n").unwrap();
        let settings = Settings::at(&file);
        assert_eq!(settings.get("files.at_once").as_deref(), Some("5"));
        assert_eq!(
            settings.get("first_run_done"),
            None,
            "a machine key written outside its section isn't this machine's"
        );
    }
}
