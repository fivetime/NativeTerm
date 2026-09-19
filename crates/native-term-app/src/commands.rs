//! The command library (`commands.toml` in the data directory): named
//! commands to send to sessions. Hand-editable:
//!
//! ```toml
//! [[command]]
//! name = "Disk usage"
//! text = "df -h"
//!
//! [[command]]
//! name = "Become root"
//! text = "sudo -i"
//! group = "Admin"
//! ```

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    /// Sent line by line, each followed by Enter unless `enter` is false
    /// (then the last line is left for the user to finish).
    pub text: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enter: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "command")]
    commands: Vec<Command>,
}

pub struct Library {
    path: PathBuf,
    pub commands: Vec<Command>,
}

impl Library {
    pub fn path_in(data_dir: &Path) -> PathBuf {
        data_dir.join("commands.toml")
    }

    /// A missing file is an empty library; a broken one is an error (and
    /// is never overwritten).
    pub fn load(path: &Path) -> io::Result<Library> {
        let commands = match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str::<File>(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
                    .commands
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Library { path: path.to_path_buf(), commands })
    }

    pub fn save(&self) -> io::Result<()> {
        let text = toml::to_string_pretty(&File { commands: self.commands.clone() })
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }

    /// Add or replace (by name).
    pub fn put(&mut self, command: Command) {
        match self.commands.iter_mut().find(|c| c.name == command.name) {
            Some(existing) => *existing = command,
            None => self.commands.push(command),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.commands.retain(|c| c.name != name);
    }
}

/// How imported commands went into the library.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub added: usize,
    /// The same command (name, text, Enter) was there already.
    pub present: usize,
    /// (the name it had, the name it got): another command had its name.
    pub renamed: Vec<(String, String)>,
}

/// Add `incoming` (e.g. SecureCRT's buttons) to `library`: one that is
/// there already (same name and text) is left out; a different one with a
/// taken name gets "<name> (<group>)", then a number.
pub fn merge(library: &mut Library, incoming: Vec<Command>) -> Merged {
    let mut out = Merged::default();
    for command in incoming {
        let same = |c: &Command| c.text == command.text && c.enter == command.enter;
        if library.commands.iter().any(|c| c.name == command.name && same(c)) {
            out.present += 1;
            continue;
        }
        let mut name = command.name.clone();
        if library.commands.iter().any(|c| c.name == name) {
            let base = match &command.group {
                Some(group) => format!("{} ({group})", command.name),
                None => command.name.clone(),
            };
            name = base.clone();
            let mut n = 2;
            while let Some(existing) = library.commands.iter().find(|c| c.name == name) {
                if same(existing) {
                    break;
                }
                name = format!("{base} {n}");
                n += 1;
            }
            if library.commands.iter().any(|c| c.name == name && same(c)) {
                out.present += 1;
                continue;
            }
            out.renamed.push((command.name.clone(), name.clone()));
        }
        library.commands.push(Command { name, ..command });
        out.added += 1;
    }
    out
}

/// The lines to type, each with whether Enter follows. Blank lines in the
/// middle are sent as a bare Enter; a trailing newline adds nothing.
pub fn lines(text: &str, enter: bool) -> Vec<(String, bool)> {
    let text = text.replace("\r\n", "\n");
    let text = text.strip_suffix('\n').unwrap_or(&text);
    if text.is_empty() {
        return if enter { vec![(String::new(), true)] } else { Vec::new() };
    }
    let parts: Vec<&str> = text.split('\n').collect();
    let last = parts.len() - 1;
    parts.into_iter().enumerate().map(|(i, l)| (l.to_string(), i < last || enter)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_imported_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut library = Library::load(&dir.path().join("commands.toml")).unwrap();
        let cmd = |name: &str, text: &str, group: &str| Command {
            name: name.into(),
            text: text.into(),
            enter: true,
            group: Some(group.into()),
        };
        library.put(cmd("uptime", "uptime", "mine"));
        library.put(cmd("disk", "df -h", "mine"));
        let merged = merge(
            &mut library,
            vec![
                cmd("uptime", "uptime", "Cisco"),
                cmd("disk", "df -hT", "Linux"),
                cmd("ip br", "sh ip int br", "Cisco"),
                cmd("disk", "df -hT", "Linux"),
            ],
        );
        assert_eq!(merged.added, 2);
        assert_eq!(merged.present, 2, "the same uptime, and the second disk once renamed");
        assert_eq!(merged.renamed, [("disk".to_string(), "disk (Linux)".to_string())]);
        let names: Vec<&str> = library.commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["uptime", "disk", "disk (Linux)", "ip br"]);
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = Library::path_in(dir.path());
        let mut lib = Library::load(&path).unwrap();
        assert!(lib.commands.is_empty());
        lib.put(Command { name: "df".into(), text: "df -h".into(), enter: true, group: None });
        lib.put(Command { name: "root".into(), text: "sudo -i".into(), enter: false, group: Some("管理".into()) });
        lib.put(Command { name: "df".into(), text: "df -hT".into(), enter: true, group: None });
        lib.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[command]]") && !text.contains("enter = true"), "{text}");
        let again = Library::load(&path).unwrap();
        assert_eq!(again.commands, lib.commands);
        assert_eq!(again.commands[0].text, "df -hT");

        std::fs::write(&path, "not toml [").unwrap();
        assert!(Library::load(&path).is_err());
    }

    #[test]
    fn splitting() {
        assert_eq!(lines("uptime", true), [("uptime".to_string(), true)]);
        assert_eq!(lines("cd /srv\r\nls\n", true), [("cd /srv".to_string(), true), ("ls".to_string(), true)]);
        assert_eq!(lines("a\n\nb", false), [("a".into(), true), ("".into(), true), ("b".into(), false)]);
        assert_eq!(lines("", true), [(String::new(), true)]);
        assert!(lines("", false).is_empty());
    }
}
