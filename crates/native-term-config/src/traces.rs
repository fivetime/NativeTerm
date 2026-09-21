//! What NativeTerm has left in `~/.ssh`, so it can be listed before
//! someone deletes the program (the "Clean up" action).
//!
//! None of it stops `ssh` from working: `IgnoreUnknown NativeTerm*` tells
//! ssh to skip the keys NativeTerm writes, the `Include` line is what
//! makes the session folder's files count at all, and the `NativeTerm*`
//! keys are those skipped values. Removing them would take the sessions
//! away from ssh too, so NativeTerm never removes them on its own — it
//! says where they are and leaves the choice to the person.

use std::path::{Path, PathBuf};

/// Windows' own separator (paths are compared with `/`).
const SEPARATOR: char = '\\';

use crate::document::Document;
use crate::header::IGNORE_PATTERN;
use crate::proxy::Proxy;

/// One line NativeTerm wrote, where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    pub file: PathBuf,
    /// 1-based, as an editor counts.
    pub line: usize,
    pub text: String,
}

impl Trace {
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{}:{} {}", self.file.display(), self.line, self.text.trim())
    }
}

/// Everything found in the ssh configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Traces {
    /// The two lines at the top of the main config.
    pub header: Vec<Trace>,
    /// `NativeTerm*` keys in the folder files.
    pub keys: Vec<Trace>,
    /// `ProxyCommand`s that run NativeTerm's helper.
    pub proxies: Vec<Trace>,
    /// The session folder's files (the sessions themselves: ssh reads
    /// these, and they are the user's own).
    pub files: Vec<PathBuf>,
}

impl Traces {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.header.is_empty() && self.keys.is_empty() && self.proxies.is_empty() && self.files.is_empty()
    }

    /// Every line, as a list to read or copy.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.header.iter().chain(&self.keys).chain(&self.proxies).map(Trace::describe).collect()
    }
}

/// What NativeTerm wrote into the ssh configuration: the main config in
/// `ssh_dir`, and the files of `folders` (the session folder, which can
/// be anywhere). `shim` is NativeTerm's helper, whose name marks the
/// `ProxyCommand` lines as ours.
#[must_use]
pub fn find(ssh_dir: &Path, folders: &Path, shim: &Path) -> Traces {
    let main = &ssh_dir.join("config");
    let home = ssh_dir.parent().unwrap_or(ssh_dir);
    let mut traces = Traces::default();
    if let Ok(text) = std::fs::read_to_string(main) {
        let doc = Document::parse(&text);
        for (at, line) in doc.lines.iter().enumerate() {
            let Some(directive) = line.directive() else { continue };
            let ours = (directive.is("IgnoreUnknown") && directive.value().contains(IGNORE_PATTERN))
                || (directive.is("Include") && includes(&directive.value(), folders, home, ssh_dir));
            if ours {
                traces.header.push(Trace { file: main.to_path_buf(), line: at + 1, text: line.text.clone() });
            }
        }
    }
    let mut files = files_in(folders);
    files.sort();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let doc = Document::parse(&text);
        for (at, line) in doc.lines.iter().enumerate() {
            let Some(directive) = line.directive() else { continue };
            let trace = || Trace { file: file.clone(), line: at + 1, text: line.text.clone() };
            if directive.keyword.to_lowercase().starts_with("nativeterm") {
                traces.keys.push(trace());
            } else if directive.is("ProxyCommand") && is_ours(line.raw_value(), shim) {
                traces.proxies.push(trace());
            }
        }
    }
    traces.files = files;
    traces
}

/// Whether an `Include` points at the session folder. The pattern is
/// read the way ssh reads it (`include.rs`): `~` is the home directory
/// and a relative path is relative to the ssh folder.
fn includes(value: &str, folders: &Path, home: &Path, ssh_dir: &Path) -> bool {
    let wanted = compared(folders);
    value.split_whitespace().any(|pattern| {
        let path = expand(pattern.trim_matches('"'), home, ssh_dir);
        // the folder itself, or a pattern for the files in it
        let dir = match path.is_dir() {
            true => Some(path),
            false => path.parent().map(Path::to_path_buf),
        };
        dir.is_some_and(|dir| compared(&dir) == wanted)
    })
}

/// A path as it is compared: without case, with one kind of separator.
fn compared(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace(SEPARATOR, "/").trim_end_matches('/').to_string()
}

/// An `Include` pattern as a path (see `include::resolve`).
fn expand(pattern: &str, home: &Path, ssh_dir: &Path) -> PathBuf {
    if pattern == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = pattern.strip_prefix('~') {
        return home.join(rest.trim_start_matches(['/', SEPARATOR]));
    }
    match Path::new(pattern).is_absolute() {
        true => PathBuf::from(pattern),
        false => ssh_dir.join(pattern),
    }
}

/// Whether a `ProxyCommand` runs NativeTerm's helper: one NativeTerm
/// wrote (`proxy.rs` reads it back) that names this program or another
/// copy of it.
fn is_ours(value: Option<&str>, shim: &Path) -> bool {
    let Some(value) = value else { return false };
    if Proxy::from_command(value).is_none() {
        return false;
    }
    let name = shim.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    !name.is_empty() && value.to_lowercase().contains(&name)
}

/// `.conf` and `.nt.toml` files in the session folder.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            p.is_file() && (name.ends_with(".conf") || name.ends_with(".nt.toml"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn every_line_nativeterm_wrote_is_found_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path();
        let folders = ssh.join("config.d");
        let shim = Path::new(r"C:\NativeTerm\nativeterm-shim.exe");
        write(
            &ssh.join("config"),
            &format!(
                concat!(
                    "IgnoreUnknown Home,{}\n",
                    "Include {}/*.conf\n",
                    "Include ~/.ssh/work.conf\n",
                    "\n",
                    "Host mine\n",
                    "    HostName mine.lan\n",
                ),
                IGNORE_PATTERN,
                folders.display().to_string().replace('\\', "/")
            ),
        );
        write(
            &folders.join("Work.conf"),
            concat!(
                "Host web01\n",
                "    HostName 10.0.0.1\n",
                "    NativeTermId 7f3c\n",
                "    NativeTermLabel Web 01\n",
                "    ProxyCommand \"C:\\NativeTerm\\nativeterm-shim.exe\" --proxy socks5://gw:1080 %h %p\n",
                "\n",
                "Host own-proxy\n",
                "    ProxyCommand /usr/bin/corkscrew gw 8080 %h %p\n",
            ),
        );
        write(&folders.join("Lab.nt.toml"), "[[session]]\nname = \"serial\"\n");

        let traces = find(ssh, &folders, shim);
        assert_eq!(traces.header.len(), 2, "{:?}", traces.header);
        assert!(traces.header[0].text.contains(IGNORE_PATTERN));
        assert!(traces.header[1].text.contains("config.d"), "not the user's own Include: {:?}", traces.header);
        assert_eq!(traces.header[1].line, 2, "counted as an editor does");
        assert_eq!(traces.keys.len(), 2, "{:?}", traces.keys);
        assert_eq!(traces.proxies.len(), 1, "the user's own proxy command is not ours: {:?}", traces.proxies);
        assert!(traces.proxies[0].describe().contains("Work.conf:5"), "{}", traces.proxies[0].describe());
        assert_eq!(traces.files.len(), 2, "the sessions themselves: {:?}", traces.files);
        assert_eq!(traces.lines().len(), 5);
    }

    #[test]
    fn the_include_is_found_however_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        let folders = ssh.join("config.d");
        std::fs::create_dir_all(&folders).unwrap();
        for pattern in ["~/.ssh/config.d/*.conf", "config.d/*.conf", "config.d"] {
            write(&ssh.join("config"), &format!("Include {pattern}\n"));
            let traces = find(&ssh, &folders, Path::new("nativeterm-shim.exe"));
            assert_eq!(traces.header.len(), 1, "{pattern}: {:?}", traces.header);
        }
        // someone else's, in the same place
        write(&ssh.join("config"), "Include ~/.ssh/other/*.conf\nInclude ~/.ssh/config.d.bak/*.conf\n");
        assert!(find(&ssh, &folders, Path::new("nativeterm-shim.exe")).header.is_empty());
    }

    #[test]
    fn a_config_nativeterm_never_touched_has_nothing_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path();
        write(&ssh.join("config"), "Host mine\n    HostName mine.lan\n    ProxyJump gw\n");
        let traces = find(ssh, &ssh.join("config.d"), Path::new("nativeterm-shim.exe"));
        assert!(traces.is_empty(), "{traces:?}");
        assert!(traces.lines().is_empty());
    }
}
