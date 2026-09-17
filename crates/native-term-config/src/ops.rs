//! Editing sessions: create, change, move and delete hosts and folders.
//! Every change goes through [`Writer`] (conflict check, backup, atomic
//! replace, owner-only ACL) and is validated with `ssh -G`; a rejected
//! change is rolled back.
//!
//! Aliases never change once written: open tabs, jump-host references and
//! the user's own scripts use them. Renaming changes the label
//! (`NativeTermLabel`).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::alias;
use crate::document::{BlockKind, Document, LineKind};
use crate::effective;
use crate::header;
use crate::tree::{HostEntry, SessionTree, FOLDER_DEFAULTS_HOST};
use crate::write::{self, edit_file, WriteError, Writer};

/// What the user can set for a host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostDraft {
    /// Display name; stored as `NativeTermLabel` when it differs from the alias.
    pub label: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub proxy_jump: Option<String>,
    pub identity_files: Vec<String>,
    /// One line (`NativeTermNote`).
    pub note: Option<String>,
}

impl HostDraft {
    pub fn from_host(host: &HostEntry) -> HostDraft {
        HostDraft {
            label: host.label().to_string(),
            hostname: host.hostname.clone().unwrap_or_else(|| host.alias().to_string()),
            user: host.user.clone(),
            port: host.port,
            proxy_jump: host.proxy_jump.clone(),
            identity_files: host.identity_files.clone(),
            note: host.nt.get("note").map(str::to_string),
        }
    }

    fn check(&self) -> Result<(), EditError> {
        let bad = |what: &str, value: &str| {
            value.is_empty() || value.contains(['\r', '\n']) || (what != "label" && value.contains(char::is_whitespace))
        };
        if bad("hostname", self.hostname.trim()) {
            return Err(EditError::Invalid(format!("host name {:?} is empty or has spaces", self.hostname)));
        }
        if self.label.trim().is_empty() || self.label.contains(['\r', '\n']) {
            return Err(EditError::Invalid("the name must be one non-empty line".into()));
        }
        for (what, value) in [("user", &self.user), ("jump host", &self.proxy_jump)] {
            if let Some(v) = value {
                if bad(what, v) {
                    return Err(EditError::Invalid(format!("{what} {v:?} is empty or has spaces")));
                }
            }
        }
        if self.note.as_deref().is_some_and(|n| n.contains(['\r', '\n'])) {
            return Err(EditError::Invalid("the note must be one line".into()));
        }
        if self.port == Some(0) {
            return Err(EditError::Invalid("port 0".into()));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum EditError {
    Invalid(String),
    NotFound(String),
    Write(WriteError),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Invalid(why) => write!(f, "{why}"),
            EditError::NotFound(what) => write!(f, "{what} not found (changed outside NativeTerm?)"),
            EditError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<WriteError> for EditError {
    fn from(e: WriteError) -> Self {
        EditError::Write(e)
    }
}

impl From<io::Error> for EditError {
    fn from(e: io::Error) -> Self {
        EditError::Write(WriteError::Io(e))
    }
}

/// A host name that doesn't exist, for checking that a config still parses.
const PARSE_CHECK_HOST: &str = "nativeterm-config-check.invalid";

pub struct Editor {
    ssh_dir: PathBuf,
    writer: Writer,
    ssh: PathBuf,
    /// `None`: ssh's own default (`~/.ssh/config`). For tests: an explicit
    /// main config, whose includes must then be absolute.
    config: Option<PathBuf>,
    /// The `Include` line NativeTerm maintains in the main config.
    include: String,
}

impl Editor {
    /// For the user's real `~/.ssh`.
    pub fn new(ssh_dir: &Path, writer: Writer, ssh: &Path) -> Editor {
        Editor {
            ssh_dir: ssh_dir.to_path_buf(),
            writer,
            ssh: ssh.to_path_buf(),
            config: None,
            include: header::DEFAULT_INCLUDE.to_string(),
        }
    }

    /// For another directory (tests): ssh is pointed at its config with
    /// `-F`, and the include is absolute.
    pub fn for_directory(ssh_dir: &Path, writer: Writer, ssh: &Path) -> Editor {
        let include = format!("{}/*.conf", ssh_dir.join("config.d").to_string_lossy().replace('\\', "/"));
        Editor {
            ssh_dir: ssh_dir.to_path_buf(),
            writer,
            ssh: ssh.to_path_buf(),
            config: Some(ssh_dir.join("config")),
            include,
        }
    }

    pub fn main_config(&self) -> PathBuf {
        self.ssh_dir.join("config")
    }

    pub fn folders_dir(&self) -> PathBuf {
        self.ssh_dir.join("config.d")
    }

    /// `ssh -G <alias>` must succeed; with `expect`, the host name must match.
    fn validate(&self, alias: &str, expect: Option<&str>) -> Result<(), String> {
        let settings = effective::effective_with(&self.ssh, self.config.as_deref(), alias).map_err(|e| e.to_string())?;
        if let Some(expected) = expect {
            let actual = settings.iter().find(|(k, _)| k == "hostname").map(|(_, v)| v.as_str()).unwrap_or("");
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(format!("ssh resolves {alias} to {actual:?}, not {expected:?} (another block takes precedence?)"));
            }
        }
        Ok(())
    }

    /// The `IgnoreUnknown` and `Include` lines in the main config.
    pub fn ensure_header(&self) -> Result<bool, EditError> {
        let changed = edit_file(
            &self.writer,
            &self.main_config(),
            |doc| {
                header::ensure(doc, &self.include);
            },
            || self.validate(PARSE_CHECK_HOST, None),
        )?;
        Ok(changed)
    }

    /// A new, empty folder file; returns its path.
    pub fn create_folder(&self, label: &str) -> Result<PathBuf, EditError> {
        let label = label.trim();
        if label.is_empty() || label.contains(['\r', '\n']) {
            return Err(EditError::Invalid("the folder name must be one non-empty line".into()));
        }
        let dir = self.folders_dir();
        std::fs::create_dir_all(&dir)?;
        let stem = alias::sanitize(label);
        let path = (1..)
            .map(|n| if n == 1 { dir.join(format!("{stem}.conf")) } else { dir.join(format!("{stem}-{n}.conf")) })
            .find(|p| !p.exists())
            .expect("unbounded");
        let mut doc = Document::parse("");
        doc.append_host(&[FOLDER_DEFAULTS_HOST], &[("NativeTermLabel", label)]);
        self.writer.write(&path, &doc.render(), None, || self.validate(PARSE_CHECK_HOST, None))?;
        self.ensure_header()?;
        Ok(path)
    }

    /// Change a folder's display name.
    pub fn rename_folder(&self, file: &Path, label: &str) -> Result<(), EditError> {
        let label = label.trim();
        if label.is_empty() || label.contains(['\r', '\n']) {
            return Err(EditError::Invalid("the folder name must be one non-empty line".into()));
        }
        edit_file(
            &self.writer,
            file,
            |doc| match doc.find_host_block(FOLDER_DEFAULTS_HOST) {
                Some(block) => doc.set(block, "NativeTermLabel", label),
                None => prepend_folder_block(doc, label),
            },
            || self.validate(PARSE_CHECK_HOST, None),
        )?;
        Ok(())
    }

    /// Add a host to `file` (a folder file, or the main config). Returns
    /// the new alias.
    pub fn create_host(&self, tree: &SessionTree, file: &Path, draft: &HostDraft) -> Result<String, EditError> {
        draft.check()?;
        let folder_name = tree.folders().find(|f| f.file == file).map(|f| f.name.clone()).unwrap_or_default();
        let base = if alias::sanitize(&draft.label) == "host" { &draft.hostname } else { &draft.label };
        let new_alias = alias::unique(base, &folder_name, &tree.taken_aliases());
        let entries = entries_for(draft, &new_alias, Some(&crate::new_id()));
        let refs: Vec<(&str, &str)> = entries.iter().map(|(k, v)| (*k, v.as_str())).collect();
        if file != self.main_config() {
            self.ensure_header()?;
        }
        edit_file(
            &self.writer,
            file,
            |doc| doc.append_host(&[&new_alias], &refs),
            || self.validate(&new_alias, Some(draft.hostname.trim())),
        )?;
        Ok(new_alias)
    }

    /// Change a host's settings in place.
    pub fn update_host(&self, host: &HostEntry, draft: &HostDraft) -> Result<(), EditError> {
        draft.check()?;
        let alias = host.alias().to_string();
        let (text, _) = write::read(&host.file)?;
        if Document::parse(&text).find_host_block(&alias).is_none() {
            return Err(EditError::NotFound(format!("host {alias} in {}", host.file.display())));
        }
        edit_file(
            &self.writer,
            &host.file,
            |doc| {
                let Some(block) = doc.find_host_block(&alias) else { return };
                doc.set(block, "HostName", draft.hostname.trim());
                set_or_remove(doc, block, "User", draft.user.as_deref());
                set_or_remove(doc, block, "Port", draft.port.map(|p| p.to_string()).as_deref());
                set_or_remove(doc, block, "ProxyJump", draft.proxy_jump.as_deref());
                let current: Vec<String> = doc.get_all(block, "IdentityFile").iter().map(|d| d.value()).collect();
                if current != draft.identity_files {
                    doc.remove(block, "IdentityFile");
                    for file in &draft.identity_files {
                        append_directive(doc, block, "IdentityFile", file);
                    }
                }
                let label = draft.label.trim();
                set_or_remove(doc, block, "NativeTermLabel", (label != alias).then_some(label));
                set_or_remove(doc, block, "NativeTermNote", draft.note.as_deref().filter(|n| !n.trim().is_empty()));
            },
            || self.validate(&alias, Some(draft.hostname.trim())),
        )?;
        Ok(())
    }

    pub fn delete_host(&self, host: &HostEntry) -> Result<(), EditError> {
        let alias = host.alias().to_string();
        let changed = edit_file(
            &self.writer,
            &host.file,
            |doc| {
                if let Some(block) = doc.find_host_block(&alias) {
                    doc.remove_block(block);
                }
            },
            || self.validate(PARSE_CHECK_HOST, None),
        )?;
        if !changed {
            return Err(EditError::NotFound(format!("host {alias} in {}", host.file.display())));
        }
        Ok(())
    }

    /// Move a host's block (comments inside it included) to another file.
    /// The copy is written first; if removing the original fails, the copy
    /// is taken out again.
    pub fn move_host(&self, host: &HostEntry, to: &Path) -> Result<(), EditError> {
        if host.file == to {
            return Ok(());
        }
        let alias = host.alias().to_string();
        let (text, _) = write::read(&host.file)?;
        let source = Document::parse(&text);
        let block_index =
            source.find_host_block(&alias).ok_or_else(|| EditError::NotFound(format!("host {alias}")))?;
        let block = source.blocks().swap_remove(block_index);
        // trailing blank lines stay behind
        let mut end = block.end;
        while end > block.start && source.lines[end - 1].kind == LineKind::Blank {
            end -= 1;
        }
        let lines: Vec<String> = source.lines[block.start..end].iter().map(|l| l.text.clone()).collect();
        if to != self.main_config() {
            self.ensure_header()?;
        }
        std::fs::create_dir_all(to.parent().unwrap_or(Path::new(".")))?;
        edit_file(&self.writer, to, |doc| append_raw(doc, &lines), || Ok(()))?;
        let removed = edit_file(
            &self.writer,
            &host.file,
            |doc| {
                if let Some(b) = doc.find_host_block(&alias) {
                    doc.remove_block(b);
                }
            },
            || self.validate(&alias, host.hostname.as_deref()),
        );
        if let Err(e) = removed {
            // take the copy out again, so the host isn't defined twice
            let _ = edit_file(
                &self.writer,
                to,
                |doc| {
                    if let Some(b) = last_host_block(doc, &alias) {
                        doc.remove_block(b);
                    }
                },
                || Ok(()),
            );
            return Err(e.into());
        }
        Ok(())
    }
}

fn entries_for(draft: &HostDraft, alias: &str, id: Option<&str>) -> Vec<(&'static str, String)> {
    let mut entries = vec![("HostName", draft.hostname.trim().to_string())];
    if let Some(user) = &draft.user {
        entries.push(("User", user.clone()));
    }
    if let Some(port) = draft.port {
        entries.push(("Port", port.to_string()));
    }
    if let Some(jump) = &draft.proxy_jump {
        entries.push(("ProxyJump", jump.clone()));
    }
    for file in &draft.identity_files {
        entries.push(("IdentityFile", file.clone()));
    }
    if draft.label.trim() != alias {
        entries.push(("NativeTermLabel", draft.label.trim().to_string()));
    }
    if let Some(note) = draft.note.as_deref().filter(|n| !n.trim().is_empty()) {
        entries.push(("NativeTermNote", note.to_string()));
    }
    if let Some(id) = id {
        entries.push(("NativeTermId", id.to_string()));
    }
    entries
}

fn set_or_remove(doc: &mut Document, block: usize, keyword: &str, value: Option<&str>) {
    match value {
        Some(v) => doc.set(block, keyword, v),
        None => {
            doc.remove(block, keyword);
        }
    }
}

/// Add a directive after the block's last one, even if the keyword exists.
fn append_directive(doc: &mut Document, block: usize, keyword: &str, value: &str) {
    let b = doc.blocks().swap_remove(block);
    let last = doc.directives(&b).map(|(i, _)| i).last().or(b.header);
    let at = last.map_or(b.end, |i| i + 1);
    let indent = doc.directives(&b).next().map(|(i, _)| {
        let text = &doc.lines[i].text;
        text[..text.len() - text.trim_start().len()].to_string()
    });
    let text = format!("{}{keyword} {}", indent.unwrap_or_else(|| "    ".into()), crate::document::quote_arg(value));
    doc.lines.insert(at, crate::document::parse_line(&text));
}

fn append_raw(doc: &mut Document, lines: &[String]) {
    if doc.lines.last().is_some_and(|l| l.kind != LineKind::Blank) {
        doc.lines.push(crate::document::parse_line(""));
    }
    doc.lines.extend(lines.iter().map(|l| crate::document::parse_line(l)));
    doc.trailing_newline = true;
}

fn prepend_folder_block(doc: &mut Document, label: &str) {
    let mut block = Document::parse("");
    block.append_host(&[FOLDER_DEFAULTS_HOST], &[("NativeTermLabel", label)]);
    let mut lines = block.lines;
    lines.push(crate::document::parse_line(""));
    // after the global part, before the first Host/Match
    let at = doc.blocks().first().map_or(0, |b| b.end);
    doc.lines.splice(at..at, lines);
}

fn last_host_block(doc: &Document, alias: &str) -> Option<usize> {
    doc.blocks().iter().rposition(|b| match &b.kind {
        BlockKind::Host(patterns) => patterns.iter().any(|p| p.eq_ignore_ascii_case(alias)),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake `~/.ssh` and an editor on it; `None` if ssh isn't installed.
    fn setup() -> Option<(tempfile::TempDir, Editor)> {
        let ssh = PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe");
        if !ssh.exists() {
            eprintln!("skipped: no Windows OpenSSH");
            return None;
        }
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".ssh");
        std::fs::create_dir_all(&dir).unwrap();
        let writer = Writer::new(home.path().join("backups"));
        let editor = Editor::for_directory(&dir, writer, &ssh);
        std::fs::write(editor.main_config(), "# mine\nHost old\n    HostName 10.0.0.9\n").unwrap();
        crate::acl::restrict_to_owner(&editor.main_config()).unwrap();
        Some((home, editor))
    }

    fn tree(editor: &Editor) -> SessionTree {
        SessionTree::load(&editor.ssh_dir)
    }

    fn draft(label: &str, hostname: &str) -> HostDraft {
        HostDraft { label: label.into(), hostname: hostname.into(), ..HostDraft::default() }
    }

    #[test]
    fn folders_and_hosts_round_trip() {
        let Some((_home, editor)) = setup() else { return };
        let folder = editor.create_folder("Ceph 集群").unwrap();
        assert_eq!(folder.file_name().unwrap(), "ceph.conf", "{}", folder.display());
        let main = std::fs::read_to_string(editor.main_config()).unwrap();
        assert!(main.contains("IgnoreUnknown NativeTerm*") && main.contains("Include "), "{main}");
        assert!(main.contains("Host old"), "own content kept: {main}");

        let mut d = draft("osd 1 (生产)", "10.32.16.70");
        d.user = Some("ops".into());
        d.port = Some(2222);
        d.identity_files = vec!["~/.ssh/id_ed25519".into()];
        d.note = Some("rack 3".into());
        let alias = editor.create_host(&tree(&editor), &folder, &d).unwrap();
        assert_eq!(alias, "osd-1");

        let t = tree(&editor);
        let (f, host) = t.find(&alias).unwrap();
        assert_eq!(f.label(), "Ceph 集群");
        assert_eq!(HostDraft::from_host(host), d);
        assert!(host.id().is_some());

        // same label again: the folder prefix keeps the alias unique
        let second = editor.create_host(&t, &folder, &draft("osd 1 (生产)", "10.32.16.71")).unwrap();
        assert_eq!(second, "ceph.osd-1");

        // edit: drop the port and the note, rename, second key
        let mut changed = d.clone();
        changed.label = "osd-1".into();
        changed.port = None;
        changed.note = None;
        changed.identity_files.push("~/.ssh/id_rsa".into());
        editor.update_host(host, &changed).unwrap();
        let t = tree(&editor);
        let host = t.find(&alias).unwrap().1;
        assert_eq!(HostDraft::from_host(host), changed);
        assert_eq!(host.nt.get("label"), None, "a label equal to the alias isn't stored");
        assert!(host.id().is_some(), "id kept");

        // move to the main config and back out
        editor.move_host(host, &editor.main_config()).unwrap();
        let t = tree(&editor);
        let (f, host) = t.find(&alias).unwrap();
        assert!(f.name.is_empty(), "now in the main config");
        assert_eq!(HostDraft::from_host(host), changed);
        let text = std::fs::read_to_string(&folder).unwrap();
        assert!(!text.contains(&format!("Host {alias}\n")), "{text}");

        editor.delete_host(host).unwrap();
        assert!(tree(&editor).find(&alias).is_none());
        assert!(tree(&editor).find("ceph.osd-1").is_some());

        editor.rename_folder(&folder, "Ceph").unwrap();
        assert_eq!(tree(&editor).folders().find(|f| f.file == folder).unwrap().label(), "Ceph");
    }

    #[test]
    fn rejected_changes_are_rolled_back() {
        let Some((_home, editor)) = setup() else { return };
        // a folder read earlier defines the same name: ssh uses its block,
        // so the new host wouldn't resolve to what the user entered
        let web = editor.create_folder("Web").unwrap();
        let earlier = editor.folders_dir().join("aaa.conf");
        std::fs::write(&earlier, "Host web
    HostName 10.9.9.9
").unwrap();
        crate::acl::restrict_to_owner(&earlier).unwrap();
        let before = std::fs::read_to_string(&web).unwrap();
        let mut t = tree(&editor);
        // pretend the alias is free, as a stale tree would
        for f in t.folders.iter_mut() {
            f.hosts.retain(|h| h.alias() != "web");
        }
        let err = editor.create_host(&t, &web, &draft("web", "10.0.0.1")).unwrap_err();
        assert!(matches!(err, EditError::Write(WriteError::Rejected { .. })), "{err}");
        assert_eq!(std::fs::read_to_string(&web).unwrap(), before, "rolled back");
    }

    #[test]
    fn drafts_are_checked() {
        for bad in [
            draft("", "h"),
            draft("x", ""),
            draft("x", "two words"),
            HostDraft { user: Some("a b".into()), ..draft("x", "h") },
            HostDraft { note: Some("a\nb".into()), ..draft("x", "h") },
            HostDraft { port: Some(0), ..draft("x", "h") },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        assert!(draft("名字 with spaces", "10.0.0.1").check().is_ok());
    }

    #[test]
    fn conflicting_edit_is_refused() {
        let Some((_home, editor)) = setup() else { return };
        let folder = editor.create_folder("Lab").unwrap();
        let alias = editor.create_host(&tree(&editor), &folder, &draft("a", "10.0.0.1")).unwrap();
        let t = tree(&editor);
        let host = t.find(&alias).unwrap().1.clone();
        // someone else edits the file meanwhile: host lookup still works,
        // and edit_file re-reads, so a later edit applies to the new text
        let text = std::fs::read_to_string(&folder).unwrap();
        std::fs::write(&folder, format!("{text}\n# note by hand\n")).unwrap();
        editor.update_host(&host, &draft("a", "10.0.0.2")).unwrap();
        let text = std::fs::read_to_string(&folder).unwrap();
        assert!(text.contains("# note by hand") && text.contains("10.0.0.2"), "{text}");
        // a host removed by hand can't be edited
        std::fs::write(&folder, "").unwrap();
        assert!(matches!(editor.update_host(&host, &draft("a", "10.0.0.3")), Err(EditError::NotFound(_))));
    }
}
